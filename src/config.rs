//! Paths and persisted user settings.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Clone, Debug)]
pub struct Paths {
    pub data_dir: PathBuf,
    pub models_dir: PathBuf,
    pub db_path: PathBuf,
    pub settings_path: PathBuf,
}

pub fn paths() -> Paths {
    let data_dir = std::env::var_os("MARGINALIA_DATA")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            dirs::data_local_dir()
                .unwrap_or_else(|| PathBuf::from("."))
                .join("marginalia")
        });
    let _ = std::fs::create_dir_all(&data_dir);
    let models_dir = data_dir.join("models");
    let _ = std::fs::create_dir_all(&models_dir);
    Paths {
        db_path: data_dir.join("marginalia.sqlite3"),
        settings_path: data_dir.join("settings.json"),
        data_dir,
        models_dir,
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ChatModel {
    /// Gemma 4 E2B — fastest, great on CPU.
    GemmaE2B,
    /// Gemma 4 E4B — stronger, prefers GPU offload.
    GemmaE4B,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum WhisperModel {
    /// whisper small (q8) — light, CPU friendly.
    Small,
    /// whisper large-v3-turbo (q5) — best quality, GPU recommended.
    LargeTurbo,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum Appearance {
    /// Follow the operating system.
    #[default]
    System,
    Light,
    Dark,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum NoteStyle {
    Standard,
    Concise,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub chat_model: ChatModel,
    pub whisper_model: WhisperModel,
    pub use_gpu: bool,
    /// ISO language code or "auto".
    pub language: String,
    pub mic_device: Option<String>,
    pub capture_system_audio: bool,
    /// Kept for settings files written before `appearance` existed.
    pub dark_mode: bool,
    pub appearance: Appearance,
    pub note_style: NoteStyle,
    pub live_assistance: bool,
    pub chunk_tokens: usize,
    pub chat_context: u32,
    pub user_name: String,
    /// Download the vision projectors so images can be embedded and captioned.
    pub vision: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            chat_model: ChatModel::GemmaE2B,
            whisper_model: WhisperModel::LargeTurbo,
            use_gpu: cfg!(any(feature = "cuda", feature = "vulkan")),
            language: "auto".into(),
            mic_device: None,
            capture_system_audio: true,
            dark_mode: true,
            appearance: Appearance::System,
            note_style: NoteStyle::Standard,
            live_assistance: true,
            chunk_tokens: 220,
            chat_context: 8192,
            user_name: "You".into(),
            vision: true,
        }
    }
}

impl Settings {
    /// Resolve the theme for the current OS appearance.
    pub fn resolve_dark(&self, system: gpui_kit::WindowAppearance) -> bool {
        match self.appearance {
            Appearance::Light => false,
            Appearance::Dark => true,
            Appearance::System => matches!(system, gpui_kit::WindowAppearance::Dark | gpui_kit::WindowAppearance::VibrantDark),
        }
    }

    pub fn load(paths: &Paths) -> Self {
        std::fs::read_to_string(&paths.settings_path)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    }

    pub fn save(&self, paths: &Paths) -> Result<()> {
        let json = serde_json::to_string_pretty(self)?;
        std::fs::write(&paths.settings_path, json)
            .with_context(|| format!("writing {}", paths.settings_path.display()))
    }
}
