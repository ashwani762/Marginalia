//! Chat generation with token streaming and priority queues. Generation itself runs in Kotlin (LiteRT-LM).


use anyhow::{anyhow, Result};
use crossbeam_channel::{unbounded, Sender};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Instant;


#[derive(Clone, Debug)]
pub struct ChatMessage {
    pub role: Role,
    pub content: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Role {
    System,
    User,
    Assistant,
}

impl ChatMessage {
    pub fn system(s: impl Into<String>) -> Self {
        Self { role: Role::System, content: s.into() }
    }
    pub fn user(s: impl Into<String>) -> Self {
        Self { role: Role::User, content: s.into() }
    }
    pub fn assistant(s: impl Into<String>) -> Self {
        Self { role: Role::Assistant, content: s.into() }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Priority {
    /// User-facing: chat answers, note enhancement, summaries.
    High,
    /// Proactive work that may be preempted: question detection, context cards.
    Low,
}

#[derive(Clone, Debug)]
pub struct GenParams {
    pub max_tokens: usize,
    pub temperature: f32,
    pub top_p: f32,
    pub top_k: i32,
    pub repeat_penalty: f32,
}

impl Default for GenParams {
    fn default() -> Self {
        Self { max_tokens: 512, temperature: 0.6, top_p: 0.95, top_k: 64, repeat_penalty: 1.05 }
    }
}

impl GenParams {
    pub fn precise(max_tokens: usize) -> Self {
        Self { max_tokens, temperature: 0.2, ..Default::default() }
    }
}

#[derive(Clone, Debug)]
pub enum GenEvent {
    Token(String),
    Done(GenStats),
    Error(String),
}

#[derive(Clone, Debug, Default)]
pub struct GenStats {
    pub prompt_tokens: usize,
    pub generated_tokens: usize,
    pub elapsed_ms: u128,
    pub preempted: bool,
}

pub struct GenRequest {
    pub messages: Vec<ChatMessage>,
    /// Images referenced by `<__media__>` markers in the last user message (in order).
    pub images: Vec<PathBuf>,
    pub params: GenParams,
    pub priority: Priority,
    pub cancel: Arc<AtomicBool>,
    pub on_event: Box<dyn Fn(GenEvent) + Send + 'static>,
}

/// The marker to place in a user message where an image should go.
pub fn image_marker() -> &'static str {
    "<__media__>"
}

enum Job {
    Gen(GenRequest),
    Shutdown,
}

#[derive(Clone)]
pub struct LlmHandle {
    high: Sender<Job>,
    low: Sender<Job>,
    pub tokens_generated: Arc<AtomicU64>,
    pub tokens_processed: Arc<AtomicU64>,
    join: Arc<parking_lot::Mutex<Option<std::thread::JoinHandle<()>>>>,
}

impl LlmHandle {
    pub fn generate(&self, req: GenRequest) {
        let tx = match req.priority {
            Priority::High => &self.high,
            Priority::Low => &self.low,
        };
        if let Err(e) = tx.send(Job::Gen(req)) {
            if let Job::Gen(r) = e.0 {
                (r.on_event)(GenEvent::Error("llm worker is gone".into()));
            }
        }
    }

    /// Convenience: run a generation to completion and return the text.
    pub fn complete(&self, messages: Vec<ChatMessage>, params: GenParams, priority: Priority) -> Result<String> {
        self.complete_with_images(messages, Vec::new(), params, priority)
    }

    pub fn complete_with_images(
        &self,
        messages: Vec<ChatMessage>,
        images: Vec<PathBuf>,
        params: GenParams,
        priority: Priority,
    ) -> Result<String> {
        let (tx, rx) = unbounded::<GenEvent>();
        self.generate(GenRequest {
            messages,
            images,
            params,
            priority,
            cancel: Arc::new(AtomicBool::new(false)),
            on_event: Box::new(move |ev| {
                let _ = tx.send(ev);
            }),
        });
        let mut out = String::new();
        while let Ok(ev) = rx.recv() {
            match ev {
                GenEvent::Token(t) => out.push_str(&t),
                GenEvent::Done(_) => break,
                GenEvent::Error(e) => return Err(anyhow!(e)),
            }
        }
        Ok(out)
    }

    pub fn shutdown(&self) {
        let _ = self.high.send(Job::Shutdown);
        if let Some(j) = self.join.lock().take() {
            let _ = j.join();
        }
    }
}

/// A generator that lives outside this crate (for example LiteRT-LM on Android). It must
/// stream text through `req.on_event(GenEvent::Token(..))`, watch `req.cancel`, and return
/// the token counts when it is done. Done / Error events are sent by the worker.
pub type ExternalGenerator = Arc<dyn Fn(&GenRequest) -> Result<GenStats> + Send + Sync>;

impl LlmHandle {
    /// A handle with high and low priority queues, whose
    /// generations are produced by [`ExternalGenerator`]. A low-priority request is cancelled
    /// (and reported as preempted) as soon as a high-priority one arrives.
    pub fn external(generator: ExternalGenerator) -> LlmHandle {
        let (high_tx, high_rx) = unbounded::<Job>();
        let (low_tx, low_rx) = unbounded::<Job>();
        let tokens_generated = Arc::new(AtomicU64::new(0));
        let tokens_processed = Arc::new(AtomicU64::new(0));
        let (gen_count, proc_count) = (tokens_generated.clone(), tokens_processed.clone());
        let watch_rx = high_rx.clone();
        let join = std::thread::Builder::new()
            .name("llm-external".into())
            .spawn(move || loop {
                let job = match high_rx.try_recv() {
                    Ok(j) => j,
                    Err(_) => crossbeam_channel::select! {
                        recv(high_rx) -> j => match j { Ok(j) => j, Err(_) => break },
                        recv(low_rx) -> j => match j { Ok(j) => j, Err(_) => break },
                    },
                };
                let req = match job {
                    Job::Shutdown => break,
                    Job::Gen(r) => r,
                };
                let started = Instant::now();
                let preempted = Arc::new(AtomicBool::new(false));
                let done = Arc::new(AtomicBool::new(false));
                let watcher = (req.priority == Priority::Low).then(|| {
                    let (rx, cancel, pre, done) = (watch_rx.clone(), req.cancel.clone(), preempted.clone(), done.clone());
                    std::thread::spawn(move || {
                        while !done.load(Ordering::Relaxed) {
                            if !rx.is_empty() {
                                pre.store(true, Ordering::Relaxed);
                                cancel.store(true, Ordering::Relaxed);
                                break;
                            }
                            std::thread::sleep(std::time::Duration::from_millis(40));
                        }
                    })
                });
                let result = generator(&req);
                done.store(true, Ordering::Relaxed);
                if let Some(w) = watcher {
                    let _ = w.join();
                }
                match result {
                    Ok(mut st) => {
                        st.elapsed_ms = started.elapsed().as_millis();
                        st.preempted = preempted.load(Ordering::Relaxed);
                        gen_count.fetch_add(st.generated_tokens as u64, Ordering::Relaxed);
                        proc_count.fetch_add(st.prompt_tokens as u64, Ordering::Relaxed);
                        (req.on_event)(GenEvent::Done(st));
                    }
                    Err(e) => (req.on_event)(GenEvent::Error(format!("{e:#}"))),
                }
            })
            .expect("spawn external llm worker");
        LlmHandle {
            high: high_tx,
            low: low_tx,
            tokens_generated,
            tokens_processed,
            join: Arc::new(parking_lot::Mutex::new(Some(join))),
        }
    }
}

