#![allow(dead_code)]
//! The engines the node runs: llama.cpp for chat and for the search model, whisper.cpp for speech to
//! text. The llama workers are the same ones the first desktop version of Marginalia used.

pub mod embed;
pub mod gate;
pub mod llm;
pub mod whisper;
mod whisper_sys;

pub use embed::{EmbedHandle, EmbedKind};
pub use llm::{ChatMessage, GenEvent, GenParams, GenRequest, LlmHandle, Priority, Role};
