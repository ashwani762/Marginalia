//! The phone's brain: everything between the model workers and the screen. Kotlin sends JSON commands and
//! raw PCM, and this module answers with JSON and streams JSON events back.
//!
//! This file holds the shared types (`Core`, `Live`, `Card` and friends), the boot sequence, the settings
//! and the `Core::call` dispatcher. The rest of `Core` lives in the submodules below, each one adding
//! its own `impl Core` block. They read the private fields of `Core` through `use super::*`.
//!
//! - `live`: the meeting being recorded now (start, resume, audio and transcript intake, the timer loop).
//! - `assist`: question cards and typed questions during a meeting.
//! - `jots`: short typed notes and how they are reworded.
//! - `photos`: photos taken in a meeting and their descriptions.
//! - `wrapup`: the write-up after a meeting stops, recovery of unfinished meetings, reading and deleting.
//! - `chat`: the Ask page.
//! - `nodes`: the home computers calls.
//! - `debug`: timing helpers behind the `debug.*` calls.
//! - `transcript`: plain helpers for transcript lines and prompt windows.

use crate::config::{NoteStyle, Paths, Settings};
use crate::engine::llm::{ChatMessage, GenEvent, GenParams, GenRequest, GenStats, Priority};
use crate::engine::{AudioSegment, AudioSource, Engine, Transcript};
use crate::audio::vad::Segmenter;
use crate::models;
use crate::rag::chunk::fmt_ts;
use crate::rag::store::{DocKind, Meeting};
use crate::rag::{snippet_of, Hit, IngestEvent, IngestInput, IngestStage, Kb, Scope};
use crate::engine::llm::image_marker;
use crate::text::*;
use anyhow::{anyhow, Result};
use chrono::FixedOffset;
use parking_lot::{Mutex, RwLock};
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

mod assist;
mod chat;
mod debug;
mod jots;
mod live;
mod nodes;
mod photos;
mod transcript;
mod wrapup;

use chat::*;
use jots::*;
use photos::*;
use transcript::*;

/// Receives every event as a JSON string. The JNI layer forwards them to Kotlin.
pub type Sink = Arc<dyn Fn(&str) + Send + Sync>;

// Phone pacing: the CPU is shared between Whisper and Gemma, so background work waits
// longer than on a fast computer.
const TICK: Duration = Duration::from_millis(500);
const ENHANCE_DEBOUNCE: Duration = Duration::from_millis(3000);
const ENHANCE_MIN_GAP: Duration = Duration::from_secs(12);
const ENHANCE_TRANSCRIPT_GAP: Duration = Duration::from_secs(45);
const SCAN_INTERVAL: Duration = Duration::from_secs(40);
const SCAN_MIN_LINES: usize = 6;
const EMIT_EVERY: Duration = Duration::from_millis(120);
const JOT_SETTLE: Duration = Duration::from_secs(5);
const ASK_LIVE_SYSTEM: &str = "You answer a question that someone asks while a meeting or talk is still going on. Use what was said first, then the library passages if they help, citing a library passage like [1] only when you used it. Answer in at most 70 words, plain and direct, and name who said it when the transcript does. If neither says, reply: That has not been said yet. Never invent.";
const IDEAS_SYSTEM: &str = "You help someone who is listening to a talk or a meeting think of good questions for the speaker. From the transcript, write exactly 3 short questions they could ask. Each question must point at something specific that was said, or at something left unclear or unproven. One question per line, plain words, no numbering, no bullets, no preface.";
/// Long meetings are condensed in stretches of about this many tokens of transcript.
const SECTION_WINDOW_TOKENS: usize = 2200;
/// While a meeting is running, earlier stretches are only condensed into notes once this much has piled up
/// (about three quarters of an hour of talk). Shorter meetings are written up entirely after they end, so
/// nothing competes with the microphone while they run.
const SECTION_TRIGGER_TOKENS: usize = 9000;

#[derive(Clone)]
struct Line {
    t0: f64,
    t1: f64,
    text: String,
    speaker: Option<i64>,
    /// What the speaker is called in the notes: the name the person gave, or "Speaker 2". Empty when speakers are not told apart.
    who: String,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum CardState {
    Retrieving,
    Answering,
    Done,
    NoMatch,
}

impl CardState {
    fn as_str(self) -> &'static str {
        match self {
            CardState::Retrieving => "retrieving",
            CardState::Answering => "answering",
            CardState::Done => "done",
            CardState::NoMatch => "nomatch",
        }
    }
}

struct Card {
    /// "" for a lookup in the library, "ask" for a question the person typed, "ideas" for questions to put to the speaker.
    kind: &'static str,
    t: f64,
    id: u64,
    db_id: Option<i64>,
    question: String,
    answer: String,
    hits: Vec<Value>,
    state: CardState,
    gen: u64,
}

impl Card {
    fn json(&self) -> Value {
        json!({
            "id": self.id,
            "kind": self.kind,
            "t": self.t,
            "question": self.question,
            "answer": self.answer,
            "state": self.state.as_str(),
            "sources": self.hits,
        })
    }
}

/// State of the meeting being recorded right now. There is at most one.
struct Live {
    meeting_id: i64,
    started: Instant,
    /// Seconds of the meeting that came before this stretch of recording (non-zero when continuing one).
    time_offset: f64,
    recording: bool,
    segmenter: Segmenter,
    lines: Vec<Line>,
    partial: String,
    last_final: f64,
    notes: String,
    notes_dirty: bool,
    last_note_edit: Instant,
    last_enhance: Instant,
    enhance_gen: u64,
    enhance_in_flight: bool,
    enhance_cancel: Option<Arc<AtomicBool>>,
    enhanced_text: String,
    enhance_buffer: String,
    enhance_lines_seen: usize,
    enhance_emit: Instant,
    cards: Vec<Card>,
    next_card_id: u64,
    scanned_upto: usize,
    last_scan: Instant,
    scan_in_flight: bool,
    scan_gen: u64,
    scan_buffer: String,
    seen_questions: Vec<HashSet<String>>,
    pending_questions: Vec<String>,
    last_tick_secs: u64,
    /// Photos taken during the meeting: (id, seconds in, description).
    photos: Vec<(i64, f64, String)>,
    /// Rolling notes on finished stretches of a long meeting, so nothing earlier is ever lost
    /// to the model's context window: (from line, to line, start secs, end secs, notes).
    sections: Vec<Section>,
    sec_upto: usize,
    sec_in_flight: bool,
    /// Quick notes taken in the meeting; each is polished on its own with the words around it.
    jots: Vec<LiveJot>,
    jot_busy: bool,
    /// Live Assistance answers only from this folder (everything when none).
    folder: Option<i64>,
    /// Photos saved during the meeting that Gemma reads afterwards, so it never competes with
    /// the live transcript: (photo id, file path).
    queued_photos: Vec<(i64, String)>,
}

#[derive(Clone)]
struct LiveJot {
    id: i64,
    t: f64,
    kind: String,
    raw: String,
    polished: String,
    ready: bool,
    added: Instant,
    /// How many times later transcript has reworded it, when it was last worded, and how long the transcript was then.
    refined: u8,
    worded_at: Instant,
    worded_lines: usize,
    updated: bool,
}

#[derive(Clone)]
struct Section {
    from: usize,
    to: usize,
    t0: f64,
    t1: f64,
    text: String,
}

/// The whole engine behind the app: settings, the knowledge base, the model workers and the live meeting.
pub struct Core {
    pub paths: Paths,
    settings: RwLock<Settings>,
    engine: Arc<Engine>,
    kb: Arc<Kb>,
    sink: RwLock<Option<Sink>>,
    live: Mutex<Option<Live>>,
    chat_cancels: Mutex<HashMap<u64, Arc<AtomicBool>>>,
    next_turn: AtomicU64,
    /// Photos whose description is still being written.
    describing: AtomicUsize,
    /// When the last provisional (partial) decode was handed to Whisper, and how long one took.
    /// Partials are a nicety; they must never starve the final transcription.
    partial_sent: Mutex<Option<Instant>>,
    partial_cost_ms: AtomicU64,
    /// True from "Stop" until the last words have been transcribed; a new meeting waits for it.
    flushing: AtomicBool,
    /// One model step of background work at a time, handed out in arrival order, so a long
    /// write-up never keeps a newer meeting waiting for its turn.
    finish_gate: parking_lot::FairMutex<()>,
    /// Meetings being written up right now.
    finishing_jobs: AtomicUsize,
    /// Meetings being written up right now, so a rescan never starts the same one twice.
    finishing_ids: Mutex<std::collections::HashSet<i64>>,
    rescanning: Mutex<()>,
    /// Photos still being described, per meeting (a finishing meeting waits for its own).
    describing_by: Mutex<HashMap<i64, usize>>,
    /// Gemma runs on the GPU, so reading a photo during a recording does not slow the transcript.
    llm_on_gpu: AtomicBool,
    /// Folder to file an ingested document under, by ingest job; and documents that finished
    /// before their folder was recorded (the two events can arrive in either order).
    job_folder: Mutex<HashMap<u64, Option<i64>>>,
    /// Running ingest job -> (its row in the saved queue, the file it reads).
    ingest_rows: Mutex<HashMap<u64, (i64, String)>>,
    done_untagged: Mutex<HashMap<u64, i64>>,
    /// What the background workers are doing right now, for the "activity" line in the UI.
    activity: Mutex<std::collections::BTreeMap<&'static str, String>>,
    /// A chat model served from outside this library (LiteRT-LM in Kotlin), when chosen.
    external_llm: Mutex<Option<crate::engine::LlmHandle>>,
    /// EmbeddingGemma served from outside this library (LiteRT-LM in Kotlin), when chosen.
    external_embed: Mutex<Option<crate::engine::EmbedHandle>>,
    engine_state: Mutex<Value>,
    tz: FixedOffset,
}

fn now_secs() -> i64 {
    chrono::Utc::now().timestamp()
}

fn hit_json(i: usize, h: &Hit) -> Value {
    json!({
        "n": i + 1,
        "title": h.title,
        "locator": h.locator(),
        "kind": h.kind.as_str(),
        "snippet": snippet_of(&h.text, 220),
        "quote": h.quote,
        "doc_id": h.doc_id,
        "chunk_id": h.chunk_id,
        "source": h.source,
        "score": h.score,
    })
}

fn meeting_json(m: &Meeting, folder: Option<i64>) -> Value {
    json!({
        "folder_id": folder,
        "id": m.id,
        "title": m.title,
        "started_at": m.started_at,
        "ended_at": m.ended_at,
        "duration_secs": m.duration_secs,
        "notes_raw": m.notes_raw,
        "notes_enhanced": m.notes_enhanced,
        "summary": m.summary,
        "segment_count": m.segment_count,
        "card_count": m.card_count,
    })
}

/// Moves a document's original into the library when it is something worth opening again.
/// Returns where it went and its type.
fn keep_original(data_dir: &Path, doc_id: i64, from: &Path) -> Option<(String, String)> {
    let ext = from.extension()?.to_str()?.to_ascii_lowercase();
    let mime = match ext.as_str() {
        "pdf" => "application/pdf",
        "jpg" | "jpeg" => "image/jpeg",
        "png" => "image/png",
        "webp" => "image/webp",
        "gif" => "image/gif",
        "docx" => "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
        "pptx" => "application/vnd.openxmlformats-officedocument.presentationml.presentation",
        "xlsx" => "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
        "epub" => "application/epub+zip",
        _ => return None,
    };
    if std::fs::metadata(from).ok()?.len() > 150 << 20 {
        return None;
    }
    let dir = data_dir.join("library").join(doc_id.to_string());
    std::fs::create_dir_all(&dir).ok()?;
    let dest = dir.join(from.file_name()?);
    if std::fs::rename(from, &dest).is_err() {
        std::fs::copy(from, &dest).ok()?;
        let _ = std::fs::remove_file(from);
    }
    Some((dest.to_string_lossy().to_string(), mime.to_string()))
}

impl Core {
    pub fn new(paths: Paths, tz_offset_secs: i32) -> Result<Arc<Self>> {
        let settings = Settings::load(&paths);
        // Live speech has first call on the processor; the language model works in the gaps.
        crate::engine::gate::enable(true);
        let engine = Arc::new(Engine::new(paths.clone())?);
        let kb = Kb::open(&paths.db_path)?;
        let _ = kb.store.prune_empty_meetings(None);
        Ok(Arc::new(Self {
            paths,
            settings: RwLock::new(settings),
            engine,
            kb,
            sink: RwLock::new(None),
            live: Mutex::new(None),
            chat_cancels: Mutex::new(HashMap::new()),
            next_turn: AtomicU64::new(1),
            describing: AtomicUsize::new(0),
            external_llm: Mutex::new(None),
            external_embed: Mutex::new(None),
            partial_sent: Mutex::new(None),
            partial_cost_ms: AtomicU64::new(0),
            activity: Mutex::new(Default::default()),
            job_folder: Mutex::new(HashMap::new()),
            ingest_rows: Mutex::new(HashMap::new()),
            done_untagged: Mutex::new(HashMap::new()),
            flushing: AtomicBool::new(false),
            finish_gate: parking_lot::FairMutex::new(()),
            finishing_jobs: AtomicUsize::new(0),
            finishing_ids: Mutex::new(Default::default()),
            rescanning: Mutex::new(()),
            describing_by: Mutex::new(HashMap::new()),
            llm_on_gpu: AtomicBool::new(false),
            engine_state: Mutex::new(json!({"type": "engine", "state": "loading", "label": "Starting"})),
            tz: FixedOffset::east_opt(tz_offset_secs).unwrap_or_else(|| FixedOffset::east_opt(0).unwrap()),
        }))
    }

    /// Start loading models again if a previous attempt did not finish (e.g. the files
    /// were still being unpacked the first time).
    pub fn boot_if_idle(self: &Arc<Self>) {
        let state = self.engine_state.lock().clone();
        let s = state.get("state").and_then(Value::as_str).unwrap_or("");
        if (s == "failed" || (s == "loading" && state.get("step").is_none())) && self.missing_models().is_empty() {
            self.boot();
        }
    }

    pub fn set_external_llm(&self, h: crate::engine::LlmHandle) {
        *self.external_llm.lock() = Some(h);
    }

    pub fn set_external_embedder(&self, h: crate::engine::EmbedHandle) {
        *self.external_embed.lock() = Some(h);
    }

    pub fn set_sink(&self, sink: Option<Sink>) {
        *self.sink.write() = sink;
        // A late subscriber still learns where the engine is.
        let st = self.engine_state.lock().clone();
        self.emit(st);
    }

    fn emit(&self, v: Value) {
        if let Some(s) = self.sink.read().as_ref() {
            s(&v.to_string());
        }
    }

    /// Say what is happening (or, with `None`, that it stopped). Emits only when something changed.
    fn busy(&self, key: &'static str, label: Option<&str>) {
        let changed = {
            let mut a = self.activity.lock();
            match label {
                Some(l) => a.insert(key, l.to_string()).as_deref() != Some(l),
                None => a.remove(key).is_some(),
            }
        };
        if changed {
            let items: Vec<String> = self.activity.lock().values().cloned().collect();
            self.emit(json!({"type": "activity", "items": items}));
        }
    }

    fn toast(&self, kind: &str, message: impl Into<String>) {
        self.emit(json!({"type": "toast", "kind": kind, "message": message.into()}));
    }

    fn set_engine_state(&self, v: Value) {
        *self.engine_state.lock() = v.clone();
        self.emit(v);
    }

    pub fn settings(&self) -> Settings {
        self.settings.read().clone()
    }

    // ---------------------------------------------------------------- boot

    pub fn missing_models(&self) -> Vec<&'static str> {
        let s = self.settings();
        let mut v = vec![models::whisper_spec(s.whisper_model)];
        if self.external_embed.lock().is_none() {
            v.push(models::EMBEDDING_GEMMA_2);
        }
        // With an external chat engine the Gemma weights live in its own file.
        if self.external_llm.lock().is_none() {
            v.push(models::chat_spec(s.chat_model));
            // Photos are described by Gemma and the description is embedded as text, so only
            // the chat model's vision projector is needed.
            if s.vision {
                v.push(models::chat_mmproj_spec(s.chat_model));
            }
        }
        v.into_iter().filter(|m| !models::is_present(&self.paths, m)).map(|m| m.file).collect()
    }

    /// Load the three models on a worker thread, reporting progress as events.
    pub fn boot(self: &Arc<Self>) {
        let core = self.clone();
        std::thread::Builder::new()
            .name("boot".into())
            .spawn(move || {
                if let Err(e) = core.boot_inner() {
                    tracing::error!("boot failed: {e:#}");
                    core.set_engine_state(json!({"type": "engine", "state": "failed", "error": format!("{e:#}")}));
                }
            })
            .expect("spawn boot");
    }

    fn boot_inner(self: &Arc<Self>) -> Result<()> {
        let missing = self.missing_models();
        if !missing.is_empty() {
            return Err(anyhow!("missing model files: {}", missing.join(", ")));
        }
        let settings = self.settings();
        self.set_engine_state(json!({"type": "engine", "state": "loading", "label": "Loading search model", "step": 1, "steps": 3}));
        match self.external_embed.lock().clone() {
            Some(h) => self.engine.attach_embedder(h),
            None => {
                self.engine.load_embedder(&settings)?;
            }
        }
        let embed = self.engine.embed().ok_or_else(|| anyhow!("embedder missing"))?;
        let (itx, irx) = crossbeam_channel::unbounded::<IngestEvent>();
        {
            let core = self.clone();
            std::thread::spawn(move || {
                while let Ok(ev) = irx.recv() {
                    core.on_ingest(ev);
                }
            });
        }
        self.kb.set_chunk_tokens(settings.chunk_tokens);
        self.kb.attach_embedder(embed, itx);

        self.set_engine_state(json!({"type": "engine", "state": "loading", "label": "Loading the AI", "step": 2, "steps": 3}));
        match self.external_llm.lock().clone() {
            Some(h) => self.engine.attach_llm(h),
            None => {
                self.engine.load_llm(&settings)?;
            }
        }
        self.kb.attach_llm(self.engine.llm());

        self.set_engine_state(json!({"type": "engine", "state": "loading", "label": "Loading speech model", "step": 3, "steps": 3}));
        let (ttx, trx) = crossbeam_channel::unbounded::<Transcript>();
        {
            let core = self.clone();
            std::thread::spawn(move || {
                while let Ok(t) = trx.recv() {
                    core.on_transcript(t);
                }
            });
        }
        self.engine.load_whisper(&settings, ttx)?;
        self.kb.attach_whisper(self.engine.whisper());

        self.set_engine_state(json!({"type": "engine", "state": "ready"}));
        self.warm_vision();
        self.reembed_if_needed();
        self.resume_ingests();
        // Unfinished meetings are picked up by the app calling meeting.recover once it knows whether it is in view.
        Ok(())
    }

    /// File the finished document under the folder the person chose when adding it.
    fn tag_job(&self, job: Option<u64>, folder: Option<i64>) {
        let Some(job) = job else { return };
        if folder.is_none() {
            return;
        }
        if let Some(doc) = self.done_untagged.lock().remove(&job) {
            let _ = self.kb.store.set_document_folder(doc, folder);
            return;
        }
        self.job_folder.lock().insert(job, folder);
    }

    /// Adds a file or link to the reading queue, and writes it down first so a restart can carry on.
    fn queue_ingest(&self, kind: &str, target: &str, folder: Option<i64>) -> Option<u64> {
        let row = self.kb.store.add_ingest(kind, target, folder).ok();
        let input = if kind == "url" { IngestInput::Url(target.to_string()) } else { IngestInput::File(target.into()) };
        let job = self.kb.enqueue(input);
        self.tag_job(job, folder);
        if let (Some(j), Some(r)) = (job, row) {
            self.ingest_rows.lock().insert(j, (r, target.to_string()));
        } else if let Some(r) = row {
            let _ = self.kb.store.remove_ingest(r);
        }
        job
    }

    /// Files and links that were waiting (or half read) when the app was ended are queued again.
    fn resume_ingests(self: &Arc<Self>) {
        let Ok(rows) = self.kb.store.pending_ingests() else { return };
        for (row, kind, target, folder) in rows {
            let _ = self.kb.store.remove_ingest(row); // re-added below with a fresh job
            if kind == "file" && !std::path::Path::new(&target).is_file() {
                continue;
            }
            tracing::info!("resuming {kind} {target}");
            self.queue_ingest(&kind, &target, folder);
        }
    }

    fn on_ingest(&self, ev: IngestEvent) {
        if matches!(ev.stage, IngestStage::Done { .. } | IngestStage::Skipped { .. } | IngestStage::Failed { .. }) {
            if let Some((row, target)) = self.ingest_rows.lock().remove(&ev.job_id) {
                let _ = self.kb.store.remove_ingest(row);
                // Copies made for reading live in the app's inbox. A PDF or a picture is kept so it can be
                // opened again from the library, anything else (decoded audio, say) is tidied away.
                let path = std::path::Path::new(&target);
                // /data/data and /data/user/0 are the same place: compare the real paths.
                let inbox = std::fs::canonicalize(self.paths.data_dir.join("inbox")).unwrap_or_else(|_| self.paths.data_dir.join("inbox"));
                let real = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
                if real.starts_with(&inbox) {
                    let kept = match &ev.stage {
                        IngestStage::Done { doc_id, .. } => keep_original(&self.paths.data_dir, *doc_id, &real)
                            .map(|(dest, mime)| self.kb.store.set_document_file(*doc_id, &dest, &mime).is_ok())
                            .unwrap_or(false),
                        _ => false,
                    };
                    if !kept {
                        let _ = std::fs::remove_file(&real);
                    }
                }
            }
        }
        if let IngestStage::Done { doc_id, .. } = &ev.stage {
            match self.job_folder.lock().remove(&ev.job_id) {
                Some(f) => {
                    let _ = self.kb.store.set_document_folder(*doc_id, f);
                }
                None => {
                    self.done_untagged.lock().insert(ev.job_id, *doc_id);
                }
            }
        }
        let (stage, extra) = match &ev.stage {
            IngestStage::Queued => ("queued", json!({})),
            IngestStage::Extracting => ("extracting", json!({})),
            IngestStage::Chunking => ("chunking", json!({})),
            IngestStage::Embedding { done, total } => ("embedding", json!({"done": done, "total": total})),
            IngestStage::Done { doc_id, title, chunks } => ("done", json!({"doc_id": doc_id, "title": title, "chunks": chunks})),
            IngestStage::Skipped { reason } => ("skipped", json!({"reason": reason})),
            IngestStage::Failed { error } => ("failed", json!({"error": error})),
        };
        let mut v = json!({"type": "ingest", "job": ev.job_id, "label": ev.label, "stage": stage});
        if let (Some(o), Some(e)) = (v.as_object_mut(), extra.as_object()) {
            o.extend(e.clone());
        }
        self.emit(v);
    }

    // ---------------------------------------------------------------- dispatch

    pub fn call(self: &Arc<Self>, method: &str, args: &Value) -> Result<Value> {
        let s = |k: &str| args.get(k).and_then(Value::as_str).unwrap_or("").to_string();
        let i = |k: &str| args.get(k).and_then(Value::as_i64).unwrap_or(0);
        match method {
            "engine.state" => Ok(self.engine_state.lock().clone()),
            "settings.get" => Ok(serde_json::to_value(self.settings())?),
            "settings.set" => self.set_settings(args),
            "meeting.list" => Ok(Value::Array({ let f = self.kb.store.meeting_folders().unwrap_or_default(); self.kb.store.list_meetings()?.iter().map(|m| meeting_json(m, f.get(&m.id).copied())).collect() })),
            "meeting.get" => self.meeting_get(i("id")),
            "meeting.start" => self.meeting_start(opt_folder(args)),
            "engine.gpu" => {
                self.llm_on_gpu.store(args.get("gpu").and_then(Value::as_bool).unwrap_or(false), Ordering::Relaxed);
                Ok(json!({}))
            }
            m if m.starts_with("nodes.") => self.nodes_call(m, args),
            "db.checkpoint" => {
                self.kb.store.checkpoint()?;
                Ok(json!({}))
            }
            "meeting.speakers_apply" => {
                // args.segments: [{"start": s, "end": s, "speaker": n}, ...] from the diarizer, in meeting seconds.
                let id = i("id");
                let diar: Vec<(f64, f64, i64)> = args
                    .get("segments")
                    .and_then(Value::as_array)
                    .map(|a| a.iter().filter_map(|x| Some((x.get("start")?.as_f64()?, x.get("end")?.as_f64()?, x.get("speaker")?.as_i64()?))).collect())
                    .unwrap_or_default();
                let segs: Vec<(i64, f64, f64)> = self.kb.store.segments(id)?.into_iter().map(|s| (s.id, s.t0, s.t1)).collect();
                let labels = assign_speakers(&segs, &diar);
                // Speakers are numbered by who spoke first, so "Speaker 1" is stable and readable.
                let mut order: Vec<i64> = Vec::new();
                let mut by_seg: HashMap<i64, i64> = HashMap::new();
                for (seg, spk) in &labels {
                    if !order.contains(spk) {
                        order.push(*spk);
                    }
                    by_seg.insert(*seg, order.iter().position(|x| x == spk).unwrap() as i64);
                }
                let renumbered: Vec<(i64, i64)> = segs.iter().filter_map(|(sid, _, _)| by_seg.get(sid).map(|n| (*sid, *n))).collect();
                self.kb.store.set_segment_speakers(id, &renumbered)?;
                let _ = self.kb.store.sync_meeting_doc_folder(id);
                self.reindex_meeting(id);
                self.emit(json!({"type": "meetings_changed"}));
                Ok(json!({"speakers": order.len(), "labelled": renumbered.len()}))
            }
            "meeting.speaker_rename" => {
                self.kb.store.set_speaker_name(i("id"), i("speaker"), &s("name"))?;
                Ok(json!({}))
            }
            "meeting.speaker_merge" => {
                self.kb.store.merge_speakers(i("id"), i("from"), i("to"))?;
                Ok(json!({}))
            }
            "meeting.speakers_clear" => {
                self.kb.store.clear_speakers(i("id"))?;
                Ok(json!({}))
            }
            "meeting.audio_add" => {
                self.kb.store.add_audio(i("id"), &s("path"), args.get("offset").and_then(Value::as_f64).unwrap_or(0.0))?;
                Ok(json!({}))
            }
            "meeting.ask" => self.meeting_ask(&s("question")),
            "meeting.suggest" => self.meeting_suggest(),
            "meeting.stop" => self.meeting_stop(),
            "meeting.resume" => self.meeting_resume(i("id")),
            "meeting.recover" => {
                let core = self.clone();
                let grace = args.get("grace").and_then(Value::as_u64).unwrap_or(0);
                std::thread::spawn(move || {
                    core.rescan_unfinished(grace);
                });
                Ok(json!({}))
            }
            "meeting.notes" => self.meeting_notes(&s("text")),
            "meeting.style" => self.meeting_style(&s("style")),
            "meeting.rename" => {
                self.kb.store.update_meeting_title(i("id"), s("title").trim())?;
                Ok(json!({}))
            }
            "meeting.delete" => {
                self.delete_meeting(i("id"));
                Ok(json!({}))
            }
            "meeting.photo" => self.meeting_photo(args),
            "meeting.jot" => self.meeting_jot(args),
            "meeting.jot_edit" => self.jot_edit(i("id"), &s("text")),
            "meeting.jot_delete" => {
                self.kb.store.delete_jot(i("id"))?;
                if let Some(l) = self.live.lock().as_mut() {
                    l.jots.retain(|j| j.id != i("id"));
                }
                Ok(json!({}))
            }
            "meeting.photo_read" => self.photo_read_now(i("id")),
            "meeting.photo_delete" => {
                if let Some(path) = self.kb.store.delete_attachment(i("id"))? {
                    let _ = std::fs::remove_file(path);
                }
                if let Some(l) = self.live.lock().as_mut() {
                    l.photos.retain(|p| p.0 != i("id"));
                    l.queued_photos.retain(|p| p.0 != i("id"));
                }
                Ok(json!({}))
            }
            "meeting.live" => Ok(self.live_snapshot()),
            "kb.docs" => self.kb_docs(),
            "kb.ingest" => {
                let job = self.queue_ingest("file", &s("path"), opt_folder(args));
                Ok(json!({"job": job}))
            }
            "kb.ingest_url" => {
                let job = self.queue_ingest("url", &s("url"), opt_folder(args));
                Ok(json!({"job": job}))
            }
            "folders.list" => self.folders_list(),
            "folders.create" => {
                let name = s("name");
                if name.trim().is_empty() {
                    return Err(anyhow!("A folder needs a name"));
                }
                Ok(json!({"id": self.kb.store.create_folder(name.trim(), i("color"))?}))
            }
            "folders.rename" => {
                self.kb.store.update_folder(i("id"), s("name").trim(), i("color"))?;
                Ok(json!({}))
            }
            "folders.delete" => {
                self.kb.store.delete_folder(i("id"))?;
                if let Some(l) = self.live.lock().as_mut() {
                    if l.folder == Some(i("id")) {
                        l.folder = None;
                    }
                }
                Ok(json!({}))
            }
            "meeting.set_folder" => {
                self.kb.store.set_meeting_folder(i("id"), opt_folder(args))?;
                Ok(json!({}))
            }
            "meeting.live_folder" => {
                let f = opt_folder(args);
                let mid = {
                    let mut g = self.live.lock();
                    let Some(l) = g.as_mut() else { return Err(anyhow!("no meeting in progress")) };
                    l.folder = f;
                    l.meeting_id
                };
                self.kb.store.set_meeting_folder(mid, f)?;
                Ok(json!({}))
            }
            "doc.set_folder" => {
                self.kb.store.set_document_folder(i("id"), opt_folder(args))?;
                Ok(json!({}))
            }
            "kb.doc" => {
                let id = i("id");
                let doc = self.kb.documents()?.into_iter().find(|d| d.id == id).ok_or_else(|| anyhow!("document not found"))?;
                let folders = self.kb.store.document_folders().unwrap_or_default();
                let mut prev = String::new();
                let mut chunks: Vec<Value> = Vec::new();
                for (ord, section, page, text) in self.kb.store.doc_chunks(id)? {
                    let shown = strip_overlap(&prev, &text);
                    prev = text;
                    if !shown.trim().is_empty() {
                        chunks.push(json!({"ord": ord, "section": section, "page": page, "text": shown}));
                    }
                }
                Ok(json!({
                    "doc": {"id": doc.id, "folder_id": folders.get(&doc.id), "title": doc.title, "source": doc.source, "kind": doc.kind.as_str(),
                            "bytes": doc.bytes, "added_at": doc.added_at, "chunk_count": doc.chunk_count, "word_count": doc.word_count},
                    "chunks": chunks,
                    "file": self.kb.store.document_file(id).filter(|(p, _)| Path::new(p).is_file()).map(|(p, m)| json!({"path": p, "mime": m})),
                }))
            }
            "kb.delete" => {
                if let Some((path, _)) = self.kb.store.document_file(i("id")) {
                    if let Some(dir) = Path::new(&path).parent() {
                        let _ = std::fs::remove_dir_all(dir);
                    }
                }
                self.kb.remove_document(i("id"))?;
                Ok(json!({}))
            }
            "kb.search" => {
                let hits = self.kb.search_in(&s("query"), 8, scope_of(&s("scope")), opt_folder(args))?;
                Ok(Value::Array(hits.iter().enumerate().map(|(n, h)| hit_json(n, h)).collect()))
            }
            "chat.ask" => self.chat_ask(args),
            "chat.stop" => {
                if let Some(c) = self.chat_cancels.lock().get(&(i("turn") as u64)) {
                    c.store(true, Ordering::Relaxed);
                }
                Ok(json!({}))
            }
            "chat.suggestions" => self.suggestions(opt_folder(args)),
            "stats.get" => self.stats(),
            "debug.embed" => self.debug_embed(args),
            "debug.llm_bench" => self.llm_bench(&s("system"), &s("user"), i("max_tokens") as usize),
            "debug.bench" => self.bench(&s("path")),
            "debug.whisper_bench" => self.debug_whisper_bench(args),
            other => Err(anyhow!("unknown method {other}")),
        }
    }

    fn set_settings(&self, args: &Value) -> Result<Value> {
        let mut s = self.settings();
        if let Some(v) = args.get("language").and_then(Value::as_str) {
            s.language = v.to_string();
            if let Some(w) = self.engine.whisper() {
                w.set_language(v.to_string());
            }
        }
        if let Some(v) = args.get("note_style").and_then(Value::as_str) {
            s.note_style = if v == "Concise" { NoteStyle::Concise } else { NoteStyle::Standard };
        }
        if let Some(v) = args.get("live_assistance").and_then(Value::as_bool) {
            s.live_assistance = v;
        }
        if let Some(v) = args.get("user_name").and_then(Value::as_str) {
            s.user_name = v.to_string();
        }
        if let Some(v) = args.get("appearance").and_then(Value::as_str) {
            s.appearance = v.to_string();
        }
        if let Some(v) = args.get("chunk_tokens").and_then(Value::as_u64) {
            s.chunk_tokens = v as usize;
            self.kb.set_chunk_tokens(s.chunk_tokens);
        }
        if let Err(e) = s.save(&self.paths) {
            tracing::warn!("saving settings: {e:#}");
        }
        *self.settings.write() = s.clone();
        Ok(serde_json::to_value(s)?)
    }

    fn stats(&self) -> Result<Value> {
        let st = self.kb.store.stats()?;
        Ok(json!({
            "meetings": st.meetings,
            "seconds_transcribed": st.seconds_transcribed,
            "words_transcribed": st.words_transcribed,
            "documents": st.documents,
            "chunks": st.chunks,
            "cards": st.cards,
            "questions_answered": st.questions_answered,
        }))
    }

    fn folders_list(&self) -> Result<Value> {
        Ok(Value::Array(
            self.kb
                .store
                .folders()?
                .into_iter()
                .map(|(id, name, color, meetings, docs)| json!({"id": id, "name": name, "color": color, "meetings": meetings, "docs": docs}))
                .collect(),
        ))
    }

    fn kb_docs(&self) -> Result<Value> {
        let docs = self.kb.documents()?;
        let folders = self.kb.store.document_folders().unwrap_or_default();
        Ok(Value::Array(
            docs.iter()
                .map(|d| {
                    json!({
                        "id": d.id, "folder_id": folders.get(&d.id), "title": d.title, "source": d.source, "kind": d.kind.as_str(),
                        "bytes": d.bytes, "added_at": d.added_at, "chunk_count": d.chunk_count, "word_count": d.word_count,
                    })
                })
                .collect(),
        ))
    }

    // ---------------------------------------------------------------- generation helper

    /// Run a generation and wait for it, streaming the running text to `on_text`.
    fn gen_blocking(
        &self,
        messages: Vec<ChatMessage>,
        params: GenParams,
        priority: Priority,
        cancel: Arc<AtomicBool>,
        mut on_text: impl FnMut(&str),
    ) -> Result<(String, GenStats)> {
        let llm = self.engine.llm().ok_or_else(|| anyhow!("chat model is not loaded"))?;
        let (tx, rx) = crossbeam_channel::unbounded::<GenEvent>();
        llm.generate(GenRequest {
            messages,
            images: Vec::new(),
            params,
            priority,
            cancel,
            on_event: Box::new(move |e| {
                let _ = tx.send(e);
            }),
        });
        let mut text = String::new();
        let mut last = Instant::now() - EMIT_EVERY;
        loop {
            match rx.recv() {
                Ok(GenEvent::Token(t)) => {
                    text.push_str(&t);
                    if last.elapsed() >= EMIT_EVERY {
                        on_text(&text);
                        last = Instant::now();
                    }
                }
                Ok(GenEvent::Done(stats)) => {
                    on_text(&text);
                    return Ok((text, stats));
                }
                Ok(GenEvent::Error(e)) => return Err(anyhow!(e)),
                Err(_) => return Err(anyhow!("generation channel closed")),
            }
        }
    }

    pub fn shutdown(&self) {
        self.engine.shutdown();
    }
}

/// `{"folder": 3}` -> Some(3); missing, null or 0 -> None (everything / unfiled).
fn opt_folder(args: &Value) -> Option<i64> {
    args.get("folder").and_then(Value::as_i64).filter(|f| *f > 0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rag::store::Store;

    fn line(t0: f64, t1: f64, text: &str) -> Line {
        Line { t0, t1, text: text.into(), speaker: None, who: String::new() }
    }

    fn temp_store(name: &str) -> (Store, std::path::PathBuf) {
        let dir = std::env::temp_dir().join(format!("marginalia-test-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let store = Store::open(&dir.join("t.sqlite3")).unwrap();
        (store, dir)
    }

    #[test]
    fn merged_lines_join_short_gaps_and_split_long_ones() {
        let mut lines = Vec::new();
        push_merged(&mut lines, 0.0, 2.0, "Hello there");
        push_merged(&mut lines, 2.5, 4.0, "and welcome.");
        assert_eq!(lines.len(), 1, "a short gap joins the line");
        push_merged(&mut lines, 30.0, 32.0, "Next topic.");
        assert_eq!(lines.len(), 2, "a long silence starts a new line");
        assert!(lines[0].text.contains("welcome"));
    }

    #[test]
    fn compiled_jots_are_not_added_twice() {
        let jots = vec![
            ("decision".to_string(), "ship friday".to_string(), "We will ship on Friday.".to_string()),
            ("action".to_string(), "priya db".to_string(), "Priya owns the database migration.".to_string()),
        ];
        let (raw, enhanced) = compile_jots(&jots);
        assert!(enhanced.contains("## Decisions") && enhanced.contains("## Action items"));
        let typed = "my own words";
        let once = format!("{typed}\n\n{enhanced}");
        let stripped = without_block(&once, &enhanced);
        assert_eq!(stripped, typed, "the compiled block comes out cleanly");
        assert_eq!(without_block(&format!("{typed}\n{raw}"), &raw), typed);
        assert_eq!(without_block("nothing here", ""), "nothing here");
    }

    #[test]
    fn saved_sections_must_match_the_transcript() {
        let lines: Vec<Line> = (0..10).map(|i| line(i as f64 * 10.0, i as f64 * 10.0 + 8.0, "text")).collect();
        let good = vec![
            (0, 4, 0.0, 38.0, "first".to_string()),
            (4, 8, 40.0, 78.0, "second".to_string()),
        ];
        let (secs, upto) = valid_sections(good, &lines);
        assert_eq!((secs.len(), upto), (2, 8));

        // a gap, a mismatch in time, or a section that reaches the last line all stop the chain
        let gap = vec![(0, 4, 0.0, 38.0, "a".to_string()), (5, 8, 50.0, 78.0, "b".to_string())];
        assert_eq!(valid_sections(gap, &lines).1, 4);
        let moved = vec![(0, 4, 0.0, 99.0, "a".to_string())];
        assert_eq!(valid_sections(moved, &lines).1, 0);
        let to_end = vec![(0, 10, 0.0, 98.0, "a".to_string())];
        assert_eq!(valid_sections(to_end, &lines).1, 0);
        let empty = vec![(0, 4, 0.0, 38.0, "  ".to_string())];
        assert_eq!(valid_sections(empty, &lines).1, 0);
    }

    #[test]
    fn speakers_follow_the_overlap() {
        let diar = vec![(0.0, 10.0, 7), (10.0, 20.0, 3), (20.0, 30.0, 7)];
        let segs = vec![(1, 1.0, 4.0), (2, 9.0, 13.0), (3, 11.0, 15.0), (4, 21.0, 25.0), (5, 31.0, 32.0), (6, 60.0, 61.0)];
        let got: HashMap<i64, i64> = assign_speakers(&segs, &diar).into_iter().collect();
        assert_eq!(got[&1], 7);
        assert_eq!(got[&2], 3, "most of it falls in the second speaker's stretch");
        assert_eq!(got[&3], 3);
        assert_eq!(got[&4], 7);
        assert_eq!(got[&5], 7, "just past the end takes the nearest speaker");
        assert_eq!(got[&6], 7, "far from everything keeps the speaker before it");
    }

    #[test]
    fn lines_never_mix_speakers() {
        let mut lines = Vec::new();
        push_merged_as(&mut lines, 0.0, 2.0, "Hello there", Some(0), "Ashwani");
        push_merged_as(&mut lines, 2.2, 3.0, "and welcome.", Some(0), "Ashwani");
        push_merged_as(&mut lines, 3.2, 4.0, "thanks", Some(1), "Speaker 2");
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[1].speaker, Some(1));
        let mut names = HashMap::new();
        names.insert(0, "Ashwani".to_string());
        assert_eq!(who_is(&names, Some(0)), "Ashwani");
        assert_eq!(who_is(&names, Some(1)), "Speaker 2");
        assert_eq!(who_is(&names, None), "");
        assert_eq!(spk("Ashwani"), "Ashwani: ");
        assert_eq!(spk(""), "");
    }

    #[test]
    fn speaker_labels_are_stored_and_survive_merging() {
        let (store, dir) = temp_store("speakers");
        let id = store.create_meeting("Meeting · test").unwrap();
        let a = store.add_segment(id, "mic", 0.0, 5.0, "one").unwrap();
        let b = store.add_segment(id, "mic", 6.0, 9.0, "two").unwrap();
        store.set_segment_speakers(id, &[(a, 0), (b, 1)]).unwrap();
        store.set_speaker_name(id, 1, "Priya").unwrap();
        assert_eq!(store.speaker_names(id).unwrap(), vec![(1, "Priya".to_string())]);
        store.merge_speakers(id, 1, 0).unwrap();
        let segs = store.segments(id).unwrap();
        assert!(segs.iter().all(|s| s.speaker == Some(0)));
        assert!(store.speaker_names(id).unwrap().is_empty());
        store.clear_speakers(id).unwrap();
        assert!(store.segments(id).unwrap().iter().all(|s| s.speaker.is_none()));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn overlapping_passages_read_in_a_row() {
        let a = "one two three four five six seven eight nine ten";
        let b = "six seven eight nine ten eleven twelve thirteen";
        assert_eq!(strip_overlap(a, b), "eleven twelve thirteen");
        // a short coincidence is not an overlap
        assert_eq!(strip_overlap("the end of it", "it is a new start"), "it is a new start");
        assert_eq!(strip_overlap("", "first passage"), "first passage");
        // nothing new left
        assert_eq!(strip_overlap(a, "six seven eight nine ten"), "");
    }

    #[test]
    fn a_short_jot_finds_the_lines_that_use_its_words() {
        let lines = vec![
            line(0.0, 5.0, "We should use RAII lock guards for every mutex in the service."),
            line(100.0, 105.0, "Lunch is at noon today."),
            line(200.0, 205.0, "The mutex guards also protect the stats counter."),
            line(300.0, 305.0, "Now the benchmark results from last week."),
        ];
        // the jot is at 300 s: its own window is skipped, the two mutex lines are found, the lunch line is not
        let found = lines_about(&lines, "mutex guard example for wiki", 210.0, 330.0, 3);
        assert!(found.contains("RAII lock guards"));
        assert!(found.contains("stats counter"));
        assert!(!found.contains("Lunch"));
        assert_eq!(lines_about(&lines, "the and for", 0.0, 1.0, 3), "", "only filler words finds nothing");
        let ctx = jot_context(&lines, &[], 300.0, "mutex guard example");
        assert!(ctx.contains("Elsewhere in the meeting"));
    }

    #[test]
    fn a_rewrite_that_leaves_the_jot_behind_is_not_kept() {
        assert_eq!(faithful_jot("prefer semaphore over condition variable", "The decision is to use C++17 parallel algorithms."), "Prefer semaphore over condition variable.");
        assert_eq!(faithful_jot("who releases the semaphore", "Who releases the counting semaphore, the producer or the consumer?"), "Who releases the counting semaphore, the producer or the consumer?");
        assert_eq!(faithful_jot("call priya", ""), "Call priya.");
        assert_eq!(tidy_jot("  ship it?"), "Ship it?");
    }

    #[test]
    fn soup_from_silence_is_dropped_but_speech_is_kept() {
        use crate::engine::whisper::clean_transcript;
        assert_eq!(clean_transcript("ᓊᓍᓊᓂᓉᓃᓆ RRRRRRRRRRensored"), "");
        assert_eq!(clean_transcript("aaaaaaaaaaaa"), "");
        assert!(!clean_transcript("Welcome everyone. Come on in and sit down.").is_empty());
        assert!(!clean_transcript("Wait... what? Really....").is_empty());
        assert!(!clean_transcript("नमस्ते, आप कैसे हैं").is_empty());
    }

    #[test]
    fn windows_always_make_progress() {
        let lines: Vec<Line> = (0..50).map(|i| line(i as f64, i as f64 + 0.9, &"word ".repeat(40))).collect();
        let mut at = 0;
        let mut steps = 0;
        while at < lines.len() {
            let next = window_end(&lines, at, 600);
            assert!(next > at, "a window must move forward");
            at = next;
            steps += 1;
            assert!(steps < 100);
        }
    }

    #[test]
    fn one_line_strips_the_decoration_models_add() {
        assert_eq!(one_line("  - Note: \"Priya owns it.\"  "), "Priya owns it.");
        assert_eq!(one_line("* a\n\n b"), "a b");
    }

    #[test]
    fn a_meeting_that_is_wrapped_stays_wrapped_until_reopened() {
        let (store, dir) = temp_store("wrapped");
        let id = store.create_meeting("Meeting · test").unwrap();
        assert!(!store.is_wrapped(id));
        store.mark_wrapped(id).unwrap();
        assert!(store.is_wrapped(id));
        store.finish_meeting(id, 60).unwrap();
        store.reopen_meeting(id).unwrap();
        store.unmark_wrapped(id).unwrap();
        assert!(!store.is_wrapped(id));
        let m = store.list_meetings().unwrap().into_iter().find(|m| m.id == id).unwrap();
        assert!(m.ended_at.is_none(), "reopened meetings are open again");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn sections_and_the_ingest_queue_survive_in_the_database() {
        let (store, dir) = temp_store("queue");
        let id = store.create_meeting("Meeting · test").unwrap();
        store.save_section(id, 0, 4, 0.0, 38.0, "first").unwrap();
        store.save_section(id, 4, 8, 40.0, 78.0, "second").unwrap();
        store.save_section(id, 0, 4, 0.0, 38.0, "first, again").unwrap();
        let secs = store.sections(id).unwrap();
        assert_eq!(secs.len(), 2);
        assert_eq!(secs[0].4, "first, again");

        let row = store.add_ingest("file", "/tmp/a.txt", Some(3)).unwrap();
        store.add_ingest("url", "https://example.org", None).unwrap();
        let pending = store.pending_ingests().unwrap();
        assert_eq!(pending.len(), 2);
        assert_eq!(pending[0].3, Some(3));
        store.remove_ingest(row).unwrap();
        assert_eq!(store.pending_ingests().unwrap().len(), 1);

        // deleting a meeting takes its saved sections with it
        store.delete_meeting(id).unwrap();
        assert!(store.sections(id).unwrap().is_empty());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn flags_persist() {
        let (store, dir) = temp_store("flags");
        assert!(!store.flag("x"));
        store.set_flag("x", true).unwrap();
        assert!(store.flag("x"));
        store.set_flag("x", false).unwrap();
        assert!(!store.flag("x"));
        let _ = std::fs::remove_dir_all(dir);
    }
}
