//! The models the node serves. They load once and stay loaded; a job only borrows them.

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
    pub cpu_only: bool,
    pub ctx: u32,
}

pub struct Host {
    _backend: Arc<LlamaBackend>,
    pub llm: Option<LlmHandle>,
    pub embed: Option<EmbedHandle>,
    pub caps: parking_lot::Mutex<Capabilities>,
    /// Jobs that have been accepted and not yet finished.
    pub chat_jobs: Arc<AtomicU32>,
    pub embed_jobs: Arc<AtomicU32>,
}

const CHAT_FILE: &str = "gemma-4-E2B-it-Q4_0.gguf";
const EMBED_FILE: &str = "embeddinggemma-2-Q8_0.gguf";

fn tiers(cpu_only: bool) -> Vec<u32> {
    if cpu_only { vec![0] } else { vec![999, 16, 0] }
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

        let embed_path = opts.embed_model.clone().unwrap_or_else(|| opts.models_dir.join(EMBED_FILE));
        let embed = if embed_path.is_file() {
            let mut found = None;
            for layers in tiers(opts.cpu_only) {
                let cfg = crate::engine::embed::EmbedConfig { model_path: embed_path.clone(), mmproj_path: None, gpu_layers: layers, use_gpu: layers > 0, threads: threads() };
                match crate::engine::embed::spawn(backend.clone(), cfg) {
                    Ok(h) => {
                        tracing::info!("search model loaded ({} GPU layers)", layers);
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

        let chat_path = opts.chat_model.clone().unwrap_or_else(|| opts.models_dir.join(CHAT_FILE));
        let llm = if chat_path.is_file() {
            let mut found = None;
            for layers in tiers(opts.cpu_only) {
                let cfg = crate::engine::llm::LlmConfig { model_path: chat_path.clone(), mmproj_path: None, gpu_layers: layers, use_gpu: layers > 0, n_ctx: opts.ctx, threads: threads() };
                match crate::engine::llm::spawn(backend.clone(), cfg) {
                    Ok(h) => {
                        tracing::info!("chat model loaded ({} GPU layers)", layers);
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
        if llm.is_none() && embed.is_none() {
            return Err(anyhow!("no models found in {}", opts.models_dir.display()));
        }
        let mut host = Host {
            _backend: backend,
            llm,
            embed,
            caps: parking_lot::Mutex::new(caps),
            chat_jobs: Arc::new(AtomicU32::new(0)),
            embed_jobs: Arc::new(AtomicU32::new(0)),
        };
        host.benchmark();
        Ok(host)
    }

    /// A short run of each model, so phones know what this computer is worth before they send work.
    fn benchmark(&mut self) {
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
        tracing::info!("measured: chat {:.0} tokens a second, search {:.0} passages a second", caps.chat_tok_s, caps.embed_per_s);
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
