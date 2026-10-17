//! Marginalia core for Android. The engine, RAG and decode sources are the very same
//! files the desktop app compiles; only config, audio capture and the UI differ.

#![allow(dead_code)]

pub mod config;

#[path = "../../../src/models.rs"]
pub mod models;
#[path = "../../../src/engine/mod.rs"]
pub mod engine;
#[path = "../../../src/rag/mod.rs"]
pub mod rag;

pub mod audio;
pub mod session;
pub mod text;
pub mod jni_bridge;
