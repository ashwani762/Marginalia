//! whisper.cpp transcription worker (in-process, shares ggml with llama.cpp).

use super::whisper_sys as ws;
use anyhow::{anyhow, Result};
use crossbeam_channel::{bounded, unbounded, Receiver, Sender};
use std::ffi::{CStr, CString};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Instant;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum AudioSource {
    Microphone,
    System,
}

impl AudioSource {
    pub fn label(self) -> &'static str {
        match self {
            AudioSource::Microphone => "You",
            AudioSource::System => "Others",
        }
    }
}

/// A voiced span of 16 kHz mono audio with timestamps relative to session start.
pub struct AudioSegment {
    pub source: AudioSource,
    pub pcm: Vec<f32>,
    pub t0: f64,
    pub t1: f64,
    /// In-progress speech; the text is provisional and will be replaced.
    pub partial: bool,
}

#[derive(Clone, Debug)]
pub struct Transcript {
    pub source: AudioSource,
    pub t0: f64,
    pub t1: f64,
    pub text: String,
    pub partial: bool,
}

pub struct WhisperConfig {
    pub model_path: PathBuf,
    pub use_gpu: bool,
    pub language: String,
    pub threads: i32,
}

/// One transcribed segment from a standalone file/buffer.
#[derive(Clone, Debug)]
pub struct Utterance {
    pub t0: f64,
    pub t1: f64,
    pub text: String,
}

enum Job {
    Segment(AudioSegment),
    /// Transcribe a whole buffer (voice memo / audio file) and reply with all segments.
    Batch { pcm: Vec<f32>, reply: Sender<Result<Vec<Utterance>>> },
    SetLanguage(String),
    Shutdown,
}

#[derive(Clone)]
pub struct WhisperHandle {
    tx: Sender<Job>,
    pub seconds_transcribed: Arc<AtomicU64>,
    join: Arc<parking_lot::Mutex<Option<std::thread::JoinHandle<()>>>>,
}

impl WhisperHandle {
    pub fn transcribe(&self, seg: AudioSegment) {
        let _ = self.tx.send(Job::Segment(seg));
    }
    /// Blocking transcription of a complete 16 kHz mono buffer.
    pub fn transcribe_buffer(&self, pcm: Vec<f32>) -> Result<Vec<Utterance>> {
        let (reply, rx) = bounded(1);
        self.tx.send(Job::Batch { pcm, reply }).map_err(|_| anyhow!("whisper worker is gone"))?;
        rx.recv().map_err(|_| anyhow!("whisper worker dropped reply"))?
    }
    pub fn set_language(&self, lang: String) {
        let _ = self.tx.send(Job::SetLanguage(lang));
    }
    pub fn shutdown(&self) {
        let _ = self.tx.send(Job::Shutdown);
        if let Some(j) = self.join.lock().take() {
            let _ = j.join();
        }
    }
    pub fn queue_len(&self) -> usize {
        self.tx.len()
    }
}

/// Owned whisper context + state. Only touched from the worker thread.
struct Ctx {
    ctx: *mut ws::whisper_context,
    state: *mut ws::whisper_state,
    multilingual: bool,
}

impl Drop for Ctx {
    fn drop(&mut self) {
        unsafe {
            if !self.state.is_null() {
                ws::whisper_free_state(self.state);
            }
            if !self.ctx.is_null() {
                ws::whisper_free(self.ctx);
            }
        }
    }
}

unsafe extern "C" fn quiet_log(level: ws::ggml_log_level, text: *const std::os::raw::c_char, _user: *mut std::os::raw::c_void) {
    if text.is_null() {
        return;
    }
    let msg = CStr::from_ptr(text).to_string_lossy();
    let msg = msg.trim_end();
    if msg.is_empty() {
        return;
    }
    match level {
        ws::GGML_LOG_LEVEL_ERROR => tracing::warn!(target: "whisper", "{msg}"),
        ws::GGML_LOG_LEVEL_WARN => tracing::debug!(target: "whisper", "{msg}"),
        _ => tracing::trace!(target: "whisper", "{msg}"),
    }
}

fn load(cfg: &WhisperConfig) -> Result<Ctx> {
    unsafe {
        ws::whisper_log_set(Some(quiet_log), std::ptr::null_mut());
        let mut params = ws::whisper_context_default_params();
        params.use_gpu = cfg.use_gpu;
        params.flash_attn = cfg.use_gpu;
        let path = CString::new(cfg.model_path.to_string_lossy().as_bytes())?;
        let ctx = ws::whisper_init_from_file_with_params_no_state(path.as_ptr(), params);
        if ctx.is_null() {
            return Err(anyhow!("failed to load whisper model {}", cfg.model_path.display()));
        }
        let state = ws::whisper_init_state(ctx);
        if state.is_null() {
            ws::whisper_free(ctx);
            return Err(anyhow!("failed to create whisper state"));
        }
        let multilingual = ws::whisper_is_multilingual(ctx) != 0;
        Ok(Ctx { ctx, state, multilingual })
    }
}

pub fn spawn(cfg: WhisperConfig, out: Sender<Transcript>) -> Result<WhisperHandle> {
    let (tx, rx) = unbounded::<Job>();
    let (ready_tx, ready_rx) = bounded::<Result<()>>(1);
    let seconds = Arc::new(AtomicU64::new(0));
    let seconds2 = seconds.clone();
    let join = std::thread::Builder::new()
        .name("whisper-worker".into())
        .spawn(move || {
            let ctx = match load(&cfg) {
                Ok(c) => c,
                Err(e) => {
                    let _ = ready_tx.send(Err(e));
                    return;
                }
            };
            let _ = ready_tx.send(Ok(()));
            tracing::info!("whisper ready (gpu={}, multilingual={})", cfg.use_gpu, ctx.multilingual);
            let mut language = cfg.language.clone();
            run_loop(ctx, rx, out, &mut language, cfg.threads, seconds2);
        })?;
    ready_rx.recv().map_err(|_| anyhow!("whisper worker died during load"))??;
    Ok(WhisperHandle { tx, seconds_transcribed: seconds, join: Arc::new(parking_lot::Mutex::new(Some(join))) })
}

fn run_loop(ctx: Ctx, rx: Receiver<Job>, out: Sender<Transcript>, language: &mut String, threads: i32, seconds: Arc<AtomicU64>) {
    while let Ok(first) = rx.recv() {
        // Drain the queue: finals first; keep only the newest partial per source.
        let mut jobs = vec![first];
        while let Ok(j) = rx.try_recv() {
            jobs.push(j);
        }
        let mut finals = Vec::new();
        let mut partial_mic: Option<Job> = None;
        let mut partial_sys: Option<Job> = None;
        for j in jobs {
            match &j {
                Job::Segment(seg) if seg.partial => match seg.source {
                    AudioSource::Microphone => partial_mic = Some(j),
                    AudioSource::System => partial_sys = Some(j),
                },
                _ => finals.push(j),
            }
        }
        finals.extend(partial_mic);
        finals.extend(partial_sys);
        for job in finals {
        match job {
            Job::Shutdown => return,
            Job::SetLanguage(l) => *language = l,
            Job::Batch { pcm, reply } => {
                let res = transcribe(&ctx, pcm, language, threads);
                let _ = reply.send(res);
            }
            Job::Segment(seg) => {
                if seg.pcm.is_empty() {
                    let _ = out.send(Transcript { source: seg.source, t0: seg.t0, t1: seg.t1, text: String::new(), partial: false });
                    continue;
                }
                let started = Instant::now();
                let dur = seg.t1 - seg.t0;
                let utterances = match transcribe_with(&ctx, seg.pcm, language, threads, seg.partial) {
                    Ok(u) => u,
                    Err(e) => {
                        tracing::warn!("whisper failed: {e:#}");
                        continue;
                    }
                };
                if !seg.partial {
                    seconds.fetch_add(dur.max(0.0).round() as u64, Ordering::Relaxed);
                }
                let text = utterances.iter().map(|u| u.text.as_str()).collect::<Vec<_>>().join(" ");
                let text = clean_transcript(&text);
                tracing::debug!("whisper{}: {:.1}s audio in {:.0} ms -> {} chars", if seg.partial { " (partial)" } else { "" }, dur, started.elapsed().as_secs_f64() * 1000.0, text.len());
                // An empty final still clears the provisional line.
                let _ = out.send(Transcript { source: seg.source, t0: seg.t0, t1: seg.t1, text, partial: seg.partial });
            }
        }
        }
    }
}

fn transcribe(ctx: &Ctx, pcm: Vec<f32>, language: &str, threads: i32) -> Result<Vec<Utterance>> {
    transcribe_with(ctx, pcm, language, threads, false)
}

/// `fast` trades a little accuracy for latency (provisional captions).
fn transcribe_with(ctx: &Ctx, mut pcm: Vec<f32>, language: &str, threads: i32, fast: bool) -> Result<Vec<Utterance>> {
    // whisper wants at least ~1 s of audio.
    let min_len = 16_000 + 1_600;
    if pcm.len() < min_len {
        pcm.resize(min_len, 0.0);
    }
    let lang = if ctx.multilingual {
        if language.is_empty() { "auto".to_string() } else { language.to_string() }
    } else {
        "en".to_string()
    };
    let lang_c = CString::new(lang)?;
    unsafe {
        let mut p = ws::whisper_full_default_params(ws::WHISPER_SAMPLING_GREEDY);
        p.n_threads = threads.max(1);
        p.translate = false;
        p.no_context = true;
        p.single_segment = false;
        p.print_special = false;
        p.print_progress = false;
        p.print_realtime = false;
        p.print_timestamps = false;
        p.suppress_blank = true;
        p.suppress_nst = true;
        p.no_speech_thold = 0.6;
        p.temperature_inc = if fast { 0.0 } else { 0.2 };
        if fast {
            // Provisional captions: one segment, no temperature fallback retries.
            p.single_segment = true;
            p.no_speech_thold = 0.5;
        }
        p.language = lang_c.as_ptr();
        p.detect_language = false;
        p.greedy.best_of = 1;
        let rc = ws::whisper_full_with_state(ctx.ctx, ctx.state, p, pcm.as_ptr(), pcm.len() as i32);
        if rc != 0 {
            return Err(anyhow!("whisper_full failed with code {rc}"));
        }
        let n = ws::whisper_full_n_segments_from_state(ctx.state);
        let mut out = Vec::with_capacity(n.max(0) as usize);
        for i in 0..n {
            if ws::whisper_full_get_segment_no_speech_prob_from_state(ctx.state, i) > 0.75 {
                continue;
            }
            let t = ws::whisper_full_get_segment_text_from_state(ctx.state, i);
            if t.is_null() {
                continue;
            }
            let text = CStr::from_ptr(t).to_string_lossy().trim().to_string();
            if text.is_empty() {
                continue;
            }
            let t0 = ws::whisper_full_get_segment_t0_from_state(ctx.state, i) as f64 / 100.0;
            let t1 = ws::whisper_full_get_segment_t1_from_state(ctx.state, i) as f64 / 100.0;
            out.push(Utterance { t0, t1, text });
        }
        Ok(out)
    }
}

/// Drop whisper's typical non-speech hallucinations.
pub fn clean_transcript(text: &str) -> String {
    let t = text.trim();
    let lowered = t.to_ascii_lowercase();
    const JUNK: [&str; 10] = [
        "[blank_audio]",
        "[music]",
        "(music)",
        "[applause]",
        "[laughter]",
        "(laughs)",
        "thank you.",
        "thanks for watching",
        "subtitles by",
        "♪",
    ];
    if lowered.is_empty() || JUNK.iter().any(|j| lowered == *j) {
        return String::new();
    }
    if t.chars().all(|c| !c.is_alphanumeric()) {
        return String::new();
    }
    let mut out = String::with_capacity(t.len());
    let mut depth = 0i32;
    for c in t.chars() {
        match c {
            '[' | '(' => depth += 1,
            ']' | ')' => depth = (depth - 1).max(0),
            _ if depth == 0 => out.push(c),
            _ => {}
        }
    }
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}
