//! EmbeddingGemma 2 embedder running in its own thread on top of llama.cpp.
//!
//! Multiple sequences are packed into one llama batch so throughput stays high
//! on both CPU and GPU. Vectors are L2-normalized; the first 256 dims form a
//! Matryoshka prefix used by the vector index as a cheap pre-filter.


use anyhow::{anyhow, Result};
use crossbeam_channel::{bounded, unbounded, Sender};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

pub const EMBED_DIM: usize = 768;
pub const COARSE_DIM: usize = 256;

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

/// Embeddings produced elsewhere (LiteRT-LM on Android): texts in, one vector per text out.
/// The texts arrive already formatted with the task prefix.
pub type ExternalEmbedder = Arc<dyn Fn(Vec<String>) -> Result<Vec<Vec<f32>>> + Send + Sync>;

impl EmbedHandle {
    /// A handle with the same interface as the llama.cpp worker, backed by [`ExternalEmbedder`].
    pub fn external(embedder: ExternalEmbedder) -> EmbedHandle {
        let (tx, rx) = unbounded::<Job>();
        let join = std::thread::Builder::new()
            .name("embed-external".into())
            .spawn(move || {
                while let Ok(job) = rx.recv() {
                    match job {
                        Job::Embed { texts, kind, reply } => {
                            let started = Instant::now();
                            let formatted: Vec<String> = texts
                                .iter()
                                .map(|t| match kind {
                                    EmbedKind::Raw => t.clone(),
                                    EmbedKind::Query => format_query(t),
                                    EmbedKind::Document => format_document("none", t),
                                })
                                .collect();
                            let result = embedder(formatted).map(|vs| {
                                vs.into_iter()
                                    .map(|v| {
                                        let mut v = normalize(&v);
                                        v.resize(EMBED_DIM, 0.0);
                                        v
                                    })
                                    .collect()
                            });
                            tracing::debug!("embedded {} texts in {:.0} ms", texts.len(), started.elapsed().as_secs_f64() * 1000.0);
                            let _ = reply.send(result);
                        }
                        Job::CountTokens { texts, reply } => {
                            // No tokenizer here: about 3.3 characters per token for plain text, one per character otherwise.
                            let _ = reply.send(
                                texts
                                    .iter()
                                    .map(|t| {
                                        let ascii = t.chars().filter(|c| c.is_ascii()).count();
                                        let other = t.chars().count() - ascii;
                                        (ascii as f32 / 3.3 + other as f32 * 0.95).ceil() as usize + 1
                                    })
                                    .collect(),
                            );
                        }
                        Job::EmbedImage { reply, .. } => {
                            let _ = reply.send(Err(anyhow!("image embedding is not available")));
                        }
                        Job::Shutdown => break,
                    }
                }
            })
            .expect("spawn external embedder");
        EmbedHandle { tx, join: Arc::new(parking_lot::Mutex::new(Some(join))) }
    }
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


#[cfg(feature = "llama")]
mod worker;
#[cfg(feature = "llama")]
pub use worker::{spawn, EmbedConfig};
