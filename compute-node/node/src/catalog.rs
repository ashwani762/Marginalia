//! The model files and memory estimates, read from `models.toml` (see that file for where it is looked for).
//! Nothing about a model is written in the code: names, repositories and sizes all come from the file.

use crate::models_dl::ModelFile;
use serde::Deserialize;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

/// The file built into the program, used for anything the other files leave out.
const BUILT_IN: &str = include_str!("../../models.toml");

#[derive(Deserialize, Clone, Debug)]
struct Entry {
    role: String,
    repo: String,
    file: String,
    #[serde(default)]
    remote: String,
    #[serde(default)]
    bytes: u64,
    #[serde(default)]
    optional: bool,
}

/// How much graphics memory a model needs: its file size times the factor plus the extra.
#[derive(Deserialize, Clone, Copy, Debug)]
pub struct Memory {
    pub keep_free_mb: u64,
    pub speech_factor: f64,
    pub speech_extra_mb: u64,
    pub search_factor: f64,
    pub search_extra_mb: u64,
    pub chat_factor: f64,
    pub chat_extra_mb: u64,
    pub photos_factor: f64,
    pub photos_extra_mb: u64,
}

#[derive(Deserialize, Default)]
struct File {
    #[serde(default)]
    models: BTreeMap<String, Entry>,
    memory: Option<Memory>,
}

#[derive(Debug)]
pub struct Catalog {
    models: BTreeMap<String, ModelFile>,
    pub memory: Memory,
    /// The file that was read, or None for the built-in one.
    pub source: Option<PathBuf>,
}

static CATALOG: OnceLock<Catalog> = OnceLock::new();
static STATE_DIR: OnceLock<PathBuf> = OnceLock::new();

/// Tells the catalog where the node's state folder is, before anything reads it.
pub fn set_state_dir(dir: &Path) {
    let _ = STATE_DIR.set(dir.to_path_buf());
}

fn candidates() -> Vec<PathBuf> {
    let mut v = vec![];
    if let Some(p) = std::env::var_os("MARGINALIA_MODELS_FILE") {
        v.push(PathBuf::from(p));
    }
    if let Some(d) = STATE_DIR.get() {
        v.push(d.join("models.toml"));
    }
    if let Some(d) = std::env::current_exe().ok().and_then(|e| e.parent().map(Path::to_path_buf)) {
        v.push(d.join("models.toml"));
    }
    v
}

fn leak(s: String) -> &'static str {
    Box::leak(s.into_boxed_str())
}

fn to_model(e: Entry) -> ModelFile {
    ModelFile { role: leak(e.role), repo: leak(e.repo), file: leak(e.file), remote: leak(e.remote), bytes: e.bytes, optional: e.optional }
}

/// Reads the catalog from `text` on top of the built-in one. Errors name the problem in the file.
fn build(text: Option<(&str, PathBuf)>) -> Result<Catalog, String> {
    let base: File = toml::from_str(BUILT_IN).map_err(|e| format!("the built-in models.toml is broken: {e}"))?;
    let mut models = base.models;
    let mut memory = base.memory.ok_or("the built-in models.toml has no [memory]")?;
    let mut source = None;
    if let Some((t, path)) = text {
        let f: File = toml::from_str(t).map_err(|e| format!("{} could not be read: {e}", path.display()))?;
        models.extend(f.models);
        if let Some(m) = f.memory {
            memory = m;
        }
        source = Some(path);
    }
    Ok(Catalog { models: models.into_iter().map(|(k, v)| (k, to_model(v))).collect(), memory, source })
}

pub fn get() -> &'static Catalog {
    CATALOG.get_or_init(|| {
        for p in candidates() {
            if let Ok(t) = std::fs::read_to_string(&p) {
                match build(Some((&t, p.clone()))) {
                    Ok(c) => return c,
                    Err(e) => tracing::warn!("{e}. Using the built-in models list instead."),
                }
            }
        }
        build(None).expect("the built-in models.toml")
    })
}

impl Catalog {
    /// The model with this id (for example `chat-e2b`). Every id the code asks for is in the built-in file.
    pub fn model(&self, id: &str) -> ModelFile {
        *self.models.get(id).unwrap_or_else(|| panic!("models.toml has no [models.{id}]"))
    }
    /// Every model, for `models list`.
    pub fn all(&self) -> Vec<(&str, ModelFile)> {
        self.models.iter().map(|(k, v)| (k.as_str(), *v)).collect()
    }
}

pub fn chat_e2b() -> ModelFile { get().model("chat-e2b") }
pub fn chat_e4b() -> ModelFile { get().model("chat-e4b") }
pub fn search() -> ModelFile { get().model("search") }
pub fn speech() -> ModelFile { get().model("speech") }
pub fn vision_e2b() -> ModelFile { get().model("vision-e2b") }
pub fn vision_e4b() -> ModelFile { get().model("vision-e4b") }
pub fn speakers_segmentation() -> ModelFile { get().model("speakers-segmentation") }
pub fn speakers_voices() -> ModelFile { get().model("speakers-voices") }

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn built_in_file_has_every_model_the_code_uses() {
        let c = build(None).unwrap();
        for id in ["chat-e2b", "chat-e4b", "search", "speech", "vision-e2b", "vision-e4b", "speakers-segmentation", "speakers-voices"] {
            let m = c.model(id);
            assert!(!m.file.is_empty() && !m.repo.is_empty(), "{id}");
        }
        assert_eq!(c.model("speakers-voices").remote, "wespeaker_en_voxceleb_resnet34_LM.onnx");
        assert!(c.memory.keep_free_mb > 0);
    }

    #[test]
    fn a_small_file_changes_only_what_it_names() {
        let mine = "[models.search]\nrole = \"search\"\nrepo = \"me/other\"\nfile = \"other.gguf\"\n";
        let c = build(Some((mine, PathBuf::from("mine.toml")))).unwrap();
        assert_eq!(c.model("search").file, "other.gguf");
        assert_eq!(c.model("chat-e2b").file, "gemma-4-E2B-it-Q4_0.gguf");
        assert_eq!(c.source.as_deref(), Some(Path::new("mine.toml")));
        assert!(build(Some(("[models.x", PathBuf::from("bad.toml")))).unwrap_err().contains("bad.toml"));
    }
}
