#![allow(dead_code)]
//! The engines the node runs: llama.cpp for chat and for the search model. The workers are the same
//! ones the first desktop version of Marginalia used.

pub mod embed;
pub mod gate;
pub mod llm;

pub use embed::{EmbedHandle, EmbedKind};
pub use llm::{ChatMessage, GenEvent, GenParams, GenRequest, LlmHandle, Priority, Role};
