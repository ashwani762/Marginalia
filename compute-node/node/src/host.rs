//! The models the node serves. They load once and stay loaded; a job only borrows them.

use crate::engine::whisper::{Whisper, WhisperConfig};
use crate::engine::{ChatMessage, EmbedHandle, EmbedKind, GenParams, LlmHandle, Priority};
use anyhow::{anyhow, Context, Result};
use llama_cpp_2::llama_backend::LlamaBackend;
use marginalia_proto::Capabilities;
use std::path::PathBuf;
use std::sync::atomic::AtomicU32;
use std::sync::Arc;
use std::time::Instant;

pub struct Options {
    pub models_dir: PathBuf,
    pub chat_model: Option<PathBuf>,
    pub embed_model: Option<PathBuf>,
    pub cpu_only: bool,
    pub ctx: u32,
    /// The speech model file, if not the default one in `models_dir`.
    pub speech_model: Option<PathBuf>,
    /// Do not load the speech model even if it is there.
    pub no_speech: bool,
    /// The most seconds of audio a short live window may hold (see `--window-max-seconds`).
    pub window_max_s: u32,
}

pub struct Host {
    _backend: Arc<LlamaBackend>,
    pub llm: Option<LlmHandle>,
    pub embed: Option<EmbedHandle>,
    /// Speech to text, if the model file was there.
    pub whisper: Option<Arc<Whisper>>,
    pub caps: parking_lot::Mutex<Capabilities>,
    /// Jobs that have been accepted and not yet finished.
    pub chat_jobs: Arc<AtomicU32>,
    pub embed_jobs: Arc<AtomicU32>,
    /// Whole recordings: accepted and not finished.
    pub speech_jobs: Arc<AtomicU32>,
    /// Short audio windows (live correction): accepted and not finished.
    pub window_jobs: Arc<AtomicU32>,
    /// One whole recording is transcribed at a time, the others wait here.
    pub speech_gate: Arc<tokio::sync::Semaphore>,
    /// One short window is transcribed at a time, on its own lane so it does not wait behind a long recording.
    /// It is the same gate as `speech_gate` when the speech model runs on the processor, where two decodes at
    /// once would only slow each other down.
    pub window_gate: Arc<tokio::sync::Semaphore>,
    /// Windows in flight per phone (certificate fingerprint), so one phone cannot fill the lane.
    pub window_by_phone: parking_lot::Mutex<std::collections::HashMap<String, u32>>,
}

pub const SPEECH_FILE: &str = "ggml-large-v3-turbo-q5_0.bin";
pub const CHAT_FILE: &str = "gemma-4-E2B-it-Q4_0.gguf";
pub const CHAT_FILE_E4B: &str = "gemma-4-E4B-it-Q4_0.gguf";

/// The chat model `run` loads when `--chat-model` is not given: the E4B file when it was downloaded, else E2B.
pub fn default_chat_path(models_dir: &std::path::Path) -> PathBuf {
    let e4b = models_dir.join(CHAT_FILE_E4B);
    if e4b.is_file() { e4b } else { models_dir.join(CHAT_FILE) }
}
pub const EMBED_FILE: &str = "embeddinggemma-2-Q8_0.gguf";

fn tiers(cpu_only: bool) -> Vec<u32> {
    if cpu_only { vec![0] } else { vec![999, 16, 0] }
}

/// `ggml-large-v3-turbo-q5_0.bin` is shown as `large-v3-turbo-q5_0`.
fn speech_name(path: &std::path::Path) -> String {
    let stem = path.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_else(|| "speech".into());
    stem.strip_prefix("ggml-").map(str::to_string).unwrap_or(stem)
}

fn threads() -> i32 {
    let n = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(8);
    ((n / 2).max(4)).min(16) as i32
}

impl Host {
    pub fn start(opts: &Options) -> Result<Host> {
        let mut backend = LlamaBackend::init().context("starting llama.cpp")?;
        if std::env::var_os("MARGINALIA_LLAMA_LOGS").is_none() {
            backend.void_logs();
        }
        let backend = Arc::new(backend);
        let mut caps = Capabilities { embed_dim: 768, ..Default::default() };

        let embed_path = opts.embed_model.clone().unwrap_or_else(|| opts.models_dir.join(EMBED_FILE));
        let embed = if embed_path.is_file() {
            let mut found = None;
            for layers in tiers(opts.cpu_only) {
                let cfg = crate::engine::embed::EmbedConfig { model_path: embed_path.clone(), mmproj_path: None, gpu_layers: layers, use_gpu: layers > 0, threads: threads() };
                match crate::engine::embed::spawn(backend.clone(), cfg) {
                    Ok(h) => {
                        tracing::info!("search model loaded ({} GPU layers)", layers);
                        found = Some(h);
                        break;
                    }
                    Err(e) => tracing::warn!("search model did not load with {layers} GPU layers: {e:#}"),
                }
            }
            if found.is_some() {
                caps.embed = Some("embeddinggemma-2-270m".into());
            }
            found
        } else {
            tracing::warn!("no search model at {}", embed_path.display());
            None
        };

        let chat_path = opts.chat_model.clone().unwrap_or_else(|| default_chat_path(&opts.models_dir));
        let llm = if chat_path.is_file() {
            let mut found = None;
            for layers in tiers(opts.cpu_only) {
                let cfg = crate::engine::llm::LlmConfig { model_path: chat_path.clone(), mmproj_path: None, gpu_layers: layers, use_gpu: layers > 0, n_ctx: opts.ctx, threads: threads() };
                match crate::engine::llm::spawn(backend.clone(), cfg) {
                    Ok(h) => {
                        tracing::info!("chat model loaded ({} GPU layers)", layers);
                        found = Some(h);
                        break;
                    }
                    Err(e) => tracing::warn!("chat model did not load with {layers} GPU layers: {e:#}"),
                }
            }
            if found.is_some() {
                caps.chat = Some(chat_path.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_else(|| "chat".into()));
            }
            found
        } else {
            tracing::warn!("no chat model at {}", chat_path.display());
            None
        };

        let speech_path = opts.speech_model.clone().unwrap_or_else(|| opts.models_dir.join(SPEECH_FILE));
        let whisper = if opts.no_speech {
            None
        } else if speech_path.is_file() {
            let mut found = None;
            // The graphics card first, then the processor.
            for gpu in if opts.cpu_only { vec![false] } else { vec![true, false] } {
                let cfg = WhisperConfig { model_path: speech_path.clone(), use_gpu: gpu, threads: threads() };
                match Whisper::load(&cfg) {
                    Ok(w) => {
                        tracing::info!("speech model loaded ({})", if gpu { "graphics card" } else { "processor" });
                        found = Some(Arc::new(w));
                        break;
                    }
                    Err(e) => tracing::warn!("speech model did not load{}: {e:#}", if gpu { " on the graphics card" } else { "" }),
                }
            }
            if found.is_some() {
                caps.transcribe = Some(speech_name(&speech_path));
                caps.window_max_s = opts.window_max_s;
            }
            found
        } else {
            tracing::warn!("no speech model at {}, so this node will not transcribe recordings", speech_path.display());
            None
        };
        if llm.is_none() && embed.is_none() && whisper.is_none() {
            return Err(anyhow!("no models found in {}", opts.models_dir.display()));
        }
        let speech_gate = Arc::new(tokio::sync::Semaphore::new(1));
        let window_gate = if whisper.as_ref().map(|w| w.on_gpu()).unwrap_or(false) { Arc::new(tokio::sync::Semaphore::new(1)) } else { speech_gate.clone() };
        let host = Host {
            _backend: backend,
            llm,
            embed,
            whisper,
            caps: parking_lot::Mutex::new(caps),
            chat_jobs: Arc::new(AtomicU32::new(0)),
            embed_jobs: Arc::new(AtomicU32::new(0)),
            speech_jobs: Arc::new(AtomicU32::new(0)),
            window_jobs: Arc::new(AtomicU32::new(0)),
            speech_gate,
            window_gate,
            window_by_phone: parking_lot::Mutex::new(Default::default()),
        };
        host.benchmark();
        Ok(host)
    }

    /// A short run of each model, so phones know what this machine is worth before they send work.
    pub fn benchmark(&self) {
        let mut caps = self.caps.lock();
        if let Some(e) = &self.embed {
            let texts: Vec<String> = (0..16).map(|i| format!("title: none | text: Benchmark passage number {i} about gardens, weather and meetings that ran long.")).collect();
            let _ = e.embed(texts.clone(), EmbedKind::Raw);
            let t = Instant::now();
            if e.embed(texts, EmbedKind::Raw).is_ok() {
                caps.embed_per_s = 16.0 / t.elapsed().as_secs_f32().max(1e-3);
            }
        }
        if let Some(l) = &self.llm {
            let before = l.tokens_generated.load(std::sync::atomic::Ordering::Relaxed);
            let t = Instant::now();
            let r = l.complete(
                vec![ChatMessage::user("Write two sentences about a garden in spring.")],
                GenParams { max_tokens: 64, temperature: 0.6, ..Default::default() },
                Priority::High,
            );
            let n = l.tokens_generated.load(std::sync::atomic::Ordering::Relaxed) - before;
            if r.is_ok() && n > 8 {
                caps.chat_tok_s = n as f32 / t.elapsed().as_secs_f32().max(1e-3);
            }
        }
        if let Some(w) = &self.whisper {
            if let Some(x) = bench_speech(w) {
                caps.transcribe_x = x;
            }
        }
        tracing::info!(
            "measured: chat {:.0} tokens a second, search {:.0} passages a second, speech {:.0} times faster than real time",
            caps.chat_tok_s, caps.embed_per_s, caps.transcribe_x
        );
    }

    /// Folds the speed of a real job into the figure phones see, so a node that slows down says so.
    pub fn note_chat_speed(&self, tok_s: f32) {
        let mut c = self.caps.lock();
        c.chat_tok_s = if c.chat_tok_s > 0.0 { c.chat_tok_s * 0.7 + tok_s * 0.3 } else { tok_s };
    }

    pub fn note_embed_speed(&self, per_s: f32) {
        let mut c = self.caps.lock();
        c.embed_per_s = if c.embed_per_s > 0.0 { c.embed_per_s * 0.7 + per_s * 0.3 } else { per_s };
    }

    /// `x` is how many times faster than real time a job transcribed.
    pub fn note_speech_speed(&self, x: f32) {
        let mut c = self.caps.lock();
        c.transcribe_x = if c.transcribe_x > 0.0 { c.transcribe_x * 0.7 + x * 0.3 } else { x };
    }

    pub fn idle(&self) -> bool {
        self.chat_jobs.load(std::sync::atomic::Ordering::Relaxed) + self.embed_jobs.load(std::sync::atomic::Ordering::Relaxed) + self.speech_jobs.load(std::sync::atomic::Ordering::Relaxed) + self.window_jobs.load(std::sync::atomic::Ordering::Relaxed) == 0
    }

    pub fn shutdown(&self) {
        if let Some(l) = &self.llm {
            l.shutdown();
        }
        if let Some(e) = &self.embed {
            e.shutdown();
        }
    }
}

/// A short public domain recording (President Kennedy, 11 s, 16 kHz mono) that ships inside the node. It is
/// repeated into about a minute of speech and transcribed once, so the speed phones are told exists from the
/// first minute instead of after the first real job.
const SPEECH_SAMPLE: &[u8] = include_bytes!("../assets/jfk.wav");

fn sample_pcm() -> Option<Vec<f32>> {
    let at = SPEECH_SAMPLE.windows(4).position(|w| w == b"data")?;
    let len = u32::from_le_bytes(SPEECH_SAMPLE.get(at + 4..at + 8)?.try_into().ok()?) as usize;
    let body = SPEECH_SAMPLE.get(at + 8..(at + 8 + len).min(SPEECH_SAMPLE.len()))?;
    Some(body.chunks_exact(2).map(|b| i16::from_le_bytes([b[0], b[1]]) as f32 / 32768.0).collect())
}

/// Times faster than real time on the sample, or None if it could not run.
fn bench_speech(w: &crate::engine::whisper::Whisper) -> Option<f32> {
    let one = sample_pcm()?;
    let gap = vec![0.0f32; 16_000];
    let mut pcm = Vec::with_capacity((one.len() + gap.len()) * 6);
    for _ in 0..6 {
        pcm.extend_from_slice(&one);
        pcm.extend_from_slice(&gap);
    }
    let cancel = std::sync::atomic::AtomicBool::new(false);
    let t = Instant::now();
    let r = w.transcribe(&pcm, crate::engine::whisper::Job { language: Some("en"), prompt: None, window: false }, &cancel, &mut |_| {}, &mut |_| {});
    let secs = t.elapsed().as_secs_f32();
    if r.is_err() || secs <= 0.0 {
        return None;
    }
    Some(pcm.len() as f32 / 16_000.0 / secs)
}

#[cfg(test)]
mod speech_sample_tests {
    #[test]
    fn the_bundled_sample_is_eleven_seconds_of_audio() {
        let pcm = super::sample_pcm().expect("sample");
        let secs = pcm.len() as f32 / 16_000.0;
        assert!((10.0..12.5).contains(&secs), "{secs}");
        assert!(pcm.iter().any(|x| x.abs() > 0.1));
    }
}
