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

/// What to transcribe with one call, besides the audio.
#[derive(Default, Clone, Copy)]
pub struct Job<'a> {
    /// ISO code, or None to detect it.
    pub language: Option<&'a str>,
    /// What was said just before this audio. Only used to steer spelling and names, never returned.
    pub prompt: Option<&'a str>,
    /// A short audio window for live correction: decoded for speed, with its own settings.
    pub window: bool,
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
    // Debug and info lines are dropped on purpose: whisper.cpp prints the decoder's prompt tokens at debug
    // level, and a prompt is somebody's words.
    match level {
        ws::GGML_LOG_LEVEL_ERROR => tracing::warn!(target: "whisper", "{msg}"),
        ws::GGML_LOG_LEVEL_WARN => tracing::debug!(target: "whisper", "{msg}"),
        _ => {}
    }
}

/// Windows of live audio are decoded greedily unless measured otherwise (see `compute-node/STATUS.md`).
const WINDOW_BEAM: i32 = 1;
/// The most prompt tokens given to the decoder. Whisper itself would take at most 223.
const PROMPT_TOKENS: usize = 200;
/// The most prompt characters looked at.
const PROMPT_CHARS: usize = 800;

fn env_beam(name: &str) -> Option<i32> {
    std::env::var(name).ok().and_then(|v| v.parse::<i32>().ok())
}

/// Shapes a prompt for whisper: the last `PROMPT_CHARS` characters starting at a whole word, with a space
/// in front as OpenAI's tool does. None when it is empty.
pub fn prompt_text(prompt: &str) -> Option<String> {
    let t = prompt.split_whitespace().collect::<Vec<_>>().join(" ");
    if t.is_empty() {
        return None;
    }
    let count = t.chars().count();
    let t = if count > PROMPT_CHARS {
        let cut: String = t.chars().skip(count - PROMPT_CHARS).collect();
        let at_word_start = t.chars().nth(count - PROMPT_CHARS - 1) == Some(' ');
        // The first word may be cut in half: drop it, unless it is all there is.
        match cut.split_once(' ') {
            Some((_, rest)) if !at_word_start && !rest.is_empty() => rest.to_string(),
            _ => cut,
        }
    } else {
        t
    };
    Some(format!(" {t}"))
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

    /// Beam size for a whole recording. A beam of 5 costs about 10% of the speed on the graphics card.
    /// On a processor it costs more, so it decodes greedily there. MARGINALIA_SPEECH_BEAM overrides it, for measuring.
    fn batch_beam(&self) -> i32 {
        env_beam("MARGINALIA_SPEECH_BEAM").unwrap_or(if self.use_gpu { 5 } else { 1 })
    }

    /// Beam size for a short window, where the wait for the answer matters more than the last bit of accuracy.
    /// MARGINALIA_SPEECH_WINDOW_BEAM overrides it, for measuring.
    fn window_beam(&self) -> i32 {
        env_beam("MARGINALIA_SPEECH_WINDOW_BEAM").unwrap_or(WINDOW_BEAM)
    }

    /// The tokens of the last part of a prompt: at most about 800 characters and 200 tokens, cut at a word.
    /// Empty when there is nothing to steer with.
    fn prompt_tokens(&self, prompt: &str) -> Vec<ws::whisper_token> {
        let Some(text) = prompt_text(prompt) else { return Vec::new() };
        let Ok(c) = CString::new(text) else { return Vec::new() };
        let mut tokens: Vec<ws::whisper_token> = vec![0; 1024];
        let mut n = unsafe { ws::whisper_tokenize(self.ctx, c.as_ptr(), tokens.as_mut_ptr(), tokens.len() as i32) };
        if n < 0 {
            tokens = vec![0; (-n) as usize];
            n = unsafe { ws::whisper_tokenize(self.ctx, c.as_ptr(), tokens.as_mut_ptr(), tokens.len() as i32) };
        }
        if n <= 0 {
            return Vec::new();
        }
        tokens.truncate(n as usize);
        if tokens.len() > PROMPT_TOKENS {
            tokens.drain(..tokens.len() - PROMPT_TOKENS);
        }
        tokens
    }

    /// Transcribes a whole recording (16 kHz mono samples between -1 and 1). `language` None detects it.
    ///
    /// `on_segment` is called for each segment as soon as whisper has finished it, `on_progress` with the
    /// percent done. When `cancel` becomes true whisper stops at the next safe point and the call returns an error.
    pub fn transcribe(
        &self,
        pcm: &[f32],
        job: Job<'_>,
        cancel: &AtomicBool,
        on_segment: &mut dyn FnMut(Segment),
        on_progress: &mut dyn FnMut(i32),
    ) -> Result<Outcome> {
        // Whisper wants at least a second of audio.
        let real_ms = pcm.len() as u64 / 16;
        // A window with nothing but a quiet room in it has no words: say so without waking the model.
        if job.window && is_quiet(pcm) {
            let language = job.language.map(str::trim).filter(|l| !l.is_empty() && *l != "auto").unwrap_or("en").to_ascii_lowercase();
            return Ok(Outcome { language, segments: 0 });
        }
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

            let wanted = job.language.map(str::trim).filter(|l| !l.is_empty() && *l != "auto").map(str::to_ascii_lowercase);
            let lang = if !self.multilingual {
                "en".to_string()
            } else if let Some(l) = wanted.filter(|l| CString::new(l.as_str()).map(|c| ws::whisper_lang_id(c.as_ptr()) >= 0).unwrap_or(false)) {
                l
            } else {
                self.detect(state, pcm).unwrap_or_else(|| "auto".to_string())
            };
            let lang_c = CString::new(lang.clone())?;

            let mut hooks = Hooks { prompt_words: job.prompt.and_then(words_of), audio: pcm, audio_ms: real_ms, ctx: self.ctx, state, on_segment, on_progress, cancel, last: None, last_t1_ms: 0, repeats: 0, count: 0 };
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
            let beam = if job.window { self.window_beam() } else { self.batch_beam() };
            if beam > 1 {
                p.strategy = ws::WHISPER_SAMPLING_BEAM_SEARCH;
                p.beam_search.beam_size = beam.min(8);
            } else {
                p.greedy.best_of = 1;
            }
            if job.window {
                // A short window is the tail of a longer talk: no spoken-language guessing from a few
                // seconds that may start in the middle of a word, and no restart at a hotter temperature
                // that could take several times longer than the answer is worth.
                p.temperature_inc = 0.0;
            }
            // The prompt text is turned into tokens here and the buffer lives until the call returns.
            let prompt_tokens = job.prompt.map(|t| self.prompt_tokens(t)).unwrap_or_default();
            if !prompt_tokens.is_empty() {
                p.prompt_tokens = prompt_tokens.as_ptr();
                p.prompt_n_tokens = prompt_tokens.len() as i32;
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
    /// The prompt as lower case words, to spot the model reading it back. Never logged.
    prompt_words: Option<String>,
    /// The audio that is being transcribed, to check that a segment has sound under it.
    audio: &'a [f32],
    audio_ms: u64,
    ctx: *mut ws::whisper_context,
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
        let n_tok = ws::whisper_full_n_tokens_from_state(state, i);
        let (mut sum, mut cnt) = (0f32, 0u32);
        for k in 0..n_tok {
            let td = ws::whisper_full_get_token_data_from_state(state, i, k);
            if td.id < ws::whisper_token_eot(h.ctx) {
                sum += td.p;
                cnt += 1;
            }
        }
        let mean_p = if cnt == 0 { 1.0 } else { sum / cnt as f32 };
        let t0 = ws::whisper_full_get_segment_t0_from_state(state, i).max(0) as u64 * 10;
        let t1 = ws::whisper_full_get_segment_t1_from_state(state, i).max(0) as u64 * 10;
        // Times are inside the audio that was sent (whisper rounds up to its 30 second windows).
        let t0 = t0.min(h.audio_ms);
        let t1 = t1.min(h.audio_ms);
        let from = (t0 as usize * 16).min(h.audio.len());
        let to = (t1 as usize * 16).min(h.audio.len());
        // Numbers only in the log, never the text.
        let kept = if is_quiet(&h.audio[from..to.max(from)]) {
            None
        } else {
            clean(&raw, no_speech).filter(|t| !is_noise_guess(t, mean_p) && !echoes_prompt(t, h.prompt_words.as_deref(), mean_p))
        };
        let Some(text) = kept else {
            tracing::debug!(target: "whisper", "dropped a segment ({} characters, no speech probability {:.2}, mean token p {:.2})", raw.len(), no_speech, mean_p);
            continue;
        };
        tracing::debug!(target: "whisper", "segment kept: {} characters, mean token p {:.2}", text.len(), mean_p);
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

/// Frames of 50 ms are compared against this loudness (root mean square, full scale is 1.0).
/// About minus 50 dB: a quiet room, not a quiet voice.
const QUIET_RMS: f32 = 0.003;

/// True when no stretch of the audio is louder than a quiet room. Whisper makes up "Thank you." over such stretches.
/// Speech has loud 50 ms frames even when it is soft, so the 95th percentile of the frames decides.
pub fn is_quiet(audio: &[f32]) -> bool {
    if audio.is_empty() {
        return true;
    }
    let mut rms: Vec<f32> = audio.chunks(800).map(|c| (c.iter().map(|x| x * x).sum::<f32>() / c.len() as f32).sqrt()).collect();
    rms.sort_by(|a, b| a.total_cmp(b));
    rms[(rms.len() * 95 / 100).min(rms.len() - 1)] < QUIET_RMS
}

/// A short scrap of text the model was unsure of word by word: what it makes up out of noise ("So,").
fn is_noise_guess(text: &str, mean_token_p: f32) -> bool {
    text.split_whitespace().count() <= 3 && mean_token_p < 0.5
}

/// The text as lower case words with single spaces and no punctuation, with a space at both ends. None when empty.
fn words_of(text: &str) -> Option<String> {
    let w: Vec<String> = text.split_whitespace().map(|w| w.chars().filter(|c| c.is_alphanumeric()).collect::<String>().to_lowercase()).filter(|w| !w.is_empty()).collect();
    (!w.is_empty()).then(|| format!(" {} ", w.join(" ")))
}

/// Over noise the model tends to read the end of its prompt back ("Thank you."). A short scrap that is
/// found in the prompt and was not said with full confidence is taken for that. Long text is never dropped here.
fn echoes_prompt(text: &str, prompt_words: Option<&str>, mean_token_p: f32) -> bool {
    let (Some(prompt), Some(seg)) = (prompt_words, words_of(text)) else { return false };
    seg.split_whitespace().count() <= 4 && mean_token_p < 0.95 && prompt.contains(&seg)
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
    use super::{clean, echoes_prompt, is_noise_guess, is_quiet, prompt_text, words_of};

    #[test]
    fn prompt_echo() {
        let p = words_of("We met Kennedy. Thank you all for joining today.");
        assert!(echoes_prompt("Thank you.", p.as_deref(), 0.8));
        assert!(!echoes_prompt("Thank you.", p.as_deref(), 0.99));
        assert!(!echoes_prompt("Thank you.", None, 0.8));
        assert!(!echoes_prompt("Hello there.", p.as_deref(), 0.5));
        // A word that is only part of a longer word is not a match.
        assert!(!echoes_prompt("join", p.as_deref(), 0.5));
        assert!(!echoes_prompt("We met Kennedy and thank you all for joining today.", p.as_deref(), 0.5));
    }

    #[test]
    fn quiet_audio() {
        assert!(is_quiet(&[]));
        assert!(is_quiet(&vec![0.0; 16_000]));
        assert!(is_quiet(&vec![0.001; 16_000]));
        // A soft voice: bursts at 0.02 inside a quiet room.
        let mut v = vec![0.001f32; 32_000];
        for x in v[8_000..16_000].iter_mut() {
            *x = 0.02;
        }
        assert!(!is_quiet(&v));
        // One click in two seconds is not speech.
        let mut v = vec![0.0f32; 32_000];
        v[100] = 0.9;
        assert!(is_quiet(&v));
    }

    #[test]
    fn noise_guesses() {
        assert!(is_noise_guess("So,", 0.2));
        assert!(!is_noise_guess("So,", 0.9));
        assert!(!is_noise_guess("We should ship the new build on Friday", 0.2));
    }

    #[test]
    fn prompt_shaping() {
        assert_eq!(prompt_text(""), None);
        assert_eq!(prompt_text("  \n "), None);
        assert_eq!(prompt_text("Hello   there\nfriend").as_deref(), Some(" Hello there friend"));
        let long = "word ".repeat(400);
        let t = prompt_text(&long).unwrap();
        assert!(t.chars().count() <= 801 + 1);
        assert!(t.starts_with(" word"));
        // The last words are the ones kept.
        let numbered: String = (0..500).map(|i| format!("w{i} ")).collect();
        let t = prompt_text(&numbered).unwrap();
        assert!(t.trim_end().ends_with("w499"));
        assert!(t.chars().count() <= 801);
        assert!(t.trim_start().starts_with('w'));
        // Multi-byte text is cut on characters, not bytes.
        let hindi = "नमस्ते दुनिया ".repeat(100);
        assert!(prompt_text(&hindi).unwrap().chars().count() <= 801);
    }

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
