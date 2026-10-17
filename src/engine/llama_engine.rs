//! The llama.cpp engine (desktop): owns the llama backend plus the embedding, chat and whisper workers.

use super::*;
use crate::config::{Paths, Settings};
use crate::models;
use anyhow::{Context as _, Result};
use crossbeam_channel::Sender;
use llama_cpp_2::llama_backend::LlamaBackend;
use parking_lot::RwLock;
use std::sync::Arc;

pub struct Engine {
    backend: Arc<LlamaBackend>,
    paths: Paths,
    embed: RwLock<Option<EmbedHandle>>,
    llm: RwLock<Option<LlmHandle>>,
    whisper: RwLock<Option<WhisperHandle>>,
}

impl Engine {
    pub fn new(paths: Paths) -> Result<Self> {
        let mut backend = LlamaBackend::init().context("initializing llama backend")?;
        if std::env::var_os("FORESIGHT_LLAMA_LOGS").is_none() {
            backend.void_logs();
            // The multimodal loader logs through its own hooks.
            unsafe extern "C" fn noop(_: llama_cpp_sys_2::ggml_log_level, _: *const std::os::raw::c_char, _: *mut std::os::raw::c_void) {}
            unsafe {
                llama_cpp_sys_2::mtmd_log_set(Some(noop), std::ptr::null_mut());
                llama_cpp_sys_2::mtmd_helper_log_set(Some(noop), std::ptr::null_mut());
            }
        }
        Ok(Self {
            backend: Arc::new(backend),
            paths,
            embed: RwLock::new(None),
            llm: RwLock::new(None),
            whisper: RwLock::new(None),
        })
    }

    pub fn embed(&self) -> Option<EmbedHandle> {
        self.embed.read().clone()
    }
    /// Use embeddings served from elsewhere (see [`embed::EmbedHandle::external`]).
    pub fn attach_embedder(&self, handle: EmbedHandle) {
        if let Some(old) = self.embed.write().replace(handle) {
            old.shutdown();
        }
    }

    /// Use a chat model served from elsewhere (see [`llm::LlmHandle::external`]).
    pub fn attach_llm(&self, handle: LlmHandle) {
        if let Some(old) = self.llm.write().replace(handle) {
            old.shutdown();
        }
    }
    pub fn llm(&self) -> Option<LlmHandle> {
        self.llm.read().clone()
    }
    pub fn whisper(&self) -> Option<WhisperHandle> {
        self.whisper.read().clone()
    }

    fn threads() -> i32 {
        let n = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(8);
        // Physical-core-ish count works best for llama.cpp on desktop CPUs.
        ((n / 2).max(4)).min(16) as i32
    }

    fn optional_model(&self, spec: &models::ModelSpec, enabled: bool) -> Option<std::path::PathBuf> {
        if enabled && models::is_present(&self.paths, spec) {
            Some(models::model_path(&self.paths, spec))
        } else {
            None
        }
    }

    /// How a model ended up placed after fallbacks.
    pub fn placement_note(gpu_requested: bool, gpu_layers_used: u32, what: &str) -> Option<String> {
        if !gpu_requested {
            return None;
        }
        match gpu_layers_used {
            0 => Some(format!("{what} is running on CPU: not enough GPU memory for full offload.")),
            l if l < 999 => Some(format!("{what} is partially offloaded ({l} layers on GPU) to fit in memory.")),
            _ => None,
        }
    }

    fn gpu_tiers(use_gpu: bool) -> Vec<u32> {
        if use_gpu { vec![999, 16, 0] } else { vec![0] }
    }

    /// Returns the number of GPU layers actually used.
    pub fn load_embedder(&self, settings: &Settings) -> Result<u32> {
        let path = models::model_path(&self.paths, &models::EMBEDDING_GEMMA_2);
        let mut last_err = None;
        for layers in Self::gpu_tiers(settings.use_gpu) {
            match embed::spawn(
                self.backend.clone(),
                embed::EmbedConfig {
                    model_path: path.clone(),
                    mmproj_path: self.optional_model(&models::EMBEDDING_GEMMA_2_MMPROJ, settings.vision),
                    gpu_layers: layers,
                    use_gpu: layers > 0,
                    threads: Self::threads(),
                },
            ) {
                Ok(handle) => {
                    if let Some(old) = self.embed.write().replace(handle) {
                        old.shutdown();
                    }
                    return Ok(layers);
                }
                Err(e) => {
                    tracing::warn!("embedder load failed with {layers} GPU layers: {e:#}");
                    last_err = Some(e);
                }
            }
        }
        Err(last_err.unwrap_or_else(|| anyhow::anyhow!("embedder failed to load")))
    }

    pub fn load_llm(&self, settings: &Settings) -> Result<u32> {
        self.load_llm_with(settings, settings.use_gpu)
    }

    /// Load the chat model, trying full GPU offload, then partial, then CPU.
    pub fn load_llm_with(&self, settings: &Settings, use_gpu: bool) -> Result<u32> {
        let spec = models::chat_spec(settings.chat_model);
        let path = models::model_path(&self.paths, &spec);
        let mut last_err = None;
        for layers in Self::gpu_tiers(use_gpu) {
            match llm::spawn(
                self.backend.clone(),
                llm::LlmConfig {
                    model_path: path.clone(),
                    mmproj_path: self.optional_model(&models::chat_mmproj_spec(settings.chat_model), settings.vision),
                    gpu_layers: layers,
                    use_gpu: layers > 0,
                    n_ctx: settings.chat_context.max(2048),
                    threads: Self::threads(),
                },
            ) {
                Ok(handle) => {
                    if let Some(old) = self.llm.write().replace(handle) {
                        old.shutdown();
                    }
                    return Ok(layers);
                }
                Err(e) => {
                    tracing::warn!("chat model load failed with {layers} GPU layers: {e:#}");
                    last_err = Some(e);
                }
            }
        }
        Err(last_err.unwrap_or_else(|| anyhow::anyhow!("chat model failed to load")))
    }

    /// Returns whether the GPU ended up being used.
    pub fn load_whisper(&self, settings: &Settings, out: Sender<Transcript>) -> Result<bool> {
        let spec = models::whisper_spec(settings.whisper_model);
        let path = models::model_path(&self.paths, &spec);
        let attempts: Vec<bool> = if settings.use_gpu { vec![true, false] } else { vec![false] };
        let mut last_err = None;
        for use_gpu in attempts {
            match whisper::spawn(
                whisper::WhisperConfig {
                    model_path: path.clone(),
                    use_gpu,
                    language: settings.language.clone(),
                    threads: Self::threads(),
                },
                out.clone(),
            ) {
                Ok(handle) => {
                    if let Some(old) = self.whisper.write().replace(handle) {
                        old.shutdown();
                    }
                    return Ok(use_gpu);
                }
                Err(e) => {
                    tracing::warn!("whisper load failed (gpu={use_gpu}): {e:#}");
                    last_err = Some(e);
                }
            }
        }
        Err(last_err.unwrap_or_else(|| anyhow::anyhow!("whisper failed to load")))
    }

    /// Stop every worker and wait for it; call before the process exits.
    pub fn shutdown(&self) {
        if let Some(w) = self.whisper.write().take() {
            w.shutdown();
        }
        if let Some(l) = self.llm.write().take() {
            l.shutdown();
        }
        if let Some(e) = self.embed.write().take() {
            e.shutdown();
        }
    }

    pub fn unload_whisper(&self) {
        if let Some(old) = self.whisper.write().take() {
            old.shutdown();
        }
    }
    pub fn unload_llm(&self) {
        if let Some(old) = self.llm.write().take() {
            old.shutdown();
        }
    }
}
