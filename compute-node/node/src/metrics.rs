//! What the node can say about its own work: how many jobs, how fast, how often it failed.
//! Only numbers are kept here, never any text, audio or vectors that passed through. The counters
//! live in memory and start again from zero when the node restarts.

use parking_lot::Mutex;
use serde_json::{json, Value};
use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicU64, Ordering::Relaxed};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

#[derive(Default)]
struct PerClient {
    name: String,
    chat_jobs: u64,
    embed_jobs: u64,
    speech_jobs: u64,
    window_jobs: u64,
    last_seen: u64,
}

struct Recent {
    at: u64,
    kind: &'static str,
    ok: bool,
    ms: u64,
    /// Tokens generated, texts embedded, or seconds of audio.
    size: u64,
}

pub struct Metrics {
    started: Instant,
    pub chat_jobs: AtomicU64,
    pub chat_errors: AtomicU64,
    pub chat_prompt_tokens: AtomicU64,
    pub chat_tokens: AtomicU64,
    pub chat_busy_ms: AtomicU64,
    pub embed_jobs: AtomicU64,
    pub embed_errors: AtomicU64,
    pub embed_texts: AtomicU64,
    pub embed_busy_ms: AtomicU64,
    pub speech_jobs: AtomicU64,
    pub speech_errors: AtomicU64,
    pub speech_audio_ms: AtomicU64,
    pub speech_busy_ms: AtomicU64,
    /// Short windows of live audio, counted apart from whole recordings.
    pub window_jobs: AtomicU64,
    pub window_errors: AtomicU64,
    pub window_audio_ms: AtomicU64,
    pub window_busy_ms: AtomicU64,
    pub pairings: AtomicU64,
    pub rejected: AtomicU64,
    clients: Mutex<HashMap<String, PerClient>>,
    recent: Mutex<VecDeque<Recent>>,
}

#[derive(Clone, Copy)]
enum Kind {
    Chat,
    Search,
    Speech,
    Window,
}

fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

impl Metrics {
    pub fn new() -> Metrics {
        Metrics {
            started: Instant::now(),
            chat_jobs: 0.into(),
            chat_errors: 0.into(),
            chat_prompt_tokens: 0.into(),
            chat_tokens: 0.into(),
            chat_busy_ms: 0.into(),
            embed_jobs: 0.into(),
            embed_errors: 0.into(),
            embed_texts: 0.into(),
            embed_busy_ms: 0.into(),
            speech_jobs: 0.into(),
            speech_errors: 0.into(),
            speech_audio_ms: 0.into(),
            speech_busy_ms: 0.into(),
            window_jobs: 0.into(),
            window_errors: 0.into(),
            window_audio_ms: 0.into(),
            window_busy_ms: 0.into(),
            pairings: 0.into(),
            rejected: 0.into(),
            clients: Mutex::new(HashMap::new()),
            recent: Mutex::new(VecDeque::new()),
        }
    }

    pub fn uptime_s(&self) -> u64 {
        self.started.elapsed().as_secs()
    }

    fn touch(&self, fp: &str, name: &str, kind: Kind) {
        let mut g = self.clients.lock();
        let c = g.entry(fp.to_string()).or_default();
        c.name = name.to_string();
        c.last_seen = now();
        match kind {
            Kind::Chat => c.chat_jobs += 1,
            Kind::Search => c.embed_jobs += 1,
            Kind::Speech => c.speech_jobs += 1,
            Kind::Window => c.window_jobs += 1,
        }
    }

    fn remember(&self, kind: &'static str, ok: bool, ms: u64, size: u64) {
        let mut r = self.recent.lock();
        if r.len() >= 30 {
            r.pop_front();
        }
        r.push_back(Recent { at: now(), kind, ok, ms, size });
    }

    pub fn chat_done(&self, fp: &str, name: &str, prompt_tokens: u64, tokens: u64, ms: u64) {
        self.chat_jobs.fetch_add(1, Relaxed);
        self.chat_prompt_tokens.fetch_add(prompt_tokens, Relaxed);
        self.chat_tokens.fetch_add(tokens, Relaxed);
        self.chat_busy_ms.fetch_add(ms, Relaxed);
        self.touch(fp, name, Kind::Chat);
        self.remember("chat", true, ms, tokens);
    }

    pub fn chat_failed(&self, fp: &str, name: &str) {
        self.chat_errors.fetch_add(1, Relaxed);
        self.touch(fp, name, Kind::Chat);
        self.remember("chat", false, 0, 0);
    }

    pub fn embed_done(&self, fp: &str, name: &str, texts: u64, ms: u64, ok: bool) {
        if ok {
            self.embed_jobs.fetch_add(1, Relaxed);
            self.embed_texts.fetch_add(texts, Relaxed);
            self.embed_busy_ms.fetch_add(ms, Relaxed);
        } else {
            self.embed_errors.fetch_add(1, Relaxed);
        }
        self.touch(fp, name, Kind::Search);
        self.remember("search", ok, ms, texts);
    }

    /// A transcription job ended. Only the length of the audio and the time it took are kept.
    pub fn speech_done(&self, fp: &str, name: &str, audio_ms: u64, ms: u64, ok: bool) {
        if ok {
            self.speech_jobs.fetch_add(1, Relaxed);
            self.speech_audio_ms.fetch_add(audio_ms, Relaxed);
            self.speech_busy_ms.fetch_add(ms, Relaxed);
        } else {
            self.speech_errors.fetch_add(1, Relaxed);
        }
        self.touch(fp, name, Kind::Speech);
        self.remember("speech", ok, ms, audio_ms / 1000);
    }

    pub fn speech_finished(&self, window: bool, fp: &str, name: &str, audio_ms: u64, ms: u64) {
        if window {
            self.window_done(fp, name, audio_ms, ms, true)
        } else {
            self.speech_done(fp, name, audio_ms, ms, true)
        }
    }

    pub fn speech_failed(&self, window: bool, fp: &str, name: &str, audio_ms: u64) {
        if window {
            self.window_done(fp, name, audio_ms, 0, false)
        } else {
            self.speech_done(fp, name, audio_ms, 0, false)
        }
    }

    /// A short window of live audio ended. Counted apart from whole recordings so it does not bend their speed figures.
    pub fn window_done(&self, fp: &str, name: &str, audio_ms: u64, ms: u64, ok: bool) {
        if ok {
            self.window_jobs.fetch_add(1, Relaxed);
            self.window_audio_ms.fetch_add(audio_ms, Relaxed);
            self.window_busy_ms.fetch_add(ms, Relaxed);
        } else {
            self.window_errors.fetch_add(1, Relaxed);
        }
        self.touch(fp, name, Kind::Window);
        self.remember("window", ok, ms, audio_ms / 1000);
    }

    pub fn snapshot(&self) -> Value {
        let (chat_ms, chat_tokens) = (self.chat_busy_ms.load(Relaxed), self.chat_tokens.load(Relaxed));
        let (emb_ms, emb_texts) = (self.embed_busy_ms.load(Relaxed), self.embed_texts.load(Relaxed));
        let (sp_ms, sp_audio) = (self.speech_busy_ms.load(Relaxed), self.speech_audio_ms.load(Relaxed));
        let rate = |n: u64, ms: u64| if ms == 0 { 0.0 } else { n as f64 * 1000.0 / ms as f64 };
        json!({
            "uptime_s": self.uptime_s(),
            "chat": {
                "jobs": self.chat_jobs.load(Relaxed), "errors": self.chat_errors.load(Relaxed),
                "prompt_tokens": self.chat_prompt_tokens.load(Relaxed), "generated_tokens": chat_tokens,
                "tokens_per_s": rate(chat_tokens, chat_ms), "busy_s": chat_ms / 1000,
            },
            "search": {
                "jobs": self.embed_jobs.load(Relaxed), "errors": self.embed_errors.load(Relaxed),
                "passages": emb_texts, "passages_per_s": rate(emb_texts, emb_ms), "busy_s": emb_ms / 1000,
            },
            "speech": {
                "jobs": self.speech_jobs.load(Relaxed), "errors": self.speech_errors.load(Relaxed),
                "audio_s": sp_audio / 1000, "times_real_time": if sp_ms == 0 { 0.0 } else { sp_audio as f64 / sp_ms as f64 }, "busy_s": sp_ms / 1000,
            },
            "window": {
                "jobs": self.window_jobs.load(Relaxed), "errors": self.window_errors.load(Relaxed),
                "audio_s": self.window_audio_ms.load(Relaxed) / 1000, "busy_s": self.window_busy_ms.load(Relaxed) / 1000,
            },
            "pairings": self.pairings.load(Relaxed),
            "refused": self.rejected.load(Relaxed),
            "phones": self.clients.lock().iter().map(|(fp, c)| json!({
                "name": c.name, "id": &fp[..16], "chat_jobs": c.chat_jobs, "search_jobs": c.embed_jobs, "speech_jobs": c.speech_jobs, "window_jobs": c.window_jobs, "last_seen": c.last_seen,
            })).collect::<Vec<_>>(),
            "recent": self.recent.lock().iter().rev().map(|r| json!({"at": r.at, "kind": r.kind, "ok": r.ok, "ms": r.ms, "size": r.size})).collect::<Vec<_>>(),
        })
    }
}

pub fn ago(secs: u64) -> String {
    match secs {
        s if s < 90 => format!("{s} s"),
        s if s < 5400 => format!("{} min", s / 60),
        s => format!("{:.1} h", s as f64 / 3600.0),
    }
}

/// The text `marginalia-node stats` prints (and the tray shows). Numbers and names only.
pub fn report(v: &Value) -> String {
    let n = |x: &Value| x.as_u64().unwrap_or(0);
    let f = |x: &Value| x.as_f64().unwrap_or(0.0);
    let (c, s, sp, w) = (&v["chat"], &v["search"], &v["speech"], &v["window"]);
    // "1 job", "2 jobs": every count says its noun in the right number.
    let pl = |count: u64, one: &str, many: &str| format!("{count} {}", if count == 1 { one } else { many });
    let mut out = format!(
        "{} up for {}\n\nChat    {}, {} failed, {} written, {:.0} tokens a second while working\nSearch  {}, {} failed, {}, {:.0} passages a second while working\nSpeech  {}, {} failed, {} s of audio, {:.1} s of audio per second of work\nWindows {}, {} failed, {} s of audio\nPaired {}, {} turned away\n",
        v["name"].as_str().unwrap_or("node"), ago(n(&v["uptime_s"])),
        pl(n(&c["jobs"]), "job", "jobs"), n(&c["errors"]), pl(n(&c["generated_tokens"]), "token", "tokens"), f(&c["tokens_per_s"]),
        pl(n(&s["jobs"]), "job", "jobs"), n(&s["errors"]), pl(n(&s["passages"]), "passage", "passages"), f(&s["passages_per_s"]),
        pl(n(&sp["jobs"]), "job", "jobs"), n(&sp["errors"]), n(&sp["audio_s"]), f(&sp["times_real_time"]),
        pl(n(&w["jobs"]), "job", "jobs"), n(&w["errors"]), n(&w["audio_s"]),
        pl(n(&v["pairings"]), "time", "times"), pl(n(&v["refused"]), "unknown phone", "unknown phones"),
    );
    let state = match (v["paused"].as_bool().unwrap_or(false), v["locked"].as_bool().unwrap_or(false)) {
        (true, true) => "Paused. New phones must be allowed first.",
        (true, false) => "Paused. New jobs are turned away.",
        (false, true) => "Running. New phones must be allowed first.",
        (false, false) => "Running.",
    };
    out.push_str(&format!("{state}\n"));
    out.push_str("\nPhones\n");
    for p in v["phones"].as_array().into_iter().flatten() {
        out.push_str(&format!("  {} ({}): {} chat, {} search, {} speech, {}\n", p["name"].as_str().unwrap_or(""), p["id"].as_str().unwrap_or(""), n(&p["chat_jobs"]), n(&p["search_jobs"]), n(&p["speech_jobs"]), pl(n(&p["window_jobs"]), "window", "windows")));
    }
    out.push_str("\nLatest jobs (newest first, sizes only)\n");
    for r in v["recent"].as_array().into_iter().flatten().take(10) {
        let size = n(&r["size"]);
        let what = match r["kind"].as_str() {
            Some("chat") => pl(size, "token", "tokens"),
            Some("speech") | Some("window") => format!("{size} s of audio"),
            _ => pl(size, "passage", "passages"),
        };
        out.push_str(&format!("  {} {} {} ms, {}\n", r["kind"].as_str().unwrap_or(""), if r["ok"].as_bool().unwrap_or(false) { "ok    " } else { "failed" }, n(&r["ms"]), what));
    }
    out
}
