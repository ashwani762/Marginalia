//! The phone's brain: everything between the model workers and the screen. Kotlin sends JSON commands and
//! raw PCM; this module answers with JSON and streams JSON events back.

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
/// Long meetings are condensed in stretches of about this many tokens of transcript.
const ASK_LIVE_SYSTEM: &str = "You answer a question that someone asks while a meeting or talk is still going on. Use what was said first, then the library passages if they help, citing a library passage like [1] only when you used it. Answer in at most 70 words, plain and direct, and name who said it when the transcript does. If neither says, reply: That has not been said yet. Never invent.";
const IDEAS_SYSTEM: &str = "You help someone who is listening to a talk or a meeting think of good questions for the speaker. From the transcript, write exactly 3 short questions they could ask. Each question must point at something specific that was said, or at something left unclear or unproven. One question per line, plain words, no numbering, no bullets, no preface.";
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

/// Everything needed to write a finished meeting up, owned by the background job.
struct FinishJob {
    id: i64,
    secs: u64,
    raw: String,
    enhanced: String,
    lines: Vec<Line>,
    jots: Vec<JobJot>,
    sections: Vec<Section>,
    covered: usize,
    title: String,
    /// Tell the person when it is done (not for a quiet recovery at launch).
    notify: bool,
    /// Photos taken during the meeting that still need reading: (id, path, seconds in).
    unread_photos: Vec<(i64, String, Option<f64>)>,
}

#[derive(Clone)]
struct JobJot {
    id: i64,
    t: Option<f64>,
    kind: String,
    raw: String,
    polished: String,
}

#[derive(Clone)]
struct Section {
    from: usize,
    to: usize,
    t0: f64,
    t1: f64,
    text: String,
}

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

fn photo_json(a: &crate::rag::store::Attachment) -> Value {
    json!({
        "id": a.id, "path": a.path, "t": a.t, "caption": a.caption,
        // "queued" until it is actually being read; the live screen overrides it while reading.
        "state": if a.caption.is_empty() { "queued" } else { "ready" },
    })
}

/// "Photos shared" block for prompts: `[mm:ss] description` per photo.
fn photos_text(photos: &[(i64, f64, String)]) -> String {
    photos
        .iter()
        .filter(|p| !p.2.trim().is_empty())
        .map(|(_, t, c)| format!("[{}] {}", fmt_ts(*t), snippet_of(c, 400)))
        .collect::<Vec<_>>()
        .join("\n")
}

fn jot_json(id: i64, t: Option<f64>, kind: &str, raw: &str, polished: &str, ready: bool) -> Value {
    json!({"id": id, "t": t, "kind": kind, "raw": raw, "polished": polished, "state": if ready { "ready" } else { "pending" }})
}
fn live_jot_json(j: &LiveJot) -> Value {
    let mut v = jot_json(j.id, Some(j.t), &j.kind, &j.raw, &j.polished, j.ready);
    v["updated"] = json!(j.updated);
    v
}
fn stored_jot_json(j: &crate::rag::store::Jot) -> Value {
    // A stored jot with no polish is either a plain note or one that never finished: show it as it is.
    jot_json(j.id, j.t, &j.kind, &j.raw, &j.polished, true)
}

/// The jots as the old two-part notes: what was typed, and one tidy Markdown document.
fn compile_jots(jots: &[(String, String, String)]) -> (String, String) {
    // (kind, raw, polished)
    let raw = jots.iter().map(|(_, r, _)| format!("- {r}")).collect::<Vec<_>>().join("\n");
    let mut out = String::new();
    for (kind, heading) in [("decision", "Decisions"), ("action", "Action items"), ("question", "Open questions"), ("note", "Notes")] {
        let items: Vec<String> = jots
            .iter()
            .filter(|(k, _, _)| if kind == "note" { k != "decision" && k != "action" && k != "question" } else { k == kind })
            .map(|(_, r, p)| format!("- {}", if p.trim().is_empty() { r } else { p }))
            .collect();
        if !items.is_empty() {
            out.push_str(&format!("## {heading}\n{}\n\n", items.join("\n")));
        }
    }
    (raw, out.trim().to_string())
}

/// Saved section notes that still match the transcript: unbroken from the start, and the lines agree.
fn valid_sections(saved: Vec<(usize, usize, f64, f64, String)>, lines: &[Line]) -> (Vec<Section>, usize) {
    let mut out: Vec<Section> = Vec::new();
    let mut upto = 0usize;
    for (from, to, t0, t1, text) in saved {
        let fits = from == upto && to > from && to <= lines.len().saturating_sub(1) && (lines[from].t0 - t0).abs() < 0.6 && (lines[to - 1].t1 - t1).abs() < 0.6;
        if !fits || text.trim().is_empty() {
            break;
        }
        out.push(Section { from, to, t0, t1, text });
        upto = to;
    }
    (out, upto)
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

/// Passages overlap on purpose so search never cuts a thought in half. To read them in a row, drop the words
/// a passage repeats from the end of the one before it (only runs of five words or more count).
fn strip_overlap(prev: &str, cur: &str) -> String {
    let a: Vec<&str> = prev.split_whitespace().collect();
    let b: Vec<&str> = cur.split_whitespace().collect();
    let max = a.len().min(b.len()).min(120);
    for k in (5..=max).rev() {
        if a[a.len() - k..] == b[..k] {
            // find the byte offset where the (k+1)th word of `cur` starts
            let mut seen = 0;
            let mut idx = 0;
            for (i, ch) in cur.char_indices() {
                if !ch.is_whitespace() && (i == 0 || cur[..i].ends_with(char::is_whitespace)) {
                    seen += 1;
                    if seen == k + 1 {
                        idx = i;
                        break;
                    }
                }
            }
            return if idx > 0 { cur[idx..].to_string() } else { String::new() };
        }
    }
    cur.to_string()
}

/// The same jots compiled again must replace what an earlier write-up put in the notes, not pile on.
fn without_block(text: &str, block: &str) -> String {
    if block.trim().is_empty() {
        return text.to_string();
    }
    text.replace(block.trim(), "").trim().to_string()
}

fn line_json(i: usize, l: &Line) -> Value {
    json!({ "i": i, "t0": l.t0, "t1": l.t1, "text": l.text, "speaker": l.speaker })
}

/// "Priya: " (or "Speaker 2: ") in front of a line in a prompt, when speakers have been told apart.
fn spk(who: &str) -> String {
    if who.is_empty() { String::new() } else { format!("{who}: ") }
}

/// What a speaker is called: the name the person gave, or "Speaker N".
fn who_is(names: &HashMap<i64, String>, speaker: Option<i64>) -> String {
    match speaker {
        Some(s) => names.get(&s).cloned().unwrap_or_else(|| format!("Speaker {}", s + 1)),
        None => String::new(),
    }
}

/// Give each transcript segment the speaker who talks for most of it. `segments` is (id, t0, t1) and
/// `diar` is (start, end, speaker) from the diarizer. A segment nobody overlaps takes the nearest
/// speaker within 1.5 s, else the one before it.
fn assign_speakers(segments: &[(i64, f64, f64)], diar: &[(f64, f64, i64)]) -> Vec<(i64, i64)> {
    let mut out = Vec::with_capacity(segments.len());
    let mut previous: Option<i64> = None;
    for &(id, t0, t1) in segments {
        let mut best: Option<(i64, f64)> = None;
        let mut totals: std::collections::BTreeMap<i64, f64> = Default::default();
        for &(a, b, spk) in diar {
            let overlap = (t1.min(b) - t0.max(a)).max(0.0);
            if overlap > 0.0 {
                *totals.entry(spk).or_insert(0.0) += overlap;
            }
        }
        for (spk, total) in totals {
            if best.map(|(_, t)| total > t).unwrap_or(true) {
                best = Some((spk, total));
            }
        }
        let who = match best {
            Some((spk, _)) => Some(spk),
            None => {
                let mid = (t0 + t1) / 2.0;
                diar.iter()
                    .map(|&(a, b, spk)| (if mid < a { a - mid } else if mid > b { mid - b } else { 0.0 }, spk))
                    .filter(|(d, _)| *d <= 1.5)
                    .min_by(|x, y| x.0.partial_cmp(&y.0).unwrap_or(std::cmp::Ordering::Equal))
                    .map(|(_, spk)| spk)
                    .or(previous)
            }
        };
        if let Some(w) = who {
            previous = Some(w);
            out.push((id, w));
        }
    }
    out
}

/// Join short continuations into readable paragraphs.
fn push_merged(lines: &mut Vec<Line>, t0: f64, t1: f64, text: &str) -> usize {
    push_merged_as(lines, t0, t1, text, None, "")
}

/// Like [`push_merged`], for a segment whose speaker is known: lines never run two speakers together.
fn push_merged_as(lines: &mut Vec<Line>, t0: f64, t1: f64, text: &str, speaker: Option<i64>, who: &str) -> usize {
    if let Some(last) = lines.last_mut() {
        let words = last.text.split_whitespace().count();
        if last.speaker == speaker
            && t0 - last.t1 < 3.5
            && words < 70
            && (!ends_sentence(&last.text)
                || words < 30
                || text.chars().next().map(|c| c.is_lowercase()).unwrap_or(false))
        {
            if !last.text.ends_with(' ') {
                last.text.push(' ');
            }
            last.text.push_str(text.trim());
            last.t1 = t1;
            return lines.len() - 1;
        }
    }
    lines.push(Line { t0, t1, text: text.trim().to_string(), speaker, who: who.to_string() });
    lines.len() - 1
}

fn transcript_text(lines: &[Line], from: usize, max_chars: usize) -> String {
    let mut out = String::new();
    for l in &lines[from.min(lines.len())..] {
        out.push_str(&format!("[{}] {}{}\n", fmt_ts(l.t0), spk(&l.who), l.text));
    }
    if out.len() > max_chars {
        let mut cut = out.len() - max_chars;
        while !out.is_char_boundary(cut) {
            cut += 1;
        }
        let idx = out[cut..].find('\n').map(|i| cut + i + 1).unwrap_or(cut);
        out = out[idx..].to_string();
    }
    out
}

/// Tokens a window of formatted transcript lines would use.
fn lines_tokens(lines: &[Line]) -> usize {
    lines.iter().map(|l| est_tokens(&l.text) + 6).sum()
}

/// Largest run of lines starting at `from` that fits in `budget` tokens (at least one line).
fn window_end(lines: &[Line], from: usize, budget: usize) -> usize {
    let mut used = 0usize;
    let mut i = from;
    while i < lines.len() {
        let t = est_tokens(&lines[i].text) + 6;
        if used + t > budget && i > from {
            break;
        }
        used += t;
        i += 1;
    }
    i
}

/// Format `[from, to)` as `[mm:ss] text` lines. A single line longer than the budget is trimmed.
fn window_text(lines: &[Line], from: usize, to: usize, budget: usize) -> String {
    let mut out = String::new();
    for l in &lines[from.min(lines.len())..to.min(lines.len())] {
        out.push_str(&format!("[{}] {}{}\n", fmt_ts(l.t0), spk(&l.who), l.text));
    }
    head_tokens(&out, budget)
}

/// Context budget for prompts, in tokens, leaving room for the reply and the chat template.
fn prompt_budget(ctx: u32, reply: usize) -> usize {
    (ctx as usize).saturating_sub(reply + 350).max(1200)
}

/// The words spoken around `t` (a little before, a little after) plus the notes on earlier parts.
fn jot_context(lines: &[Line], sections: &[Section], t: f64, raw: &str) -> String {
    let near: Vec<&Line> = lines.iter().filter(|l| l.t1 >= t - 90.0 && l.t0 <= t + 30.0).collect();
    let mut text = String::new();
    for l in &near {
        text.push_str(&format!("[{}] {}{}\n", fmt_ts(l.t0), spk(&l.who), l.text));
    }
    let text = tail_tokens(&text, 650);
    let earlier = sections.last().map(|s| head_tokens(&s.text, 160)).unwrap_or_default();
    let mut out = if earlier.is_empty() {
        format!("Transcript around that moment:\n{text}")
    } else {
        format!("Earlier in the meeting:\n{earlier}\n\nTranscript around that moment:\n{text}")
    };
    // A short jot often names something said minutes earlier or later: pull in the lines that share its words.
    let elsewhere = lines_about(lines, raw, t - 90.0, t + 30.0, 3);
    if !elsewhere.is_empty() {
        out.push_str("\n\nElsewhere in the meeting, lines that use the same words:\n");
        out.push_str(&elsewhere);
    }
    out
}

/// The lines (outside the window `skip_from..skip_to`) that share the most words with `query`, best first.
fn lines_about(lines: &[Line], query: &str, skip_from: f64, skip_to: f64, take: usize) -> String {
    let words = |t: &str| -> std::collections::HashSet<String> {
        t.split(|c: char| !c.is_alphanumeric())
            .map(|w| w.to_lowercase())
            .filter(|w| w.chars().count() > 3 && !STOP_WORDS.contains(&w.as_str()))
            .collect()
    };
    let q = words(query);
    if q.is_empty() {
        return String::new();
    }
    let mut scored: Vec<(usize, usize)> = lines
        .iter()
        .enumerate()
        .filter(|(_, l)| l.t1 < skip_from || l.t0 > skip_to)
        .filter_map(|(i, l)| {
            let n = words(&l.text).intersection(&q).count();
            if n > 0 { Some((n, i)) } else { None }
        })
        .collect();
    scored.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
    let mut picked: Vec<usize> = scored.into_iter().take(take).map(|(_, i)| i).collect();
    picked.sort();
    let mut out = String::new();
    for i in picked {
        out.push_str(&format!("[{}] {}{}\n", fmt_ts(lines[i].t0), spk(&lines[i].who), snippet_of(&lines[i].text, 260)));
    }
    out
}

const STOP_WORDS: [&str; 24] = [
    "that", "this", "with", "from", "have", "what", "when", "where", "which", "about", "there", "their", "would", "could",
    "should", "into", "than", "then", "them", "they", "will", "your", "were", "been",
];

fn is_default_title(t: &str) -> bool {
    let t = t.trim();
    t.is_empty() || t.starts_with("Untitled") || t.starts_with("Meeting ·")
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
            "debug.bench" => self.bench(&s("path")),
            "debug.whisper_bench" => {
                // Loads another speech model next to the running one, times it on a file, and drops it.
                // args: model, path, lang, threads (default 4), flash (0 or 1), runs (default 1)
                let model = self.paths.models_dir.join(s("model"));
                let (pcm, secs) = crate::audio::decode::decode_to_16k_mono(Path::new(&s("path")))?;
                let (tx, _rx) = crossbeam_channel::unbounded::<Transcript>();
                let threads = args.get("threads").and_then(Value::as_i64).unwrap_or(4) as i32;
                std::env::set_var("MARGINALIA_FLASH", if args.get("flash").and_then(Value::as_i64).unwrap_or(0) == 1 { "1" } else { "0" });
                let t = Instant::now();
                let h = crate::engine::whisper::spawn(
                    crate::engine::whisper::WhisperConfig { model_path: model, use_gpu: false, language: s("lang"), threads },
                    tx,
                )?;
                let load_ms = t.elapsed().as_millis();
                let runs = args.get("runs").and_then(Value::as_i64).unwrap_or(1).max(1);
                let mut best = u128::MAX;
                let mut text = String::new();
                for _ in 0..runs {
                    let t = Instant::now();
                    let utt = h.transcribe_buffer(pcm.clone())?;
                    best = best.min(t.elapsed().as_millis());
                    text = utt.iter().map(|u| u.text.as_str()).collect::<Vec<_>>().join(" ");
                }
                h.shutdown();
                Ok(json!({"threads": threads, "audio_secs": secs, "load_ms": load_ms, "decode_ms": best, "rtf": best as f64 / 1000.0 / secs.max(0.01), "text": snippet_of(&text, 80)}))
            }
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

    /// Times transcription and generation on this device. Used while tuning for phones.
    fn bench(&self, path: &str) -> Result<Value> {
        let w = self.engine.whisper().ok_or_else(|| anyhow!("speech model not loaded"))?;
        let (pcm, secs) = crate::audio::decode::decode_to_16k_mono(Path::new(path))?;
        let t = Instant::now();
        let utt = w.transcribe_buffer(pcm)?;
        let whisper_ms = t.elapsed().as_millis();
        let text: String = utt.iter().map(|u| u.text.as_str()).collect::<Vec<_>>().join(" ");
        let llm = self.engine.llm().ok_or_else(|| anyhow!("chat model not loaded"))?;
        let t = Instant::now();
        let (out, st) = {
            let (tx, rx) = crossbeam_channel::unbounded::<GenEvent>();
            llm.generate(GenRequest {
                images: Vec::new(),
                messages: vec![ChatMessage::user(format!("Summarize in two sentences: {text}"))],
                params: GenParams { max_tokens: 64, temperature: 0.2, ..Default::default() },
                priority: Priority::High,
                cancel: Arc::new(AtomicBool::new(false)),
                on_event: Box::new(move |e| { let _ = tx.send(e); }),
            });
            let mut out = String::new();
            let mut stats = GenStats::default();
            while let Ok(e) = rx.recv() {
                match e { GenEvent::Token(t) => out.push_str(&t), GenEvent::Done(s) => { stats = s; break } GenEvent::Error(e) => return Err(anyhow!(e)) }
            }
            (out, stats)
        };
        Ok(json!({
            "audio_secs": secs, "whisper_ms": whisper_ms, "whisper_rtf": whisper_ms as f64 / 1000.0 / secs.max(0.01),
            "text": text, "llm_ms": t.elapsed().as_millis(), "llm_prompt_tokens": st.prompt_tokens, "llm_tokens": st.generated_tokens,
            "llm_tok_per_s": st.generated_tokens as f64 / (st.elapsed_ms.max(1) as f64 / 1000.0), "llm_out": out,
            "threads": std::thread::available_parallelism().map(|n| n.get()).unwrap_or(0),
        }))
    }

    /// Development check: fabricates a long meeting with facts planted at the start, middle and
    /// end, finishes it through the real wrap-up, and reports whether the summary kept them.
    fn long_meeting_test(self: &Arc<Self>, minutes: usize) -> Result<Value> {
        let r = self.meeting_start(None)?;
        let id = r["id"].as_i64().unwrap_or(0);
        let topics = ["pricing", "the onboarding flow", "hiring plans", "the data pipeline", "customer support load", "mobile performance", "the security review", "partner contracts"];
        let verbs = ["reviewed", "debated", "questioned", "postponed", "benchmarked", "rewrote", "documented", "estimated"];
        let n = minutes * 6; // a line every ten seconds, about 25 words each
        {
            let mut g = self.live.lock();
            let l = g.as_mut().ok_or_else(|| anyhow!("no live meeting"))?;
            for k in 0..n {
                let t0 = k as f64 * 10.0;
                let text = match k {
                    3 => "Before anything else, the project codename for this release is Orchid, please use it in every document from now on.".to_string(),
                    x if x == n / 2 => "Finance approved a budget of forty two thousand dollars for the load test, and Marco owns the spending report.".to_string(),
                    x if x == n - 4 => "To close, we will launch on October thirtieth and Priya owns the database migration, with the dry run on Thursday.".to_string(),
                    _ => format!(
                        "Then we {} {} for a while, noting that item {} moved by {} percent and that team {} still waits for feedback before the next checkpoint.",
                        verbs[k % verbs.len()], topics[(k / 3) % topics.len()], k, (k * 7) % 31 + 2, (k % 5) + 1
                    ),
                };
                let _ = self.kb.store.add_segment(id, "mic", t0, t0 + 8.0, &text);
                push_merged(&mut l.lines, t0, t0 + 8.0, &text);
            }
            l.started = Instant::now() - Duration::from_secs((minutes * 60) as u64);
        }
        let t = Instant::now();
        self.meeting_stop()?;
        // wait for the background write-up to finish
        std::thread::sleep(Duration::from_secs(6));
        let mut waited = 0;
        while (self.live.lock().is_some() || self.flushing.load(Ordering::Relaxed) || self.finishing_jobs.load(Ordering::Relaxed) > 0) && waited < 7200 {
            std::thread::sleep(Duration::from_secs(1));
            waited += 1;
        }
        let m = self.kb.store.list_meetings()?.into_iter().find(|m| m.id == id).ok_or_else(|| anyhow!("meeting vanished"))?;
        let low = m.summary.to_lowercase();
        Ok(json!({
            "minutes": minutes, "lines": n, "wrapup_s": t.elapsed().as_secs(),
            "title": m.title, "summary": m.summary,
            "kept_start_codename": low.contains("orchid"),
            "kept_middle_budget": low.contains("42,000") || low.contains("42000") || low.contains("forty two") || low.contains("42k"),
            "kept_end_launch": low.contains("thirtieth") || low.contains("october 30") || low.contains("oct 30") || low.contains("30th"),
            "kept_end_owner": low.contains("priya"),
        }))
    }

    /// One generation with the numbers that matter: time to first token, prefill and decode speed.
    fn llm_bench(&self, system: &str, user: &str, max_tokens: usize) -> Result<Value> {
        let llm = self.engine.llm().ok_or_else(|| anyhow!("chat model not loaded"))?;
        let (tx, rx) = crossbeam_channel::unbounded::<(Instant, GenEvent)>();
        let t0 = Instant::now();
        llm.generate(GenRequest {
            images: Vec::new(),
            messages: vec![ChatMessage::system(system), ChatMessage::user(user)],
            params: GenParams { max_tokens, temperature: 0.2, ..Default::default() },
            priority: Priority::High,
            cancel: Arc::new(AtomicBool::new(false)),
            on_event: Box::new(move |e| { let _ = tx.send((Instant::now(), e)); }),
        });
        let mut first: Option<Instant> = None;
        let mut out = String::new();
        let mut stats = GenStats::default();
        let mut end = t0;
        while let Ok((at, e)) = rx.recv() {
            match e {
                GenEvent::Token(t) => { first.get_or_insert(at); out.push_str(&t); }
                GenEvent::Done(s) => { stats = s; end = at; break }
                GenEvent::Error(e) => return Err(anyhow!(e)),
            }
        }
        let ttft = first.map(|f| f.duration_since(t0).as_secs_f64()).unwrap_or(0.0);
        let decode_s = first.map(|f| end.duration_since(f).as_secs_f64()).unwrap_or(0.0).max(1e-3);
        Ok(json!({
            "engine": "litert-lm", "ttft_s": ttft, "total_s": end.duration_since(t0).as_secs_f64(),
            "prefill_tokens": stats.prompt_tokens, "prefill_tok_s": stats.prompt_tokens as f64 / ttft.max(1e-3),
            "decode_tokens": stats.generated_tokens, "decode_tok_s": (stats.generated_tokens.saturating_sub(1)) as f64 / decode_s,
            "text": out,
        }))
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

    fn suggestions(&self, folder: Option<i64>) -> Result<Value> {
        let short = |t: &str| snippet_of(t.trim(), 38);
        let mut out: Vec<String> = Vec::new();
        let mfolders = self.kb.store.meeting_folders().unwrap_or_default();
        let dfolders = self.kb.store.document_folders().unwrap_or_default();
        let meetings: Vec<Meeting> = self
            .kb
            .store
            .list_meetings()
            .unwrap_or_default()
            .into_iter()
            .filter(|m| folder.is_none() || mfolders.get(&m.id).copied() == folder)
            .collect();
        if let Some(m) = meetings.iter().find(|m| !is_default_title(&m.title) && !m.summary.trim().is_empty()) {
            out.push(format!("What were the key points of “{}”?", short(&m.title)));
        } else if let Some(m) = meetings.iter().find(|m| m.segment_count > 0) {
            let day = chrono::DateTime::from_timestamp(m.started_at, 0)
                .map(|d| d.with_timezone(&self.tz).format("%b %-d").to_string())
                .unwrap_or_default();
            out.push(format!("What was said in the meeting on {day}?"));
        }
        if meetings.iter().filter(|m| !m.summary.trim().is_empty()).count() >= 2 {
            out.push("List every action item from my recent meetings".to_string());
        }
        for d in self.kb.documents().unwrap_or_default().iter().filter(|d| d.kind != DocKind::Meeting && (folder.is_none() || dfolders.get(&d.id).copied() == folder)).take(2) {
            out.push(format!("Summarize “{}”", short(&d.title)));
        }
        out.truncate(3);
        Ok(json!(out))
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

    // ---------------------------------------------------------------- live meeting

    fn meeting_start(self: &Arc<Self>, folder: Option<i64>) -> Result<Value> {
        // The previous meeting is still taking in its last words (a few seconds at most).
        let t = Instant::now();
        while self.flushing.load(Ordering::Relaxed) && t.elapsed() < Duration::from_secs(30) {
            std::thread::sleep(Duration::from_millis(100));
        }
        if self.live.lock().is_some() {
            return Err(anyhow!("a meeting is already in progress"));
        }
        if self.engine.whisper().is_none() {
            return Err(anyhow!("The speech model is still loading."));
        }
        let title = format!(
            "Meeting · {}",
            chrono::DateTime::from_timestamp(now_secs(), 0)
                .map(|d| d.with_timezone(&self.tz).format("%b %-d, %H:%M").to_string())
                .unwrap_or_default()
        );
        // A new recording starts with whatever language setting the person chose (auto lets Whisper decide again).
        if let Some(w) = self.engine.whisper() {
            w.set_language(self.settings().language.clone());
        }
        let meeting_id = self.kb.store.create_meeting(&title)?;
        if folder.is_some() {
            self.kb.store.set_meeting_folder(meeting_id, folder)?;
        }
        *self.live.lock() = Some(Self::blank_live(meeting_id, folder));
        tracing::info!("meeting {meeting_id} started");
        let core = self.clone();
        std::thread::Builder::new()
            .name("live-tick".into())
            .spawn(move || core.tick_loop(meeting_id))
            .expect("spawn tick");
        Ok(json!({"id": meeting_id, "title": title}))
    }

    /// A fresh live session for a meeting: nothing heard yet.
    fn blank_live(meeting_id: i64, folder: Option<i64>) -> Live {
        let past = Instant::now() - Duration::from_secs(3600);
        Live {
            meeting_id,
            started: Instant::now(),
            time_offset: 0.0,
            recording: true,
            segmenter: Segmenter::new(AudioSource::Microphone),
            lines: Vec::new(),
            partial: String::new(),
            last_final: -1.0,
            notes: String::new(),
            notes_dirty: false,
            last_note_edit: past,
            last_enhance: Instant::now(),
            enhance_gen: 0,
            enhance_in_flight: false,
            enhance_cancel: None,
            enhanced_text: String::new(),
            enhance_buffer: String::new(),
            enhance_lines_seen: 0,
            enhance_emit: past,
            cards: Vec::new(),
            next_card_id: 1,
            scanned_upto: 0,
            last_scan: Instant::now(),
            scan_in_flight: false,
            scan_gen: 0,
            scan_buffer: String::new(),
            seen_questions: Vec::new(),
            pending_questions: Vec::new(),
            last_tick_secs: 0,
            photos: Vec::new(),
            sections: Vec::new(),
            sec_upto: 0,
            sec_in_flight: false,
            jots: Vec::new(),
            jot_busy: false,
            folder,
            queued_photos: Vec::new(),
        }
    }

    /// Carries on recording an earlier meeting: the same entry, the same transcript, the same notes.
    /// The write-up is redone from everything once the recording ends.
    fn meeting_resume(self: &Arc<Self>, id: i64) -> Result<Value> {
        if self.live.lock().is_some() {
            return Err(anyhow!("a meeting is already in progress"));
        }
        if self.finishing_ids.lock().contains(&id) {
            return Err(anyhow!("The notes for this meeting are still being written. Try again in a moment."));
        }
        if self.engine.whisper().is_none() {
            return Err(anyhow!("The speech model is still loading."));
        }
        if let Some(w) = self.engine.whisper() {
            w.set_language(self.settings().language.clone());
        }
        let m = self.kb.store.list_meetings()?.into_iter().find(|m| m.id == id).ok_or_else(|| anyhow!("meeting not found"))?;
        let names: HashMap<i64, String> = self.kb.store.speaker_names(id).unwrap_or_default().into_iter().collect();
        let mut lines: Vec<Line> = Vec::new();
        let mut last = 0.0f64;
        for s in self.kb.store.segments(id)? {
            last = last.max(s.t1);
            push_merged_as(&mut lines, s.t0, s.t1, &s.text, s.speaker, &who_is(&names, s.speaker));
        }
        let offset = last.max(m.duration_secs.max(0) as f64) + 1.0;
        let folder = self.kb.store.meeting_folder(id);
        let mut live = Self::blank_live(id, folder);
        live.started = Instant::now() - Duration::from_secs_f64(offset);
        live.time_offset = offset;
        live.last_final = offset - 1.0;
        live.lines = lines;
        // What the person typed, without the jots an earlier write-up put into the notes.
        let stored_jots = self.kb.store.jots(id)?;
        let tuples: Vec<(String, String, String)> = stored_jots.iter().map(|j| (j.kind.clone(), j.raw.clone(), j.polished.clone())).collect();
        let (jr, je) = compile_jots(&tuples);
        live.notes = without_block(&m.notes_raw, &jr);
        live.enhanced_text = without_block(&m.notes_enhanced, &je);
        live.enhance_lines_seen = live.lines.len();
        live.scanned_upto = live.lines.len(); // earlier questions were already looked at
        let (secs_saved, covered) = valid_sections(self.kb.store.sections(id).unwrap_or_default(), &live.lines);
        live.sections = secs_saved;
        live.sec_upto = covered;
        for j in self.kb.store.jots(id)? {
            let ready = !j.polished.trim().is_empty() || j.kind == "keep";
            live.jots.push(LiveJot { id: j.id, t: j.t.unwrap_or(0.0), kind: j.kind, raw: j.raw, polished: j.polished, ready, added: Instant::now(), refined: 2, worded_at: Instant::now(), worded_lines: usize::MAX / 2, updated: false });
        }
        for a in self.kb.store.attachments(id)? {
            live.photos.push((a.id, a.t.unwrap_or(0.0), a.caption.clone()));
        }
        // The meeting is open again until it is stopped, and its notes are no longer final.
        self.kb.store.reopen_meeting(id)?;
        self.kb.store.unmark_wrapped(id)?;
        *self.live.lock() = Some(live);
        tracing::info!("meeting {id} continued from {offset:.0}s");
        let core = self.clone();
        std::thread::Builder::new()
            .name("live-tick".into())
            .spawn(move || core.tick_loop(id))
            .expect("spawn tick");
        self.emit(json!({"type": "meetings_changed"}));
        Ok(json!({"id": id, "title": m.title, "snapshot": self.live_snapshot()}))
    }

    fn live_snapshot(&self) -> Value {
        let g = self.live.lock();
        match g.as_ref() {
            None => json!({"active": false}),
            Some(l) => json!({
                "active": true,
                "recording": l.recording,
                "meeting_id": l.meeting_id,
                "secs": l.started.elapsed().as_secs(),
                "lines": l.lines.iter().enumerate().map(|(i, x)| line_json(i, x)).collect::<Vec<_>>(),
                "partial": l.partial,
                "notes": l.notes,
                "enhanced": l.enhanced_text,
                "cards": l.cards.iter().rev().map(Card::json).collect::<Vec<_>>(),
                "photos": self.kb.store.attachments(l.meeting_id).unwrap_or_default().iter().map(photo_json).collect::<Vec<_>>(),
                "jots": l.jots.iter().map(live_jot_json).collect::<Vec<_>>(),
            }),
        }
    }

    /// 16 kHz mono PCM from the microphone. Called from the Kotlin audio thread.
    pub fn push_audio(&self, pcm: &[f32]) -> f32 {
        let (segs, level) = {
            let mut g = self.live.lock();
            let Some(l) = g.as_mut().filter(|l| l.recording) else {
                return 0.0;
            };
            let off = l.time_offset;
            let mut segs = l.segmenter.push(pcm);
            if off > 0.0 {
                for s in segs.iter_mut() {
                    s.t0 += off;
                    s.t1 += off;
                }
            }
            (segs, l.segmenter.level)
        };
        if let Some(w) = self.engine.whisper() {
            for s in segs {
                if s.partial {
                    // A provisional re-decode is wasted work while Whisper is behind, and when one
                    // costs more than a second the live preview would only add lag, so space them out.
                    if w.queue_len() > 0 {
                        continue;
                    }
                    let cost = self.partial_cost_ms.load(Ordering::Relaxed);
                    let mut sent = self.partial_sent.lock();
                    if let Some(at) = *sent {
                        let waited = at.elapsed().as_millis() as u64;
                        // One in flight (give up on it after 8 s), then rest for twice its cost.
                        if waited < 8000 && self.partial_cost_ms.load(Ordering::Relaxed) == u64::MAX {
                            continue;
                        }
                        if waited < cost.saturating_mul(2).min(6000) {
                            continue;
                        }
                    }
                    *sent = Some(Instant::now());
                    self.partial_cost_ms.store(u64::MAX, Ordering::Relaxed); // in flight
                } else {
                    tracing::debug!("submit final {:.1}-{:.1}s ({} samples), queue={}", s.t0, s.t1, s.pcm.len(), w.queue_len());
                }
                w.transcribe(s);
            }
        }
        level
    }

    pub fn on_transcript(&self, t: Transcript) {
        let mut g = self.live.lock();
        let Some(l) = g.as_mut() else { return };
        if t.partial {
            if let Some(at) = *self.partial_sent.lock() {
                self.partial_cost_ms.store(at.elapsed().as_millis() as u64, Ordering::Relaxed);
            }
            if !l.recording || t.t1 <= l.last_final + 0.05 {
                return;
            }
            // Hold back the last word: it is usually cut mid-syllable.
            let words: Vec<&str> = t.text.split_whitespace().collect();
            let shown = if words.len() >= 3 {
                words[..words.len() - 1].join(" ")
            } else if words.len() < 2 {
                String::new()
            } else {
                t.text.trim().to_string()
            };
            l.partial = shown.clone();
            drop(g);
            self.emit(json!({"type": "partial", "text": shown}));
            return;
        }
        l.partial.clear();
        l.last_final = l.last_final.max(t.t1);
        if t.text.trim().is_empty() {
            drop(g);
            self.emit(json!({"type": "partial", "text": ""}));
            return;
        }
        tracing::info!("final t={:.1}-{:.1}s lag={:.1}s queue={} words={}", t.t0, t.t1, l.started.elapsed().as_secs_f64() - t.t1, self.engine.whisper().map(|w| w.queue_len()).unwrap_or(0), t.text.split_whitespace().count());
        let _ = self.kb.store.add_segment(l.meeting_id, "mic", t.t0, t.t1, &t.text);
        if is_question(&t.text) {
            l.pending_questions.push(t.text.clone());
        }
        let idx = push_merged(&mut l.lines, t.t0, t.t1, &t.text);
        let line = line_json(idx, &l.lines[idx]);
        drop(g);
        self.emit(json!({"type": "partial", "text": ""}));
        self.emit(json!({"type": "line", "line": line}));
    }

    fn meeting_notes(&self, text: &str) -> Result<Value> {
        if let Some(l) = self.live.lock().as_mut() {
            if l.notes != text {
                l.notes = text.to_string();
                l.notes_dirty = true;
                l.last_note_edit = Instant::now();
            }
        }
        Ok(json!({}))
    }

    fn meeting_style(&self, style: &str) -> Result<Value> {
        let mut s = self.settings();
        s.note_style = if style == "Concise" { NoteStyle::Concise } else { NoteStyle::Standard };
        let _ = s.save(&self.paths);
        *self.settings.write() = s;
        if let Some(l) = self.live.lock().as_mut() {
            l.notes_dirty = true;
            l.last_note_edit = Instant::now() - ENHANCE_DEBOUNCE;
            l.last_enhance = Instant::now() - ENHANCE_MIN_GAP;
        }
        Ok(json!({}))
    }

    fn tick_loop(self: Arc<Self>, meeting_id: i64) {
        loop {
            std::thread::sleep(TICK);
            let mut work = Work::default();
            {
                let mut g = self.live.lock();
                let Some(l) = g.as_mut().filter(|l| l.meeting_id == meeting_id) else { return };
                if !l.recording {
                    return;
                }
                let secs = l.started.elapsed().as_secs();
                if secs != l.last_tick_secs {
                    l.last_tick_secs = secs;
                    work.tick = Some(secs);
                }
                // Transcription that is falling behind has priority over notes and cards.
                let busy = self.engine.whisper().map(|w| w.queue_len() > 1).unwrap_or(false);
                let settled = l.last_note_edit.elapsed() >= ENHANCE_DEBOUNCE;
                let new_transcript = l.lines.len() > l.enhance_lines_seen;
                let want_enhance = !l.notes.trim().is_empty()
                    && settled
                    && (l.notes_dirty || (new_transcript && l.last_enhance.elapsed() >= ENHANCE_TRANSCRIPT_GAP))
                    && l.last_enhance.elapsed() >= ENHANCE_MIN_GAP;
                if want_enhance && !l.enhance_in_flight && !busy {
                    work.enhance = true;
                }
                // A jot waits a few seconds so the words that follow it are in the transcript too.
                if !l.jot_busy && self.engine.llm().is_some() {
                    if let Some(j) = l.jots.iter().find(|j| !j.ready && j.added.elapsed() >= JOT_SETTLE) {
                        let ctx = jot_context(&l.lines, &l.sections, j.t, &j.raw);
                        l.jot_busy = true;
                        work.jot = Some((j.id, j.kind.clone(), j.raw.clone(), ctx));
                    }
                }
                // A finished jot is looked at again when the talk after it adds or changes something.
                if !l.jot_busy && !busy && work.jot.is_none() && self.engine.llm().is_some() && !l.jots.iter().any(|j| !j.ready) {
                    let now_lines = l.lines.len();
                    let t_now = l.started.elapsed().as_secs_f64();
                    if let Some(j) = l.jots.iter().find(|j| {
                        j.ready && j.refined < 2 && !j.polished.trim().is_empty() && j.kind != "keep"
                            && now_lines >= j.worded_lines + 3
                            && j.worded_at.elapsed() >= Duration::from_secs(if j.refined == 0 { 50 } else { 150 })
                            && t_now - j.t >= 40.0
                    }) {
                        let ctx = jot_context(&l.lines, &l.sections, j.t + 45.0, &j.raw);
                        l.jot_busy = true;
                        work.refine = Some((j.id, j.kind.clone(), j.raw.clone(), j.polished.clone(), ctx));
                    }
                }
                {
                    let behind = self.engine.whisper().map(|w| w.queue_len() >= 2).unwrap_or(false);
                    self.busy("lag", if behind { Some("Transcribing") } else { None });
                    self.busy("section", if l.sec_in_flight { Some("Condensing earlier minutes") } else { None });
                    let working = l.cards.iter().any(|c| matches!(c.state, CardState::Retrieving | CardState::Answering));
                    self.busy("assist", if working { Some("Looking up an answer") } else { None });
                    let photos = self.describing.load(Ordering::Relaxed) > 0;
                    self.busy("photo", if photos { Some("Reading a photo") } else { None });
                }
                // Long meeting: condense finished stretches into notes while it is still going on.
                if !busy && !l.sec_in_flight && self.engine.llm().is_some() {
                    let from = l.sec_upto;
                    let end_cap = l.lines.len().saturating_sub(1); // the last line may still grow
                    if from < end_cap && lines_tokens(&l.lines[from..end_cap]) >= SECTION_TRIGGER_TOKENS {
                        let to = window_end(&l.lines[..end_cap], from, SECTION_WINDOW_TOKENS);
                        if to > from {
                            l.sec_in_flight = true;
                            let text = window_text(&l.lines, from, to, SECTION_WINDOW_TOKENS + 200);
                            work.section = Some((from, to, l.lines[from].t0, l.lines[to - 1].t1, text));
                        }
                    }
                }
                if self.settings.read().live_assistance && !busy {
                    let unscanned = l.lines.len().saturating_sub(l.scanned_upto);
                    if !l.scan_in_flight && unscanned > 0 && (unscanned >= SCAN_MIN_LINES || l.last_scan.elapsed() >= SCAN_INTERVAL) {
                        work.scan = true;
                    }
                    if !l.pending_questions.is_empty() {
                        work.question = Some(l.pending_questions.remove(0));
                    }
                }
            }
            if let Some(s) = work.tick {
                self.emit(json!({"type": "tick", "secs": s}));
            }
            if work.enhance {
                self.run_enhance();
            }
            if work.scan {
                self.run_scan();
            }
            if let Some((id, kind, raw, ctx)) = work.jot.take() {
                self.spawn_polish(Some(id), kind, raw, ctx);
            }
            if let Some((id, kind, raw, current, ctx)) = work.refine.take() {
                self.spawn_refine(id, kind, raw, current, ctx);
            }
            if let Some((from, to, t0, t1, text)) = work.section.take() {
                self.spawn_section(from, to, t0, t1, text);
            }
            if let Some(q) = work.question {
                self.create_card(q);
            }
        }
    }

    // ---- Live Enhance

    fn run_enhance(self: &Arc<Self>) {
        let Some(llm) = self.engine.llm() else { return };
        let style = match self.settings().note_style {
            NoteStyle::Standard => "Standard: complete sentences, keep every point, group under short headings when natural.",
            NoteStyle::Concise => "Concise: tight bullets, one line each, no filler, keep names/numbers/decisions.",
        };
        let (gen, cancel, user) = {
            let mut g = self.live.lock();
            let Some(l) = g.as_mut() else { return };
            if let Some(c) = l.enhance_cancel.take() {
                c.store(true, Ordering::Relaxed);
            }
            l.enhance_gen += 1;
            l.enhance_in_flight = true;
            l.notes_dirty = false;
            l.last_enhance = Instant::now();
            l.enhance_lines_seen = l.lines.len();
            l.enhance_buffer.clear();
            let cancel = Arc::new(AtomicBool::new(false));
            l.enhance_cancel = Some(cancel.clone());
            let budget = prompt_budget(self.settings.read().chat_context, 450);
            let notes = tail_tokens(&l.notes, budget / 4);
            let photos = head_tokens(&photos_text(&l.photos), budget / 8);
            let photos = if photos.is_empty() { String::new() } else { format!("Photos shared in the meeting:\n{photos}\n\n") };
            let earlier = {
                let all: String = l.sections.iter().map(|s| format!("[{}–{}] {}\n", fmt_ts(s.t0), fmt_ts(s.t1), s.text)).collect();
                let t = tail_tokens(&all, budget / 4);
                if t.is_empty() { String::new() } else { format!("Notes on earlier parts of the meeting:\n{t}\n\n") }
            };
            let used = est_tokens(&notes) + est_tokens(&photos) + est_tokens(&earlier) + 120;
            let recent_budget = budget.saturating_sub(used).clamp(300, 1600);
            let recent_from = l.sec_upto.min(l.lines.len());
            let recent = tail_tokens(&window_text(&l.lines, recent_from, l.lines.len(), usize::MAX / 8), recent_budget);
            let user = format!(
                "{earlier}Transcript so far (most recent last):\n{recent}\n\n{photos}My shorthand notes:\n{notes}\n\nStyle: {style}\n\nRewrite my notes now.",
            );
            (l.enhance_gen, cancel, user)
        };
        let core = self.clone();
        llm.generate(GenRequest {
            images: Vec::new(),
            messages: vec![ChatMessage::system(ENHANCE_SYSTEM), ChatMessage::user(user)],
            params: GenParams { max_tokens: 450, temperature: 0.25, ..Default::default() },
            priority: Priority::High,
            cancel,
            on_event: Box::new(move |ev| core.on_enhance_event(gen, ev)),
        });
        self.emit(json!({"type": "enhance_state", "busy": true}));
    }

    fn on_enhance_event(&self, gen: u64, ev: GenEvent) {
        let mut emit = None;
        {
            let mut g = self.live.lock();
            let Some(l) = g.as_mut() else { return };
            if l.enhance_gen != gen {
                return;
            }
            match ev {
                GenEvent::Token(t) => {
                    l.enhance_buffer.push_str(&t);
                    // The first version streams live; later ones swap in when complete.
                    if l.enhanced_text.is_empty() && l.enhance_emit.elapsed() >= EMIT_EVERY {
                        l.enhance_emit = Instant::now();
                        emit = Some(json!({"type": "enhance", "text": l.enhance_buffer, "streaming": true}));
                    }
                }
                GenEvent::Done(stats) => {
                    l.enhance_in_flight = false;
                    if !stats.preempted && !l.enhance_buffer.trim().is_empty() {
                        l.enhanced_text = std::mem::take(&mut l.enhance_buffer);
                    }
                    l.enhance_buffer.clear();
                    let _ = self.kb.store.update_meeting_notes(l.meeting_id, &l.notes, &l.enhanced_text);
                    emit = Some(json!({"type": "enhance", "text": l.enhanced_text, "streaming": false}));
                }
                GenEvent::Error(e) => {
                    l.enhance_in_flight = false;
                    tracing::warn!("enhance failed: {e}");
                    emit = Some(json!({"type": "enhance_state", "busy": false}));
                }
            }
        }
        if let Some(v) = emit {
            self.emit(v);
        }
    }

    /// Writes notes for one finished stretch of the meeting in the background. Low priority:
    /// it yields to anything the person is waiting for.
    fn spawn_section(self: &Arc<Self>, from: usize, to: usize, t0: f64, t1: f64, text: String) {
        let core = self.clone();
        std::thread::Builder::new()
            .name("section".into())
            .spawn(move || {
                let r = core.gen_blocking(
                    vec![ChatMessage::system(SECTION_SYSTEM), ChatMessage::user(format!("Part of the meeting:\n{text}\n\nWrite the notes for this part."))],
                    GenParams { max_tokens: 260, temperature: 0.2, ..Default::default() },
                    Priority::Low,
                    Arc::new(AtomicBool::new(false)),
                    |_| {},
                );
                let mut g = core.live.lock();
                let Some(l) = g.as_mut() else { return };
                l.sec_in_flight = false;
                match r {
                    Ok((t, st)) if !st.preempted && !t.trim().is_empty() && l.sec_upto == from => {
                        let _ = core.kb.store.save_section(l.meeting_id, from, to, t0, t1, t.trim());
                        l.sections.push(Section { from, to, t0, t1, text: t.trim().to_string() });
                        l.sec_upto = to;
                    }
                    Ok(_) => {}
                    Err(e) => tracing::warn!("section notes failed: {e}"),
                }
            })
            .expect("spawn section");
    }

    // ---- Live Assistance

    fn run_scan(self: &Arc<Self>) {
        let Some(llm) = self.engine.llm() else { return };
        let (gen, user) = {
            let mut g = self.live.lock();
            let Some(l) = g.as_mut() else { return };
            let window = transcript_text(&l.lines, l.scanned_upto.saturating_sub(2), 3000);
            l.scanned_upto = l.lines.len();
            l.last_scan = Instant::now();
            l.scan_in_flight = true;
            l.scan_gen += 1;
            l.scan_buffer.clear();
            (
                l.scan_gen,
                format!("Transcript window:\n{window}\n\nList the questions or lookups. Output only a JSON array of strings."),
            )
        };
        let core = self.clone();
        llm.generate(GenRequest {
            images: Vec::new(),
            messages: vec![ChatMessage::system(SCAN_SYSTEM), ChatMessage::user(user)],
            params: GenParams { max_tokens: 160, temperature: 0.1, ..Default::default() },
            priority: Priority::Low,
            cancel: Arc::new(AtomicBool::new(false)),
            on_event: Box::new(move |ev| {
                let mut g = core.live.lock();
                let Some(l) = g.as_mut() else { return };
                if l.scan_gen != gen {
                    return;
                }
                match ev {
                    GenEvent::Token(t) => l.scan_buffer.push_str(&t),
                    GenEvent::Done(stats) => {
                        l.scan_in_flight = false;
                        if stats.preempted {
                            l.scanned_upto = l.scanned_upto.saturating_sub(2);
                        } else {
                            let qs = parse_questions(&l.scan_buffer);
                            l.pending_questions.extend(qs);
                        }
                    }
                    GenEvent::Error(_) => l.scan_in_flight = false,
                }
            }),
        });
    }

    fn create_card(self: &Arc<Self>, question: String) {
        let question = question.trim().trim_matches('"').to_string();
        if question.split_whitespace().count() < 3 {
            return;
        }
        let id = {
            let mut g = self.live.lock();
            let Some(l) = g.as_mut() else { return };
            let words = word_set(&question);
            if words.len() < 2 || l.seen_questions.iter().any(|s| jaccard(s, &words) >= 0.6) {
                return;
            }
            l.seen_questions.push(words);
            let id = l.next_card_id;
            l.next_card_id += 1;
            l.cards.insert(
                0,
                Card { kind: "", t: l.started.elapsed().as_secs_f64(), id, db_id: None, question: question.clone(), answer: String::new(), hits: Vec::new(), state: CardState::Retrieving, gen: 0 },
            );
            l.cards.truncate(12);
            self.emit(json!({"type": "card", "card": l.cards[0].json()}));
            id
        };
        let core = self.clone();
        std::thread::Builder::new()
            .name("card".into())
            .spawn(move || {
                let folder = core.live.lock().as_ref().and_then(|l| l.folder);
                let hits = core.kb.search_in(&question, 4, Scope::All, folder).unwrap_or_default();
                core.on_card_hits(id, hits);
            })
            .expect("spawn card");
    }

    fn on_card_hits(self: &Arc<Self>, id: u64, hits: Vec<Hit>) {
        let strong = hits.iter().any(|h| h.dense >= 0.40 || h.lexical >= 0.55);
        let mut request = None;
        {
            let mut g = self.live.lock();
            let Some(l) = g.as_mut() else { return };
            let meeting_id = l.meeting_id;
            let Some(pos) = l.cards.iter().position(|c| c.id == id) else { return };
            if hits.is_empty() || !strong {
                l.cards[pos].state = CardState::NoMatch;
                let json = l.cards[pos].json();
                // Keep only a few quiet "heard this" entries.
                let no_match = l.cards.iter().filter(|c| c.state == CardState::NoMatch).count();
                if no_match > 3 {
                    if let Some(old) = l.cards.iter().rposition(|c| c.state == CardState::NoMatch) {
                        let gone = l.cards.remove(old).id;
                        self.emit(json!({"type": "card_remove", "id": gone}));
                    }
                }
                self.emit(json!({"type": "card", "card": json}));
                return;
            }
            let passages = hits
                .iter()
                .enumerate()
                .map(|(i, h)| format!("[{}] {} ({})\n{}", i + 1, h.title, h.locator(), snippet_of(&h.context, 1200)))
                .collect::<Vec<_>>()
                .join("\n\n");
            let c = &mut l.cards[pos];
            c.hits = hits.iter().enumerate().map(|(i, h)| hit_json(i, h)).collect();
            c.state = CardState::Answering;
            c.gen += 1;
            let cites = serde_json::to_string(&c.hits).unwrap_or_default();
            c.db_id = self.kb.store.add_card(meeting_id, &c.question, "", &cites).ok();
            request = Some((c.gen, format!("Question raised in the meeting: {}\n\nPassages:\n{passages}\n\nAnswer now.", c.question)));
            self.emit(json!({"type": "card", "card": c.json()}));
        }
        let _ = self.kb.store.bump_counter("questions_answered", 1);
        let Some((gen, user)) = request else { return };
        self.generate_card(id, gen, user);
    }

    fn generate_card(self: &Arc<Self>, card: u64, gen: u64, user: String) {
        self.generate_card_with(card, gen, user, CARD_SYSTEM, 140, Priority::Low);
    }

    fn generate_card_with(self: &Arc<Self>, card: u64, gen: u64, user: String, system: &'static str, max_tokens: usize, priority: Priority) {
        let Some(llm) = self.engine.llm() else { return };
        let core = self.clone();
        let user2 = user.clone();
        let acc = Arc::new(Mutex::new(Instant::now() - EMIT_EVERY));
        llm.generate(GenRequest {
            images: Vec::new(),
            messages: vec![ChatMessage::system(system), ChatMessage::user(user)],
            params: GenParams { max_tokens, temperature: 0.2, ..Default::default() },
            priority,
            cancel: Arc::new(AtomicBool::new(false)),
            on_event: Box::new(move |ev| {
                let mut retry = None;
                let mut out = None;
                {
                    let mut g = core.live.lock();
                    let Some(l) = g.as_mut() else { return };
                    let Some(c) = l.cards.iter_mut().find(|c| c.id == card) else { return };
                    if c.gen != gen {
                        return;
                    }
                    match ev {
                        GenEvent::Token(t) => {
                            c.answer.push_str(&t);
                            let mut last = acc.lock();
                            if last.elapsed() >= EMIT_EVERY {
                                *last = Instant::now();
                                out = Some(c.json());
                            }
                        }
                        GenEvent::Done(stats) => {
                            if stats.preempted && c.answer.trim().is_empty() {
                                c.gen += 1;
                                retry = Some(c.gen);
                            } else {
                                c.state = CardState::Done;
                                if let Some(db) = c.db_id {
                                    let _ = core.kb.store.update_card_answer(db, &c.answer);
                                }
                                out = Some(c.json());
                            }
                        }
                        GenEvent::Error(_) => {
                            c.state = CardState::Done;
                            out = Some(c.json());
                        }
                    }
                }
                if let Some(v) = out {
                    core.emit(json!({"type": "card", "card": v}));
                }
                if let Some(g) = retry {
                    core.generate_card_with(card, g, user2.clone(), system, max_tokens, priority);
                }
            }),
        });
    }


    // ---- questions typed in the meeting, and questions to put to the speaker

    /// Adds a card to the live timeline in the "answering" state and returns what the model should read:
    /// the meeting so far, the person's own notes and (when the folder allows) a few library passages.
    fn live_card(self: &Arc<Self>, kind: &'static str, question: &str, with_library: bool) -> Result<(u64, u64, String)> {
        let (id, gen, transcript, folder) = {
            let mut g = self.live.lock();
            let l = g.as_mut().ok_or_else(|| anyhow!("There is no meeting in progress."))?;
            if l.lines.is_empty() && l.jots.is_empty() {
                return Err(anyhow!("Nothing has been said yet."));
            }
            let id = l.next_card_id;
            l.next_card_id += 1;
            let mut ctx = String::new();
            if let Some(sec) = l.sections.last() {
                ctx.push_str(&format!("Notes on earlier parts of the meeting:\n{}\n\n", head_tokens(&sec.text, 260)));
            }
            let from = l.sec_upto.min(l.lines.len());
            ctx.push_str("Transcript:\n");
            ctx.push_str(&tail_tokens(&transcript_text(&l.lines, from.saturating_sub(2), 60_000), 1600));
            if !l.jots.is_empty() {
                ctx.push_str("\nMy notes so far:\n");
                for j in l.jots.iter().rev().take(8).collect::<Vec<_>>().into_iter().rev() {
                    ctx.push_str(&format!("- {}\n", if j.polished.trim().is_empty() { &j.raw } else { &j.polished }));
                }
            }
            l.cards.insert(
                0,
                Card { kind, t: l.started.elapsed().as_secs_f64(), id, db_id: None, question: question.to_string(), answer: String::new(), hits: Vec::new(), state: CardState::Answering, gen: 1 },
            );
            l.cards.truncate(14);
            self.emit(json!({"type": "card", "card": l.cards[0].json()}));
            (id, 1, ctx, l.folder)
        };
        let mut context = transcript;
        if with_library {
            let hits = self.kb.search_in(question, 3, Scope::All, folder).unwrap_or_default();
            if hits.iter().any(|h| h.dense >= 0.40 || h.lexical >= 0.55) {
                context.push_str("\nFrom the library:\n");
                for (i, h) in hits.iter().enumerate() {
                    context.push_str(&format!("[{}] {} ({})\n{}\n", i + 1, h.title, h.locator(), snippet_of(&h.context, 700)));
                }
            }
        }
        Ok((id, gen, context))
    }

    /// "What did they say about the budget?" typed while the meeting is still going.
    fn meeting_ask(self: &Arc<Self>, question: &str) -> Result<Value> {
        let q = question.trim();
        if q.is_empty() {
            return Err(anyhow!("empty question"));
        }
        let core = self.clone();
        let q2 = q.to_string();
        let (id, gen, context) = {
            // The search of the library can take a moment, so the card is added first and filled after.
            let (id, gen, context) = self.live_card("ask", q, false)?;
            (id, gen, context)
        };
        std::thread::Builder::new()
            .name("ask-live".into())
            .spawn(move || {
                let folder = core.live.lock().as_ref().and_then(|l| l.folder);
                let mut context = context;
                let hits = core.kb.search_in(&q2, 3, Scope::All, folder).unwrap_or_default();
                if hits.iter().any(|h| h.dense >= 0.45 || h.lexical >= 0.6) {
                    context.push_str("\nFrom the library:\n");
                    for (i, h) in hits.iter().enumerate() {
                        context.push_str(&format!("[{}] {} ({})\n{}\n", i + 1, h.title, h.locator(), snippet_of(&h.context, 700)));
                    }
                }
                let user = format!("{context}\n\nQuestion: {q2}\n\nAnswer now.");
                core.generate_card_with(id, gen, user, ASK_LIVE_SYSTEM, 220, Priority::High);
            })
            .expect("spawn ask");
        Ok(json!({"id": id}))
    }

    /// Three questions the listener could put to the speaker, from what has been said so far.
    fn meeting_suggest(self: &Arc<Self>) -> Result<Value> {
        let (id, gen, context) = self.live_card("ideas", "Questions you could ask", false)?;
        let user = format!("{context}\n\nThe 3 questions:");
        self.generate_card_with(id, gen, user, IDEAS_SYSTEM, 170, Priority::High);
        Ok(json!({"id": id}))
    }

    // ---------------------------------------------------------------- stop + wrap-up

    /// Progress of a meeting being written up in the background.
    fn wrap(&self, meeting_id: i64, step: &str, label: &str, progress: f32) {
        self.emit(json!({"type": "wrap", "meeting_id": meeting_id, "step": step, "label": label, "progress": progress}));
    }

    /// Stop recording. Returns at once; the rest happens in the background and ends with a
    /// `wrap_done` event, so the person can leave, start another meeting, or close the app.
    fn meeting_stop(self: &Arc<Self>) -> Result<Value> {
        let (segment, meeting_id) = {
            let mut g = self.live.lock();
            let Some(l) = g.as_mut().filter(|l| l.recording) else {
                return Err(anyhow!("no recording in progress"));
            };
            l.recording = false;
            if let Some(c) = l.enhance_cancel.take() {
                c.store(true, Ordering::Relaxed);
            }
            (l.segmenter.flush(), l.meeting_id)
        };
        self.flushing.store(true, Ordering::Relaxed);
        if let (Some(seg), Some(w)) = (segment, self.engine.whisper()) {
            w.transcribe(AudioSegment { partial: false, ..seg });
        }
        self.emit(json!({"type": "partial", "text": ""}));
        let core = self.clone();
        std::thread::Builder::new()
            .name("finish".into())
            .spawn(move || core.finish_after_stop(meeting_id))
            .expect("spawn finish");
        Ok(json!({"id": meeting_id}))
    }

    /// Phase A: let the trailing speech finish transcribing, then hand the meeting over to the
    /// background writer. Phase B: write it up.
    fn finish_after_stop(self: &Arc<Self>, meeting_id: i64) {
        self.wrap(meeting_id, "finalizing", "Finalizing the transcript", 8.0);
        let t0 = Instant::now();
        std::thread::sleep(Duration::from_millis(1500));
        while self.engine.whisper().map(|w| w.queue_len() > 0).unwrap_or(false) && t0.elapsed() < Duration::from_secs(25) {
            std::thread::sleep(Duration::from_millis(250));
        }
        std::thread::sleep(Duration::from_millis(1500));

        let Some(l) = self.live.lock().take() else {
            self.flushing.store(false, Ordering::Relaxed);
            return;
        };
        self.flushing.store(false, Ordering::Relaxed);
        self.emit(json!({"type": "activity", "items": []}));
        let secs = l.started.elapsed().as_secs();
        let jots: Vec<JobJot> = l.jots.iter().map(|j| JobJot { id: j.id, t: Some(j.t), kind: j.kind.clone(), raw: j.raw.clone(), polished: j.polished.clone() }).collect();
        if l.lines.is_empty() && l.notes.trim().is_empty() && l.photos.is_empty() && jots.is_empty() {
            let _ = self.kb.store.delete_meeting(l.meeting_id);
            self.emit(json!({"type": "discarded", "meeting_id": l.meeting_id}));
            return;
        }
        let job = FinishJob {
            id: l.meeting_id, secs, raw: l.notes.clone(), enhanced: l.enhanced_text.clone(),
            lines: l.lines.clone(), jots, sections: l.sections.clone(), covered: l.sec_upto, title: String::new(),
            notify: true,
            unread_photos: l.queued_photos.iter().map(|(id, path)| (*id, path.clone(), l.photos.iter().find(|p| p.0 == *id).map(|p| p.1))).collect(),
        };
        self.run_finish(job);
    }

    /// Write up a meeting: polish any jots still waiting, wait for photo descriptions, then
    /// summary, title and search index. Runs one at a time, at low priority so a meeting being
    /// recorded right now always has the model first.
    fn run_finish(self: &Arc<Self>, mut job: FinishJob) {
        let id = job.id;
        if !self.finishing_ids.lock().insert(id) {
            return;
        }
        self.finishing_jobs.fetch_add(1, Ordering::Relaxed);
        let result = (|| -> Result<(String, String)> {
            // 1. jots that were still waiting for their polish
            let waiting: Vec<usize> = job.jots.iter().enumerate().filter(|(_, j)| j.polished.trim().is_empty() && j.kind != "keep").map(|(i, _)| i).collect();
            for (n, i) in waiting.iter().enumerate() {
                self.wrap(id, "jots", &format!("Polishing note {} of {}", n + 1, waiting.len()), 12.0 + 12.0 * n as f32 / waiting.len().max(1) as f32);
                let j = job.jots[*i].clone();
                let ctx = jot_context(&job.lines, &job.sections, j.t.unwrap_or(0.0), &j.raw);
                if let Ok((t, _)) = self.gen_job(Self::polish_prompt(&j.kind, &j.raw, &ctx), GenParams { max_tokens: 110, temperature: 0.2, ..Default::default() }, |_| {}) {
                    let polished = faithful_jot(&j.raw, &one_line(&t));
                    let _ = self.kb.store.set_jot_polished(j.id, &polished);
                    job.jots[*i].polished = polished.clone();
                    if let Ok(Some(sj)) = self.kb.store.jots(id).map(|v| v.into_iter().find(|x| x.id == j.id)) {
                        self.emit(json!({"type": "jot", "meeting_id": id, "jot": stored_jot_json(&sj)}));
                    }
                }
            }
            // 2. photos taken during the meeting, read now that the model is free
            let unread = std::mem::take(&mut job.unread_photos);
            for (n, (pid, path, pt)) in unread.iter().enumerate() {
                self.wrap(id, "photos", &format!("Reading photo {} of {}", n + 1, unread.len()), 24.0 + 4.0 * n as f32 / unread.len().max(1) as f32);
                let caption = {
                    let _turn = self.finish_gate.lock();
                    self.describe_photo(path)
                };
                let _ = self.kb.store.set_attachment_caption(*pid, &caption);
                self.emit(json!({"type": "photo", "meeting_id": id, "photo": {"id": pid, "path": path, "t": pt, "caption": caption, "state": "ready"}}));
            }
            // and any that were already being read
            let t1 = Instant::now();
            while self.describing_by.lock().get(&id).copied().unwrap_or(0) > 0 && t1.elapsed() < Duration::from_secs(180) {
                self.wrap(id, "photos", "Reading your photos", 26.0);
                std::thread::sleep(Duration::from_millis(400));
            }
            let photos_db = self.kb.store.attachments(id).unwrap_or_default();
            let photos: Vec<(i64, f64, String)> = photos_db.iter().map(|a| (a.id, a.t.unwrap_or(0.0), a.caption.clone())).collect();

            // 3. notes
            let tuples: Vec<(String, String, String)> = job.jots.iter().map(|j| (j.kind.clone(), j.raw.clone(), j.polished.clone())).collect();
            if !tuples.is_empty() {
                let (r, e) = compile_jots(&tuples);
                job.raw = without_block(&job.raw, &r);
                job.enhanced = without_block(&job.enhanced, &e);
                job.raw = if job.raw.trim().is_empty() { r } else { format!("{}\n{r}", job.raw) };
                job.enhanced = if job.enhanced.trim().is_empty() { e } else { format!("{}\n\n{e}", job.enhanced) };
            }
            let _ = self.kb.store.update_meeting_notes(id, &job.raw, &job.enhanced);
            let _ = self.kb.store.finish_meeting(id, job.secs as i64);
            self.emit(json!({"type": "meetings_changed"}));

            // 4. summary
            let words: usize = job.lines.iter().map(|l| l.text.split_whitespace().count()).sum::<usize>()
                + photos.iter().map(|p| p.2.split_whitespace().count()).sum::<usize>();
            let mut summary = String::new();
            if words >= 25 && self.engine.llm().is_some() {
                self.wrap(id, "summarizing", "Writing the summary", 30.0);
                // A failed model call (the model was reloading, memory ran short) is tried again before giving up.
                let mut attempt = 0;
                let text = loop {
                    attempt += 1;
                    match self.summarize_meeting(id, &job.lines, job.sections.clone(), job.covered, &job.enhanced, &photos, false) {
                        Ok(t) if !t.trim().is_empty() => break t,
                        Ok(_) if attempt >= 3 => break String::new(),
                        Err(e) if attempt >= 3 => return Err(e),
                        other => {
                            tracing::warn!("summary attempt {attempt} for meeting {id} did not work: {:?}", other.err().map(|e| format!("{e:#}")));
                            std::thread::sleep(Duration::from_secs(4 * attempt as u64));
                            // Parts already condensed were saved: carry on from them.
                            let (sec, cov) = valid_sections(self.kb.store.sections(id).unwrap_or_default(), &job.lines);
                            if cov > job.covered {
                                job.sections = sec;
                                job.covered = cov;
                            }
                        }
                    }
                };
                summary = clean_summary(&text);
                let _ = self.kb.store.set_meeting_summary(id, &summary);
                self.emit(json!({"type": "meetings_changed"}));
            }

            // 5. title
            let meeting = self.kb.store.list_meetings()?.into_iter().find(|m| m.id == id);
            let mut title = meeting.map(|m| m.title).unwrap_or_default();
            let notes_basis = if job.enhanced.trim().is_empty() { job.raw.clone() } else { job.enhanced.clone() };
            if is_default_title(&title) && (words >= 25 || notes_basis.split_whitespace().count() >= 3) && self.engine.llm().is_some() {
                self.wrap(id, "titling", "Naming the meeting", 86.0);
                let basis = if !summary.trim().is_empty() { summary.clone() } else if !notes_basis.trim().is_empty() { notes_basis.clone() } else { window_text(&job.lines, 0, job.lines.len(), 700) };
                if let Ok((t, _)) = self.gen_job(
                    vec![ChatMessage::system(TITLE_SYSTEM), ChatMessage::user(format!("Meeting notes:\n{basis}\n\nTitle:"))],
                    GenParams { max_tokens: 24, temperature: 0.2, ..Default::default() },
                    |_| {},
                ) {
                    let t = clean_title(&t);
                    if !t.is_empty() {
                        let _ = self.kb.store.update_meeting_title(id, &t);
                        title = t;
                    }
                }
            }

            // 6. searchable
            self.wrap(id, "indexing", "Making it searchable", 94.0);
            let segs: Vec<(String, f64, f64, String)> = self.kb.store.segments(id).unwrap_or_default().into_iter().map(|s| (s.source, s.t0, s.t1, s.text)).collect();
            let extras: Vec<(String, String)> = photos.iter().filter(|p| !p.2.trim().is_empty()).map(|(_, t, c)| (format!("Photo · {}", fmt_ts(*t)), c.clone())).collect();
            if let Err(e) = self.kb.ingest_meeting_with(id, &title, &segs, &job.enhanced, &summary, &extras) {
                self.toast("warning", format!("Meeting indexing failed: {e:#}"));
            }
            let _ = self.kb.store.sync_meeting_doc_folder(id);
            Ok((title, summary))
        })();
        self.finishing_jobs.fetch_sub(1, Ordering::Relaxed);
        self.finishing_ids.lock().remove(&id);
        match result {
            Ok((title, summary)) => {
                let _ = self.kb.store.mark_wrapped(id);
                self.emit(json!({"type": "wrap_done", "meeting_id": id, "title": title, "preview": snippet_of(&summary.lines().find(|l| !l.trim().is_empty() && !l.starts_with('#')).unwrap_or(""), 140), "notify": job.notify}));
                self.emit(json!({"type": "meetings_changed"}));
            }
            Err(e) => {
                tracing::warn!("finishing meeting {id} failed: {e:#}");
                self.emit(json!({"type": "wrap_failed", "meeting_id": id, "error": format!("{e:#}")}));
            }
        }
    }

    /// A model call for background work: low priority, and retried when something the person is
    /// waiting for takes the model first.
    fn gen_job(&self, messages: Vec<ChatMessage>, params: GenParams, mut on_text: impl FnMut(&str)) -> Result<(String, GenStats)> {
        for _ in 0..400 {
            // A meeting being recorded has the phone to itself: transcription must keep up in real time.
            let mut told = false;
            while self.live.lock().as_ref().map(|l| l.recording).unwrap_or(false) {
                if !told {
                    told = true;
                    self.emit(json!({"type": "wrap_hold", "waiting": true}));
                }
                std::thread::sleep(Duration::from_secs(1));
            }
            if told {
                self.emit(json!({"type": "wrap_hold", "waiting": false}));
            }
            let _turn = self.finish_gate.lock();
            let (t, st) = self.gen_blocking(messages.clone(), params.clone(), Priority::Low, Arc::new(AtomicBool::new(false)), &mut on_text)?;
            if st.preempted {
                std::thread::sleep(Duration::from_millis(1500));
                continue;
            }
            return Ok((t, st));
        }
        Err(anyhow!("the model stayed busy"))
    }

    /// Summary of a meeting of any length. A short one goes to the model whole. A long one is
    /// condensed part by part (the parts already written while recording are reused), the part
    /// notes are merged until they fit the context window, and only then is the summary written.
    fn summarize_meeting(
        &self,
        mid: i64,
        lines: &[Line],
        mut sections: Vec<Section>,
        mut covered: usize,
        enhanced: &str,
        photos: &[(i64, f64, String)],
        interactive: bool,
    ) -> Result<String> {
        let ctx = self.settings.read().chat_context;
        let budget = prompt_budget(ctx, 600);
        let ptxt = head_tokens(&photos_text(photos), budget / 8);
        let ptxt = if ptxt.is_empty() { String::new() } else { format!("Photos shared:\n{ptxt}\n\n") };
        let notes = head_tokens(enhanced, budget / 5);
        let fixed = est_tokens(&ptxt) + est_tokens(&notes) + 160;
        let room = budget.saturating_sub(fixed).max(600);
        let cancel = || Arc::new(AtomicBool::new(false));
        let stream = |t: &str| {
            if interactive {
                self.emit(json!({"type": "summary", "text": t, "streaming": true}))
            }
        };

        // Short enough to read in one go.
        if sections.is_empty() && lines_tokens(lines) <= room {
            let transcript = window_text(lines, 0, lines.len(), room);
            let user = format!("Transcript:\n{transcript}\n\n{ptxt}Notes:\n{notes}\n\nWrite the meeting summary now.");
            let (t, _) = self.gen_job(vec![ChatMessage::system(SUMMARY_SYSTEM), ChatMessage::user(user)], GenParams { max_tokens: 600, temperature: 0.3, ..Default::default() }, stream)?;
            return Ok(t);
        }

        // Map: write notes for whatever stretch has not been condensed yet.
        let remaining_parts = {
            let mut n = 0;
            let mut i = covered.min(lines.len());
            while i < lines.len() {
                i = window_end(lines, i, SECTION_WINDOW_TOKENS);
                n += 1;
            }
            n
        };
        let total_parts = sections.len() + remaining_parts;
        let mut done_parts = sections.len();
        while covered < lines.len() {
            let to = window_end(lines, covered, SECTION_WINDOW_TOKENS);
            let text = window_text(lines, covered, to, SECTION_WINDOW_TOKENS + 200);
            done_parts += 1;
            self.wrap(mid, "summarizing", &format!("Summarizing part {done_parts} of {total_parts}"), 30.0 + 50.0 * done_parts as f32 / total_parts.max(1) as f32);
            let (t, _) = self.gen_job(vec![ChatMessage::system(SECTION_SYSTEM), ChatMessage::user(format!("Part of the meeting:\n{text}\n\nWrite the notes for this part."))], GenParams { max_tokens: 260, temperature: 0.2, ..Default::default() }, |_| {})?;
            let sec = Section { from: covered, to, t0: lines[covered].t0, t1: lines[to - 1].t1, text: t.trim().to_string() };
            let _ = self.kb.store.save_section(mid, sec.from, sec.to, sec.t0, sec.t1, &sec.text);
            sections.push(sec);
            covered = to;
        }

        // Reduce: merge neighbouring part notes until the whole of them fits.
        let mut parts: Vec<String> = sections.iter().map(|s| format!("[{}–{}]\n{}", fmt_ts(s.t0), fmt_ts(s.t1), s.text)).collect();
        let mut round = 0;
        while parts.iter().map(|p| est_tokens(p) + 4).sum::<usize>() > room && parts.len() > 1 && round < 6 {
            round += 1;
            self.wrap(mid, "summarizing", "Combining the notes", 80.0);
            let mut next: Vec<String> = Vec::new();
            let mut group = String::new();
            let flush = |group: &mut String, next: &mut Vec<String>| -> Result<()> {
                if group.is_empty() {
                    return Ok(());
                }
                let (t, _) = self.gen_job(vec![ChatMessage::system(MERGE_SYSTEM), ChatMessage::user(format!("Notes on consecutive parts:\n{group}\n\nWrite the combined notes."))], GenParams { max_tokens: 420, temperature: 0.2, ..Default::default() }, |_| {})?;
                next.push(t.trim().to_string());
                group.clear();
                Ok(())
            };
            for p in &parts {
                if !group.is_empty() && est_tokens(&group) + est_tokens(p) > SECTION_WINDOW_TOKENS + 400 {
                    flush(&mut group, &mut next)?;
                }
                group.push_str(p);
                group.push_str("\n\n");
            }
            flush(&mut group, &mut next)?;
            if next.len() >= parts.len() {
                break; // not shrinking: stop and truncate below
            }
            parts = next;
        }
        let body = head_tokens(&parts.join("\n\n"), room);
        self.wrap(mid, "summarizing", "Writing the summary", 84.0);
        let user = format!("Notes on the parts of the meeting, in order:\n{body}\n\n{ptxt}My notes:\n{notes}\n\nWrite the meeting summary now.");
        let (t, _) = self.gen_job(vec![ChatMessage::system(SUMMARY_SYSTEM), ChatMessage::user(user)], GenParams { max_tokens: 600, temperature: 0.3, ..Default::default() }, stream)?;
        Ok(t)
    }

    /// The vectors in the library were made by whichever model was current. If that is not the
    /// model answering now, make them again so search and Ask keep finding the right passages.
    fn reembed_if_needed(self: &Arc<Self>) {
        const FLAG: &str = "embedder:eg2-270m-litert";
        if self.kb.store.flag(FLAG) {
            return;
        }
        let core = self.clone();
        std::thread::Builder::new()
            .name("reembed".into())
            .spawn(move || {
                let n = core.kb.chunk_count();
                if n == 0 {
                    let _ = core.kb.store.set_flag(FLAG, true);
                    return;
                }
                core.emit(json!({"type": "reindex", "done": 0, "total": n}));
                let t = Instant::now();
                let c2 = core.clone();
                match core.kb.reembed_all(|d, tot| c2.emit(json!({"type": "reindex", "done": d, "total": tot}))) {
                    Ok(done) => {
                        let _ = core.kb.store.set_flag(FLAG, true);
                        tracing::info!("search index rebuilt: {done} passages in {:?}", t.elapsed());
                    }
                    Err(e) => tracing::warn!("search index rebuild failed: {e:#}"),
                }
                core.emit(json!({"type": "reindex_done"}));
            })
            .expect("spawn reembed");
    }

    /// A meeting can be left half-done: the process was killed while recording, or while the
    /// notes were being written. Everything it captured is in the database, so finish it now.
    fn recover_unfinished(self: &Arc<Self>) {
        let core = self.clone();
        std::thread::Builder::new()
            .name("recover".into())
            .spawn(move || {
                std::thread::sleep(Duration::from_secs(3)); // let the app settle first
                core.rescan_unfinished(0);
            })
            .expect("spawn recover");
    }

    /// Finds meetings whose notes were never completed and finishes them. Safe to call at any time:
    /// it skips the live recording and anything already being written. Returns how many it started.
    fn rescan_unfinished(self: &Arc<Self>, grace_secs: u64) -> usize {
        let Some(_scan) = self.rescanning.try_lock() else {
            self.emit(json!({"type": "recovery", "pending": 1})); // another scan is already on it
            return 0;
        };
        let live_id = self.live.lock().as_ref().map(|l| l.meeting_id);
        let Ok(meetings) = self.kb.store.list_meetings() else {
            self.emit(json!({"type": "recovery", "pending": 0}));
            return 0;
        };
        let todo: Vec<Meeting> = meetings
            .into_iter()
            .filter(|m| Some(m.id) != live_id)
            .filter(|m| !self.kb.store.is_wrapped(m.id) && (m.ended_at.is_none() || (m.summary.trim().is_empty() && m.segment_count > 0)))
            .filter(|m| !self.finishing_ids.lock().contains(&m.id))
            .collect();
        // Tell the app first, so it can keep the process alive while the notes are written.
        let interrupted: Vec<i64> = todo.iter().filter(|m| m.ended_at.is_none()).map(|m| m.id).collect();
        self.emit(json!({"type": "recovery", "pending": todo.len(), "interrupted": interrupted}));
        let n = todo.len();
        // A recording that was cut short can be carried on by the person: wait a little for that.
        if grace_secs > 0 && !interrupted.is_empty() {
            let t = Instant::now();
            while t.elapsed() < Duration::from_secs(grace_secs) && self.live.lock().is_none() {
                std::thread::sleep(Duration::from_millis(500));
            }
        }
        for m in todo {
            if self.live.lock().as_ref().map(|l| l.meeting_id == m.id).unwrap_or(false) {
                continue; // carried on by the person
            }
            if self.live.lock().is_some() {
                break;
            }
            if let Err(e) = self.recover_one(&m) {
                tracing::warn!("could not finish meeting {}: {e:#}", m.id);
            }
        }
        n
    }

    fn recover_one(self: &Arc<Self>, m: &Meeting) -> Result<()> {
        let id = m.id;
        let segs = self.kb.store.segments(id)?;
        let jots = self.kb.store.jots(id)?;
        let photos = self.kb.store.attachments(id)?;
        if segs.is_empty() && jots.is_empty() && photos.is_empty() {
            return Ok(());
        }
        let names: HashMap<i64, String> = self.kb.store.speaker_names(id).unwrap_or_default().into_iter().collect();
        let mut lines: Vec<Line> = Vec::new();
        for s in &segs {
            push_merged_as(&mut lines, s.t0, s.t1, &s.text, s.speaker, &who_is(&names, s.speaker));
        }
        let (secs_saved, covered) = valid_sections(self.kb.store.sections(id).unwrap_or_default(), &lines);
        let secs = if m.ended_at.is_none() { segs.last().map(|s| s.t1).unwrap_or(0.0) as u64 } else { m.duration_secs.max(0) as u64 };
        self.toast("info", format!("Finishing the notes for “{}”", snippet_of(&m.title, 40)));
        let job = FinishJob {
            id, secs, raw: m.notes_raw.clone(), enhanced: m.notes_enhanced.clone(), lines,
            jots: jots.into_iter().map(|j| JobJot { id: j.id, t: j.t, kind: j.kind, raw: j.raw, polished: j.polished }).collect(),
            sections: secs_saved, covered, title: String::new(), notify: true,
            unread_photos: photos.iter().filter(|a| a.caption.trim().is_empty()).map(|a| (a.id, a.path.clone(), a.t)).collect(),
        };
        self.run_finish(job);
        Ok(())
    }

    fn meeting_get(&self, id: i64) -> Result<Value> {
        let m = self
            .kb
            .store
            .list_meetings()?
            .into_iter()
            .find(|m| m.id == id)
            .ok_or_else(|| anyhow!("meeting not found"))?;
        let names: HashMap<i64, String> = self.kb.store.speaker_names(id).unwrap_or_default().into_iter().collect();
        let mut lines: Vec<Line> = Vec::new();
        for s in self.kb.store.segments(id)? {
            push_merged_as(&mut lines, s.t0, s.t1, &s.text, s.speaker, &who_is(&names, s.speaker));
        }
        let cards: Vec<Value> = self
            .kb
            .store
            .cards(id)?
            .into_iter()
            .map(|(cid, q, a, c)| json!({"id": cid, "question": q, "answer": a, "state": "done", "sources": serde_json::from_str::<Value>(&c).unwrap_or(json!([]))}))
            .collect();
        let photos: Vec<Value> = self.kb.store.attachments(id)?.iter().map(photo_json).collect();
        let jots: Vec<Value> = self.kb.store.jots(id)?.iter().map(stored_jot_json).collect();
        let mut mj = meeting_json(&m, self.kb.store.meeting_folder(id));
        mj["summary"] = json!(clean_summary(&m.summary));
        let audio: Vec<Value> = self.kb.store.audios(id)?.into_iter().map(|(p, o)| json!({"path": p, "offset": o})).collect();
        let speakers: Vec<Value> = self.kb.store.speaker_names(id)?.into_iter().map(|(i, n)| json!({"speaker": i, "name": n})).collect();
        Ok(json!({
            "audio": audio,
            "speakers": speakers,
            "meeting": mj,
            "lines": lines.iter().enumerate().map(|(i, l)| line_json(i, l)).collect::<Vec<_>>(),
            "cards": cards,
            "photos": photos,
            "jots": jots,
        }))
    }

    /// A jot: one quick note, saved at once, polished in the background.
    fn meeting_jot(self: &Arc<Self>, args: &Value) -> Result<Value> {
        let text = args.get("text").and_then(Value::as_str).unwrap_or("").trim().to_string();
        if text.is_empty() {
            return Err(anyhow!("empty note"));
        }
        let kind = args.get("kind").and_then(Value::as_str).unwrap_or("note").to_string();
        let explicit = args.get("id").and_then(Value::as_i64).filter(|v| *v > 0);
        let given_t = args.get("t").and_then(Value::as_f64);
        let keep = kind == "keep"; // a line lifted from the transcript needs no polishing
        let live_id = self.live.lock().as_ref().filter(|l| l.recording).map(|l| l.meeting_id);
        match (live_id, explicit) {
            (Some(mid), None) => {
                let t = {
                    let g = self.live.lock();
                    g.as_ref().map(|l| l.started.elapsed().as_secs_f64()).unwrap_or(0.0)
                };
                let t = given_t.unwrap_or(t);
                let id = self.kb.store.add_jot(mid, Some(t), &kind, &text, if keep { &text } else { "" })?;
                if let Some(l) = self.live.lock().as_mut() {
                    l.jots.push(LiveJot { id, t, kind: kind.clone(), raw: text.clone(), polished: if keep { text.clone() } else { String::new() }, ready: keep, added: Instant::now(), refined: if keep { 2 } else { 0 }, worded_at: Instant::now(), worded_lines: l.lines.len(), updated: false });
                }
                self.emit(json!({"type": "jot", "meeting_id": mid, "jot": jot_json(id, Some(t), &kind, &text, if keep { &text } else { "" }, keep)}));
                if !keep {
                    self.busy("jot", Some("Polishing your note"));
                }
                Ok(json!({"id": id}))
            }
            (_, Some(mid)) => {
                // Added afterwards, to a finished meeting: polish straight away.
                let id = self.kb.store.add_jot(mid, given_t, &kind, &text, if keep { &text } else { "" })?;
                self.emit(json!({"type": "jot", "meeting_id": mid, "jot": jot_json(id, given_t, &kind, &text, if keep { &text } else { "" }, keep)}));
                if !keep {
                    let ctx = self.stored_context(mid, given_t);
                    self.spawn_polish_stored(mid, id, kind, text, ctx);
                }
                Ok(json!({"id": id}))
            }
            _ => Err(anyhow!("no meeting to add the note to")),
        }
    }

    fn jot_edit(self: &Arc<Self>, id: i64, text: &str) -> Result<Value> {
        let text = text.trim();
        if text.is_empty() {
            return Err(anyhow!("empty note"));
        }
        self.kb.store.set_jot_raw(id, text)?;
        let mut live_t = None;
        if let Some(l) = self.live.lock().as_mut() {
            if let Some(j) = l.jots.iter_mut().find(|j| j.id == id) {
                j.raw = text.to_string();
                j.polished.clear();
                j.ready = false;
                j.added = Instant::now() - JOT_SETTLE; // already settled: polish on the next tick
                live_t = Some((l.meeting_id, j.t, j.kind.clone()));
            }
        }
        if let Some((mid, t, kind)) = live_t {
            self.emit(json!({"type": "jot", "meeting_id": mid, "jot": jot_json(id, Some(t), &kind, text, "", false)}));
        }
        Ok(json!({}))
    }

    /// Transcript near `t` in a finished meeting, from the saved segments.
    fn stored_context(&self, meeting_id: i64, t: Option<f64>) -> String {
        let Some(t) = t else { return String::new() };
        let segs = self.kb.store.segments(meeting_id).unwrap_or_default();
        let names: HashMap<i64, String> = self.kb.store.speaker_names(meeting_id).unwrap_or_default().into_iter().collect();
        let mut text = String::new();
        for s in segs.iter().filter(|s| s.t1 >= t - 90.0 && s.t0 <= t + 30.0) {
            text.push_str(&format!("[{}] {}{}\n", fmt_ts(s.t0), spk(&who_is(&names, s.speaker)), s.text));
        }
        format!("Transcript around that moment:\n{}", tail_tokens(&text, 650))
    }

    fn polish_prompt(kind: &str, raw: &str, ctx: &str) -> Vec<ChatMessage> {
        let hint = match kind {
            "action" => "This is an action item. Say who owns it if the transcript says so, and keep any deadline.",
            "decision" => "This is a decision. State plainly what was decided.",
            "question" => "This is an open question. Keep it a question.",
            _ => "This is a general note.",
        };
        let ctx = if ctx.trim().is_empty() { String::new() } else { format!("{ctx}\n\n") };
        vec![ChatMessage::system(JOT_SYSTEM), ChatMessage::user(format!("{ctx}{hint}\nMy jot: {raw}\n\nWrite the note."))]
    }

    /// Live-meeting jot: the result goes back into the live state.
    fn spawn_polish(self: &Arc<Self>, id: Option<i64>, kind: String, raw: String, ctx: String) {
        let core = self.clone();
        self.busy("jot", Some("Polishing your note"));
        std::thread::Builder::new()
            .name("polish".into())
            .spawn(move || {
                let r = core.gen_blocking(
                    Self::polish_prompt(&kind, &raw, &ctx),
                    GenParams { max_tokens: 110, temperature: 0.2, ..Default::default() },
                    Priority::High,
                    Arc::new(AtomicBool::new(false)),
                    |_| {},
                );
                let polished = faithful_jot(&raw, &r.map(|(t, _)| one_line(&t)).unwrap_or_default());
                let (mut emit, mut meeting) = (None, 0);
                {
                    let mut g = core.live.lock();
                    if let Some(l) = g.as_mut() {
                        l.jot_busy = false;
                        meeting = l.meeting_id;
                        if let Some(j) = l.jots.iter_mut().find(|j| Some(j.id) == id) {
                            // Edited again while polishing: the newer wording is polished next.
                            if j.raw == raw {
                                j.polished = polished.clone();
                                j.ready = true;
                                j.worded_at = Instant::now();
                                j.worded_lines = l.lines.len();
                                j.refined = if j.kind == "keep" { 2 } else { 0 };
                                j.updated = false;
                                let _ = core.kb.store.set_jot_polished(j.id, &polished);
                                emit = Some(live_jot_json(j));
                            }
                        }
                    }
                }
                let more = core.live.lock().as_ref().map(|l| l.jots.iter().any(|j| !j.ready)).unwrap_or(false);
                if !more {
                    core.busy("jot", None);
                }
                if let Some(j) = emit {
                    core.emit(json!({"type": "jot", "meeting_id": meeting, "jot": j}));
                }
            })
            .expect("spawn polish");
    }

    /// A jot already worded: look at what was said after it, and reword it only if that adds or changes something.
    fn spawn_refine(self: &Arc<Self>, id: i64, kind: String, raw: String, current: String, ctx: String) {
        let core = self.clone();
        std::thread::Builder::new()
            .name("refine".into())
            .spawn(move || {
                let prompt = vec![
                    ChatMessage::system(JOT_SYSTEM),
                    ChatMessage::user(format!(
                        "{ctx}\n\nMy jot: {raw}\nThe note so far: {current}\n\nThe transcript above now runs past the moment of the jot. If what was said after it adds a detail (an owner, a date, a number, a reason) or changes the outcome, write the improved note in the same style, one or two sentences. If nothing was added, write exactly the note so far."
                    )),
                ];
                let r = core.gen_blocking(
                    prompt,
                    GenParams { max_tokens: 110, temperature: 0.2, ..Default::default() },
                    Priority::Low,
                    Arc::new(AtomicBool::new(false)),
                    |_| {},
                );
                let better = faithful_jot(&raw, &r.map(|(t, _)| one_line(&t)).unwrap_or_default());
                let norm = |t: &str| t.chars().filter(|c| c.is_alphanumeric()).collect::<String>().to_lowercase();
                let changed = !better.trim().is_empty() && norm(&better) != norm(&current);
                let mut emit = None;
                let mut meeting = 0;
                {
                    let mut g = core.live.lock();
                    if let Some(l) = g.as_mut() {
                        l.jot_busy = false;
                        meeting = l.meeting_id;
                        let lines_now = l.lines.len();
                        if let Some(j) = l.jots.iter_mut().find(|j| j.id == id) {
                            j.refined += 1;
                            j.worded_at = Instant::now();
                            j.worded_lines = lines_now;
                            // Not edited meanwhile, and actually different.
                            if changed && j.raw == raw && j.polished == current {
                                j.polished = better.clone();
                                j.updated = true;
                                let _ = core.kb.store.set_jot_polished(j.id, &better);
                                emit = Some(live_jot_json(j));
                            }
                        }
                    }
                }
                if let Some(j) = emit {
                    core.emit(json!({"type": "jot", "meeting_id": meeting, "jot": j}));
                }
            })
            .expect("spawn refine");
    }

    /// A jot added after the meeting.
    fn spawn_polish_stored(self: &Arc<Self>, meeting_id: i64, id: i64, kind: String, raw: String, ctx: String) {
        let core = self.clone();
        self.busy("jot", Some("Polishing your note"));
        std::thread::Builder::new()
            .name("polish-stored".into())
            .spawn(move || {
                let r = core.gen_blocking(
                    Self::polish_prompt(&kind, &raw, &ctx),
                    GenParams { max_tokens: 110, temperature: 0.2, ..Default::default() },
                    Priority::High,
                    Arc::new(AtomicBool::new(false)),
                    |_| {},
                );
                let polished = faithful_jot(&raw, &r.map(|(t, _)| one_line(&t)).unwrap_or_default());
                let _ = core.kb.store.set_jot_polished(id, &polished);
                core.busy("jot", None);
                let found = core.kb.store.jots(meeting_id).unwrap_or_default().into_iter().find(|j| j.id == id);
                if let Some(j) = found {
                    core.emit(json!({"type": "jot", "meeting_id": meeting_id, "jot": stored_jot_json(&j)}));
                }
                core.reindex_meeting(meeting_id);
            })
            .expect("spawn polish stored");
    }

    /// A photo taken (or picked) for a meeting. Stored at once; Gemma describes it in the
    /// background and the description joins the meeting's searchable text.
    fn meeting_photo(self: &Arc<Self>, args: &Value) -> Result<Value> {
        let path = args.get("path").and_then(Value::as_str).ok_or_else(|| anyhow!("missing path"))?.to_string();
        let explicit = args.get("id").and_then(Value::as_i64).filter(|v| *v > 0);
        let (meeting_id, t, live) = {
            let g = self.live.lock();
            match (g.as_ref(), explicit) {
                (Some(l), None) => (l.meeting_id, Some(l.started.elapsed().as_secs_f64()), true),
                (_, Some(id)) => (id, None, g.as_ref().map(|l| l.meeting_id == id && l.recording).unwrap_or(false)),
                (None, None) => return Err(anyhow!("no meeting to attach the photo to")),
            }
        };
        let aid = self.kb.store.add_attachment(meeting_id, "photo", &path, t)?;
        let recording = self.live.lock().as_ref().map(|l| l.recording && l.meeting_id == meeting_id).unwrap_or(false);
        if live && recording && !self.llm_on_gpu.load(Ordering::Relaxed) {
            // Saved at once. Gemma reads it after the meeting (or now, if the person asks), so the
            // live transcript keeps all the processor it needs.
            if let Some(l) = self.live.lock().as_mut() {
                l.photos.push((aid, t.unwrap_or(0.0), String::new()));
                l.queued_photos.push((aid, path.clone()));
            }
            self.emit(json!({"type": "photo", "meeting_id": meeting_id, "photo": {"id": aid, "path": path, "t": t, "caption": "", "state": "queued"}}));
            return Ok(json!({"id": aid, "queued": true}));
        }
        // On the GPU a photo is read at once on its own thread, even while recording.
        if live && recording {
            if let Some(l) = self.live.lock().as_mut() {
                l.photos.push((aid, t.unwrap_or(0.0), String::new()));
            }
        }
        self.emit(json!({"type": "photo", "meeting_id": meeting_id, "photo": {"id": aid, "path": path, "t": t, "caption": "", "state": "describing"}}));
        self.start_describing(meeting_id, aid, path, t, live);
        Ok(json!({"id": aid}))
    }

    /// Have Gemma read a photo that was waiting. Used by "Read now" on the card.
    fn photo_read_now(self: &Arc<Self>, id: i64) -> Result<Value> {
        let (mid, path, t) = {
            let mut g = self.live.lock();
            let Some(l) = g.as_mut() else { return Err(anyhow!("no meeting in progress")) };
            let Some(pos) = l.queued_photos.iter().position(|q| q.0 == id) else { return Ok(json!({})) };
            let (pid, path) = l.queued_photos.remove(pos);
            let t = l.photos.iter().find(|p| p.0 == pid).map(|p| p.1);
            (l.meeting_id, path, t)
        };
        self.emit(json!({"type": "photo", "meeting_id": mid, "photo": {"id": id, "path": path, "t": t, "caption": "", "state": "describing"}}));
        self.start_describing(mid, id, path, t, true);
        Ok(json!({}))
    }

    fn start_describing(self: &Arc<Self>, meeting_id: i64, aid: i64, path: String, t: Option<f64>, live: bool) {
        self.describing.fetch_add(1, Ordering::Relaxed);
        *self.describing_by.lock().entry(meeting_id).or_insert(0) += 1;
        let core = self.clone();
        std::thread::Builder::new()
            .name("photo".into())
            .spawn(move || {
                let caption = core.describe_photo(&path);
                core.finish_photo(meeting_id, aid, &path, t, &caption, live);
            })
            .expect("spawn photo");
    }

    /// Record a photo's description and tell the screens.
    fn finish_photo(self: &Arc<Self>, meeting_id: i64, aid: i64, path: &str, t: Option<f64>, caption: &str, live: bool) {
        let _ = self.kb.store.set_attachment_caption(aid, caption);
        if let Some(l) = self.live.lock().as_mut() {
            if let Some(p) = l.photos.iter_mut().find(|p| p.0 == aid) {
                p.2 = caption.to_string();
            }
        }
        self.describing.fetch_sub(1, Ordering::Relaxed);
        if let Some(n) = self.describing_by.lock().get_mut(&meeting_id) {
            *n = n.saturating_sub(1);
        }
        self.emit(json!({"type": "photo", "meeting_id": meeting_id, "photo": {"id": aid, "path": path, "t": t, "caption": caption, "state": "ready"}}));
        if !live {
            self.reindex_meeting(meeting_id);
        }
    }

    /// Load Gemma's vision projector now, in the background, so the first photo of a session does
    /// not pay for it.
    fn warm_vision(self: &Arc<Self>) {
        if !self.settings.read().vision {
            return;
        }
        let core = self.clone();
        std::thread::Builder::new()
            .name("warm-vision".into())
            .spawn(move || {
                let Some(llm) = core.engine.llm() else { return };
                let path = core.paths.data_dir.join("warmup.png");
                if std::fs::write(&path, include_bytes!("warm.png")).is_err() {
                    return;
                }
                let t = Instant::now();
                let _ = llm.complete_with_images(
                    vec![ChatMessage::user(format!("{}\nOk", image_marker()))],
                    vec![path],
                    GenParams { max_tokens: 1, temperature: 0.0, ..Default::default() },
                    Priority::Low,
                );
                tracing::info!("vision projector warmed in {:.1}s", t.elapsed().as_secs_f32());
            })
            .expect("spawn warm vision");
    }

    fn describe_photo(&self, path: &str) -> String {
        let Some(llm) = self.engine.llm() else { return String::new() };
        let prompt = format!(
            "{}\nThis photo was taken during a meeting. In 2 to 4 sentences describe what it shows. If it contains writing (whiteboard, slide, document, screen, sticky notes), transcribe the important text exactly. Plain prose, no preamble.",
            image_marker()
        );
        match llm.complete_with_images(
            vec![ChatMessage::user(prompt)],
            vec![std::path::PathBuf::from(path)],
            GenParams { max_tokens: 170, temperature: 0.2, ..Default::default() },
            Priority::High,
        ) {
            Ok(c) => c.trim().to_string(),
            Err(e) => {
                tracing::warn!("photo description failed: {e:#}");
                String::new()
            }
        }
    }

    /// Rebuild a finished meeting's passages (after a late photo, for example).
    fn reindex_meeting(&self, id: i64) {
        let Ok(Some(m)) = self.kb.store.list_meetings().map(|ms| ms.into_iter().find(|m| m.id == id)) else { return };
        let segs: Vec<(String, f64, f64, String)> =
            self.kb.store.segments(id).unwrap_or_default().into_iter().map(|s| (s.source, s.t0, s.t1, s.text)).collect();
        let extras: Vec<(String, String)> = self
            .kb
            .store
            .attachments(id)
            .unwrap_or_default()
            .into_iter()
            .filter(|a| !a.caption.trim().is_empty())
            .map(|a| (format!("Photo · {}", a.t.map(fmt_ts).unwrap_or_else(|| "added later".into())), a.caption))
            .collect();
        if let Err(e) = self.kb.ingest_meeting_with(id, &m.title, &segs, &m.notes_enhanced, &m.summary, &extras) {
            tracing::warn!("reindex failed: {e:#}");
        }
        let _ = self.kb.store.sync_meeting_doc_folder(id);
    }

    fn delete_meeting(&self, id: i64) {
        for a in self.kb.store.attachments(id).unwrap_or_default() {
            let _ = std::fs::remove_file(a.path);
        }
        let _ = self.kb.store.delete_meeting(id);
        if let Ok(Some(doc)) = self.kb.store.find_document_by_source(&format!("meeting://{id}")) {
            let _ = self.kb.remove_document(doc.id);
        }
    }

    // ---------------------------------------------------------------- Ask

    fn chat_ask(self: &Arc<Self>, args: &Value) -> Result<Value> {
        let question = args.get("question").and_then(Value::as_str).unwrap_or("").trim().to_string();
        if question.is_empty() {
            return Err(anyhow!("empty question"));
        }
        if self.engine.llm().is_none() {
            return Err(anyhow!("The chat model is still loading."));
        }
        let scope = scope_of(args.get("scope").and_then(Value::as_str).unwrap_or("all"));
        let folder = opt_folder(args);
        let history: Vec<(String, String)> = args
            .get("history")
            .and_then(Value::as_array)
            .map(|a| {
                a.iter()
                    .filter_map(|p| Some((p.get(0)?.as_str()?.to_string(), p.get(1)?.as_str()?.to_string())))
                    .collect()
            })
            .unwrap_or_default();
        let turn = self.next_turn.fetch_add(1, Ordering::Relaxed);
        let cancel = Arc::new(AtomicBool::new(false));
        self.chat_cancels.lock().insert(turn, cancel.clone());
        let core = self.clone();
        std::thread::Builder::new()
            .name("ask".into())
            .spawn(move || {
                core.answer(turn, question, scope, folder, history, cancel);
                core.chat_cancels.lock().remove(&turn);
            })
            .expect("spawn ask");
        Ok(json!({"turn": turn}))
    }

    fn answer(&self, turn: u64, question: String, scope: Scope, folder: Option<i64>, history: Vec<(String, String)>, cancel: Arc<AtomicBool>) {
        let hits = self.kb.search_in(&question, 6, scope, folder).unwrap_or_default();
        let sources: Vec<Value> = hits.iter().enumerate().map(|(i, h)| hit_json(i, h)).collect();
        self.emit(json!({"type": "chat", "turn": turn, "state": "generating", "text": "", "sources": sources}));

        let n_ctx = self.settings().chat_context.max(2048) as usize;
        let budget_chars = n_ctx.saturating_sub(600 + 400) * 36 / 10;
        let passage_budget = budget_chars * 6 / 10;
        let history_budget = budget_chars * 25 / 100;
        let passages = if hits.is_empty() {
            "(no relevant passages found)".to_string()
        } else {
            let per = (passage_budget / hits.len().max(1)).clamp(300, 2200);
            hits.iter()
                .enumerate()
                .map(|(i, h)| {
                    let kind = if h.kind == DocKind::Meeting { "meeting" } else { "document" };
                    format!("[{}] {} — {} ({})\n{}", i + 1, h.title, h.locator(), kind, snippet_of(&h.context, per))
                })
                .collect::<Vec<_>>()
                .join("\n\n")
        };
        let mut messages = vec![ChatMessage::system(CHAT_SYSTEM)];
        let mut used = 0usize;
        let mut kept = Vec::new();
        for (q, a) in history.into_iter().rev().take(3) {
            let a = snippet_of(&a, 700);
            if used + q.len() + a.len() > history_budget {
                break;
            }
            used += q.len() + a.len();
            kept.push((q, a));
        }
        for (q, a) in kept.into_iter().rev() {
            messages.push(ChatMessage::user(q));
            messages.push(ChatMessage::assistant(a));
        }
        messages.push(ChatMessage::user(format!("Passages:\n{passages}\n\nQuestion: {question}")));

        let started = Instant::now();
        let r = self.gen_blocking(
            messages,
            GenParams { max_tokens: 600, temperature: 0.35, ..Default::default() },
            Priority::High,
            cancel,
            |t| self.emit(json!({"type": "chat", "turn": turn, "state": "generating", "text": t})),
        );
        match r {
            Ok((text, stats)) => {
                let tps = if stats.elapsed_ms > 0 { stats.generated_tokens as f64 / (stats.elapsed_ms as f64 / 1000.0) } else { 0.0 };
                let _ = started;
                self.emit(json!({
                    "type": "chat", "turn": turn, "state": "done", "text": text,
                    "stats": format!("{} tokens · {:.1} tok/s · on device", stats.generated_tokens, tps),
                }));
            }
            Err(e) => self.emit(json!({"type": "chat", "turn": turn, "state": "error", "error": e.to_string()})),
        }
    }

    pub fn shutdown(&self) {
        self.engine.shutdown();
    }
}

#[derive(Default)]
struct Work {
    jot: Option<(i64, String, String, String)>,
    refine: Option<(i64, String, String, String, String)>,
    section: Option<(usize, usize, f64, f64, String)>,
    tick: Option<u64>,
    enhance: bool,
    scan: bool,
    question: Option<String>,
}

fn scope_of(s: &str) -> Scope {
    match s {
        "documents" => Scope::Documents,
        "meetings" => Scope::Meetings,
        _ => Scope::All,
    }
}

/// A rewrite is only kept if it is still about the jot: it must keep at least one of the jot's own words.
/// Otherwise the jot is used as the person wrote it, tidied.
fn faithful_jot(raw: &str, rewritten: &str) -> String {
    let stem = |w: &str| -> String { w.to_lowercase().chars().filter(|c| c.is_alphanumeric()).take(5).collect() };
    let keep = |t: &str| -> std::collections::HashSet<String> {
        t.split_whitespace().filter(|w| w.chars().filter(|c| c.is_alphanumeric()).count() >= 4).map(stem).collect()
    };
    let (a, b) = (keep(raw), keep(rewritten));
    if rewritten.trim().is_empty() || (!a.is_empty() && a.intersection(&b).count() == 0) {
        tidy_jot(raw)
    } else {
        rewritten.trim().to_string()
    }
}

/// The jot as written, with a capital letter and a full stop.
fn tidy_jot(raw: &str) -> String {
    let t = raw.trim();
    let mut c = t.chars();
    let mut out = match c.next() {
        Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
        None => return String::new(),
    };
    if !out.ends_with(['.', '?', '!']) {
        out.push('.');
    }
    out
}

/// One line of plain text: no quotes, bullets or label prefixes the model likes to add.
fn one_line(raw: &str) -> String {
    let t = raw.trim().trim_start_matches(['-', '*', '•', ' ']).trim();
    let t = t.strip_prefix("Note:").unwrap_or(t).trim().trim_matches(['"', '“', '”']);
    t.lines().map(str::trim).filter(|l| !l.is_empty()).collect::<Vec<_>>().join(" ")
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
