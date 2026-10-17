//! Meeting workspace: recording, live transcript, shorthand notes with Live
//! Enhance, and proactive Live Assistance context cards with citations.

use super::anim::{fade_in_up, live, pulse, scale_in, settle_glow, slide_in_right};
use super::components::{
    card, card_header, chip, dot, empty_state, eyebrow, glow_dot, hit_card,
    icon_tile, level_meter, mini_meter, page_title, pal, segment_item, segmented_track, waveform,
};
use super::theme::{with_alpha, TypeScale};
use crate::app::{GenTarget, NoteKind, Services};
use crate::audio::{AudioCapture, AudioMeters, CaptureConfig};
use crate::config::NoteStyle;
use crate::engine::{
    AudioSource, ChatMessage, GenEvent, GenParams, GenRequest, Priority, Transcript,
};
use crate::rag::chunk::fmt_ts;
use crate::rag::store::Meeting;
use crate::rag::{Hit, Scope};
use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::input::{Input, InputEvent, InputState, Textarea, TextareaState};
use gpui_kit::component::progress::Progress;
use gpui_kit::component::shimmer::ShimmerText;
use gpui_kit::component::text::{TextView, TextViewState};
use gpui_kit::component::notification::Notification;
use gpui_kit::component::{h_flex, v_flex, Icon, Sizable, WindowExt};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use std::collections::HashSet;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

#[derive(Clone, Debug)]
pub struct Line {
    pub source: AudioSource,
    pub t0: f64,
    pub t1: f64,
    pub text: String,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum CardState {
    Retrieving,
    Answering,
    Done,
    /// A question was heard but the knowledge base had nothing strong for it.
    NoMatch,
}

struct ContextCard {
    id: u64,
    db_id: Option<i64>,
    question: String,
    answer: Entity<TextViewState>,
    answer_text: String,
    hits: Vec<Hit>,
    state: CardState,
    gen: u64,
    expanded: bool,
}

enum Mode {
    Idle,
    Live,
    Review(Meeting),
}

/// What the app is doing between pressing Stop and the review being ready.
#[derive(Clone, Copy, PartialEq, Eq)]
enum WrapStep {
    Finalizing,
    Summarizing,
    Titling,
    Indexing,
}

impl WrapStep {
    fn label(self) -> &'static str {
        match self {
            WrapStep::Finalizing => "Finalizing transcript",
            WrapStep::Summarizing => "Writing summary",
            WrapStep::Titling => "Naming the meeting",
            WrapStep::Indexing => "Making it searchable",
        }
    }
    fn progress(self) -> f32 {
        match self {
            WrapStep::Finalizing => 12.0,
            WrapStep::Summarizing => 45.0,
            WrapStep::Titling => 82.0,
            WrapStep::Indexing => 95.0,
        }
    }
}

/// A meeting that is hidden but not yet erased, so Delete can be undone.
struct PendingDelete {
    meeting: Meeting,
    _timer: Task<()>,
}

/// Marker for the single Undo toast.
struct UndoDeleteToast;

#[derive(Clone, Copy, PartialEq, Eq)]
enum ReviewTab {
    Summary,
    Notes,
    Transcript,
    Assistance,
}

pub struct MeetingView {
    services: Arc<Services>,
    mode: Mode,
    meetings: Vec<Meeting>,
    meeting_id: Option<i64>,
    title: Entity<InputState>,
    search_input: Entity<InputState>,
    notes: Entity<TextareaState>,
    enhanced: Entity<TextViewState>,
    summary: Entity<TextViewState>,
    lines: Vec<Line>,
    /// Provisional (in-progress) line per source.
    partial_mic: Option<Line>,
    partial_sys: Option<Line>,
    /// Streaming views for the provisional lines (new words fade in).
    partial_view_mic: Entity<TextViewState>,
    partial_view_sys: Entity<TextViewState>,
    transcript_scroll: ScrollHandle,
    cards_scroll: ScrollHandle,
    capture: Option<AudioCapture>,
    meters: Option<Arc<AudioMeters>>,
    started: Option<Instant>,
    elapsed_secs: u64,
    elapsed_f: f64,
    warnings: Vec<String>,
    note_style: NoteStyle,
    // live enhance
    notes_dirty: bool,
    last_note_edit: Instant,
    last_enhance: Instant,
    enhance_gen: u64,
    enhance_in_flight: bool,
    enhance_cancel: Option<Arc<AtomicBool>>,
    enhanced_text: String,
    /// Tokens of an in-flight regeneration while the previous version stays visible.
    enhance_buffer: String,
    enhance_lines_seen: usize,
    // live assistance
    cards: Vec<ContextCard>,
    next_card_id: u64,
    scanned_upto: usize,
    last_scan: Instant,
    scan_in_flight: bool,
    scan_gen: u64,
    scan_buffer: String,
    seen_questions: Vec<HashSet<String>>,
    pending_questions: Vec<String>,
    // summary
    summary_in_flight: bool,
    summary_text: String,
    review_tab: ReviewTab,
    transcript_open: bool,
    wrap: Option<WrapStep>,
    copied: bool,
    pending_delete: Option<PendingDelete>,
    title_buf: String,
    /// End time of the last finalized segment per source (mic, system), used to drop stale partials.
    last_final: [f64; 2],
    /// Window-width driven layout flags, refreshed every render.
    narrow: bool,
    stacked: bool,
    /// Too little room for the transcript pane beside the notes: it takes over the detail area.
    cramped: bool,
    detail_w: f32,
    _tick: Task<()>,
    _subs: Vec<Subscription>,
}

const SCAN_INTERVAL: Duration = Duration::from_secs(18);
const ENHANCE_DEBOUNCE: Duration = Duration::from_millis(2500);
const ENHANCE_MIN_GAP: Duration = Duration::from_secs(6);

impl MeetingView {
    pub fn new(services: Arc<Services>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let title = cx.new(|cx| InputState::new(window, cx).placeholder("Untitled meeting"));
        let search_input = cx.new(|cx| InputState::new(window, cx).placeholder("Search meetings"));
        let notes = cx.new(|cx| {
            TextareaState::new(window, cx).placeholder("Jot shorthand: bullets, fragments, names…")
        });
        let enhanced = cx.new(|cx| TextViewState::markdown("", cx));
        let summary = cx.new(|cx| TextViewState::markdown("", cx));
        let partial_view_mic = cx.new(|cx| TextViewState::markdown("", cx));
        let partial_view_sys = cx.new(|cx| TextViewState::markdown("", cx));
        let subs = vec![
            cx.subscribe_in(&notes, window, |this, _, ev: &InputEvent, _w, cx| {
                if let InputEvent::Change = ev {
                    this.notes_dirty = true;
                    this.last_note_edit = Instant::now();
                    cx.notify();
                }
            }),
            cx.subscribe_in(&search_input, window, |_, _, ev: &InputEvent, _w, cx| {
                if let InputEvent::Change = ev {
                    cx.notify();
                }
            }),
        ];
        let tick = cx.spawn_in(window, async move |this, cx| loop {
            cx.background_executor()
                .timer(Duration::from_millis(120))
                .await;
            if this
                .update_in(cx, |v, window, cx| v.tick(window, cx))
                .is_err()
            {
                break;
            }
        });
        let mut view = Self {
            services,
            search_input,
            mode: Mode::Idle,
            meetings: Vec::new(),
            meeting_id: None,
            title,
            notes,
            enhanced,
            summary,
            lines: Vec::new(),
            partial_mic: None,
            partial_sys: None,
            partial_view_mic,
            partial_view_sys,
            transcript_scroll: ScrollHandle::new(),
            cards_scroll: ScrollHandle::new(),
            capture: None,
            meters: None,
            started: None,
            elapsed_secs: 0,
            elapsed_f: 0.0,
            warnings: Vec::new(),
            note_style: NoteStyle::Standard,
            notes_dirty: false,
            last_note_edit: Instant::now(),
            last_enhance: Instant::now() - ENHANCE_MIN_GAP,
            enhance_gen: 0,
            enhance_in_flight: false,
            enhance_cancel: None,
            enhanced_text: String::new(),
            enhance_buffer: String::new(),
            enhance_lines_seen: 0,
            cards: Vec::new(),
            next_card_id: 1,
            scanned_upto: 0,
            last_scan: Instant::now(),
            scan_in_flight: false,
            scan_gen: 0,
            scan_buffer: String::new(),
            seen_questions: Vec::new(),
            pending_questions: Vec::new(),
            summary_in_flight: false,
            summary_text: String::new(),
            review_tab: ReviewTab::Summary,
            transcript_open: true,
            wrap: None,
            copied: false,
            pending_delete: None,
            title_buf: String::new(),
            last_final: [-1.0; 2],
            narrow: false,
            stacked: false,
            cramped: false,
            detail_w: 1000.,
            _tick: tick,
            _subs: subs,
        };
        view.note_style = view.services.settings().note_style;
        let _ = view.services.kb.store.prune_empty_meetings(None);
        view.refresh_meetings(cx);
        view
    }

    /// Ctrl+Enter: start from the ready screen, or stop an active recording.
    pub fn toggle_recording(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.is_recording() {
            self.stop(window, cx);
        } else if !self.is_wrapping() {
            if matches!(self.mode, Mode::Review(_)) {
                self.new_meeting(window, cx);
            }
            self.start(window, cx);
        }
    }

    pub fn toggle_transcript(&mut self, cx: &mut Context<Self>) {
        if self.is_recording() {
            self.transcript_open = !self.transcript_open;
            cx.notify();
        }
    }

    /// Open a stored meeting by id (command palette).
    pub fn open_by_id(&mut self, id: i64, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(m) = self.meetings.iter().find(|m| m.id == id).cloned() {
            self.open_review(m, window, cx);
        }
    }

    /// Ctrl+F: jump to the meeting search field.
    pub fn focus_search(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.search_input.update(cx, |s, cx| s.focus(window, cx));
    }

    pub fn is_wrapping(&self) -> bool {
        self.wrap.is_some()
    }

    pub fn is_recording(&self) -> bool {
        matches!(self.mode, Mode::Live) && self.capture.is_some()
    }

    pub fn meeting_count(&self) -> usize {
        self.meetings.len()
    }

    pub fn refresh_meetings(&mut self, cx: &mut Context<Self>) {
        if let Ok(m) = self.services.kb.store.list_meetings() {
            let hidden = self.pending_delete.as_ref().map(|d| d.meeting.id);
            self.meetings = m.into_iter().filter(|m| Some(m.id) != hidden).collect();
        }
        cx.notify();
    }

    // ---------- recording lifecycle ----------

    fn start(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(whisper) = self.services.engine.whisper() else {
            self.services
                .notify(NoteKind::Warning, "Transcription model is still loading.");
            return;
        };
        let mut title = self.title.read(cx).value().to_string();
        if title.trim().is_empty() {
            title = "Untitled meeting".to_string();
            self.title
                .update(cx, |s, cx| s.set_value(title.clone(), window, cx));
        }
        let meeting_id = match self.services.kb.store.create_meeting(&title) {
            Ok(id) => id,
            Err(e) => {
                self.services
                    .notify(NoteKind::Error, format!("Could not create meeting: {e:#}"));
                return;
            }
        };
        let settings = self.services.settings();
        let cfg = CaptureConfig {
            mic_device: settings.mic_device.clone(),
            capture_system: settings.capture_system_audio,
        };
        match AudioCapture::start(&cfg, whisper) {
            Ok(cap) => {
                self.warnings = cap.warnings.clone();
                for w in &cap.warnings {
                    self.services.notify(NoteKind::Warning, w.clone());
                }
                self.meters = Some(cap.meters.clone());
                *self.services.meters.write() = Some(cap.meters.clone());
                self.capture = Some(cap);
            }
            Err(e) => {
                self.services
                    .notify(NoteKind::Error, format!("Audio capture failed: {e:#}"));
                let _ = self.services.kb.store.delete_meeting(meeting_id);
                return;
            }
        }
        tracing::info!("meeting {meeting_id} started");
        self.transcript_open = !self.cramped;
        self.wrap = None;
        self.last_final = [-1.0; 2];
        self.meeting_id = Some(meeting_id);
        self.mode = Mode::Live;
        self.refresh_meetings(cx);
        self.started = Some(Instant::now());
        self.elapsed_secs = 0;
        self.lines.clear();
        self.partial_mic = None;
        self.partial_sys = None;
        self.cards.clear();
        self.seen_questions.clear();
        self.pending_questions.clear();
        self.scanned_upto = 0;
        self.last_scan = Instant::now();
        self.enhance_lines_seen = 0;
        self.enhanced_text.clear();
        self.enhance_buffer.clear();
        self.summary_text.clear();
        self.enhanced.update(cx, |s, cx| s.set_text("", cx));
        self.summary.update(cx, |s, cx| s.set_text("", cx));
        self.notes.update(cx, |s, cx| s.set_value("", window, cx));
        self.notes_dirty = false;
        cx.notify();
    }

    fn stop(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(cap) = self.capture.take() else {
            return;
        };
        tracing::info!(
            "meeting {:?} stopped after {}s",
            self.meeting_id,
            self.started.map(|s| s.elapsed().as_secs()).unwrap_or(0)
        );
        if let Some(w) = self.services.engine.whisper() {
            cap.stop(&w);
        } else {
            drop(cap);
        }
        *self.services.meters.write() = None;
        let secs = self.started.map(|s| s.elapsed().as_secs()).unwrap_or(0);
        let raw = self.notes.read(cx).value().to_string();
        if let Some(id) = self.meeting_id.filter(|_| self.lines.is_empty() && raw.trim().is_empty()) {
            let _ = self.services.kb.store.delete_meeting(id);
            self.services
                .notify(NoteKind::Info, "Nothing was captured, so the meeting was discarded.");
            self.meeting_id = None;
            self.mode = Mode::Idle;
            self.started = None;
            self.partial_mic = None;
            self.partial_sys = None;
            self.refresh_meetings(cx);
            return;
        }
        if let Some(id) = self.meeting_id {
            let _ = self
                .services
                .kb
                .store
                .update_meeting_notes(id, &raw, &self.enhanced_text);
            let _ = self.services.kb.store.finish_meeting(id, secs as i64);
            self.wrap = Some(WrapStep::Finalizing);
            if let Some(c) = self.enhance_cancel.take() {
                c.store(true, Ordering::Relaxed);
            }
            // Give trailing transcripts a moment, then summarize + index.
            let sv = self.services.clone();
            let task = cx.spawn_in(window, async move |this, cx| {
                cx.background_executor()
                    .timer(Duration::from_millis(2500))
                    .await;
                let _ = this.update_in(cx, |v, _w, cx| v.generate_summary(id, cx));
                let _ = sv;
            });
            task.detach();
        }
        self.elapsed_secs = secs;
        self.elapsed_f = secs as f64;
        self.started = None;
        self.partial_mic = None;
        self.partial_sys = None;
        cx.notify();
    }

    fn generate_summary(&mut self, meeting_id: i64, cx: &mut Context<Self>) {
        let words: usize = self.lines.iter().map(|l| l.text.split_whitespace().count()).sum();
        let Some(llm) = self.services.engine.llm().filter(|_| words >= 25) else {
            self.finish_review(meeting_id, cx);
            return;
        };
        self.wrap = Some(WrapStep::Summarizing);
        self.summary_in_flight = true;
        self.summary_text.clear();
        self.summary.update(cx, |s, cx| s.set_text("", cx));
        let transcript = self.transcript_text(self.lines.len().saturating_sub(400), 12_000);
        let notes = self.enhanced_text.clone();
        let user = format!(
            "Transcript:\n{transcript}\n\nNotes:\n{notes}\n\nWrite the meeting summary now."
        );
        llm.generate(GenRequest {
            images: Vec::new(),
            messages: vec![ChatMessage::system(SUMMARY_SYSTEM), ChatMessage::user(user)],
            params: GenParams {
                max_tokens: 700,
                temperature: 0.3,
                ..Default::default()
            },
            priority: Priority::High,
            cancel: Arc::new(AtomicBool::new(false)),
            on_event: self.services.gen_sink(GenTarget::Summary(meeting_id)),
        });
        cx.notify();
    }

    fn title_is_default(&self, cx: &App) -> bool {
        let t = self.title.read(cx).value();
        let t = t.trim();
        t.is_empty() || t.starts_with("Untitled") || t.starts_with("Meeting ·")
    }

    /// Summary is done: tidy it, then (when the title is still the default) ask the
    /// model for a short descriptive title before the review is shown.
    fn finish_review(&mut self, meeting_id: i64, cx: &mut Context<Self>) {
        self.summary_in_flight = false;
        self.summary_text = clean_summary(&self.summary_text);
        let cleaned = self.summary_text.clone();
        self.summary.update(cx, |s, cx| s.set_text(&cleaned, cx));
        let words: usize = self.lines.iter().map(|l| l.text.split_whitespace().count()).sum();
        let notes = if self.enhanced_text.trim().is_empty() {
            self.notes.read(cx).value().to_string()
        } else {
            self.enhanced_text.clone()
        };
        if self.title_is_default(cx) && (words >= 25 || notes.split_whitespace().count() >= 3) {
            if let Some(llm) = self.services.engine.llm() {
                self.wrap = Some(WrapStep::Titling);
                self.title_buf.clear();
                let basis = if !cleaned.trim().is_empty() {
                    cleaned
                } else if !notes.trim().is_empty() {
                    notes
                } else {
                    self.transcript_text(0, 3000)
                };
                llm.generate(GenRequest {
                    images: Vec::new(),
                    messages: vec![
                        ChatMessage::system(TITLE_SYSTEM),
                        ChatMessage::user(format!("Meeting notes:\n{basis}\n\nTitle:")),
                    ],
                    params: GenParams { max_tokens: 24, temperature: 0.2, ..Default::default() },
                    priority: Priority::High,
                    cancel: Arc::new(AtomicBool::new(false)),
                    on_event: self.services.gen_sink(GenTarget::Title(meeting_id)),
                });
                cx.notify();
                return;
            }
        }
        self.complete_review(meeting_id, cx);
    }

    fn complete_review(&mut self, meeting_id: i64, cx: &mut Context<Self>) {
        self.wrap = Some(WrapStep::Indexing);
        let _ = self
            .services
            .kb
            .store
            .set_meeting_summary(meeting_id, &self.summary_text);
        // Index into the knowledge base in the background.
        let sv = self.services.clone();
        let title = self.title.read(cx).value().to_string();
        let segs: Vec<(String, f64, f64, String)> = self
            .lines
            .iter()
            .map(|l| (l.source.label().to_string(), l.t0, l.t1, l.text.clone()))
            .collect();
        let notes = self.enhanced_text.clone();
        let summary = self.summary_text.clone();
        std::thread::spawn(move || {
            match sv
                .kb
                .ingest_meeting(meeting_id, &title, &segs, &notes, &summary)
            {
                Ok(n) if n > 0 => sv.notify(
                    NoteKind::Success,
                    format!("Meeting indexed · {n} passages searchable"),
                ),
                Ok(_) => {}
                Err(e) => sv.notify(NoteKind::Warning, format!("Meeting indexing failed: {e:#}")),
            }
        });
        self.refresh_meetings(cx);
        if let Some(m) = self.meetings.iter().find(|m| m.id == meeting_id).cloned() {
            self.review_tab = ReviewTab::Summary;
            self.mode = Mode::Review(m);
        } else {
            self.mode = Mode::Idle;
        }
        self.wrap = None;
        self.meeting_id = None;
        cx.notify();
    }

    fn open_review(&mut self, m: Meeting, window: &mut Window, cx: &mut Context<Self>) {
        if self.is_recording() {
            self.services
                .notify(NoteKind::Warning, "Stop the current recording first.");
            return;
        }
        self.lines = self
            .services
            .kb
            .store
            .segments(m.id)
            .unwrap_or_default()
            .into_iter()
            .map(|s| Line {
                source: if s.source == "mic" {
                    AudioSource::Microphone
                } else {
                    AudioSource::System
                },
                t0: s.t0,
                t1: s.t1,
                text: s.text,
            })
            .collect();
        self.title
            .update(cx, |s, cx| s.set_value(m.title.clone(), window, cx));
        self.notes
            .update(cx, |s, cx| s.set_value(m.notes_raw.clone(), window, cx));
        self.enhanced_text = m.notes_enhanced.clone();
        self.enhanced
            .update(cx, |s, cx| s.set_text(&m.notes_enhanced, cx));
        self.summary_text = clean_summary(&m.summary);
        let shown = self.summary_text.clone();
        self.summary.update(cx, |s, cx| s.set_text(&shown, cx));
        self.cards.clear();
        self.elapsed_secs = m.duration_secs.max(0) as u64;
        self.elapsed_f = self.elapsed_secs as f64;
        self.review_tab = if m.summary.trim().is_empty() {
            ReviewTab::Transcript
        } else {
            ReviewTab::Summary
        };
        self.mode = Mode::Review(m);
        self.notes_dirty = false;
        cx.notify();
    }

    /// Sidebar / Ctrl+N: open a fresh meeting and start recording at once.
    /// If a recording is live or finishing up, just show it.
    pub fn start_new(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.is_recording() || self.is_wrapping() {
            return;
        }
        self.new_meeting(window, cx);
        self.start(window, cx);
    }

    fn new_meeting(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.mode = Mode::Idle;
        self.lines.clear();
        self.cards.clear();
        self.enhanced_text.clear();
        self.summary_text.clear();
        self.title.update(cx, |s, cx| s.set_value("", window, cx));
        self.notes.update(cx, |s, cx| s.set_value("", window, cx));
        self.enhanced.update(cx, |s, cx| s.set_text("", cx));
        self.summary.update(cx, |s, cx| s.set_text("", cx));
        cx.notify();
    }

    /// Hide the meeting now and erase it after 8 s unless the user taps Undo.
    fn delete_meeting(&mut self, id: i64, window: &mut Window, cx: &mut Context<Self>) {
        // A second delete commits the first immediately.
        self.commit_delete(window, cx);
        let Some(meeting) = self
            .meetings
            .iter()
            .find(|m| m.id == id)
            .cloned()
            .or_else(|| match &self.mode {
                Mode::Review(m) if m.id == id => Some(m.clone()),
                _ => None,
            })
        else {
            return;
        };
        let name = if meeting.title.trim().is_empty() { "Untitled meeting".to_string() } else { meeting.title.clone() };
        let timer = cx.spawn_in(window, async move |this, cx| {
            cx.background_executor().timer(Duration::from_secs(8)).await;
            let _ = this.update_in(cx, |v, window, cx| v.commit_delete(window, cx));
        });
        self.pending_delete = Some(PendingDelete { meeting, _timer: timer });
        if matches!(&self.mode, Mode::Review(m) if m.id == id) {
            self.mode = Mode::Idle;
        }
        self.refresh_meetings(cx);

        let weak = cx.weak_entity();
        let note = Notification::new()
            .id1::<UndoDeleteToast>("undo-delete")
            .message(format!("Deleted “{}”", crate::rag::snippet_of(&name, 40)))
            .placement(Anchor::BottomRight)
            .action(move |_note, _window, cx| {
                let weak = weak.clone();
                let toast = cx.entity();
                Button::new("undo-delete-btn")
                    .label("Undo")
                    .small()
                    .outline()
                    .on_click(move |_, window, cx| {
                        let _ = weak.update(cx, |v, cx| v.undo_delete(cx));
                        toast.update(cx, |n, cx| n.dismiss(window, cx));
                    })
            });
        window.push_notification(note, cx);
    }

    /// Markdown for the open meeting: title, summary, then the notes.
    fn notes_markdown(&self, cx: &App) -> String {
        let title = self.title.read(cx).value().to_string();
        let mut md = format!("# {}\n\n", title.trim());
        if !self.summary_text.trim().is_empty() {
            md.push_str(self.summary_text.trim());
            md.push_str("\n\n");
        }
        let notes = if self.enhanced_text.trim().is_empty() {
            self.notes.read(cx).value().to_string()
        } else {
            self.enhanced_text.clone()
        };
        if !notes.trim().is_empty() {
            md.push_str("## Notes\n\n");
            md.push_str(notes.trim());
            md.push('\n');
        }
        md
    }

    fn copy_notes(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let md = self.notes_markdown(cx);
        cx.write_to_clipboard(ClipboardItem::new_string(md));
        self.copied = true;
        cx.notify();
        let task = cx.spawn_in(window, async move |this, cx| {
            cx.background_executor().timer(Duration::from_millis(1600)).await;
            let _ = this.update(cx, |v, cx| {
                v.copied = false;
                cx.notify();
            });
        });
        task.detach();
    }

    fn export_notes(&mut self, cx: &mut Context<Self>) {
        let md = self.notes_markdown(cx);
        let title = self.title.read(cx).value().to_string();
        let slug: String = title
            .trim()
            .chars()
            .map(|c| if c.is_alphanumeric() { c } else { '-' })
            .collect::<String>()
            .split('-')
            .filter(|p| !p.is_empty())
            .collect::<Vec<_>>()
            .join("-");
        let name = if slug.is_empty() { "meeting-notes.md".to_string() } else { format!("{slug}.md") };
        if let Some(path) = rfd::FileDialog::new()
            .set_title("Export notes")
            .set_file_name(&name)
            .add_filter("Markdown", &["md"])
            .save_file()
        {
            match std::fs::write(&path, md) {
                Ok(()) => self.services.notify(NoteKind::Success, format!("Saved {}", path.display())),
                Err(e) => self.services.notify(NoteKind::Error, format!("Could not save: {e}")),
            }
        }
    }

    fn undo_delete(&mut self, cx: &mut Context<Self>) {
        // Dropping the pending entry also cancels its timer.
        if self.pending_delete.take().is_some() {
            self.refresh_meetings(cx);
        }
    }

    fn commit_delete(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(pending) = self.pending_delete.take() else {
            return;
        };
        let id = pending.meeting.id;
        let _ = self.services.kb.store.delete_meeting(id);
        if let Ok(Some(doc)) = self
            .services
            .kb
            .store
            .find_document_by_source(&format!("meeting://{id}"))
        {
            let _ = self.services.kb.remove_document(doc.id);
        }
        window.remove_notification1::<UndoDeleteToast>("undo-delete", cx);
        self.refresh_meetings(cx);
    }

    // ---------- periodic work ----------

    fn tick(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        if !matches!(self.mode, Mode::Live) {
            return;
        }
        if let Some(s) = self.started {
            let f = s.elapsed().as_secs_f64();
            if (f * 10.0).floor() != (self.elapsed_f * 10.0).floor() {
                self.elapsed_f = f;
                self.elapsed_secs = f as u64;
                cx.notify();
            }
        }
        if self.capture.is_none() {
            // Wrapping up: the model is busy with the summary; no background work.
            return;
        }
        let notes_text = self.notes.read(cx).value().to_string();
        let new_transcript = self.lines.len() > self.enhance_lines_seen;
        let note_settled = self.last_note_edit.elapsed() >= ENHANCE_DEBOUNCE;
        let want_enhance = !notes_text.trim().is_empty()
            && note_settled
            && (self.notes_dirty
                || (new_transcript && self.last_enhance.elapsed() >= Duration::from_secs(25)))
            && self.last_enhance.elapsed() >= ENHANCE_MIN_GAP;
        if want_enhance && !self.enhance_in_flight {
            self.run_enhance(notes_text, cx);
        }
        if self.services.settings().live_assistance && self.capture.is_some() {
            let unscanned = self.lines.len().saturating_sub(self.scanned_upto);
            if !self.scan_in_flight
                && unscanned > 0
                && (unscanned >= 4 || self.last_scan.elapsed() >= SCAN_INTERVAL)
            {
                self.run_scan(cx);
            }
        }
        if !self.pending_questions.is_empty() {
            let q = self.pending_questions.remove(0);
            self.create_card(q, cx);
        }
    }

    pub fn on_transcript(&mut self, t: Transcript, _window: &mut Window, cx: &mut Context<Self>) {
        if !matches!(self.mode, Mode::Live) {
            return;
        }
        let si = match t.source {
            AudioSource::Microphone => 0,
            AudioSource::System => 1,
        };
        // A provisional line that ends before the last finalized segment is stale.
        if t.partial && t.t1 <= self.last_final[si] + 0.05 {
            return;
        }
        let (slot, view) = match t.source {
            AudioSource::Microphone => (&mut self.partial_mic, &self.partial_view_mic),
            AudioSource::System => (&mut self.partial_sys, &self.partial_view_sys),
        };
        if t.partial {
            // Hold back the last word: it is usually cut mid-syllable and gets
            // re-decoded on the next pass.
            let mut shown = t.text.trim().to_string();
            let words: Vec<&str> = shown.split_whitespace().collect();
            if words.len() >= 3 {
                shown = words[..words.len() - 1].join(" ");
            } else if words.len() < 2 {
                shown.clear();
            }
            let t = Transcript { text: shown, ..t };
            let new_text = escape_md(&t.text);
            let prev = slot
                .as_ref()
                .map(|l| escape_md(&l.text))
                .unwrap_or_default();
            view.update(cx, |st, cx| {
                if !prev.is_empty() && new_text.starts_with(&prev) {
                    // Only the newly recognized words fade in.
                    st.push_str(&new_text[prev.len()..], cx);
                } else {
                    st.set_text(&new_text, cx);
                }
            });
            *slot = Some(Line {
                source: t.source,
                t0: t.t0,
                t1: t.t1,
                text: t.text,
            });
            self.transcript_scroll.scroll_to_bottom();
            cx.notify();
            return;
        }
        *slot = None;
        self.last_final[si] = self.last_final[si].max(t.t1);
        view.update(cx, |st, cx| st.set_text("", cx));
        if t.text.trim().is_empty() {
            cx.notify();
            return;
        }
        if let Some(id) = self.meeting_id {
            let src = match t.source {
                AudioSource::Microphone => "mic",
                AudioSource::System => "sys",
            };
            let _ = self
                .services
                .kb
                .store
                .add_segment(id, src, t.t0, t.t1, &t.text);
        }
        if is_question(&t.text) {
            self.pending_questions.push(t.text.clone());
        }
        // Merge short continuations from the same speaker into one readable line.
        let merged = match self.lines.last_mut() {
            Some(last)
                if last.source == t.source
                    && t.t0 - last.t1 < 3.5
                    && last.text.split_whitespace().count() < 70
                    && (!ends_sentence(&last.text)
                        || last.text.split_whitespace().count() < 30
                        || t.text
                            .chars()
                            .next()
                            .map(|c| c.is_lowercase())
                            .unwrap_or(false)) =>
            {
                if !last.text.ends_with(' ') {
                    last.text.push(' ');
                }
                last.text.push_str(t.text.trim());
                last.t1 = t.t1;
                true
            }
            _ => false,
        };
        if !merged {
            self.lines.push(Line {
                source: t.source,
                t0: t.t0,
                t1: t.t1,
                text: t.text,
            });
        }
        self.transcript_scroll.scroll_to_bottom();
        cx.notify();
    }

    fn transcript_text(&self, from: usize, max_chars: usize) -> String {
        let mut out = String::new();
        for l in &self.lines[from.min(self.lines.len())..] {
            out.push_str(&format!(
                "[{}] {}: {}\n",
                fmt_ts(l.t0),
                l.source.label(),
                l.text
            ));
        }
        if out.len() > max_chars {
            let cut = out.len() - max_chars;
            let idx = out[cut..].find('\n').map(|i| cut + i + 1).unwrap_or(cut);
            out = out[idx..].to_string();
        }
        out
    }

    // ---------- Live Enhance ----------

    fn run_enhance(&mut self, notes: String, cx: &mut Context<Self>) {
        let Some(llm) = self.services.engine.llm() else {
            return;
        };
        if let Some(c) = self.enhance_cancel.take() {
            c.store(true, Ordering::Relaxed);
        }
        self.enhance_gen += 1;
        let gen = self.enhance_gen;
        self.enhance_in_flight = true;
        self.notes_dirty = false;
        self.last_enhance = Instant::now();
        self.enhance_lines_seen = self.lines.len();
        let cancel = Arc::new(AtomicBool::new(false));
        self.enhance_cancel = Some(cancel.clone());
        let transcript = self.transcript_text(self.lines.len().saturating_sub(60), 6000);
        let style = match self.note_style {
            NoteStyle::Standard => "Standard: complete sentences, keep every point, group under short headings when natural.",
            NoteStyle::Concise => "Concise: tight bullets, one line each, no filler, keep names/numbers/decisions.",
        };
        let user = format!(
            "Transcript so far (most recent last):\n{transcript}\n\nMy shorthand notes:\n{notes}\n\nStyle: {style}\n\nRewrite my notes now."
        );
        llm.generate(GenRequest {
            images: Vec::new(),
            messages: vec![ChatMessage::system(ENHANCE_SYSTEM), ChatMessage::user(user)],
            params: GenParams {
                max_tokens: 450,
                temperature: 0.25,
                ..Default::default()
            },
            priority: Priority::High,
            cancel,
            on_event: self.services.gen_sink(GenTarget::Enhance(gen)),
        });
        // First version streams live; later versions regenerate quietly and swap in.
        self.enhance_buffer.clear();
        cx.notify();
    }

    // ---------- Live Assistance ----------

    fn run_scan(&mut self, cx: &mut Context<Self>) {
        let Some(llm) = self.services.engine.llm() else {
            return;
        };
        let from = self.scanned_upto;
        let window_text = self.transcript_text(from.saturating_sub(2), 3500);
        self.scanned_upto = self.lines.len();
        self.last_scan = Instant::now();
        self.scan_in_flight = true;
        self.scan_gen += 1;
        self.scan_buffer.clear();
        let user = format!(
            "Transcript window:\n{window_text}\n\nList the questions or lookups. Output only a JSON array of strings."
        );
        llm.generate(GenRequest {
            images: Vec::new(),
            messages: vec![ChatMessage::system(SCAN_SYSTEM), ChatMessage::user(user)],
            params: GenParams {
                max_tokens: 160,
                temperature: 0.1,
                ..Default::default()
            },
            priority: Priority::Low,
            cancel: Arc::new(AtomicBool::new(false)),
            on_event: self
                .services
                .gen_sink(GenTarget::QuestionScan(self.scan_gen)),
        });
        cx.notify();
    }

    fn is_duplicate_question(&self, q: &str) -> bool {
        let words = word_set(q);
        if words.len() < 2 {
            return true;
        }
        self.seen_questions
            .iter()
            .any(|s| jaccard(s, &words) >= 0.6)
    }

    fn create_card(&mut self, question: String, cx: &mut Context<Self>) {
        let question = question.trim().trim_matches('"').to_string();
        if question.split_whitespace().count() < 3 || self.is_duplicate_question(&question) {
            return;
        }
        self.seen_questions.push(word_set(&question));
        let id = self.next_card_id;
        self.next_card_id += 1;
        let answer = cx.new(|cx| TextViewState::markdown("", cx));
        self.cards.insert(
            0,
            ContextCard {
                id,
                db_id: None,
                question: question.clone(),
                answer,
                answer_text: String::new(),
                hits: Vec::new(),
                state: CardState::Retrieving,
                gen: 0,
                expanded: false,
            },
        );
        self.cards.truncate(12);
        let sv = self.services.clone();
        let task = cx.spawn(async move |this, cx| {
            let sv2 = sv.clone();
            let q = question.clone();
            let hits = cx
                .background_executor()
                .spawn(async move { sv2.kb.search(&q, 4, Scope::All).unwrap_or_default() })
                .await;
            let _ = this.update(cx, |v, cx| v.on_card_hits(id, hits, cx));
        });
        task.detach();
        cx.notify();
    }

    fn on_card_hits(&mut self, id: u64, hits: Vec<Hit>, cx: &mut Context<Self>) {
        let Some(pos) = self.cards.iter().position(|c| c.id == id) else {
            return;
        };
        let strong = hits.iter().any(|h| h.dense >= 0.40 || h.lexical >= 0.55);
        if hits.is_empty() || !strong {
            // Keep it visible as a quiet "heard this" entry so the feature reads as alive.
            self.cards[pos].state = CardState::NoMatch;
            let no_match = self
                .cards
                .iter()
                .filter(|c| c.state == CardState::NoMatch)
                .count();
            if no_match > 3 {
                if let Some(oldest) = self
                    .cards
                    .iter()
                    .rposition(|c| c.state == CardState::NoMatch)
                {
                    self.cards.remove(oldest);
                }
            }
            cx.notify();
            return;
        }
        let question = self.cards[pos].question.clone();
        let passages = hits
            .iter()
            .enumerate()
            .map(|(i, h)| format!("[{}] {} ({})\n{}", i + 1, h.title, h.locator(), h.context))
            .collect::<Vec<_>>()
            .join("\n\n");
        let card = &mut self.cards[pos];
        card.hits = hits;
        card.state = CardState::Answering;
        card.gen += 1;
        let gen = card.gen;
        if let Some(mid) = self.meeting_id {
            let cites =
                serde_json::to_string(&card.hits.iter().map(|h| h.citation()).collect::<Vec<_>>())
                    .unwrap_or_default();
            card.db_id = self
                .services
                .kb
                .store
                .add_card(mid, &question, "", &cites)
                .ok();
        }
        if let Some(llm) = self.services.engine.llm() {
            let user = format!("Question raised in the meeting: {question}\n\nPassages:\n{passages}\n\nAnswer now.");
            llm.generate(GenRequest {
                images: Vec::new(),
                messages: vec![ChatMessage::system(CARD_SYSTEM), ChatMessage::user(user)],
                params: GenParams {
                    max_tokens: 140,
                    temperature: 0.2,
                    ..Default::default()
                },
                priority: Priority::Low,
                cancel: Arc::new(AtomicBool::new(false)),
                on_event: self.services.gen_sink(GenTarget::Card { card: id, gen }),
            });
        } else {
            card.state = CardState::Done;
        }
        let _ = self.services.kb.store.bump_counter("questions_answered", 1);
        self.cards_scroll.set_offset(point(px(0.), px(0.)));
        cx.notify();
    }

    pub fn on_gen(
        &mut self,
        target: GenTarget,
        event: GenEvent,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match target {
            GenTarget::Enhance(gen) => {
                if gen != self.enhance_gen {
                    return;
                }
                match event {
                    GenEvent::Token(t) => {
                        if self.enhanced_text.is_empty() {
                            self.enhance_buffer.push_str(&t);
                            self.enhanced.update(cx, |s, cx| s.push_str(&t, cx));
                        } else {
                            self.enhance_buffer.push_str(&t);
                        }
                    }
                    GenEvent::Done(stats) => {
                        self.enhance_in_flight = false;
                        if !stats.preempted && !self.enhance_buffer.trim().is_empty() {
                            let new_text = std::mem::take(&mut self.enhance_buffer);
                            if new_text != self.enhanced_text {
                                self.enhanced_text = new_text;
                                self.enhanced
                                    .update(cx, |s, cx| s.set_text(&self.enhanced_text, cx));
                            }
                        }
                        self.enhance_buffer.clear();
                        if let Some(id) = self.meeting_id {
                            let raw = self.notes.read(cx).value().to_string();
                            let _ = self.services.kb.store.update_meeting_notes(
                                id,
                                &raw,
                                &self.enhanced_text,
                            );
                        }
                    }
                    GenEvent::Error(e) => {
                        self.enhance_in_flight = false;
                        tracing::warn!("enhance failed: {e}");
                        if crate::app::is_backend_failure(&e) && self.services.settings().use_gpu {
                            crate::app::degrade_llm_to_cpu(self.services.clone());
                        }
                    }
                }
                cx.notify();
            }
            GenTarget::QuestionScan(gen) => {
                if gen != self.scan_gen {
                    return;
                }
                match event {
                    GenEvent::Token(t) => self.scan_buffer.push_str(&t),
                    GenEvent::Done(stats) => {
                        self.scan_in_flight = false;
                        if stats.preempted {
                            // Re-scan these lines next time.
                            self.scanned_upto = self.scanned_upto.saturating_sub(2);
                        } else {
                            for q in parse_questions(&self.scan_buffer) {
                                self.pending_questions.push(q);
                            }
                        }
                    }
                    GenEvent::Error(_) => self.scan_in_flight = false,
                }
            }
            GenTarget::Card { card, gen } => {
                let Some(c) = self.cards.iter_mut().find(|c| c.id == card) else {
                    return;
                };
                if c.gen != gen {
                    return;
                }
                match event {
                    GenEvent::Token(t) => {
                        c.answer_text.push_str(&t);
                        c.answer.update(cx, |s, cx| s.push_str(&t, cx));
                    }
                    GenEvent::Done(stats) => {
                        if stats.preempted && c.answer_text.trim().is_empty() {
                            // Retry later with the same passages.
                            c.state = CardState::Answering;
                            let question = c.question.clone();
                            let passages = c
                                .hits
                                .iter()
                                .enumerate()
                                .map(|(i, h)| {
                                    format!(
                                        "[{}] {} ({})\n{}",
                                        i + 1,
                                        h.title,
                                        h.locator(),
                                        h.context
                                    )
                                })
                                .collect::<Vec<_>>()
                                .join("\n\n");
                            c.gen += 1;
                            let gen = c.gen;
                            if let Some(llm) = self.services.engine.llm() {
                                llm.generate(GenRequest {
            images: Vec::new(),
                                    messages: vec![
                                        ChatMessage::system(CARD_SYSTEM),
                                        ChatMessage::user(format!("Question raised in the meeting: {question}\n\nPassages:\n{passages}\n\nAnswer now.")),
                                    ],
                                    params: GenParams { max_tokens: 140, temperature: 0.2, ..Default::default() },
                                    priority: Priority::Low,
                                    cancel: Arc::new(AtomicBool::new(false)),
                                    on_event: self.services.gen_sink(GenTarget::Card { card, gen }),
                                });
                            }
                        } else {
                            c.state = CardState::Done;
                            if let Some(db) = c.db_id {
                                let _ = self
                                    .services
                                    .kb
                                    .store
                                    .update_card_answer(db, &c.answer_text);
                            }
                        }
                    }
                    GenEvent::Error(_) => c.state = CardState::Done,
                }
                cx.notify();
            }
            GenTarget::Summary(meeting_id) => match event {
                GenEvent::Token(t) => {
                    self.summary_text.push_str(&t);
                    self.summary.update(cx, |s, cx| s.push_str(&t, cx));
                    cx.notify();
                }
                GenEvent::Done(_) | GenEvent::Error(_) => {
                    self.summary_in_flight = false;
                    self.finish_review(meeting_id, cx);
                }
            },
            GenTarget::Title(meeting_id) => match event {
                GenEvent::Token(t) => self.title_buf.push_str(&t),
                GenEvent::Done(_) | GenEvent::Error(_) => {
                    let title = clean_title(&self.title_buf);
                    if !title.is_empty() {
                        let _ = self.services.kb.store.update_meeting_title(meeting_id, &title);
                        self.title
                            .update(cx, |s, cx| s.set_value(title.clone(), _window, cx));
                    }
                    self.complete_review(meeting_id, cx);
                }
            },
            GenTarget::Chat(_) => {}
        }
    }

    fn set_style(&mut self, style: NoteStyle, cx: &mut Context<Self>) {
        if self.note_style != style {
            self.note_style = style;
            let mut s = self.services.settings();
            s.note_style = style;
            self.services.save_settings(s);
            self.notes_dirty = true;
            self.last_note_edit = Instant::now() - ENHANCE_DEBOUNCE;
            self.last_enhance = Instant::now() - ENHANCE_MIN_GAP;
            cx.notify();
        }
    }

    // ---------- rendering ----------

    fn render_control_bar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let p = pal(cx);
        let recording = self.is_recording();
        let reviewing = matches!(self.mode, Mode::Review(_));
        let wrapping = self.is_wrapping();
        let elapsed = if recording {
            fmt_tc(self.elapsed_f)
        } else {
            fmt_duration(self.elapsed_secs)
        };
        let transcript_open = self.transcript_open;
        let meters = self.meters.clone();
        let when = match &self.mode {
            Mode::Review(m) => chrono::DateTime::from_timestamp(m.started_at, 0)
                .map(|d| {
                    d.with_timezone(&chrono::Local)
                        .format("%A, %b %-d · %H:%M")
                        .to_string()
                })
                .unwrap_or_default(),
            _ => chrono::Local::now().format("%A, %b %-d").to_string(),
        };
        v_flex().gap_3().flex_shrink_0().child(
            h_flex()
                .gap_3()
                .flex_wrap()
                .items_center()
                .child(
                    v_flex()
                        .flex_1()
                        .min_w_0()
                        .gap_0p5()
                        .child(
                            div()
                                .font_family(p.display_font())
                                .text_2xl()
                                .font_weight(if p.dark {
                                    FontWeight::SEMIBOLD
                                } else {
                                    FontWeight::MEDIUM
                                })
                                .text_color(p.text)
                                .child(Input::new(&self.title).appearance(false).text_2xl().line_height(px(36.)).h(px(44.))),
                        )
                        .child(
                            h_flex()
                                .gap_2()
                                .items_center()
                                .child(div().text_xs().text_color(p.text_faint).child(when))
                                .when(recording || reviewing || wrapping, |el| {
                                    el.child(div().text_xs().text_color(p.text_faint).child("·"))
                                        .child(
                                            h_flex()
                                                .gap_1p5()
                                                .items_center()
                                                .when(recording, |el| {
                                                    el.child(pulse(
                                                        glow_dot(p.danger, 7.),
                                                        "hdr-rec",
                                                        1100,
                                                        0.35,
                                                        1.0,
                                                    ))
                                                })
                                                .child(
                                                    div()
                                                        .when(recording, |el| el.font_family(super::theme::MONO_FONT))
                                                        .text_sm()
                                                        .font_weight(FontWeight::MEDIUM)
                                                        .text_color(if recording {
                                                            p.text
                                                        } else {
                                                            p.text_faint
                                                        })
                                                        .child(elapsed),
                                                ),
                                        )
                                }),
                        ),
                )
                .when(recording && self.detail_w >= 900., |el| {
                    let m = meters.clone();
                    el.child(live(
                        h_flex()
                            .gap_3()
                            .items_center()
                            .when_some(m.clone(), |el, m| {
                                el.child(
                                    div()
                                        .px_3()
                                        .py_1()
                                        .rounded_full()
                                        .bg(p.surface_3)
                                        .child(waveform(m, 26., cx)),
                                )
                            })
                            .child(level_meter(
                                meters.clone(),
                                AudioSource::Microphone,
                                "Mic",
                                cx,
                            ))
                            .child(level_meter(
                                meters.clone(),
                                AudioSource::System,
                                "System",
                                cx,
                            )),
                        "meters",
                    ))
                })
                .when(recording && self.detail_w < 900., |el| {
                    el.child(live(mini_meter(meters.clone(), cx), "mini-meters"))
                })
                .when(recording, |el| {
                    el.child(
                        Button::new("toggle-transcript")
                            .outline()
                            .icon(IconName::Captions)
                            .label(if transcript_open {
                                "Hide transcript"
                            } else {
                                "Transcript"
                            })
                            .tooltip(if transcript_open {
                                "Hide the transcript pane"
                            } else {
                                "Show the transcript pane"
                            })
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.transcript_open = !this.transcript_open;
                                cx.notify();
                            })),
                    )
                })
                .child(if recording {
                    Button::new("stop")
                        .danger()
                        .icon(IconName::Square)
                        .label("Stop")
                        .on_click(cx.listener(|this, _, window, cx| this.stop(window, cx)))
                        .into_any_element()
                } else if reviewing {
                    let review_id = match &self.mode {
                        Mode::Review(m) => m.id,
                        _ => 0,
                    };
                    h_flex()
                        .gap_2()
                        .child(
                            Button::new("copy-notes")
                                .outline()
                                .small()
                                .icon(if self.copied { IconName::Check } else { IconName::Copy })
                                .label(if self.copied { "Copied" } else { "Copy" })
                                .tooltip("Copy the notes as Markdown")
                                .on_click(cx.listener(|this, _, window, cx| this.copy_notes(window, cx))),
                        )
                        .child(
                            Button::new("export-notes")
                                .outline()
                                .small()
                                .icon(IconName::Download)
                                .label("Export")
                                .tooltip("Save the notes as a Markdown file")
                                .on_click(cx.listener(|this, _, _, cx| this.export_notes(cx))),
                        )
                        .child(
                            Button::new("delete-meeting")
                                .ghost()
                                .small()
                                .icon(IconName::Trash)
                                .tooltip("Delete this meeting (undo for 8 seconds)")
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    this.delete_meeting(review_id, window, cx)
                                })),
                        )
                        .into_any_element()
                } else if let Some(step) = self.wrap {
                    // Between Stop and the finished review: say exactly what is happening.
                    h_flex()
                        .gap_3()
                        .items_center()
                        .px_3()
                        .py_1p5()
                        .rounded_full()
                        .bg(p.surface_3)
                        .border_1()
                        .border_color(p.line)
                        .child(ShimmerText::new(step.label()).id("wrap-shimmer"))
                        .child(
                            div()
                                .w(px(88.))
                                .child(Progress::new("wrap-progress").value(step.progress()).h(px(4.))),
                        )
                        .into_any_element()
                } else {
                    Button::new("start")
                        .primary()
                        .icon(IconName::Mic)
                        .label("Start recording")
                        .on_click(cx.listener(|this, _, window, cx| this.start(window, cx)))
                        .into_any_element()
                }),
        )
    }

    /// Panel chrome shared by the four live panels so they line up exactly.
    fn panel(
        &self,
        title: &'static str,
        icon: IconName,
        trailing: Option<AnyElement>,
        body: AnyElement,
        cx: &mut Context<Self>,
    ) -> Div {
        card(cx)
            .flex_1()
            .when(!self.stacked, |c| c.h_full())
            .min_h_0()
            .min_w_0()
            .child(
                card_header(title, icon, cx)
                    .child(div().flex_1())
                    .children(trailing),
            )
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .w_full()
                    .flex()
                    .flex_col()
                    .child(body),
            )
    }

    fn transcript_row(&self, l: &Line, provisional: bool, cx: &mut Context<Self>) -> Div {
        let p = pal(cx);
        let (color, initial) = match l.source {
            AudioSource::Microphone => (p.mic, "Y"),
            AudioSource::System => (p.system, "O"),
        };
        h_flex()
            .gap_2()
            .items_start()
            .px_2()
            .py_1()
            .rounded(px(6.))
            .hover(move |s| s.bg(p.surface_3))
            .child(
                div()
                    .w(px(72.))
                    .flex_shrink_0()
                    .font_family(super::theme::MONO_FONT)
                    .text_xs()
                    .text_color(p.text_faint)
                    .pt_0p5()
                    .child(fmt_tc(l.t0)),
            )
            .child(
                div()
                    .size(px(16.))
                    .flex_shrink_0()
                    .mt_0p5()
                    .rounded_full()
                    .bg(with_alpha(color, if p.dark { 0.22 } else { 0.16 }))
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_color(color)
                    .child(div().text_xs().font_weight(FontWeight::BOLD).child(initial)),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .text_sm()
                    .line_height(rems(1.4))
                    .when(provisional, |el| el.italic().text_color(p.text_dim))
                    .when(!provisional, |el| el.text_color(p.text))
                    .child(if provisional {
                        let view = match l.source {
                            AudioSource::Microphone => &self.partial_view_mic,
                            AudioSource::System => &self.partial_view_sys,
                        };
                        TextView::new(view).stream_fade(true).into_any_element()
                    } else {
                        l.text.clone().into_any_element()
                    }),
            )
            .when(provisional, |el| {
                el.child(
                    pulse(
                        dot(color, 5.),
                        SharedString::from(format!("partial-dot-{initial}")),
                        900,
                        0.3,
                        1.0,
                    )
                    .into_any_element(),
                )
            })
    }

    fn render_transcript_body(&self, cx: &mut Context<Self>) -> AnyElement {
        let recording = self.is_recording();
        let partials: Vec<&Line> = [self.partial_mic.as_ref(), self.partial_sys.as_ref()]
            .into_iter()
            .flatten()
            .filter(|l| !l.text.trim().is_empty())
            .collect();
        if self.lines.is_empty() && partials.is_empty() {
            return empty_state(
                IconName::AudioLines,
                if recording {
                    "Listening…"
                } else {
                    "No transcript yet"
                },
                if recording {
                    "Speech from your mic and system audio appears here as it is recognized."
                } else {
                    "Start transcribing to capture the conversation from any meeting app."
                },
                cx,
            )
            .into_any_element();
        }
        div()
            .id("transcript-scroll")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .track_scroll(&self.transcript_scroll)
            .px_2()
            .py_2()
            .child(
                v_flex()
                    .gap_0p5()
                    .children(self.lines.iter().enumerate().map(|(i, l)| {
                        let row = self.transcript_row(l, false, cx);
                        if i + 1 == self.lines.len() {
                            fade_in_up(row, SharedString::from(format!("line-{i}")), 0)
                                .into_any_element()
                        } else {
                            row.into_any_element()
                        }
                    }))
                    .children(
                        partials
                            .into_iter()
                            .map(|l| self.transcript_row(l, true, cx)),
                    ),
            )
            .into_any_element()
    }

    fn render_notes_body(&self, _cx: &mut Context<Self>) -> AnyElement {
        let reviewing = matches!(self.mode, Mode::Review(_));
        div()
            .flex_1()
            .min_h_0()
            .w_full()
            .p_2()
            .child(
                Textarea::new(&self.notes)
                    .appearance(false)
                    .readonly(reviewing)
                    .h(relative(1.0))
                    .text_base()
                    .line_height(rems(1.65)),
            )
            .into_any_element()
    }

    fn style_toggle(&self, cx: &mut Context<Self>) -> AnyElement {
        let style = self.note_style;
        segmented_track(cx)
            .child(
                segment_item("style-std", "Standard", None, style == NoteStyle::Standard, cx)
                    .on_click(cx.listener(|this, _, _, cx| this.set_style(NoteStyle::Standard, cx))),
            )
            .child(
                segment_item("style-concise", "Concise", None, style == NoteStyle::Concise, cx)
                    .on_click(cx.listener(|this, _, _, cx| this.set_style(NoteStyle::Concise, cx))),
            )
            .into_any_element()
    }

    fn render_enhanced_body(&self, cx: &mut Context<Self>) -> AnyElement {
        let p = pal(cx);
        if self.enhanced_text.is_empty() && !self.enhance_in_flight {
            return empty_state(
                IconName::Sparkles,
                "Your notes, expanded",
                "Type a few words in Notes. Gemma rewrites them into complete notes using the live transcript and keeps them current as the meeting moves on.",
                cx,
            )
            .into_any_element();
        }
        div()
            .id("enhanced-scroll")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .p_5()
            .text_base()
            .text_color(p.text)
            .line_height(rems(1.65))
            .child(
                TextView::new(&self.enhanced)
                    .selectable(true)
                    .stream_fade(true),
            )
            .into_any_element()
    }

    fn render_assistance_body(&self, cx: &mut Context<Self>) -> AnyElement {
        if self.cards.is_empty() {
            return empty_state(
                IconName::Lightbulb,
                "Context cards appear here",
                "When a question comes up that your knowledge base can answer, Marginalia surfaces the passage with an exact citation, before anyone has to search.",
                cx,
            )
            .into_any_element();
        }
        div()
            .id("cards-scroll")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .track_scroll(&self.cards_scroll)
            .p_3()
            .child(
                v_flex()
                    .gap_3()
                    .children(self.cards.iter().map(|c| self.render_card(c, cx))),
            )
            .into_any_element()
    }

    fn render_summary_body(&self, cx: &mut Context<Self>) -> AnyElement {
        let p = pal(cx);
        if self.summary_text.is_empty() && !self.summary_in_flight {
            let words: usize = self.lines.iter().map(|l| l.text.split_whitespace().count()).sum();
            let (title, body) = if words < 25 {
                ("Too short to summarize", "There wasn't enough speech to write a summary. Your notes and transcript are saved in the other tabs.")
            } else {
                ("No summary yet", "A summary is written automatically when a recording stops.")
            };
            return empty_state(IconName::ScrollText, title, body, cx).into_any_element();
        }
        div()
            .id("summary-scroll")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .p_5()
            .text_sm()
            .text_color(p.text)
            .line_height(rems(1.6))
            .child(
                TextView::new(&self.summary)
                    .selectable(true)
                    .stream_fade(true),
            )
            .into_any_element()
    }

    fn render_card(&self, c: &ContextCard, cx: &mut Context<Self>) -> AnyElement {
        let p = pal(cx);
        let id = c.id;
        let expanded = c.expanded;
        let body = v_flex()
            .relative()
            .gap_2()
            .p_3()
            .pl_4()
            .rounded(p.radius_sm())
            .bg(p.surface_3)
            .border_1()
            .border_color(p.line)
            .overflow_hidden()
            .child(
                div()
                    .absolute()
                    .left_0()
                    .top_0()
                    .bottom_0()
                    .w(px(3.))
                    .bg(p.accent_gradient(180.)),
            )
            .child(
                h_flex()
                    .gap_2()
                    .items_start()
                    .child(
                        Icon::new(IconName::MessageSquare)
                            .small()
                            .text_color(p.accent_2)
                            .mt_0p5(),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .text_sm()
                            .font_weight(if c.state == CardState::NoMatch {
                                FontWeight::MEDIUM
                            } else {
                                FontWeight::SEMIBOLD
                            })
                            .text_color(if c.state == CardState::NoMatch {
                                p.text_dim
                            } else {
                                p.text
                            })
                            .line_height(rems(1.4))
                            .child(c.question.clone()),
                    )
                    .child(match c.state {
                        CardState::Retrieving => ShimmerText::new("searching")
                            .id(SharedString::from(format!("c-sh-{id}")))
                            .into_any_element(),
                        CardState::Answering => ShimmerText::new("answering")
                            .id(SharedString::from(format!("c-sh-{id}")))
                            .into_any_element(),
                        CardState::Done => chip("answered", p.success, cx).into_any_element(),
                        CardState::NoMatch => {
                            chip("not in your documents", p.text_faint, cx).into_any_element()
                        }
                    })
                    .child(
                        Button::new(SharedString::from(format!("dismiss-{id}")))
                            .ghost()
                            .small()
                            .tooltip("Dismiss")
                            .icon(IconName::X)
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.cards.retain(|c| c.id != id);
                                cx.notify();
                            })),
                    ),
            )
            .when(
                !c.answer_text.is_empty() || c.state == CardState::Answering,
                |el| {
                    el.child(
                        div()
                            .text_sm()
                            .text_color(p.text)
                            .line_height(rems(1.5))
                            .child(TextView::new(&c.answer).stream_fade(true)),
                    )
                },
            )
            .when(!c.hits.is_empty(), |el| {
                el.child(
                    v_flex()
                        .gap_2()
                        .child(
                            h_flex()
                                .id(SharedString::from(format!("card-toggle-{id}")))
                                .gap_1()
                                .items_center()
                                .cursor_pointer()
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    if let Some(c) = this.cards.iter_mut().find(|c| c.id == id) {
                                        c.expanded = !c.expanded;
                                        cx.notify();
                                    }
                                }))
                                .child(eyebrow(format!("{} sources", c.hits.len()), cx))
                                .child(
                                    Icon::new(if expanded {
                                        IconName::ChevronDown
                                    } else {
                                        IconName::ChevronRight
                                    })
                                    .xsmall()
                                    .text_color(p.text_faint),
                                ),
                        )
                        .children(
                            c.hits
                                .iter()
                                .enumerate()
                                .filter(|(i, _)| expanded || *i == 0)
                                .map(|(i, h)| hit_card(h, i, cx)),
                        ),
                )
            });
        let glow = settle_glow(
            body,
            SharedString::from(format!("glow-{id}")),
            p.accent,
            p.line,
        );
        slide_in_right(div().child(glow), SharedString::from(format!("card-{id}")))
            .into_any_element()
    }

    // ---------- list pane ----------

    fn render_list_pane(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let p = pal(cx);
        let selected: Option<i64> = match &self.mode {
            Mode::Review(m) => Some(m.id),
            Mode::Live => self.meeting_id,
            Mode::Idle => None,
        };
        let query = self.search_input.read(cx).value().trim().to_lowercase();
        let visible: Vec<&Meeting> = self
            .meetings
            .iter()
            .filter(|m| {
                query.is_empty()
                    || m.title.to_lowercase().contains(&query)
                    || m.summary.to_lowercase().contains(&query)
                    || m.notes_enhanced.to_lowercase().contains(&query)
                    || m.notes_raw.to_lowercase().contains(&query)
            })
            .collect();
        let no_results = !query.is_empty() && visible.is_empty();
        // Group by day.
        let mut groups: Vec<(String, Vec<&Meeting>)> = Vec::new();
        let today = chrono::Local::now().date_naive();
        for m in visible {
            let d = chrono::DateTime::from_timestamp(m.started_at, 0)
                .map(|d| d.with_timezone(&chrono::Local).date_naive())
                .unwrap_or(today);
            let label = if d == today {
                "Today".to_string()
            } else if d == today.pred_opt().unwrap_or(today) {
                "Yesterday".to_string()
            } else {
                d.format("%b %-d, %Y").to_string()
            };
            match groups.last_mut() {
                Some((l, v)) if *l == label => v.push(m),
                _ => groups.push((label, vec![m])),
            }
        }
        v_flex()
            .w(px(if self.narrow { 240. } else { 300. }))
            .h_full()
            .flex_shrink_0()
            .border_r_1()
            .border_color(p.line)
            .child(
                h_flex()
                    .px_4()
                    .pt_5()
                    .pb_3()
                    .items_center()
                    .gap_2()
                    .child(div().flex_1().child(page_title(
                        "Meetings",
                        format!("{} recorded", self.meetings.len()),
                        cx,
                    ))),
            )
            .when(!self.meetings.is_empty(), |el| {
                el.child(
                    div().px_3().pb_2().child(
                        Input::new(&self.search_input)
                            .cleanable(true)
                            .prefix(Icon::new(IconName::Search).small().text_color(p.text_faint)),
                    ),
                )
            })
            .child(
                div()
                    .id("meeting-list")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .px_3()
                    .pb_3()
                    .child(
                        v_flex()
                            .gap_1()
                            .when(no_results, |el| {
                                el.child(
                                    div()
                                        .px_3()
                                        .py_6()
                                        .t_footnote()
                                        .text_color(p.text_dim)
                                        .text_center()
                                        .child(format!("No meetings match “{}”.", crate::rag::snippet_of(&query, 28))),
                                )
                            })
                            .children(groups.into_iter().flat_map(|(label, items)| {
                                let mut v: Vec<AnyElement> = vec![div()
                                    .px_2()
                                    .pt_3()
                                    .pb_1()
                                    .child(eyebrow(label, cx))
                                    .into_any_element()];
                                for (i, m) in items.into_iter().enumerate() {
                                    let active = selected == Some(m.id);
                                    let m2 = m.clone();
                                    let id = m.id;
                                    let time = chrono::DateTime::from_timestamp(m.started_at, 0)
                                        .map(|d| {
                                            d.with_timezone(&chrono::Local)
                                                .format("%H:%M")
                                                .to_string()
                                        })
                                        .unwrap_or_default();
                                    let dur = if m.duration_secs >= 60 {
                                        format!("{} min", m.duration_secs / 60)
                                    } else {
                                        format!("{} s", m.duration_secs)
                                    };
                                    let preview = plain_preview(
                                        &clean_summary(if m.summary.is_empty() {
                                            &m.notes_enhanced
                                        } else {
                                            &m.summary
                                        }),
                                        90,
                                    );
                                    let row = v_flex()
                                        .id(SharedString::from(format!("meeting-{id}")))
                                        .relative()
                                        .gap_1()
                                        .px_3()
                                        .py_2p5()
                                        .min_h(px(56.))
                                        .rounded(p.radius_sm())
                                        .cursor_pointer()
                                        .when(active, |el| {
                                            el.bg(p.surface_3).child(
                                                div()
                                                    .absolute()
                                                    .left_0()
                                                    .top(px(10.))
                                                    .bottom(px(10.))
                                                    .w(px(2.))
                                                    .rounded_full()
                                                    .bg(p.accent_2),
                                            )
                                        })
                                        .hover(move |s| s.bg(p.surface_3))
                                        .on_click(cx.listener(move |this, _, window, cx| {
                                            this.open_review(m2.clone(), window, cx)
                                        }))
                                        .child(
                                            h_flex()
                                                .gap_2()
                                                .items_center()
                                                .child(
                                                    div()
                                                        .flex_1()
                                                        .min_w_0()
                                                        .truncate()
                                                        .t_body()
                                                        .font_weight(FontWeight::SEMIBOLD)
                                                        .text_color(p.text)
                                                        .child(m.title.clone()),
                                                )
                                                .child(
                                                    div()
                                                        .t_caption()
                                                        .text_color(p.text_faint)
                                                        .child(time),
                                                ),
                                        )
                                        .child(
                                            h_flex()
                                                .gap_2()
                                                .items_center()
                                                .child(
                                                    div()
                                                        .flex_1()
                                                        .min_w_0()
                                                        .truncate()
                                                        .t_footnote()
                                                        .text_color(p.text_dim)
                                                        .child(if preview.is_empty() {
                                                            "No notes".to_string()
                                                        } else {
                                                            preview
                                                        }),
                                                )
                                                .child(
                                                    div()
                                                        .t_caption()
                                                        .text_color(p.text_faint)
                                                        .child(dur),
                                                ),
                                        );
                                    v.push(
                                        fade_in_up(
                                            row,
                                            SharedString::from(format!("meet-anim-{id}")),
                                            (i as u64).min(6) * 55,
                                        )
                                        .into_any_element(),
                                    );
                                }
                                v
                            })),
                    ),
            )
    }

    fn render_welcome(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let p = pal(cx);
        let feature = |icon: IconName, title: &'static str, body: &'static str, cx: &App| {
            h_flex()
                .gap_3()
                .items_start()
                .w(px(268.))
                .flex_shrink_0()
                .child(icon_tile(icon, p.accent_2, 32., cx))
                .child(
                    v_flex()
                        .flex_1()
                        .min_w_0()
                        .gap_0p5()
                        .child(
                            div()
                                .t_body()
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(p.text)
                                .child(title),
                        )
                        .child(div().t_footnote().text_color(p.text_dim).child(body)),
                )
        };
        // 3D orbital graphic: two perspective-tilted concentric rings + drifting nodes
        let orbital_bg = if p.dark {
            let accent = p.accent;
            let accent2 = p.accent_2;
            let accent3 = p.accent_3;
            canvas(
                move |_, _, _| (),
                move |bounds, _, window, _| {
                    let w = f32::from(bounds.size.width);
                    let h = f32::from(bounds.size.height);
                    let ox = f32::from(bounds.origin.x);
                    let oy = f32::from(bounds.origin.y);
                    let cx_f = ox + w * 0.5;
                    let cy_f = oy + h * 0.38;
                    let t = std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .map(|d| d.as_secs_f32())
                        .unwrap_or(0.0);

                    // Draw three perspective-tilted orbital rings
                    let rings = [
                        (w * 0.30, w * 0.062, 0.9f32, accent, 0.0f32),
                        (w * 0.42, w * 0.088, -0.55, accent2, 1.8),
                        (w * 0.55, w * 0.105, 0.35, accent3, 3.5),
                    ];
                    for (rx, ry, speed, color, phase) in rings {
                        let segments = 48usize;
                        for seg in 0..segments {
                            let frac = seg as f32 / segments as f32;
                            let a0 = frac * std::f32::consts::TAU + t * speed + phase;
                            let a1 = a0 + std::f32::consts::TAU / segments as f32;
                            let x0 = cx_f + rx * a0.cos();
                            let y0 = cy_f + ry * a0.sin();
                            let x1 = cx_f + rx * a1.cos();
                            let y1 = cy_f + ry * a1.sin();
                            // depth alpha: front = bright, back = dim
                            let vis = ((a0 + t * speed + phase).sin() * 0.5 + 0.5).powf(1.6);
                            let alpha = (0.04 + 0.09 * vis).min(0.14);
                            let seg_w = 0.8 + vis * 0.8;
                            let mut pb = PathBuilder::stroke(px(seg_w));
                            pb.move_to(point(px(x0), px(y0)));
                            pb.line_to(point(px(x1), px(y1)));
                            if let Ok(path) = pb.build() {
                                window.paint_path(path, super::theme::with_alpha(color, alpha));
                            }
                        }
                        // Satellite dot on each ring: faint, and never over the headline band
                        // (a bright dot between letters reads as stray punctuation).
                        let lead = t * speed + phase;
                        let sx = cx_f + rx * lead.cos();
                        let sy = cy_f + ry * lead.sin();
                        let in_text_band = (sy - oy) > h * 0.34 && (sy - oy) < h * 0.66 && (sx - cx_f).abs() < w * 0.34;
                        if !in_text_band {
                            let vis = (lead.sin() * 0.5 + 0.5).powf(1.6);
                            let dot_r = 2.0 + vis * 1.2;
                            window.paint_quad(
                                fill(
                                    Bounds::new(
                                        point(px(sx - dot_r / 2.0), px(sy - dot_r / 2.0)),
                                        gpui_kit::size(px(dot_r), px(dot_r)),
                                    ),
                                    super::theme::with_alpha(color, 0.18 + 0.20 * vis),
                                )
                                .corner_radii(px(dot_r * 0.5)),
                            );
                        }
                    }
                },
            )
            .absolute()
            .inset_0()
            .into_any_element()
        } else {
            div().into_any_element()
        };

        // First run gets the full pitch; once there is history, the screen is a calm "ready".
        let first_run = self.meetings.is_empty();
        let headline_weight = if p.dark { FontWeight::SEMIBOLD } else { FontWeight::MEDIUM };
        let hero = v_flex()
            .relative()
            .overflow_hidden()
            .size_full()
            .items_center()
            .justify_center()
            .p_8()
            .gap_8()
            .child(live(orbital_bg, "hero-orbital"))
            .child(scale_in(
                v_flex()
                    .items_center()
                    .gap_4()
                    .max_w(px(620.))
                    .child(chip("On this device · nothing is uploaded", p.success, cx))
                    .child(if first_run {
                        v_flex()
                            .items_center()
                            .child(
                                div()
                                    .font_family(p.display_font())
                                    .t_display()
                                    .font_weight(headline_weight)
                                    .text_color(p.text)
                                    .text_center()
                                    .child("Take notes in shorthand."),
                            )
                            .child(
                                div()
                                    .font_family(p.display_font())
                                    .t_display()
                                    .font_weight(headline_weight)
                                    .text_color(p.text)
                                    .text_center()
                                    .child("Get the whole story."),
                            )
                            .into_any_element()
                    } else {
                        div()
                            .font_family(p.display_font())
                            .t_display()
                            .font_weight(headline_weight)
                            .text_color(p.text)
                            .text_center()
                            .child("Ready when you are.")
                            .into_any_element()
                    })
                    .child(
                        div()
                            .t_body()
                            .text_color(p.text_dim)
                            .text_center()
                            .max_w(px(500.))
                            .child(if first_run {
                                "Marginalia listens to your microphone and system audio, transcribes it, expands your bullets as you type, and answers questions from your own documents, with exact citations."
                            } else {
                                "Jot fragments while you listen. Marginalia writes them up, and finds answers in your documents."
                            }),
                    )
                    .child(
                        v_flex()
                            .items_center()
                            .gap_2()
                            .mt_2()
                            .child(
                                Button::new("hero-start")
                                    .primary()
                                    .large()
                                    .icon(IconName::Mic)
                                    .label("Start recording")
                                    .on_click(cx.listener(|this, _, window, cx| this.start(window, cx))),
                            )
                            .child(div().t_caption().text_color(p.text_faint).child("or press Ctrl+Enter")),
                    ),
                "hero-content",
                80,
            ))
            .when(first_run, |el| {
                el.child(
                    h_flex()
                        .items_start()
                        .justify_center()
                        .gap_6()
                        .w_full()
                        .flex_wrap()
                        .child(fade_in_up(feature(IconName::Captions, "Live transcript", "Both sides of any call, tagged You and Others, in any meeting app.", cx), "feat-0", 160))
                        .child(fade_in_up(feature(IconName::WandSparkles, "Live Enhance", "Your fragments become complete notes while the meeting runs.", cx), "feat-1", 240))
                        .child(fade_in_up(feature(IconName::Lightbulb, "Live Assistance", "Questions are answered from your documents before anyone searches.", cx), "feat-2", 320)),
                )
            });
        fade_in_up(hero, "hero", 0)
    }

    fn render_detail(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let live = matches!(self.mode, Mode::Live);
        let reviewing = matches!(self.mode, Mode::Review(_));
        if !live && !reviewing {
            return div()
                .flex_1()
                .min_w_0()
                .h_full()
                .child(self.render_welcome(cx))
                .into_any_element();
        }
        let p = pal(cx);
        let panels: AnyElement = if live {
            h_flex()
                .flex_1()
                .min_h_0()
                .items_start()
                .when(!self.stacked, |el| el.h_full())
                .gap_4()
                .when(self.stacked, |el| el.flex_col().items_stretch())
                .child(
                    self.panel(
                        "Notes",
                        IconName::NotebookPen,
                        Some(
                            div()
                                .text_xs()
                                .text_color(p.text_faint)
                                .child("shorthand is fine")
                                .into_any_element(),
                        ),
                        self.render_notes_body(cx),
                        cx,
                    ),
                )
                .child(
                    self.panel(
                        "Live Enhance",
                        IconName::WandSparkles,
                        Some(
                            h_flex()
                                .gap_2()
                                .items_center()
                                .when(self.enhance_in_flight, |el| {
                                    el.child(
                                        ShimmerText::new(if self.enhanced_text.is_empty() {
                                            "writing…"
                                        } else {
                                            "updating…"
                                        })
                                        .id("enh-shimmer"),
                                    )
                                })
                                .child(self.style_toggle(cx))
                                .into_any_element(),
                        ),
                        self.render_enhanced_body(cx),
                        cx,
                    ),
                )
                .into_any_element()
        } else {
            let tab = self.review_tab;
            let body: AnyElement = match tab {
                ReviewTab::Summary => self
                    .panel(
                        "Summary",
                        IconName::ScrollText,
                        self.summary_in_flight.then(|| {
                            ShimmerText::new("summarizing…")
                                .id("sum-shimmer")
                                .into_any_element()
                        }),
                        self.render_summary_body(cx),
                        cx,
                    )
                    .into_any_element(),
                ReviewTab::Notes => h_flex()
                    .flex_1()
                    .min_h_0()
                    .items_start()
                    .when(!self.stacked, |el| el.h_full())
                    .gap_4()
                    .when(self.stacked, |el| el.flex_col().items_stretch())
                    .child(self.panel(
                        "Notes",
                        IconName::NotebookPen,
                        None,
                        self.render_notes_body(cx),
                        cx,
                    ))
                    .child(self.panel(
                        "Enhanced notes",
                        IconName::WandSparkles,
                        None,
                        self.render_enhanced_body(cx),
                        cx,
                    ))
                    .into_any_element(),
                ReviewTab::Transcript => self
                    .panel(
                        "Transcript",
                        IconName::Captions,
                        Some(
                            chip(format!("{} lines", self.lines.len()), p.text_faint, cx)
                                .into_any_element(),
                        ),
                        self.render_transcript_body(cx),
                        cx,
                    )
                    .into_any_element(),
                ReviewTab::Assistance => self
                    .panel(
                        "Live Assistance",
                        IconName::Lightbulb,
                        None,
                        self.render_assistance_body(cx),
                        cx,
                    )
                    .into_any_element(),
            };
            v_flex()
                .flex_1()
                .min_h_0()
                .gap_4()
                .child(
                    h_flex().child(
                        segmented_track(cx)
                            .child(
                                segment_item("tab-summary", "Summary", None, tab == ReviewTab::Summary, cx)
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.review_tab = ReviewTab::Summary;
                                        cx.notify();
                                    })),
                            )
                            .child(
                                segment_item("tab-notes", "Notes", None, tab == ReviewTab::Notes, cx)
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.review_tab = ReviewTab::Notes;
                                        cx.notify();
                                    })),
                            )
                            .child(
                                segment_item(
                                    "tab-transcript",
                                    "Transcript",
                                    Some(self.lines.len()),
                                    tab == ReviewTab::Transcript,
                                    cx,
                                )
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.review_tab = ReviewTab::Transcript;
                                    cx.notify();
                                })),
                            ),
                    ),
                )
                .child(body)
                .into_any_element()
        };
        // Where the transcript lives: a side pane when there is room, otherwise a drawer
        // (expanded) or a one-line caption dock (collapsed). Notes never lose their space.
        let side_pane = live && self.transcript_open && !self.cramped;
        let drawer = live && self.transcript_open && self.cramped;
        let dock = live && !side_pane && !drawer;
        let show_inline_cards = live && !side_pane && !drawer && !self.cards.is_empty();
        let main = v_flex()
            .flex_1()
            .min_w_0()
            .h_full()
            .p_5()
            .gap_4()
            .child(self.render_control_bar(cx))
            .when(show_inline_cards, |el| {
                el.child(
                    div()
                        .id("cards-strip")
                        .max_h(px(220.))
                        .overflow_y_scroll()
                        .child(v_flex().gap_2().children(
                            self.cards.iter().take(2).map(|c| self.render_card(c, cx)),
                        )),
                )
            })
            .child(panels)
            .when(dock, |el| el.child(self.render_caption_dock(cx)))
            .when(drawer, |el| el.child(self.render_transcript_pane(true, cx)));
        if side_pane {
            h_flex()
                .flex_1()
                .min_w_0()
                .h_full()
                .items_start()
                .child(main)
                .child(self.render_transcript_pane(false, cx))
                .into_any_element()
        } else {
            main.into_any_element()
        }
    }

    /// One calm line that proves the app is listening, with the latest words fading in.
    fn render_caption_dock(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let p = pal(cx);
        let recording = self.is_recording();
        let partial = [
            (&self.partial_sys, &self.partial_view_sys),
            (&self.partial_mic, &self.partial_view_mic),
        ]
        .into_iter()
        .find(|(l, _)| l.as_ref().map(|l| !l.text.trim().is_empty()).unwrap_or(false));
        let last_final = self.lines.last();
        let enabled = self.services.settings().live_assistance;
        let answers = self
            .cards
            .iter()
            .filter(|c| c.state == CardState::Done)
            .count();
        h_flex()
            .id("caption-dock")
            .w_full()
            .flex_shrink_0()
            .h(px(44.))
            .px_3()
            .gap_3()
            .items_center()
            .rounded(p.radius_sm())
            .bg(p.surface_2)
            .border_1()
            .border_color(p.line)
            .child(Icon::new(IconName::Captions).small().text_color(p.text_faint))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .h(px(22.))
                    .overflow_hidden()
                    .t_footnote()
                    .text_color(p.text_dim)
                    .child(if let Some((_, view)) = partial {
                        TextView::new(view).stream_fade(true).into_any_element()
                    } else if let Some(l) = last_final {
                        l.text.clone().into_any_element()
                    } else {
                        div()
                            .text_color(p.text_faint)
                            .child(if recording { "Listening…" } else { "Recording stopped" })
                            .into_any_element()
                    }),
            )
            .when(recording && enabled && answers > 0, |el| {
                el.child(chip(format!("{answers} answered"), p.success, cx))
            })
            .when(recording && enabled && answers == 0, |el| {
                el.child(
                    h_flex()
                        .gap_1p5()
                        .items_center()
                        .child(pulse(dot(p.success, 6.), "dock-dot", 1600, 0.3, 1.0))
                        .child(div().t_caption().text_color(p.text_faint).child("watching")),
                )
            })
            .when(recording, |el| el.child(
                Button::new("dock-expand")
                    .ghost()
                    .small()
                    .icon(IconName::ChevronUp)
                    .label("Transcript")
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.transcript_open = true;
                        cx.notify();
                    })),
            ))
    }

    /// Collapsible right-hand transcript pane for live sessions.
    fn render_transcript_pane(&self, docked: bool, cx: &mut Context<Self>) -> impl IntoElement {
        let p = pal(cx);
        let queued = self
            .services
            .engine
            .whisper()
            .map(|w| w.queue_len())
            .unwrap_or(0);
        v_flex()
            .bg(p.surface_2)
            .border_color(p.line)
            .flex_shrink_0()
            .when(!docked, |el| {
                el.w(px(if self.narrow { 320. } else { 400. }))
                    .h_full()
                    .border_l_1()
            })
            .when(docked, |el| {
                el.w_full()
                    .h(px(300.))
                    .rounded(p.radius_sm())
                    .border_1()
                    .overflow_hidden()
            })
            .child(
                h_flex()
                    .px_3()
                    .py_2p5()
                    .gap_2()
                    .items_center()
                    .border_b_1()
                    .border_color(p.line)
                    .child(icon_tile(IconName::Captions, p.accent_2, 26., cx))
                    .child(
                        div()
                            .t_body()
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(p.text)
                            .child("Transcript"),
                    )
                    .child(div().flex_1())
                    .when(queued > 0, |el| {
                        el.child(chip("transcribing", p.text_faint, cx))
                    })
                    .when(self.is_recording(), |el| el.child(chip("Live", p.danger, cx)))
                    .when(docked, |el| {
                        el.child(
                            Button::new("drawer-collapse")
                                .ghost()
                                .small()
                                .icon(IconName::ChevronDown)
                                .tooltip("Collapse transcript")
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.transcript_open = false;
                                    cx.notify();
                                })),
                        )
                    }),
            )
            .child(self.render_assistance_section(cx))
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .w_full()
                    .flex()
                    .flex_col()
                    .child(self.render_transcript_body(cx)),
            )
    }

    /// Live Assistance lives at the top of the side pane: a quiet one-line
    /// status while nothing is found, a bounded card stack when there is.
    fn render_assistance_section(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let p = pal(cx);
        let enabled = self.services.settings().live_assistance;
        let header = h_flex()
            .px_3()
            .py_2()
            .gap_2()
            .items_center()
            .border_b_1()
            .border_color(p.line)
            .child(icon_tile(IconName::Lightbulb, p.warning, 22., cx))
            .child(
                div()
                    .text_xs()
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(p.text)
                    .child("Live Assistance"),
            )
            .child(div().flex_1())
            .when(self.scan_in_flight, |el| {
                el.child(ShimmerText::new("scanning").id("scan-shimmer"))
            })
            .when(
                !self.scan_in_flight && enabled && self.cards.is_empty(),
                |el| {
                    el.child(
                        h_flex()
                            .gap_1p5()
                            .items_center()
                            .child(pulse(dot(p.success, 6.), "assist-dot", 1600, 0.3, 1.0))
                            .child(div().text_xs().text_color(p.text_faint).child("watching")),
                    )
                },
            )
            .when(!enabled, |el| {
                el.child(div().text_xs().text_color(p.text_faint).child("off"))
            });
        v_flex()
            .w_full()
            .flex_shrink_0()
            .max_h(relative(0.5))
            .border_b_1()
            .border_color(p.line)
            .child(header)
            .when(self.cards.is_empty(), |el| {
                let docs = self.services.kb.chunk_count();
                el.child(
                    div().px_3().py_2().t_footnote().text_color(p.text_faint).child(if enabled {
                        if docs == 0 {
                            "Add documents in Knowledge so questions can be answered here.".to_string()
                        } else {
                            format!("Answers appear here when a question matches your {docs} indexed passages.")
                        }
                    } else {
                        "Turned off in Settings.".to_string()
                    }),
                )
            })
            .when(!self.cards.is_empty(), |el| {
                el.child(
                    div()
                        .id("cards-scroll")
                        .max_h(px(420.))
                        .overflow_y_scroll()
                        .track_scroll(&self.cards_scroll)
                        .p_2()
                        .child(v_flex().gap_2().children(self.cards.iter().map(|c| self.render_card(c, cx)))),
                )
            })
    }
}

impl Render for MeetingView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let w = f32::from(window.viewport_size().width);
        self.narrow = w < 1280.;
        let chrome = (if w < 1100. { 68. } else { 240. }) + if self.narrow { 240. } else { 300. };
        let pane = if self.narrow { 320. } else { 400. };
        self.cramped = w - chrome - pane < 640.;
        let side = if self.transcript_open && !self.cramped && matches!(self.mode, Mode::Live) {
            pane
        } else {
            0.
        };
        self.detail_w = w - chrome - side;
        self.stacked = self.detail_w < 780.;
        h_flex()
            .size_full()
            .child(self.render_list_pane(cx))
            .child(self.render_detail(cx))
    }
}

/// Markdown-free one-liner for list previews.
fn plain_preview(md: &str, max: usize) -> String {
    let lines: Vec<&str> = md.lines().map(str::trim).filter(|l| !l.is_empty()).collect();
    let has_body = lines.iter().any(|l| !l.starts_with('#'));
    let cleaned: String = lines
        .into_iter()
        .filter(|l| !(has_body && l.starts_with('#')))
        .map(|l| l.trim_start_matches(['#', '-', '*', '>']).trim())
        .filter(|l| !l.is_empty() && !l.eq_ignore_ascii_case("summary"))
        .collect::<Vec<_>>()
        .join(" ");
    crate::rag::snippet_of(&cleaned.replace("**", ""), max)
}

/// Studio-style timecode: `MM:SS.t`, with hours when needed.
pub fn fmt_tc(secs: f64) -> String {
    let secs = secs.max(0.0);
    let total = secs.floor() as u64;
    let tenths = ((secs - total as f64) * 10.0).floor() as u64;
    let (h, m, s) = (total / 3600, (total / 60) % 60, total % 60);
    if h > 0 {
        format!("{h}:{m:02}:{s:02}.{tenths}")
    } else {
        format!("{m:02}:{s:02}.{tenths}")
    }
}

/// Keep whisper text literal inside the markdown-backed streaming view.
fn escape_md(t: &str) -> String {
    t.replace('*', "\\*")
        .replace('_', "\\_")
        .replace('#', "\\#")
        .replace('`', "\\`")
}

fn ends_sentence(text: &str) -> bool {
    let t = text.trim_end();
    t.ends_with('.') || t.ends_with('?') || t.ends_with('!')
}

fn is_question(text: &str) -> bool {
    let t = text.trim();
    let words = t.split_whitespace().count();
    if words < 4 {
        return false;
    }
    if t.ends_with('?') {
        return true;
    }
    let lower = t.to_lowercase();
    const STARTS: [&str; 16] = [
        "what is",
        "what's",
        "what are",
        "how do",
        "how does",
        "how much",
        "how many",
        "do we have",
        "does anyone know",
        "where is",
        "when did",
        "when is",
        "who is",
        "can someone",
        "is there",
        "did we",
    ];
    STARTS.iter().any(|s| lower.starts_with(s))
}

fn word_set(s: &str) -> HashSet<String> {
    s.split(|c: char| !c.is_alphanumeric())
        .filter(|w| w.len() > 2)
        .map(|w| w.to_lowercase())
        .collect()
}

fn jaccard(a: &HashSet<String>, b: &HashSet<String>) -> f32 {
    let inter = a.intersection(b).count() as f32;
    let union = a.union(b).count() as f32;
    if union == 0.0 {
        0.0
    } else {
        inter / union
    }
}

fn parse_questions(raw: &str) -> Vec<String> {
    if let Some(start) = raw.find('[') {
        if let Some(end) = raw[start..].rfind(']') {
            if let Ok(v) = serde_json::from_str::<Vec<String>>(&raw[start..=start + end]) {
                return v
                    .into_iter()
                    .map(|s| s.trim().to_string())
                    .filter(|s| s.len() > 8)
                    .collect();
            }
        }
    }
    // Fallback: quoted strings or bullet lines.
    let mut out = Vec::new();
    for cap in regex::Regex::new(r#""([^"]{8,200})""#)
        .unwrap()
        .captures_iter(raw)
    {
        out.push(cap[1].to_string());
    }
    if out.is_empty() {
        for line in raw.lines() {
            let l = line.trim().trim_start_matches(['-', '*', '•']).trim();
            if l.len() > 8 && l.ends_with('?') {
                out.push(l.to_string());
            }
        }
    }
    out
}

const ENHANCE_SYSTEM: &str = "You are Marginalia, an on-device meeting assistant. The user types shorthand notes during a live meeting. Rewrite their notes into clear, complete meeting notes in Markdown. Rules: keep the user's structure, order and intent; expand abbreviations and fragments using the transcript for context; add names, numbers, decisions and action items that the transcript confirms; never invent facts that are in neither the notes nor the transcript; do not summarize the whole meeting, only enrich what the user wrote; no preamble, output only the notes.";

const SCAN_SYSTEM: &str = "You extract questions from a live meeting transcript. Return a JSON array of concise, self-contained questions or lookups that someone would answer from documents (specs, policies, contracts, pricing, past meetings). Rephrase fragments into full questions. Ignore small talk, rhetorical questions and questions already answered in the window. Return [] if there are none. Output only JSON.";

const CARD_SYSTEM: &str = "You answer a question raised during a live meeting using only the provided passages. Reply in at most 60 words, plain and direct, then cite the passage numbers you used like [1]. If the passages do not answer it, say 'Not covered in your documents.' Output only the answer.";

const TITLE_SYSTEM: &str = "Write a short descriptive title for this meeting, 3 to 7 words, in sentence case (capitalise only the first word and proper nouns). No quotes, no trailing punctuation, no date, no prefix like 'Title:'. Output only the title.";


const SUMMARY_SYSTEM: &str = "You are Marginalia, an on-device meeting note-taker. Write the notes in Markdown. Begin directly with two to four plain sentences saying what the meeting was about and what was concluded: no heading before them. Then add only the sections that have real content, each as a '## ' heading with bullets: 'Decisions', 'Action items' (owner first when known), 'Open questions'. Never write a section that would say None or N/A. Never mention 'the transcript', 'the recording' or 'the provided text'; write as the person who took the notes. Base everything on the material; do not invent. If there is too little substance, write one sentence saying so.";

/// Drop sentences that talk about the input instead of the meeting, e.g.
/// "No decisions were made based on the provided material."
fn strip_meta(text: &str) -> String {
    const META: [&str; 6] = [
        "provided material",
        "provided transcript",
        "provided text",
        "the transcript",
        "the recording",
        "the material provided",
    ];
    let mut out = Vec::new();
    for line in text.lines() {
        let mut kept = String::new();
        for sentence in line.split_inclusive(['.', '!', '?']) {
            let lower = sentence.to_ascii_lowercase();
            let is_meta = META.iter().any(|m| lower.contains(m));
            let is_negative = lower.trim_start().starts_with("no ") || lower.contains("were made") || lower.contains("was made");
            if is_meta && is_negative {
                continue;
            }
            kept.push_str(sentence);
        }
        let kept = kept
            .replace("The provided transcript", "The discussion")
            .replace("the provided transcript", "the discussion")
            .replace("The transcript", "The discussion")
            .replace("the transcript", "the discussion");
        if !kept.trim().is_empty() || line.trim().is_empty() {
            out.push(kept.trim_end().to_string());
        }
    }
    out.join("\n")
}

/// Remove the model's habits that add noise: a redundant leading "Summary" heading
/// and sections that only say "none".
fn clean_summary(md: &str) -> String {
    let mut blocks: Vec<(Option<String>, Vec<String>)> = vec![(None, Vec::new())];
    for line in md.lines() {
        if let Some(h) = line.trim_start().strip_prefix("## ") {
            blocks.push((Some(h.trim().to_string()), Vec::new()));
        } else {
            blocks.last_mut().unwrap().1.push(line.to_string());
        }
    }
    let is_none = |l: &str| {
        let t = l.trim().trim_start_matches(['-', '*', '•']).trim().to_ascii_lowercase();
        t.is_empty()
            || t.starts_with("none")
            || t.starts_with("n/a")
            || t.starts_with("no specific")
            || t.starts_with("no action")
            || t.starts_with("no open")
            || t.starts_with("no decisions")
            || t.starts_with("nothing")
    };
    let mut out = String::new();
    let blocks: Vec<(Option<String>, Vec<String>)> = blocks
        .into_iter()
        .map(|(h, l)| (h, strip_meta(&l.join("\n")).lines().map(String::from).collect()))
        .collect();
    for (i, (heading, lines)) in blocks.iter().enumerate() {
        let body: Vec<&String> = lines.iter().filter(|l| !l.trim().is_empty()).collect();
        let empty = body.is_empty() || body.iter().all(|l| is_none(l));
        match heading {
            // A "Summary" heading above the lead paragraph just repeats the card title.
            Some(h) if h.eq_ignore_ascii_case("summary") => {
                if !empty {
                    out.push_str(&lines.join("\n").trim_matches('\n').to_string());
                    out.push_str("\n\n");
                }
            }
            Some(h) => {
                if !empty {
                    out.push_str(&format!("## {h}\n{}\n\n", lines.join("\n").trim_matches('\n')));
                }
            }
            None => {
                if i == 0 && !empty {
                    out.push_str(&lines.join("\n").trim_matches('\n').to_string());
                    out.push_str("\n\n");
                }
            }
        }
    }
    out.trim().to_string()
}

/// First line, no quotes/markup/trailing punctuation, at most ~60 characters.
fn clean_title(raw: &str) -> String {
    let line = raw.lines().map(str::trim).find(|l| !l.is_empty()).unwrap_or("");
    let line = line
        .trim_start_matches(['#', '*', '-', '>', ' '])
        .trim_start_matches("Title:")
        .trim()
        .trim_matches(['"', '\'', '“', '”', '*', '`'])
        .trim_end_matches(['.', ':', ';'])
        .trim();
    if line.chars().count() <= 60 {
        return line.to_string();
    }
    let mut cut: String = line.chars().take(60).collect();
    if let Some(i) = cut.rfind(' ') {
        cut.truncate(i);
    }
    cut
}

/// "1 min 3 s", "42 s", "1 h 05 min".
fn fmt_duration(secs: u64) -> String {
    if secs >= 3600 {
        format!("{} h {:02} min", secs / 3600, (secs / 60) % 60)
    } else if secs >= 60 {
        format!("{} min {} s", secs / 60, secs % 60)
    } else {
        format!("{secs} s")
    }
}
