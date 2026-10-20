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
    /// Where each model runs (see `runon.rs`).
    pub run_on: crate::runon::Setting,
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
    /// The chat model has its vision projector next to it, so this node can read photos.
    pub vision: bool,
    /// The vision projector's name, for the status and the tray.
    pub vision_model: Option<String>,
    /// Where each model ended up, for the status and the tray.
    pub placement: serde_json::Value,
    /// The models folder, for the tray's downloads.
    pub models_dir: PathBuf,
    /// Telling speakers apart, opt-in (see `speakers.rs`).
    pub speakers: crate::speakers::Speakers,
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

/// The vision projector that goes with a chat model: `mmproj-<model family>-*.gguf` in the same folder, for example
/// `mmproj-gemma-4-E2B-it-Q8_0.gguf` for `gemma-4-E2B-it-Q4_0.gguf`. None when it was not downloaded.
pub fn find_projector(chat_model: &std::path::Path) -> Option<PathBuf> {
    let stem = chat_model.file_stem()?.to_string_lossy().to_string();
    let family = stem.rsplit_once('-').map(|(f, _)| f.to_string()).unwrap_or(stem);
    let prefix = format!("mmproj-{family}-");
    let mut found: Vec<PathBuf> = std::fs::read_dir(chat_model.parent()?).ok()?.filter_map(|e| e.ok().map(|e| e.path())).filter(|p| {
        let n = p.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
        n.starts_with(&prefix) && n.ends_with(".gguf")
    }).collect();
    found.sort();
    found.into_iter().next()
}

/// The model file names come from models.toml (see `catalog.rs`).
pub fn speech_file() -> &'static str {
    crate::catalog::speech().file
}
pub fn chat_file() -> &'static str {
    crate::catalog::chat_e2b().file
}
pub fn chat_file_e4b() -> &'static str {
    crate::catalog::chat_e4b().file
}

/// The chat model `run` loads when nothing was chosen: E2B, or E4B when that is the only one downloaded.
pub fn default_chat_path(models_dir: &std::path::Path) -> PathBuf {
    let e2b = models_dir.join(chat_file());
    let e4b = models_dir.join(chat_file_e4b());
    if !e2b.is_file() && e4b.is_file() { e4b } else { e2b }
}
pub fn embed_file() -> &'static str {
    crate::catalog::search().file
}

fn tiers(cpu_only: bool) -> Vec<u32> {
    if cpu_only { vec![0] } else { vec![999, 16, 0] }
}

/// The layer counts to try for a model with `share` percent on the graphics card: that share, then the processor only.
fn share_tiers(share: u8, n_layer: Option<u32>) -> Vec<u32> {
    match (share, n_layer) {
        (0, _) => vec![0],
        (100, _) => vec![999, 0],
        // The layer count is unknown, so keep the whole model on one side: the card when most of it was asked for.
        (s, None) => if s >= 50 { vec![999, 0] } else { vec![0] },
        (s, Some(n)) => vec![((n as u64 * s as u64 + 99) / 100).max(1) as u32, 0],
    }
}

/// How many layers a model file has (`<architecture>.block_count`), read from its GGUF header without loading the weights.
fn layer_count(path: &std::path::Path) -> Option<u32> {
    use std::io::Read;
    let mut f = std::io::BufReader::new(std::fs::File::open(path).ok()?);
    fn u32_(f: &mut impl Read) -> Option<u32> { let mut b = [0u8; 4]; f.read_exact(&mut b).ok()?; Some(u32::from_le_bytes(b)) }
    fn u64_(f: &mut impl Read) -> Option<u64> { let mut b = [0u8; 8]; f.read_exact(&mut b).ok()?; Some(u64::from_le_bytes(b)) }
    fn text(f: &mut impl Read) -> Option<String> {
        let n = u64_(f)?;
        if n > 1 << 20 { return None; }
        let mut b = vec![0u8; n as usize]; f.read_exact(&mut b).ok()?; Some(String::from_utf8_lossy(&b).into_owned())
    }
    fn skip(f: &mut impl Read, ty: u32) -> Option<()> {
        let n = match ty { 0 | 1 | 7 => 1, 2 | 3 => 2, 4 | 5 | 6 => 4, 10 | 11 | 12 => 8, 8 => { text(f)?; return Some(()) }
            9 => { let inner = u32_(f)?; let count = u64_(f)?; for _ in 0..count { skip(f, inner)?; } return Some(()) }
            _ => return None };
        std::io::copy(&mut f.take(n), &mut std::io::sink()).ok()?;
        Some(())
    }
    let mut magic = [0u8; 4];
    f.read_exact(&mut magic).ok()?;
    if &magic != b"GGUF" { return None; }
    let _version = u32_(&mut f)?;
    let _tensors = u64_(&mut f)?;
    let kvs = u64_(&mut f)?;
    for _ in 0..kvs {
        let key = text(&mut f)?;
        let ty = u32_(&mut f)?;
        if key.ends_with(".block_count") && (ty == 4 || ty == 5) {
            return u32_(&mut f);
        }
        if key.ends_with(".block_count") && (ty == 10 || ty == 11) {
            return u64_(&mut f).map(|v| v as u32);
        }
        skip(&mut f, ty)?;
    }
    None
}

fn file_size(p: &std::path::Path) -> Option<u64> {
    std::fs::metadata(p).ok().filter(|m| m.is_file()).map(|m| m.len())
}

/// The model sizes this node would load from `models_dir`, counting only what mixed mode puts on the graphics card.
pub fn sizes(models_dir: &std::path::Path, chat: Option<&std::path::Path>) -> crate::runon::Sizes {
    let chat_path = chat.map(|p| p.to_path_buf()).unwrap_or_else(|| default_chat_path(models_dir));
    crate::runon::Sizes {
        chat: file_size(&chat_path),
        search: file_size(&models_dir.join(embed_file())),
        speech: file_size(&models_dir.join(speech_file())),
        photos: find_projector(&chat_path).and_then(|p| file_size(&p)),
    }
}

/// What mixed mode would pick on this machine right now, with the reason.
pub fn suggestion(models_dir: &std::path::Path, s: &crate::runon::Setting) -> (crate::runon::Split, String) {
    crate::runon::suggest(crate::runon::free_vram(), s, &sizes(models_dir, None))
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

        use crate::models_dl::RunOn;
        let mode = opts.run_on.mode;
        let embed_path = opts.embed_model.clone().unwrap_or_else(|| opts.models_dir.join(embed_file()));
        let chat_path = opts.chat_model.clone().unwrap_or_else(|| default_chat_path(&opts.models_dir));
        let projector = if chat_path.is_file() { find_projector(&chat_path) } else { None };
        // Where each model goes. Mixed mode fills in what the setting leaves open from the free graphics memory.
        let mut z = sizes(&opts.models_dir, Some(&chat_path));
        z.search = file_size(&embed_path);
        // Auto becomes a mixed setting with every choice made from the free graphics memory.
        let free = if matches!(mode, RunOn::Auto | RunOn::Mixed) { crate::runon::free_vram() } else { None };
        let run_on = match (mode, free) {
            (RunOn::Auto, Some(f)) => {
                let s = crate::runon::Setting { speakers_gpu: opts.run_on.speakers_gpu, ..crate::runon::auto_all(f, &z) };
                tracing::info!("auto: {} ({:.1} GB of graphics memory is free)", s.words(), f as f64 / 1073741824.0);
                s
            }
            (RunOn::Auto, None) => {
                tracing::info!("auto: the free graphics memory could not be read, so the models try the graphics card first");
                crate::runon::Setting { mode: RunOn::Gpu, ..opts.run_on }
            }
            _ => opts.run_on,
        };
        let mode = run_on.mode;
        let split = (mode == RunOn::Mixed).then(|| {
            let (sp, why) = crate::runon::suggest(free, &run_on, &z);
            tracing::info!("mixed mode: {why}");
            sp
        });
        let speech_gpu = match mode { RunOn::Cpu => false, RunOn::Mixed => run_on.speech_gpu.unwrap_or(true), _ => true };
        let photos_gpu = match mode { RunOn::Cpu => false, RunOn::Mixed => run_on.photos_gpu.unwrap_or(true), _ => true };
        let plan = |share: Option<u8>, path: &std::path::Path| -> (Vec<u32>, Option<u32>) {
            match (mode, share) {
                (RunOn::Gpu, _) => (tiers(false), None),
                (RunOn::Cpu, _) => (tiers(true), None),
                (RunOn::Mixed, Some(s)) => {
                    let n = if s > 0 && s < 100 { layer_count(path) } else { None };
                    (share_tiers(s, n), n)
                }
                (RunOn::Mixed, None) | (RunOn::Auto, _) => (tiers(false), None),
            }
        };
        let (embed_tiers, embed_layers) = plan(split.map(|s| s.search), &embed_path);
        let (chat_tiers, chat_layers) = plan(split.map(|s| s.chat), &chat_path);
        let mut embed_on: Option<u32> = None;
        let mut chat_on: Option<u32> = None;
        let embed = if embed_path.is_file() {
            let mut found = None;
            for layers in embed_tiers {
                let cfg = crate::engine::embed::EmbedConfig { model_path: embed_path.clone(), mmproj_path: None, gpu_layers: layers, use_gpu: layers > 0, threads: threads() };
                match crate::engine::embed::spawn(backend.clone(), cfg) {
                    Ok(h) => {
                        tracing::info!("search model loaded ({} GPU layers)", layers);
                        embed_on = Some(layers);
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

        let llm = if chat_path.is_file() {
            let mut found = None;
            match &projector {
                Some(p) => tracing::info!("vision projector {} found, this node can read photos", p.display()),
                None => tracing::info!("no vision projector next to the chat model, this node does not read photos"),
            }
            for layers in chat_tiers {
                let cfg = crate::engine::llm::LlmConfig { model_path: chat_path.clone(), mmproj_path: projector.clone(), gpu_layers: layers, use_gpu: photos_gpu && (layers > 0 || mode == RunOn::Mixed), n_ctx: opts.ctx, threads: threads() };
                match crate::engine::llm::spawn(backend.clone(), cfg) {
                    Ok(h) => {
                        tracing::info!("chat model loaded ({} GPU layers)", layers);
                        chat_on = Some(layers);
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

        let speech_path = opts.speech_model.clone().unwrap_or_else(|| opts.models_dir.join(speech_file()));
        let whisper = if opts.no_speech {
            None
        } else if speech_path.is_file() {
            let mut found = None;
            // The graphics card first, then the processor.
            for gpu in if speech_gpu { vec![true, false] } else { vec![false] } {
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
        let side = |layers: Option<u32>, total: Option<u32>| match (layers, total) {
            (None, _) => serde_json::Value::Null,
            (Some(0), _) => serde_json::json!({"on": "cpu"}),
            (Some(l), Some(n)) if l < n => serde_json::json!({"on": "mixed", "gpu_layers": l, "layers": n, "percent": l * 100 / n.max(1)}),
            (Some(_), _) => serde_json::json!({"on": "gpu"}),
        };
        let placement = serde_json::json!({
            "mode": opts.run_on.mode.name(),
            "setting": opts.run_on.line(),
            "chat": side(chat_on, chat_layers),
            "search": side(embed_on, embed_layers),
            "speech": whisper.as_ref().map(|w| serde_json::json!({"on": if w.on_gpu() { "gpu" } else { "cpu" }})),
            "photos": (llm.is_some() && projector.is_some()).then(|| serde_json::json!({"on": if photos_gpu { "gpu" } else { "cpu" }})),
        });
        tracing::info!("models placed: {placement}");
        let host = Host {
            _backend: backend,
            placement,
            models_dir: opts.models_dir.clone(),
            speakers: crate::speakers::Speakers::new(opts.models_dir.clone(), opts.run_on.speakers_gpu.unwrap_or(false)),
            vision: llm.is_some() && projector.is_some(),
            vision_model: projector.as_ref().filter(|_| llm.is_some()).and_then(|p| p.file_stem()).map(|s| s.to_string_lossy().to_string()),
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
        let cleared = crate::speakers::clear_leftovers(&std::env::temp_dir());
        if cleared > 0 {
            tracing::info!("removed {cleared} audio or photo file(s) that an earlier run left in the temporary folder");
        }
        match host.speakers.why_not() {
            None => tracing::info!("this node can tell speakers apart for phones"),
            Some(why) => tracing::info!("this node does not tell speakers apart: {why}"),
        }
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

#[cfg(test)]
mod layer_tests {
    use super::*;

    #[test]
    fn shares_become_layer_counts() {
        assert_eq!(share_tiers(0, Some(35)), vec![0]);
        assert_eq!(share_tiers(100, Some(35)), vec![999, 0]);
        assert_eq!(share_tiers(45, Some(35)), vec![16, 0]);
        assert_eq!(share_tiers(1, Some(35)), vec![1, 0]);
        assert_eq!(share_tiers(70, None), vec![999, 0]);
        assert_eq!(share_tiers(30, None), vec![0]);
    }

    #[test]
    fn layer_count_reads_a_gguf_header() {
        // A minimal header: two keys, an array to skip, then the block count.
        let mut b: Vec<u8> = b"GGUF".to_vec();
        b.extend(3u32.to_le_bytes());
        b.extend(0u64.to_le_bytes());
        b.extend(3u64.to_le_bytes());
        let key = |b: &mut Vec<u8>, k: &str| { b.extend((k.len() as u64).to_le_bytes()); b.extend(k.as_bytes()); };
        key(&mut b, "general.architecture"); b.extend(8u32.to_le_bytes()); key(&mut b, "gemma4");
        key(&mut b, "tokenizer.ggml.scores"); b.extend(9u32.to_le_bytes()); b.extend(6u32.to_le_bytes()); b.extend(2u64.to_le_bytes()); b.extend([0u8; 8]);
        key(&mut b, "gemma4.block_count"); b.extend(4u32.to_le_bytes()); b.extend(35u32.to_le_bytes());
        let p = std::env::temp_dir().join(format!("marginalia-layers-{}.gguf", std::process::id()));
        std::fs::write(&p, &b).unwrap();
        assert_eq!(layer_count(&p), Some(35));
        std::fs::write(&p, b"nope").unwrap();
        assert_eq!(layer_count(&p), None);
        let _ = std::fs::remove_file(&p);
    }
}
