//! Local inference engine: the speech, chat and embedding workers. Everything runs in-process.
//!
//! The desktop build serves chat and embeddings from llama.cpp (`llama` feature). The Android
//! build has no llama.cpp: Gemma and EmbeddingGemma come from LiteRT-LM, handed in through
//! [`LlmHandle::external`] and [`EmbedHandle::external`], and only speech runs here (whisper.cpp).

pub mod embed;
pub mod gate;
pub mod llm;
pub mod whisper;
pub mod whisper_sys;

pub use embed::EmbedHandle;
pub use llm::{ChatMessage, GenEvent, GenParams, GenRequest, LlmHandle, Priority};
pub use whisper::{AudioSegment, AudioSource, Transcript, WhisperHandle};

#[cfg(feature = "llama")]
mod llama_engine;
#[cfg(feature = "llama")]
pub use llama_engine::Engine;

#[cfg(not(feature = "llama"))]
mod lean_engine;
#[cfg(not(feature = "llama"))]
pub use lean_engine::Engine;
