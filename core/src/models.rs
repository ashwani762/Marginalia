//! Which model files the core expects to find in the models folder.
//! The app installs the files itself, so this only names them and checks that they are present.

use crate::config::{ChatModel, Paths, WhisperModel};
use std::path::PathBuf;

/// A model file the core can ask for.
#[derive(Clone, Copy, Debug)]
pub struct ModelSpec {
    /// File name inside the models folder.
    pub file: &'static str,
    /// Expected size in bytes, or 0 when any size is accepted.
    pub bytes: u64,
}

/// Search model weights used for the knowledge base (768 dimensions).
pub const EMBEDDING_GEMMA_2: ModelSpec = ModelSpec { file: "embeddinggemma-2-Q8_0.gguf", bytes: 309_855_456 };

/// Chat model that expands notes, answers questions and writes summaries.
pub const GEMMA_4_E2B: ModelSpec = ModelSpec { file: "gemma-4-E2B-it-Q4_0.gguf", bytes: 2_841_481_184 };
/// Larger chat model with stronger reasoning.
pub const GEMMA_4_E4B: ModelSpec = ModelSpec { file: "gemma-4-E4B-it-Q4_0.gguf", bytes: 0 };

/// Light multilingual speech model, comfortable on the CPU.
pub const WHISPER_SMALL: ModelSpec = ModelSpec { file: "ggml-small-q8_0.bin", bytes: 264_464_607 };
/// Best transcription quality, GPU recommended.
pub const WHISPER_LARGE_TURBO: ModelSpec = ModelSpec { file: "ggml-large-v3-turbo-q5_0.bin", bytes: 574_041_195 };

/// Vision projector that lets the chat model caption imported images.
pub const GEMMA_4_E2B_MMPROJ: ModelSpec = ModelSpec { file: "mmproj-gemma-4-E2B-it-Q8_0.gguf", bytes: 557_368_064 };
/// Vision projector for the larger chat model.
pub const GEMMA_4_E4B_MMPROJ: ModelSpec = ModelSpec { file: "mmproj-gemma-4-E4B-it-Q8_0.gguf", bytes: 559_874_816 };

pub fn chat_spec(choice: ChatModel) -> ModelSpec {
    match choice {
        ChatModel::GemmaE2B => GEMMA_4_E2B,
        ChatModel::GemmaE4B => GEMMA_4_E4B,
    }
}

pub fn chat_mmproj_spec(choice: ChatModel) -> ModelSpec {
    match choice {
        ChatModel::GemmaE2B => GEMMA_4_E2B_MMPROJ,
        ChatModel::GemmaE4B => GEMMA_4_E4B_MMPROJ,
    }
}

pub fn whisper_spec(choice: WhisperModel) -> ModelSpec {
    match choice {
        WhisperModel::Small => WHISPER_SMALL,
        WhisperModel::LargeTurbo => WHISPER_LARGE_TURBO,
    }
}

pub fn model_path(paths: &Paths, spec: &ModelSpec) -> PathBuf {
    paths.models_dir.join(spec.file)
}

/// True when the file exists and, if a size is known, has exactly that size.
pub fn is_present(paths: &Paths, spec: &ModelSpec) -> bool {
    let p = model_path(paths, spec);
    match std::fs::metadata(&p) {
        Ok(m) if m.is_file() => spec.bytes == 0 || m.len() == spec.bytes,
        _ => false,
    }
}
