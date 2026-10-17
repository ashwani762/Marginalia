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
    last_seen: u64,
}

struct Recent {
    at: u64,
    kind: &'static str,
    ok: bool,
    ms: u64,
    /// Tokens generated, or texts embedded.
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
    pub pairings: AtomicU64,
    pub rejected: AtomicU64,
    clients: Mutex<HashMap<String, PerClient>>,
    recent: Mutex<VecDeque<Recent>>,
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
            pairings: 0.into(),
            rejected: 0.into(),
            clients: Mutex::new(HashMap::new()),
            recent: Mutex::new(VecDeque::new()),
        }
    }

    pub fn uptime_s(&self) -> u64 {
        self.started.elapsed().as_secs()
    }

    fn touch(&self, fp: &str, name: &str, chat: bool) {
        let mut g = self.clients.lock();
        let c = g.entry(fp.to_string()).or_default();
        c.name = name.to_string();
        c.last_seen = now();
        if chat {
            c.chat_jobs += 1;
        } else {
            c.embed_jobs += 1;
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
        self.touch(fp, name, true);
        self.remember("chat", true, ms, tokens);
    }

    pub fn chat_failed(&self, fp: &str, name: &str) {
        self.chat_errors.fetch_add(1, Relaxed);
        self.touch(fp, name, true);
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
        self.touch(fp, name, false);
        self.remember("search", ok, ms, texts);
    }

    pub fn snapshot(&self) -> Value {
        let (chat_ms, chat_tokens) = (self.chat_busy_ms.load(Relaxed), self.chat_tokens.load(Relaxed));
        let (emb_ms, emb_texts) = (self.embed_busy_ms.load(Relaxed), self.embed_texts.load(Relaxed));
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
            "pairings": self.pairings.load(Relaxed),
            "refused": self.rejected.load(Relaxed),
            "phones": self.clients.lock().iter().map(|(fp, c)| json!({
                "name": c.name, "id": &fp[..16], "chat_jobs": c.chat_jobs, "search_jobs": c.embed_jobs, "last_seen": c.last_seen,
            })).collect::<Vec<_>>(),
            "recent": self.recent.lock().iter().rev().map(|r| json!({"at": r.at, "kind": r.kind, "ok": r.ok, "ms": r.ms, "size": r.size})).collect::<Vec<_>>(),
        })
    }
}
