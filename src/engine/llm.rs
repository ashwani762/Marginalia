//! Gemma 4 chat worker on llama.cpp with token streaming and priority queues.

use anyhow::{anyhow, Context as _, Result};
use crossbeam_channel::{bounded, unbounded, Receiver, Sender};
use llama_cpp_2::context::params::LlamaContextParams;
use llama_cpp_2::context::LlamaContext;
use llama_cpp_2::llama_backend::LlamaBackend;
use llama_cpp_2::llama_batch::LlamaBatch;
use llama_cpp_2::model::params::LlamaModelParams;
use llama_cpp_2::model::LlamaModel;
use llama_cpp_2::mtmd::{mtmd_default_marker, MtmdBitmap, MtmdContext, MtmdContextParams, MtmdInputText};
use llama_cpp_2::sampling::LlamaSampler;
use std::num::NonZeroU32;
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
    mtmd_default_marker()
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

pub struct LlmConfig {
    pub model_path: PathBuf,
    pub mmproj_path: Option<PathBuf>,
    pub gpu_layers: u32,
    pub use_gpu: bool,
    pub n_ctx: u32,
    pub threads: i32,
}

pub fn spawn(backend: Arc<LlamaBackend>, cfg: LlmConfig) -> Result<LlmHandle> {
    let (high_tx, high_rx) = unbounded::<Job>();
    let (low_tx, low_rx) = unbounded::<Job>();
    let (ready_tx, ready_rx) = bounded::<Result<()>>(1);
    let tokens_generated = Arc::new(AtomicU64::new(0));
    let tokens_processed = Arc::new(AtomicU64::new(0));
    let stats = (tokens_generated.clone(), tokens_processed.clone());
    let join = std::thread::Builder::new()
        .name("llm-worker".into())
        .spawn(move || {
            let params = LlamaModelParams::default().with_n_gpu_layers(cfg.gpu_layers);
            let model = match LlamaModel::load_from_file(&backend, &cfg.model_path, &params)
                .with_context(|| format!("loading {}", cfg.model_path.display()))
            {
                Ok(m) => m,
                Err(e) => {
                    let _ = ready_tx.send(Err(e));
                    return;
                }
            };
            let ctx_params = LlamaContextParams::default()
                .with_n_ctx(NonZeroU32::new(cfg.n_ctx))
                .with_n_batch(512)
                .with_n_ubatch(512)
                .with_n_threads(cfg.threads)
                .with_n_threads_batch(cfg.threads);
            let mut ctx = match model.new_context(&backend, ctx_params) {
                Ok(c) => c,
                Err(e) => {
                    let _ = ready_tx.send(Err(anyhow!("llm context: {e}")));
                    return;
                }
            };
            let _ = ready_tx.send(Ok(()));
            tracing::info!("chat model ready: n_ctx={}", ctx.n_ctx());
            let mut worker = Worker {
                model: &model,
                ctx: &mut ctx,
                high_rx,
                low_rx,
                stats,
                mmproj_path: cfg.mmproj_path.clone(),
                use_gpu: cfg.use_gpu,
                threads: cfg.threads,
                mtmd: None,
            };
            worker.run();
        })?;
    ready_rx.recv().map_err(|_| anyhow!("llm worker died during load"))??;
    Ok(LlmHandle {
        high: high_tx,
        low: low_tx,
        tokens_generated,
        tokens_processed,
        join: Arc::new(parking_lot::Mutex::new(Some(join))),
    })
}

struct Worker<'a, 'b> {
    model: &'a LlamaModel,
    ctx: &'b mut LlamaContext<'a>,
    high_rx: Receiver<Job>,
    low_rx: Receiver<Job>,
    stats: (Arc<AtomicU64>, Arc<AtomicU64>),
    mmproj_path: Option<PathBuf>,
    use_gpu: bool,
    threads: i32,
    /// Vision projector, loaded lazily on the first image request.
    mtmd: Option<MtmdContext>,
}

impl Worker<'_, '_> {
    fn run(&mut self) {
        loop {
            // Drain high priority first, then fall back to whichever arrives.
            let job = match self.high_rx.try_recv() {
                Ok(j) => j,
                Err(_) => crossbeam_channel::select! {
                    recv(self.high_rx) -> j => match j { Ok(j) => j, Err(_) => break },
                    recv(self.low_rx) -> j => match j { Ok(j) => j, Err(_) => break },
                },
            };
            match job {
                Job::Shutdown => break,
                Job::Gen(req) => self.generate(req),
            }
        }
    }

    fn generate(&mut self, req: GenRequest) {
        let started = Instant::now();
        let prompt = render_gemma_prompt(&req.messages);
        let vocab = self.model.vocab();
        let mut tokens = vocab.tokenize(prompt.as_bytes(), false, true);
        let n_ctx = self.ctx.n_ctx() as usize;
        let reserve = req.params.max_tokens.min(n_ctx / 2).max(64);
        if tokens.len() + reserve > n_ctx {
            // Keep the head (system prompt) and the tail; drop the middle.
            let keep_head = (n_ctx / 4).min(tokens.len());
            let keep_tail = n_ctx.saturating_sub(reserve + keep_head);
            let tail_start = tokens.len().saturating_sub(keep_tail);
            let mut trimmed = tokens[..keep_head].to_vec();
            trimmed.extend_from_slice(&tokens[tail_start.max(keep_head)..]);
            tokens = trimmed;
        }
        let prompt_tokens = tokens.len();
        self.stats.1.fetch_add(prompt_tokens as u64, Ordering::Relaxed);

        self.ctx.clear_kv_cache();
        let n_batch = self.ctx.n_batch() as usize;
        let mut batch = LlamaBatch::new(n_batch.max(512), 1);
        let mut pos: i32 = 0;
        let total = tokens.len();
        if !req.images.is_empty() {
            match self.eval_multimodal_prompt(&prompt, &req.images) {
                Ok(n_past) => {
                    pos = n_past;
                    // Skip the plain-text prompt path below.
                    return self.sample_loop(req, pos, prompt_tokens, started, batch);
                }
                Err(e) => {
                    (req.on_event)(GenEvent::Error(format!("vision prompt failed: {e}")));
                    return;
                }
            }
        }
        for chunk in tokens.chunks(n_batch) {
            batch.clear();
            for (k, t) in chunk.iter().enumerate() {
                let is_last = (pos as usize + k) == total - 1;
                if batch.add(*t, pos + k as i32, &[0], is_last).is_err() {
                    (req.on_event)(GenEvent::Error("batch overflow".into()));
                    return;
                }
            }
            if let Err(e) = self.ctx.decode(&mut batch) {
                (req.on_event)(GenEvent::Error(format!("prompt decode failed: {e}")));
                return;
            }
            pos += chunk.len() as i32;
            if req.cancel.load(Ordering::Relaxed) {
                (req.on_event)(GenEvent::Done(GenStats {
                    prompt_tokens,
                    generated_tokens: 0,
                    elapsed_ms: started.elapsed().as_millis(),
                    preempted: false,
                }));
                return;
            }
        }
        self.sample_loop(req, pos, prompt_tokens, started, batch);
    }

    /// Tokenize a prompt with image markers through the vision projector and
    /// evaluate it; returns the next position.
    fn eval_multimodal_prompt(&mut self, prompt: &str, images: &[PathBuf]) -> anyhow::Result<i32> {
        if self.mtmd.is_none() {
            let path = self.mmproj_path.clone().ok_or_else(|| anyhow!("vision projector not downloaded"))?;
            let mut params = MtmdContextParams::default();
            params.use_gpu = self.use_gpu;
            params.n_threads = self.threads;
            let m = MtmdContext::init_from_file(&path.to_string_lossy(), self.model, &params)
                .map_err(|e| anyhow!("loading vision projector: {e}"))?;
            tracing::info!("chat vision projector ready");
            self.mtmd = Some(m);
        }
        let mtmd = self.mtmd.as_mut().expect("mtmd loaded");
        let mut bitmaps = Vec::with_capacity(images.len());
        for img in images {
            bitmaps.push(
                MtmdBitmap::from_file(mtmd, &img.to_string_lossy(), false)
                    .map_err(|e| anyhow!("decoding {}: {e}", img.display()))?,
            );
        }
        let refs: Vec<&MtmdBitmap> = bitmaps.iter().collect();
        let chunks = mtmd
            .tokenize(MtmdInputText { text: prompt.to_string(), add_special: false, parse_special: true }, &refs)
            .map_err(|e| anyhow!("multimodal tokenize: {e}"))?;
        let n_batch = self.ctx.n_batch() as i32;
        let n_past = chunks
            .eval_chunks(mtmd, self.ctx, 0, 0, n_batch, true)
            .map_err(|e| anyhow!("multimodal eval: {e}"))?;
        Ok(n_past)
    }

    fn sample_loop(&mut self, req: GenRequest, mut pos: i32, prompt_tokens: usize, started: Instant, mut batch: LlamaBatch) {
        let vocab = self.model.vocab();
        let n_ctx = self.ctx.n_ctx() as usize;
        let mut sampler = LlamaSampler::chain_simple([
            LlamaSampler::penalties(vocab.n_tokens(), 64, req.params.repeat_penalty, 0.0, 0.0),
            LlamaSampler::top_k(req.params.top_k),
            LlamaSampler::top_p(req.params.top_p, 1),
            LlamaSampler::temp(req.params.temperature.max(0.0)),
            if req.params.temperature <= 0.01 {
                LlamaSampler::greedy()
            } else {
                LlamaSampler::dist(rand_seed())
            },
        ]);

        let mut generated = 0usize;
        let mut utf8_buf: Vec<u8> = Vec::new();
        let mut preempted = false;
        let max_tokens = req.params.max_tokens;
        while generated < max_tokens && (pos as usize) < n_ctx - 1 {
            if req.cancel.load(Ordering::Relaxed) {
                break;
            }
            if req.priority == Priority::Low && !self.high_rx.is_empty() {
                preempted = true;
                break;
            }
            let token = sampler.sample(self.ctx, -1);
            sampler.accept(token);
            if vocab.is_eog(token) {
                break;
            }
            let piece = vocab.token_to_piece(token, false, None);
            if piece == b"<turn|>" || piece == b"<|turn>" {
                break;
            }
            utf8_buf.extend_from_slice(&piece);
            match std::str::from_utf8(&utf8_buf) {
                Ok(s) => {
                    if !s.is_empty() {
                        (req.on_event)(GenEvent::Token(s.to_string()));
                    }
                    utf8_buf.clear();
                }
                Err(e) => {
                    let valid = e.valid_up_to();
                    if valid > 0 {
                        let s = String::from_utf8_lossy(&utf8_buf[..valid]).to_string();
                        (req.on_event)(GenEvent::Token(s));
                        utf8_buf.drain(..valid);
                    }
                    if utf8_buf.len() > 8 {
                        utf8_buf.clear();
                    }
                }
            }
            generated += 1;
            batch.clear();
            if batch.add(token, pos, &[0], true).is_err() {
                break;
            }
            pos += 1;
            if let Err(e) = self.ctx.decode(&mut batch) {
                (req.on_event)(GenEvent::Error(format!("decode failed: {e}")));
                return;
            }
        }
        self.stats.0.fetch_add(generated as u64, Ordering::Relaxed);
        (req.on_event)(GenEvent::Done(GenStats {
            prompt_tokens,
            generated_tokens: generated,
            elapsed_ms: started.elapsed().as_millis(),
            preempted,
        }));
    }
}

fn rand_seed() -> u32 {
    use std::time::{SystemTime, UNIX_EPOCH};
    let nanos = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.subsec_nanos()).unwrap_or(1234);
    nanos ^ 0x9E37_79B9
}

/// Gemma 4 canonical chat format:
/// `<bos><|turn>system\n…<turn|>\n<|turn>user\n…<turn|>\n<|turn>model\n`
pub fn render_gemma_prompt(messages: &[ChatMessage]) -> String {
    let mut out = String::with_capacity(messages.iter().map(|m| m.content.len() + 24).sum::<usize>() + 16);
    out.push_str("<bos>");
    let mut rest = messages;
    if let Some(first) = messages.first() {
        if first.role == Role::System {
            out.push_str("<|turn>system\n");
            out.push_str(first.content.trim());
            out.push_str("<turn|>\n");
            rest = &messages[1..];
        }
    }
    for m in rest {
        let role = match m.role {
            Role::User => "user",
            Role::Assistant => "model",
            Role::System => "user",
        };
        out.push_str("<|turn>");
        out.push_str(role);
        out.push('\n');
        out.push_str(m.content.trim());
        out.push_str("<turn|>\n");
    }
    out.push_str("<|turn>model\n");
    out
}
