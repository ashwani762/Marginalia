//! EmbeddingGemma 2 embedder running in its own thread on top of llama.cpp.
//!
//! Multiple sequences are packed into one llama batch so throughput stays high
//! on both CPU and GPU. Vectors are L2-normalized; the first 256 dims form a
//! Matryoshka prefix used by the vector index as a cheap pre-filter.

use anyhow::{anyhow, Context as _, Result};
use crossbeam_channel::{bounded, unbounded, Receiver, Sender};
use llama_cpp_2::context::params::{LlamaContextParams, LlamaPoolingType};
use llama_cpp_2::llama_backend::LlamaBackend;
use llama_cpp_2::llama_batch::LlamaBatch;
use llama_cpp_2::model::params::LlamaModelParams;
use llama_cpp_2::model::LlamaModel;
use llama_cpp_2::mtmd::{MtmdBitmap, MtmdContext, MtmdContextParams, MtmdInputText};
use std::num::NonZeroU32;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

pub const EMBED_DIM: usize = 768;
pub const COARSE_DIM: usize = 256;
const MAX_SEQ_TOKENS: usize = 1024;
const BATCH_TOKENS: usize = 2048;
const MAX_SEQS: usize = 32;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EmbedKind {
    /// Already formatted with the model's task prefix.
    Raw,
    /// A search query ("task: search result | query: ...").
    Query,
    /// A document chunk ("title: ... | text: ...").
    Document,
}

pub fn format_query(q: &str) -> String {
    format!("task: search result | query: {}", q.trim())
}

pub fn format_document(title: &str, text: &str) -> String {
    let title = title.trim();
    let title = if title.is_empty() { "none" } else { title };
    format!("title: {} | text: {}", title, text.trim())
}

enum Job {
    Embed {
        texts: Vec<String>,
        kind: EmbedKind,
        reply: Sender<Result<Vec<Vec<f32>>>>,
    },
    CountTokens {
        texts: Vec<String>,
        reply: Sender<Vec<usize>>,
    },
    EmbedImage {
        path: PathBuf,
        reply: Sender<Result<Vec<f32>>>,
    },
    Shutdown,
}

#[derive(Clone)]
pub struct EmbedHandle {
    tx: Sender<Job>,
    join: Arc<parking_lot::Mutex<Option<std::thread::JoinHandle<()>>>>,
}

impl EmbedHandle {
    pub fn embed(&self, texts: Vec<String>, kind: EmbedKind) -> Result<Vec<Vec<f32>>> {
        if texts.is_empty() {
            return Ok(vec![]);
        }
        let (reply, rx) = bounded(1);
        self.tx
            .send(Job::Embed { texts, kind, reply })
            .map_err(|_| anyhow!("embedding worker is gone"))?;
        rx.recv().map_err(|_| anyhow!("embedding worker dropped reply"))?
    }

    pub fn embed_one(&self, text: &str, kind: EmbedKind) -> Result<Vec<f32>> {
        Ok(self
            .embed(vec![text.to_string()], kind)?
            .pop()
            .unwrap_or_default())
    }

    /// Embed an image file into the shared text/image space (needs the vision projector).
    pub fn embed_image(&self, path: &std::path::Path) -> Result<Vec<f32>> {
        let (reply, rx) = bounded(1);
        self.tx
            .send(Job::EmbedImage { path: path.to_path_buf(), reply })
            .map_err(|_| anyhow!("embedding worker is gone"))?;
        rx.recv().map_err(|_| anyhow!("embedding worker dropped reply"))?
    }

    pub fn count_tokens(&self, texts: Vec<String>) -> Vec<usize> {
        if texts.is_empty() {
            return vec![];
        }
        let fallback: Vec<usize> = texts.iter().map(|t| t.len() / 4 + 1).collect();
        let (reply, rx) = bounded(1);
        if self.tx.send(Job::CountTokens { texts, reply }).is_err() {
            return fallback;
        }
        rx.recv().unwrap_or(fallback)
    }

    /// Ask the worker to exit and wait for it, so GPU buffers are released
    /// before the CUDA runtime tears down at process exit.
    pub fn shutdown(&self) {
        let _ = self.tx.send(Job::Shutdown);
        if let Some(j) = self.join.lock().take() {
            let _ = j.join();
        }
    }
}

pub struct EmbedConfig {
    pub model_path: PathBuf,
    pub mmproj_path: Option<PathBuf>,
    pub gpu_layers: u32,
    pub use_gpu: bool,
    pub threads: i32,
}

/// Spawn the embedding worker; resolves once the model is loaded (or failed).
pub fn spawn(backend: Arc<LlamaBackend>, cfg: EmbedConfig) -> Result<EmbedHandle> {
    let (tx, rx) = unbounded::<Job>();
    let (ready_tx, ready_rx) = bounded::<Result<()>>(1);
    let join = std::thread::Builder::new()
        .name("embed-worker".into())
        .spawn(move || {
            let loaded = (|| -> Result<(LlamaModel, LlamaContextParams)> {
                let params = LlamaModelParams::default().with_n_gpu_layers(cfg.gpu_layers);
                let model = LlamaModel::load_from_file(&backend, &cfg.model_path, &params)
                    .with_context(|| format!("loading {}", cfg.model_path.display()))?;
                let ctx_params = LlamaContextParams::default()
                    .with_n_ctx(NonZeroU32::new(BATCH_TOKENS as u32))
                    .with_n_batch(BATCH_TOKENS as u32)
                    .with_n_ubatch(BATCH_TOKENS as u32)
                    .with_n_seq_max(MAX_SEQS as u32)
                    .with_embeddings(true)
                    .with_pooling_type(LlamaPoolingType::Mean)
                    .with_n_threads(cfg.threads)
                    .with_n_threads_batch(cfg.threads);
                Ok((model, ctx_params))
            })();
            let (model, ctx_params) = match loaded {
                Ok(v) => v,
                Err(e) => {
                    let _ = ready_tx.send(Err(e));
                    return;
                }
            };
            let mut ctx = match model.new_context(&backend, ctx_params) {
                Ok(c) => c,
                Err(e) => {
                    let _ = ready_tx.send(Err(anyhow!("embedding context: {e}")));
                    return;
                }
            };
            let mtmd = cfg.mmproj_path.as_ref().and_then(|p| {
                let mut params = MtmdContextParams::default();
                params.use_gpu = cfg.use_gpu;
                params.n_threads = cfg.threads;
                match MtmdContext::init_from_file(&p.to_string_lossy(), &model, &params) {
                    Ok(m) => {
                        tracing::info!("embedding vision projector ready (vision={})", m.support_vision());
                        Some(m)
                    }
                    Err(e) => {
                        tracing::warn!("vision projector failed to load: {e}");
                        None
                    }
                }
            });
            let _ = ready_tx.send(Ok(()));
            tracing::info!(
                "embedding model ready: n_embd={} n_embd_out={}",
                model.n_embd(),
                model.n_embd_out()
            );
            run_loop(&model, &mut ctx, mtmd, rx);
        })?;
    ready_rx
        .recv()
        .map_err(|_| anyhow!("embedding worker died during load"))??;
    Ok(EmbedHandle { tx, join: Arc::new(parking_lot::Mutex::new(Some(join))) })
}

fn run_loop(
    model: &LlamaModel,
    ctx: &mut llama_cpp_2::context::LlamaContext<'_>,
    mut mtmd: Option<MtmdContext>,
    rx: Receiver<Job>,
) {
    let vocab = model.vocab();
    while let Ok(job) = rx.recv() {
        match job {
            Job::Shutdown => break,
            Job::EmbedImage { path, reply } => {
                let result = match mtmd.as_mut() {
                    Some(m) => embed_image(m, ctx, &path),
                    None => Err(anyhow!("vision projector not loaded (enable Vision in Settings)")),
                };
                let _ = reply.send(result);
            }
            Job::CountTokens { texts, reply } => {
                let counts = texts
                    .iter()
                    .map(|t| vocab.tokenize(t.as_bytes(), false, false).len())
                    .collect();
                let _ = reply.send(counts);
            }
            Job::Embed { texts, kind, reply } => {
                let started = Instant::now();
                let result = embed_all(model, ctx, &texts, kind);
                if let Ok(v) = &result {
                    tracing::debug!(
                        "embedded {} texts in {:.0} ms",
                        v.len(),
                        started.elapsed().as_secs_f64() * 1000.0
                    );
                }
                let _ = reply.send(result);
            }
        }
    }
}

fn embed_all(
    model: &LlamaModel,
    ctx: &mut llama_cpp_2::context::LlamaContext<'_>,
    texts: &[String],
    kind: EmbedKind,
) -> Result<Vec<Vec<f32>>> {
    let vocab = model.vocab();
    let formatted: Vec<String> = texts
        .iter()
        .map(|t| match kind {
            EmbedKind::Raw => t.clone(),
            EmbedKind::Query => format_query(t),
            EmbedKind::Document => format_document("none", t),
        })
        .collect();
    let mut token_lists: Vec<Vec<_>> = formatted
        .iter()
        .map(|t| {
            let mut toks = vocab.tokenize(t.as_bytes(), true, false);
            if toks.len() > MAX_SEQ_TOKENS {
                toks.truncate(MAX_SEQ_TOKENS);
            }
            if toks.is_empty() {
                toks = vocab.tokenize(b" ", true, false);
            }
            toks
        })
        .collect();

    let mut out: Vec<Vec<f32>> = Vec::with_capacity(texts.len());
    let mut batch = LlamaBatch::new(BATCH_TOKENS, MAX_SEQS as i32);
    let mut i = 0;
    while i < token_lists.len() {
        batch.clear();
        let mut seqs = 0usize;
        let mut used = 0usize;
        let start = i;
        while i < token_lists.len() && seqs < MAX_SEQS {
            let n = token_lists[i].len();
            if used + n > BATCH_TOKENS && seqs > 0 {
                break;
            }
            batch.add_sequence(&token_lists[i], seqs as i32, false)?;
            used += n;
            seqs += 1;
            i += 1;
        }
        ctx.clear_kv_cache();
        ctx.decode(&mut batch).context("embedding decode")?;
        for s in 0..seqs {
            let emb = ctx
                .embeddings_seq_ith(s as i32)
                .map_err(|e| anyhow!("embeddings_seq_ith: {e}"))?;
            out.push(normalize(emb));
        }
        for list in &mut token_lists[start..i] {
            list.clear();
        }
    }
    Ok(out)
}

/// Encode an image through the projector and mean-pool it in the embedding model.
fn embed_image(
    mtmd: &mut MtmdContext,
    ctx: &mut llama_cpp_2::context::LlamaContext<'_>,
    path: &std::path::Path,
) -> Result<Vec<f32>> {
    let started = Instant::now();
    let bitmap = MtmdBitmap::from_file(mtmd, &path.to_string_lossy(), false)
        .map_err(|e| anyhow!("decoding image {}: {e}", path.display()))?;
    let marker = llama_cpp_2::mtmd::mtmd_default_marker().to_string();
    let chunks = mtmd
        .tokenize(MtmdInputText { text: marker, add_special: false, parse_special: true }, &[&bitmap])
        .map_err(|e| anyhow!("image tokenize: {e}"))?;
    if chunks.is_empty() {
        return Err(anyhow!("image produced no tokens"));
    }
    ctx.clear_kv_cache();
    chunks
        .eval_chunks(mtmd, ctx, 0, 0, BATCH_TOKENS as i32, false)
        .map_err(|e| anyhow!("image eval: {e}"))?;
    let emb = ctx.embeddings_seq_ith(0).map_err(|e| anyhow!("image embeddings: {e}"))?;
    tracing::debug!(
        "embedded image {} ({} tokens) in {:.0} ms",
        path.display(),
        chunks.total_tokens(),
        started.elapsed().as_secs_f64() * 1000.0
    );
    Ok(normalize(emb))
}

pub fn normalize(v: &[f32]) -> Vec<f32> {
    let norm = v.iter().map(|x| x * x).sum::<f32>().sqrt();
    if norm <= f32::EPSILON {
        return v.to_vec();
    }
    v.iter().map(|x| x / norm).collect()
}

pub fn cosine(a: &[f32], b: &[f32]) -> f32 {
    a.iter().zip(b).map(|(x, y)| x * y).sum()
}
