//! Marginalia core for Android: the speech worker, retrieval, meeting sessions and the JNI surface.
//! Chat and search models run in Kotlin (LiteRT-LM) and are attached through the engine handles.

//!
//! Modules:
//! - `jni_bridge`: the four native functions Kotlin calls, everything else travels as JSON.
//! - `session`: the `Core` that answers those JSON calls and runs meetings.
//! - `engine`: the speech, chat and embedding workers.
//! - `rag`: ingestion, SQLite storage and search for the knowledge base.
//! - `audio`: file decoding and voice activity detection.
//! - `remote`: offloading AI work to home computers on the same network.
//! - `config`, `models`, `text`: settings and paths, expected model files, small text helpers.

pub mod audio;
pub mod config;
pub mod engine;
pub mod jni_bridge;
pub mod models;
pub mod rag;
pub mod remote;
pub mod session;
pub mod text;
