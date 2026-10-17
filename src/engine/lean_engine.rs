//! The engine without llama.cpp (Android). Chat and embeddings are attached from outside
//! (LiteRT-LM); speech runs on whisper.cpp here.

use super::*;
use crate::config::{Paths, Settings};
use crate::models;
use anyhow::{anyhow, Result};
use crossbeam_channel::Sender;
use parking_lot::RwLock;

pub struct Engine {
    paths: Paths,
    embed: RwLock<Option<EmbedHandle>>,
    llm: RwLock<Option<LlmHandle>>,
    whisper: RwLock<Option<WhisperHandle>>,
}

impl Engine {
    pub fn new(paths: Paths) -> Result<Self> {
        Ok(Self { paths, embed: RwLock::new(None), llm: RwLock::new(None), whisper: RwLock::new(None) })
    }

    pub fn embed(&self) -> Option<EmbedHandle> {
        self.embed.read().clone()
    }
    pub fn llm(&self) -> Option<LlmHandle> {
        self.llm.read().clone()
    }
    pub fn whisper(&self) -> Option<WhisperHandle> {
        self.whisper.read().clone()
    }

    pub fn attach_embedder(&self, handle: EmbedHandle) {
        if let Some(old) = self.embed.write().replace(handle) {
            old.shutdown();
        }
    }

    pub fn attach_llm(&self, handle: LlmHandle) {
        if let Some(old) = self.llm.write().replace(handle) {
            old.shutdown();
        }
    }

    pub fn load_embedder(&self, _settings: &Settings) -> Result<u32> {
        Err(anyhow!("this build serves embeddings from LiteRT-LM; the model file is missing"))
    }

    pub fn load_llm(&self, _settings: &Settings) -> Result<u32> {
        Err(anyhow!("this build serves Gemma from LiteRT-LM; the model file is missing"))
    }

    fn threads() -> i32 {
        let n = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(8);
        ((n / 2).max(4)).min(16) as i32
    }

    /// Returns whether the GPU ended up being used (never, on the phone: speech runs on the CPU).
    pub fn load_whisper(&self, settings: &Settings, out: Sender<Transcript>) -> Result<bool> {
        let spec = models::whisper_spec(settings.whisper_model);
        let path = models::model_path(&self.paths, &spec);
        let handle = whisper::spawn(
            whisper::WhisperConfig { model_path: path, use_gpu: false, language: settings.language.clone(), threads: Self::threads() },
            out,
        )?;
        if let Some(old) = self.whisper.write().replace(handle) {
            old.shutdown();
        }
        Ok(false)
    }

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
