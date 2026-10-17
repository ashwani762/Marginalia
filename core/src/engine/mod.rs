//! Local inference engine: the speech, chat and embedding workers. Everything runs in-process.
//!
//! Gemma and EmbeddingGemma come from LiteRT-LM, handed in through [`LlmHandle::external`] and
//! [`EmbedHandle::external`], and only speech runs here (whisper.cpp).

pub mod embed;
pub mod gate;
pub mod llm;
pub mod whisper;
pub mod whisper_sys;

pub use embed::EmbedHandle;
pub use llm::{ChatMessage, GenEvent, GenParams, GenRequest, LlmHandle, Priority};
pub use whisper::{AudioSegment, AudioSource, Transcript, WhisperHandle};

mod lean_engine;
pub use lean_engine::Engine;
