//! Paths and settings for the phone build. Mirrors the desktop `config` API that the
//! shared engine code expects (`Paths`, `Settings`, `ChatModel`, `WhisperModel`).

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Clone, Debug)]
pub struct Paths {
    pub data_dir: PathBuf,
    pub models_dir: PathBuf,
    pub db_path: PathBuf,
    pub settings_path: PathBuf,
}

impl Paths {
    /// `data_dir` is the app's private files dir; `models_dir` is where the bundled
    /// models are unpacked on first launch.
    pub fn new(data_dir: &Path, models_dir: &Path) -> Self {
        let _ = std::fs::create_dir_all(data_dir);
        let _ = std::fs::create_dir_all(models_dir);
        Self {
            db_path: data_dir.join("marginalia.sqlite3"),
            settings_path: data_dir.join("settings.json"),
            data_dir: data_dir.to_path_buf(),
            models_dir: models_dir.to_path_buf(),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ChatModel {
    GemmaE2B,
    GemmaE4B,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum WhisperModel {
    Small,
    LargeTurbo,
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
    pub note_style: NoteStyle,
    pub live_assistance: bool,
    pub chunk_tokens: usize,
    pub chat_context: u32,
    pub user_name: String,
    /// Load the vision projectors so images can be embedded and captioned.
    pub vision: bool,
    /// "System", "Light" or "Dark" (rendered by the UI layer).
    pub appearance: String,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            chat_model: ChatModel::GemmaE2B,
            whisper_model: WhisperModel::Small,
            use_gpu: false,
            language: "auto".into(),
            note_style: NoteStyle::Standard,
            live_assistance: true,
            chunk_tokens: 220,
            chat_context: 4096,
            user_name: "You".into(),
            vision: true,
            appearance: "System".into(),
        }
    }
}

impl Settings {
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
