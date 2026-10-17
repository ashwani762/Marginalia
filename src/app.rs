//! Shared services and the event bus between background workers and the UI.

use crate::audio::AudioMeters;
use crate::config::{Paths, Settings};
use crate::engine::{Engine, GenEvent, Transcript};
use crate::models::{self, DownloadProgress, ModelSpec};
use crate::rag::{IngestEvent, Kb};
use anyhow::Result;
use parking_lot::RwLock;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

#[derive(Clone, Debug)]
pub enum EngineStatus {
    Downloading,
    Loading(String),
    Ready,
    Failed(String),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum GenTarget {
    Enhance(u64),
    Card { card: u64, gen: u64 },
    Chat(u64),
    Summary(i64),
    Title(i64),
    QuestionScan(u64),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NoteKind {
    Info,
    Success,
    Warning,
    Error,
}

#[derive(Clone, Debug)]
pub enum UiEvent {
    Download(DownloadProgress),
    DownloadFailed { id: &'static str, error: String },
    Engine(EngineStatus),
    Transcript(Transcript),
    Ingest(IngestEvent),
    Gen { target: GenTarget, event: GenEvent },
    Notify { kind: NoteKind, message: String },
}

pub struct Services {
    pub paths: Paths,
    pub settings: RwLock<Settings>,
    pub engine: Arc<Engine>,
    pub kb: Arc<Kb>,
    pub events: smol::channel::Sender<UiEvent>,
    pub meters: RwLock<Option<Arc<AudioMeters>>>,
    pub download_cancel: Arc<AtomicBool>,
    /// Set once the chat model has been moved to CPU after a GPU failure.
    pub degraded: AtomicBool,
}

impl Services {
    pub fn emit(&self, ev: UiEvent) {
        let _ = self.events.try_send(ev);
    }

    pub fn notify(&self, kind: NoteKind, message: impl Into<String>) {
        self.emit(UiEvent::Notify { kind, message: message.into() });
    }

    pub fn settings(&self) -> Settings {
        self.settings.read().clone()
    }

    pub fn save_settings(&self, s: Settings) {
        if let Err(e) = s.save(&self.paths) {
            tracing::warn!("saving settings: {e:#}");
        }
        *self.settings.write() = s;
    }

    /// Build a streaming callback that routes generation events to the UI.
    pub fn gen_sink(&self, target: GenTarget) -> Box<dyn Fn(GenEvent) + Send + 'static> {
        let tx = self.events.clone();
        Box::new(move |event| {
            let _ = tx.try_send(UiEvent::Gen { target, event });
        })
    }

    /// Models needed for the current settings.
    pub fn required_models(&self) -> Vec<ModelSpec> {
        let s = self.settings();
        let mut v = vec![
            models::EMBEDDING_GEMMA_2,
            models::chat_spec(s.chat_model),
            models::whisper_spec(s.whisper_model),
        ];
        if s.vision {
            v.push(models::EMBEDDING_GEMMA_2_MMPROJ);
            v.push(models::chat_mmproj_spec(s.chat_model));
        }
        v
    }

    pub fn missing_models(&self) -> Vec<ModelSpec> {
        self.required_models()
            .into_iter()
            .filter(|m| !models::is_present(&self.paths, m))
            .collect()
    }
}

/// Download anything missing, then load the models. Runs on its own thread and
/// reports through the event bus. Safe to call again after a settings change.
pub fn bootstrap(services: Arc<Services>) {
    std::thread::Builder::new()
        .name("bootstrap".into())
        .spawn(move || {
            if let Err(e) = bootstrap_inner(&services) {
                services.emit(UiEvent::Engine(EngineStatus::Failed(format!("{e:#}"))));
            }
        })
        .expect("spawn bootstrap");
}

fn bootstrap_inner(services: &Arc<Services>) -> Result<()> {
    let missing = services.missing_models();
    if !missing.is_empty() {
        services.emit(UiEvent::Engine(EngineStatus::Downloading));
        for spec in missing {
            let sv = services.clone();
            let res = models::download(&services.paths, &spec, services.download_cancel.clone(), move |p| {
                sv.emit(UiEvent::Download(p));
            });
            if let Err(e) = res {
                services.emit(UiEvent::DownloadFailed { id: spec.id, error: format!("{e:#}") });
                anyhow::bail!("download of {} failed: {e:#}", spec.label);
            }
        }
    }
    let settings = services.settings();

    services.emit(UiEvent::Engine(EngineStatus::Loading("Loading EmbeddingGemma 2".into())));
    let layers = services.engine.load_embedder(&settings)?;
    if let Some(n) = Engine::placement_note(settings.use_gpu, layers, "The embedding model") {
        services.notify(NoteKind::Warning, n);
    }
    let embed = services.engine.embed().expect("embedder loaded");
    let (ingest_tx, ingest_rx) = crossbeam_channel::unbounded::<IngestEvent>();
    {
        let sv = services.clone();
        std::thread::spawn(move || {
            while let Ok(ev) = ingest_rx.recv() {
                sv.emit(UiEvent::Ingest(ev));
            }
        });
    }
    services.kb.set_chunk_tokens(settings.chunk_tokens);
    services.kb.attach_embedder(embed, ingest_tx);

    services.emit(UiEvent::Engine(EngineStatus::Loading(format!(
        "Loading {}",
        models::chat_spec(settings.chat_model).label
    ))));
    let layers = services.engine.load_llm(&settings)?;
    if let Some(n) = Engine::placement_note(settings.use_gpu, layers, "The chat model") {
        services.notify(NoteKind::Warning, n);
    }
    services.kb.attach_llm(services.engine.llm());

    services.emit(UiEvent::Engine(EngineStatus::Loading(format!(
        "Loading {}",
        models::whisper_spec(settings.whisper_model).label
    ))));
    let gpu = load_whisper(services, &settings)?;
    if settings.use_gpu && !gpu {
        services.notify(NoteKind::Warning, "Transcription is running on CPU: not enough GPU memory.");
    }
    services.kb.attach_whisper(services.engine.whisper());

    services.emit(UiEvent::Engine(EngineStatus::Ready));
    Ok(())
}

pub fn load_whisper(services: &Arc<Services>, settings: &Settings) -> Result<bool> {
    let (tx, rx) = crossbeam_channel::unbounded::<Transcript>();
    let sv = services.clone();
    std::thread::spawn(move || {
        while let Ok(t) = rx.recv() {
            sv.emit(UiEvent::Transcript(t));
        }
    });
    services.engine.load_whisper(settings, tx)
}

/// Called when a generation fails on the GPU at runtime (e.g. CUDA out of memory):
/// reload the chat model on the CPU once so the session keeps working.
pub fn degrade_llm_to_cpu(services: Arc<Services>) {
    if services.degraded.swap(true, std::sync::atomic::Ordering::SeqCst) {
        return;
    }
    std::thread::Builder::new()
        .name("llm-degrade".into())
        .spawn(move || {
            services.notify(NoteKind::Warning, "GPU generation failed; moving the chat model to CPU…");
            services.emit(UiEvent::Engine(EngineStatus::Loading("Reloading chat model on CPU".into())));
            let settings = services.settings();
            services.engine.unload_llm();
            match services.engine.load_llm_with(&settings, false) {
                Ok(_) => {
                    services.kb.attach_llm(services.engine.llm());
                    services.notify(NoteKind::Info, "Chat model is now running on CPU.");
                    services.emit(UiEvent::Engine(EngineStatus::Ready));
                }
                Err(e) => services.emit(UiEvent::Engine(EngineStatus::Failed(format!("{e:#}")))),
            }
        })
        .expect("spawn degrade");
}

/// True when the error text looks like a GPU/backend failure rather than a bad prompt.
pub fn is_backend_failure(msg: &str) -> bool {
    let m = msg.to_ascii_lowercase();
    m.contains("decode failed") || m.contains("cuda") || m.contains("out of memory") || m.contains("ggml")
}

/// Reload chat / whisper models after a settings change (downloads if needed).
pub fn reload_models(services: Arc<Services>) {
    std::thread::Builder::new()
        .name("reload-models".into())
        .spawn(move || {
            let res = (|| -> Result<()> {
                let missing = services.missing_models();
                if !missing.is_empty() {
                    services.emit(UiEvent::Engine(EngineStatus::Downloading));
                    for spec in missing {
                        let sv = services.clone();
                        models::download(&services.paths, &spec, services.download_cancel.clone(), move |p| {
                            sv.emit(UiEvent::Download(p));
                        })?;
                    }
                }
                let settings = services.settings();
                services.emit(UiEvent::Engine(EngineStatus::Loading("Reloading chat model".into())));
                services.engine.unload_llm();
                let layers = services.engine.load_llm(&settings)?;
                if let Some(n) = Engine::placement_note(settings.use_gpu, layers, "The chat model") {
                    services.notify(NoteKind::Warning, n);
                }
                services.kb.attach_llm(services.engine.llm());
                services.emit(UiEvent::Engine(EngineStatus::Loading("Reloading transcription model".into())));
                services.engine.unload_whisper();
                let gpu = load_whisper(&services, &settings)?;
                if settings.use_gpu && !gpu {
                    services.notify(NoteKind::Warning, "Transcription is running on CPU: not enough GPU memory.");
                }
                services.kb.attach_whisper(services.engine.whisper());
                services.kb.set_chunk_tokens(settings.chunk_tokens);
                Ok(())
            })();
            match res {
                Ok(()) => services.emit(UiEvent::Engine(EngineStatus::Ready)),
                Err(e) => services.emit(UiEvent::Engine(EngineStatus::Failed(format!("{e:#}")))),
            }
        })
        .expect("spawn reload");
}
