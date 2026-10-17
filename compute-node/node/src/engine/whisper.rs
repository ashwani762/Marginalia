//! Speech to text on whole recordings, with whisper.cpp. A large model is loaded once and stays loaded.
//! Each job makes its own decoder state (so memory is free when the node is idle), runs whisper's own
//! sliding 30 second windows over the audio and reports every segment as soon as it is final.
//!
//! Nothing is kept: the audio, the segments and the text exist only while a job runs.

use super::whisper_sys as ws;
use anyhow::{anyhow, Result};
use std::ffi::{c_void, CStr, CString};
use std::os::raw::c_char;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};

pub struct WhisperConfig {
    pub model_path: PathBuf,
    pub use_gpu: bool,
    pub threads: i32,
}

/// One finished stretch of speech. Times are in milliseconds from the start of the audio.
#[derive(Clone, Debug)]
pub struct Segment {
    pub t0_ms: u64,
    pub t1_ms: u64,
    pub text: String,
}

pub struct Outcome {
    /// Language code the audio was transcribed as.
    pub language: String,
    pub segments: u32,
}

pub struct Whisper {
    ctx: *mut ws::whisper_context,
    multilingual: bool,
    use_gpu: bool,
    threads: i32,
}

// The context is only read after loading. Every job makes its own state, which is what whisper.cpp
// provides for running from several threads.
unsafe impl Send for Whisper {}
unsafe impl Sync for Whisper {}

impl Drop for Whisper {
    fn drop(&mut self) {
        unsafe { ws::whisper_free(self.ctx) };
    }
}

unsafe extern "C" fn quiet_log(level: ws::ggml_log_level, text: *const c_char, _user: *mut c_void) {
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

/// The windows are 30 seconds long, 16 kHz.
const WINDOW: usize = 30 * 16_000;

impl Whisper {
    pub fn load(cfg: &WhisperConfig) -> Result<Whisper> {
        unsafe {
            ws::whisper_log_set(Some(quiet_log), std::ptr::null_mut());
            let mut params = ws::whisper_context_default_params();
            params.use_gpu = cfg.use_gpu;
            params.flash_attn = cfg.use_gpu;
            let path = CString::new(cfg.model_path.to_string_lossy().as_bytes())?;
            let ctx = ws::whisper_init_from_file_with_params_no_state(path.as_ptr(), params);
            if ctx.is_null() {
                return Err(anyhow!("failed to load the speech model {}", cfg.model_path.display()));
            }
            let multilingual = ws::whisper_is_multilingual(ctx) != 0;
            Ok(Whisper { ctx, multilingual, use_gpu: cfg.use_gpu, threads: cfg.threads })
        }
    }

    pub fn on_gpu(&self) -> bool {
        self.use_gpu
    }

    /// Transcribes a whole recording (16 kHz mono samples between -1 and 1). `language` None detects it.
    ///
    /// `on_segment` is called for each segment as soon as whisper has finished it, `on_progress` with the
    /// percent done. When `cancel` becomes true whisper stops at the next safe point and the call returns an error.
    pub fn transcribe(
        &self,
        pcm: &[f32],
        language: Option<&str>,
        cancel: &AtomicBool,
        on_segment: &mut dyn FnMut(Segment),
        on_progress: &mut dyn FnMut(i32),
    ) -> Result<Outcome> {
        // Whisper wants at least a second of audio.
        let padded;
        let pcm = if pcm.len() < 17_600 {
            let mut v = pcm.to_vec();
            v.resize(17_600, 0.0);
            padded = v;
            &padded[..]
        } else {
            pcm
        };
        unsafe {
            let state = ws::whisper_init_state(self.ctx);
            if state.is_null() {
                return Err(anyhow!("could not set up the speech model (is the graphics card out of memory?)"));
            }
            let _free = StateGuard(state);

            let wanted = language.map(str::trim).filter(|l| !l.is_empty() && *l != "auto").map(str::to_ascii_lowercase);
            let lang = if !self.multilingual {
                "en".to_string()
            } else if let Some(l) = wanted.filter(|l| CString::new(l.as_str()).map(|c| ws::whisper_lang_id(c.as_ptr()) >= 0).unwrap_or(false)) {
                l
            } else {
                self.detect(state, pcm).unwrap_or_else(|| "auto".to_string())
            };
            let lang_c = CString::new(lang.clone())?;

            let mut hooks = Hooks { state, on_segment, on_progress, cancel, last: None, last_t1_ms: 0, repeats: 0, count: 0 };
            let mut p = ws::whisper_full_default_params(ws::WHISPER_SAMPLING_GREEDY);
            p.n_threads = self.threads.max(1);
            p.translate = false;
            // Each window is told what came before, as OpenAI's tool does. The fallbacks below stop a
            // loop from carrying on.
            p.no_context = false;
            p.single_segment = false;
            p.print_special = false;
            p.print_progress = false;
            p.print_realtime = false;
            p.print_timestamps = false;
            p.suppress_blank = true;
            p.suppress_nst = true;
            p.no_speech_thold = 0.6;
            p.logprob_thold = -1.0;
            p.entropy_thold = 2.4;
            p.temperature = 0.0;
            p.temperature_inc = 0.2;
            p.language = lang_c.as_ptr();
            p.detect_language = false;
            // A beam of 5 costs about 10% of the speed on the graphics card. On a processor it costs more, so it decodes greedily there.
            // MARGINALIA_SPEECH_BEAM overrides it, for measuring.
            let beam = std::env::var("MARGINALIA_SPEECH_BEAM").ok().and_then(|v| v.parse::<i32>().ok()).unwrap_or(if self.use_gpu { 5 } else { 1 });
            if beam > 1 {
                p.strategy = ws::WHISPER_SAMPLING_BEAM_SEARCH;
                p.beam_search.beam_size = beam.min(8);
            } else {
                p.greedy.best_of = 1;
            }
            let hooks_ptr = &mut hooks as *mut Hooks as *mut c_void;
            p.new_segment_callback = Some(new_segment_cb);
            p.new_segment_callback_user_data = hooks_ptr;
            p.progress_callback = Some(progress_cb);
            p.progress_callback_user_data = hooks_ptr;
            p.abort_callback = Some(abort_cb);
            p.abort_callback_user_data = hooks_ptr;

            let rc = ws::whisper_full_with_state(self.ctx, state, p, pcm.as_ptr(), pcm.len() as i32);
            if cancel.load(Ordering::Relaxed) {
                return Err(anyhow!("cancelled"));
            }
            if rc != 0 {
                return Err(anyhow!("the speech model stopped with code {rc}"));
            }
            let id = ws::whisper_full_lang_id_from_state(state);
            let c = ws::whisper_lang_str(id);
            let language = if lang != "auto" {
                lang
            } else if id >= 0 && !c.is_null() {
                CStr::from_ptr(c).to_string_lossy().to_string()
            } else {
                "en".to_string()
            };
            Ok(Outcome { language, segments: hooks.count })
        }
    }

    /// Picks the language from a few of the loudest 30 second stretches, so a quiet start or a short
    /// greeting in another language does not decide it for the whole meeting. None for short audio,
    /// where whisper's own detection on the first window is as good.
    unsafe fn detect(&self, state: *mut ws::whisper_state, pcm: &[f32]) -> Option<String> {
        if pcm.len() < 2 * WINDOW {
            return None;
        }
        let mut chunks: Vec<(f32, usize)> = pcm
            .chunks(WINDOW)
            .enumerate()
            .filter(|(_, c)| c.len() >= WINDOW / 3)
            .map(|(i, c)| ((c.iter().map(|x| x * x).sum::<f32>() / c.len() as f32).sqrt(), i))
            .collect();
        chunks.sort_by(|a, b| b.0.total_cmp(&a.0));
        chunks.truncate(3);
        let n = ws::whisper_lang_max_id() as usize + 1;
        let mut total = vec![0f32; n];
        let mut probs = vec![0f32; n];
        let mut used = 0;
        for (rms, i) in chunks {
            if rms < 0.002 {
                continue;
            }
            let part = &pcm[i * WINDOW..((i + 1) * WINDOW).min(pcm.len())];
            if ws::whisper_pcm_to_mel_with_state(self.ctx, state, part.as_ptr(), part.len() as i32, self.threads.max(1)) != 0 {
                continue;
            }
            if ws::whisper_lang_auto_detect_with_state(self.ctx, state, 0, self.threads.max(1), probs.as_mut_ptr()) < 0 {
                continue;
            }
            for (t, p) in total.iter_mut().zip(&probs) {
                *t += p;
            }
            used += 1;
        }
        if used == 0 {
            return None;
        }
        let best = total.iter().enumerate().max_by(|a, b| a.1.total_cmp(b.1)).map(|(i, _)| i as i32)?;
        let s = ws::whisper_lang_str(best);
        (!s.is_null()).then(|| CStr::from_ptr(s).to_string_lossy().to_string())
    }
}

struct StateGuard(*mut ws::whisper_state);
impl Drop for StateGuard {
    fn drop(&mut self) {
        unsafe { ws::whisper_free_state(self.0) };
    }
}

/// Hands memory the allocator is holding on to back to the system once a long job is over.
pub fn give_back_memory() {
    #[cfg(all(target_os = "linux", target_env = "gnu"))]
    {
        extern "C" {
            fn malloc_trim(pad: usize) -> i32;
        }
        unsafe { malloc_trim(0) };
    }
}

struct Hooks<'a> {
    state: *mut ws::whisper_state,
    on_segment: &'a mut dyn FnMut(Segment),
    on_progress: &'a mut dyn FnMut(i32),
    cancel: &'a AtomicBool,
    /// The last text sent and how many times in a row it came, to stop a loop.
    last: Option<String>,
    last_t1_ms: u64,
    repeats: u32,
    count: u32,
}

unsafe extern "C" fn new_segment_cb(_ctx: *mut ws::whisper_context, state: *mut ws::whisper_state, n_new: i32, user: *mut c_void) {
    let h = &mut *(user as *mut Hooks);
    let n = ws::whisper_full_n_segments_from_state(state);
    for i in (n - n_new).max(0)..n {
        let t = ws::whisper_full_get_segment_text_from_state(state, i);
        if t.is_null() {
            continue;
        }
        let raw = CStr::from_ptr(t).to_string_lossy().to_string();
        let no_speech = ws::whisper_full_get_segment_no_speech_prob_from_state(state, i);
        let Some(text) = clean(&raw, no_speech) else {
            tracing::debug!(target: "whisper", "dropped a segment ({} characters, no speech probability {:.2})", raw.len(), no_speech);
            continue;
        };
        let t0 = ws::whisper_full_get_segment_t0_from_state(state, i).max(0) as u64 * 10;
        let t1 = ws::whisper_full_get_segment_t1_from_state(state, i).max(0) as u64 * 10;
        // The same line again and again with no gap is the model stuck, not a person talking.
        if h.last.as_deref() == Some(text.as_str()) && t0 < h.last_t1_ms + 1500 {
            h.repeats += 1;
            if h.repeats >= 3 {
                h.last_t1_ms = t1;
                continue;
            }
        } else {
            h.last = Some(text.clone());
            h.repeats = 0;
        }
        h.last_t1_ms = t1;
        h.count += 1;
        (h.on_segment)(Segment { t0_ms: t0, t1_ms: t1.max(t0), text });
    }
    let _ = h.state;
}

unsafe extern "C" fn progress_cb(_ctx: *mut ws::whisper_context, _state: *mut ws::whisper_state, progress: i32, user: *mut c_void) {
    let h = &mut *(user as *mut Hooks);
    (h.on_progress)(progress);
}

unsafe extern "C" fn abort_cb(user: *mut c_void) -> bool {
    let h = &*(user as *const Hooks);
    h.cancel.load(Ordering::Relaxed)
}

/// Removes sound tags, and drops text that is whisper making things up over silence or noise.
pub fn clean(text: &str, no_speech_prob: f32) -> Option<String> {
    let t = text.trim();
    if t.is_empty() || no_speech_prob > 0.75 {
        return None;
    }
    // One character over and over, or letters from scripts nobody was speaking (Canadian syllabics, Ethiopic, Cherokee).
    let mut run = 0usize;
    let mut prev = None;
    for c in t.chars() {
        if Some(c) == prev {
            run += 1;
        } else {
            run = 1;
            prev = Some(c);
        }
        if run >= 8 && !c.is_whitespace() && c != '.' && c != '-' {
            return None;
        }
        let u = c as u32;
        if (0x1200..=0x167F).contains(&u) {
            return None;
        }
    }
    // Sound tags such as [BLANK_AUDIO], (music) or [laughter].
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
    let out = out.split_whitespace().collect::<Vec<_>>().join(" ");
    if out.chars().all(|c| !c.is_alphanumeric()) {
        return None;
    }
    // Stock closing lines that whisper invents over quiet stretches. A real "thank you" has speech around it.
    let low = out.to_ascii_lowercase();
    if no_speech_prob > 0.3 && (low == "thank you." || low == "thanks for watching." || low.starts_with("subtitles by")) {
        return None;
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::clean;

    #[test]
    fn cleaning() {
        assert_eq!(clean(" Hello there. ", 0.01).as_deref(), Some("Hello there."));
        assert_eq!(clean("[BLANK_AUDIO]", 0.1), None);
        assert_eq!(clean("Yes (laughs) really", 0.1).as_deref(), Some("Yes really"));
        assert_eq!(clean("aaaaaaaaaaaa", 0.1), None);
        assert_eq!(clean("Hello", 0.9), None);
        assert_eq!(clean("Thank you.", 0.05).as_deref(), Some("Thank you."));
        assert_eq!(clean("Thank you.", 0.5), None);
        assert_eq!(clean("...", 0.0), None);
    }
}
