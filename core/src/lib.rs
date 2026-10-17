//! Marginalia core for Android: the speech worker, retrieval, meeting sessions and the JNI surface.
//! Chat and search models run in Kotlin (LiteRT-LM) and are attached through the engine handles.

#![allow(dead_code)]

pub mod config;

pub mod models;
pub mod engine;
pub mod rag;

pub mod audio;
pub mod session;
pub mod text;
pub mod jni_bridge;
