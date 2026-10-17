//! Live correction. While a meeting is recorded, a compute node with a large speech model listens again in short
//! windows and the captions from the phone's small model are replaced a few seconds later with what it heard.
//!
//! Everything that decides something is a plain function here (when to cut a window, which answers to accept, how
//! lines are swapped), so it can be tested without a node. `Core::tick_loop` cuts windows, `Core::spawn_correction` sends
//! one on a thread of its own and `Core::apply_correction` puts the answer in place.
//!
//! Times are meeting seconds, the same ones the live segments and lines carry.

use super::*;
use crate::remote::{node_window_s, PcmFrames, Remote, TranscribeJob};
use std::collections::VecDeque;

/// The window lengths the person can choose, in seconds of new talk per window.
pub(super) const WINDOW_CHOICES: [u32; 4] = [20, 30, 45, 60];
pub(super) const DEFAULT_WINDOW_S: u32 = 30;
/// No window is cut shorter than this.
const MIN_WINDOW_S: f64 = 15.0;
/// A window starts this long before the corrected text ends, so a word cut in half at its start is heard whole.
const LEAD_IN: f64 = 5.0;
/// A window ends this long before now, so words still being said are not cut.
const END_LAG: f64 = 4.0;
/// The most a window covers when earlier ones were skipped or failed.
const MAX_BACKLOG_S: f64 = 180.0;
/// A segment that ends this close to the end of the window may be cut short, so it is left for the next window.
const END_MARGIN: f64 = 0.5;
/// The recent audio kept: a backlog window, its lead-in, and the lag before it.
pub(super) const RING_SECS: f64 = MAX_BACKLOG_S + LEAD_IN + END_LAG + 11.0;
/// A segment may start this much before the corrected text ends.
const ACCEPT_SLACK: f64 = 1.0;
/// How much of the earlier text goes along as the speech model's prompt.
pub(super) const PROMPT_CHARS: usize = 200;
/// After a window failed, the next one waits this long.
const RETRY_AFTER: Duration = Duration::from_secs(15);
/// When recording stops, a window that is running gets this long to finish.
pub(super) const FINISH_WAIT: Duration = Duration::from_secs(8);
const RATE: f64 = 16_000.0;

// ------------------------------------------------------------------------------------------------ the recent audio

/// The last stretch of the live audio (16 kHz mono) as 16 bit samples, with meeting times.
pub(super) struct PcmRing {
    buf: VecDeque<i16>,
    /// Meeting seconds of the very first sample ever pushed.
    origin: f64,
    /// Samples pushed so far, kept or not, and how many of them were dropped from the front.
    pushed: u64,
    dropped: u64,
    cap: usize,
}

impl PcmRing {
    pub fn new(origin: f64, secs: f64) -> PcmRing {
        PcmRing { buf: VecDeque::new(), origin, pushed: 0, dropped: 0, cap: (secs * RATE) as usize }
    }

    /// Takes new samples. With `keep` false the time still moves on, but nothing is held (and what was held is let go).
    pub fn push(&mut self, pcm: &[f32], keep: bool) {
        self.pushed += pcm.len() as u64;
        if !keep {
            self.buf.clear();
            self.buf.shrink_to_fit();
            self.dropped = self.pushed;
            return;
        }
        self.buf.extend(pcm.iter().map(|s| (s.clamp(-1.0, 1.0) * 32767.0).round() as i16));
        if self.buf.len() > self.cap {
            let extra = self.buf.len() - self.cap;
            self.buf.drain(..extra);
            self.dropped += extra as u64;
        }
    }

    /// Meeting time at the end of what has been pushed.
    pub fn end_secs(&self) -> f64 {
        self.origin + self.pushed as f64 / RATE
    }

    /// Meeting time of the oldest sample held (the end when nothing is held).
    pub fn start_secs(&self) -> f64 {
        self.origin + self.dropped as f64 / RATE
    }

    #[cfg(test)]
    pub fn held_secs(&self) -> f64 {
        self.buf.len() as f64 / RATE
    }

    /// The audio from `t0` to `t1` as 16 bit little endian bytes, and the time it really starts at (a start before
    /// the oldest sample held moves up to it). None when nothing is held there.
    pub fn slice_bytes(&self, t0: f64, t1: f64) -> Option<(f64, Vec<u8>)> {
        let first = ((t0 - self.origin) * RATE).round().max(self.dropped as f64) as u64;
        let last = (((t1 - self.origin) * RATE).round().max(0.0) as u64).min(self.pushed);
        if last <= first || self.buf.is_empty() {
            return None;
        }
        let from = (first - self.dropped) as usize;
        let to = (last - self.dropped) as usize;
        let mut out = Vec::with_capacity((to - from) * 2);
        for s in self.buf.range(from..to) {
            out.extend_from_slice(&s.to_le_bytes());
        }
        Some((self.origin + first as f64 / RATE, out))
    }
}

/// Audio bytes as pieces for a node, from `start_ms` on, in pieces of 10 seconds.
pub(super) fn frames_from(pcm: Arc<Vec<u8>>, start_ms: u64) -> PcmFrames {
    const PIECE: usize = 320_000;
    let from = ((start_ms as usize * 32).min(pcm.len())) & !1;
    let pieces = (pcm.len() - from).div_ceil(PIECE);
    Box::new((0..pieces).map(move |i| {
        let a = from + i * PIECE;
        pcm[a..(a + PIECE).min(pcm.len())].to_vec()
    }))
}

// ------------------------------------------------------------------------------------------------ when to cut

/// How long windows are for the node at hand.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct Plan {
    /// New talk per window, in seconds: a window is cut when this much has piled up.
    pub every: f64,
    /// The longest window ever cut, lead-in included.
    pub max_len: f64,
}

/// The person's choice (20, 30, 45 or 60 seconds, else 30) held to what the node treats as a quick job (`node_max_s`,
/// 0 for "not stated" meaning 120), and never below 15 seconds.
pub(super) fn plan_for(setting_s: u32, node_max_s: u32) -> Plan {
    let wanted = if WINDOW_CHOICES.contains(&setting_s) { setting_s } else { DEFAULT_WINDOW_S } as f64;
    let max_len = (node_window_s(node_max_s) as f64).clamp(MIN_WINDOW_S, MAX_BACKLOG_S);
    Plan { every: wanted.min(max_len - LEAD_IN).max(MIN_WINDOW_S), max_len }
}

/// One stretch of audio to send.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct Window {
    pub start: f64,
    pub end: f64,
    /// The window starts where this recording started, so nothing before it was cut.
    pub at_origin: bool,
}

impl Window {
    #[cfg(test)]
    pub fn secs(&self) -> f64 {
        self.end - self.start
    }

    /// Node segments must start here or later: just before the text already corrected, and not at the very first
    /// moment of a window that cut into the audio.
    pub fn accept_from(&self, upto: f64) -> f64 {
        let a = upto - ACCEPT_SLACK;
        if self.at_origin { a } else { a.max(self.start + 1.0) }
    }
}

/// Whether a window should be cut now, and which one. `audio_end` is the meeting time of the newest audio, `upto`
/// how far the text is corrected, `ring_start` the oldest audio held, `origin` where this recording started and
/// `busy` whether a window is still out. Only one is ever out: if the last one is still running, the next one simply
/// covers the gap, up to the longest window the node takes as a quick job.
pub(super) fn plan_window(audio_end: f64, upto: f64, ring_start: f64, origin: f64, busy: bool, plan: &Plan) -> Option<Window> {
    if busy {
        return None;
    }
    let end = audio_end - END_LAG;
    if end - upto < plan.every {
        return None;
    }
    let start = (upto - LEAD_IN).max(end - plan.max_len).max(ring_start).max(origin);
    if end - start < MIN_WINDOW_S.min(plan.every).min(10.0) {
        return None;
    }
    Some(Window { start, end, at_origin: start <= origin + 1e-6 })
}

// ------------------------------------------------------------------------------------------------ what comes back

/// The segments of an answer that may be used: they start at or after the corrected text ends (less a second) and end
/// before the window does, because the last one may be cut short.
pub(super) fn accept(heard: &[(f64, f64, String)], accept_from: f64, win_end: f64) -> Vec<(f64, f64, String)> {
    heard.iter().filter(|s| s.0 >= accept_from && s.1 <= win_end - END_MARGIN && !s.2.trim().is_empty()).cloned().collect()
}

/// What to do with an answer.
#[derive(Debug, PartialEq)]
pub(super) enum Decision {
    /// Change nothing. The window is tried again as part of the next one.
    Nothing,
    /// Change no text, but there was nothing to fix up to here: move on.
    Skip { upto: f64 },
    /// Swap `r0..r1` for the new segments.
    Swap { r0: f64, r1: f64, new: Vec<(f64, f64, String)> },
}

fn words(segs: impl Iterator<Item = String>) -> usize {
    segs.map(|t| t.split_whitespace().count()).sum()
}

/// `new` is the tidied accepted text, `live` the segments now stored (t0, t1, text) and `complete` says the node
/// reached the end of the window.
pub(super) fn decide(win: &Window, upto: f64, new: Vec<(f64, f64, String)>, live: &[(f64, f64, String)], complete: bool) -> Decision {
    let from = win.accept_from(upto);
    if new.is_empty() {
        // A node that listened to everything and heard nothing, where the phone heard nothing either: nothing to fix.
        let quiet = !live.iter().any(|s| s.1 > from && s.0 < win.end - END_MARGIN);
        return if complete && quiet { Decision::Skip { upto: win.end } } else { Decision::Nothing };
    }
    let r0 = from;
    let r1 = new.iter().map(|s| s.1).fold(f64::MIN, f64::max);
    let inside = |s: &&(f64, f64, String)| {
        let mid = (s.0 + s.1) / 2.0;
        mid >= r0 && mid < r1
    };
    let old_words = words(live.iter().filter(inside).map(|s| s.2.clone()));
    let new_words = words(new.iter().map(|s| s.2.clone()));
    if old_words > 0 && new_words * 10 < old_words * 4 {
        // Much shorter than what the phone heard: keep the phone's text, and do not ask about this stretch again.
        return Decision::Skip { upto: r1 };
    }
    Decision::Swap { r0, r1, new }
}

// ------------------------------------------------------------------------------------------------ lines

/// Which lines `[a, b)` a stretch `r0..r1` touches: those that overlap it. If none does, `a == b` is where text
/// in the stretch belongs.
pub(super) fn touched_lines(lines: &[Line], r0: f64, r1: f64) -> (usize, usize) {
    let a = lines.iter().position(|l| l.t1 > r0).unwrap_or(lines.len());
    let b = lines.iter().rposition(|l| l.t0 < r1).map(|i| i + 1).unwrap_or(0).max(a);
    (a, b)
}

/// The meeting seconds the replaced lines and the new text cover together.
pub(super) fn span_of(lines: &[Line], a: usize, b: usize, r0: f64, r1: f64) -> (f64, f64) {
    if a < b {
        (r0.min(lines[a].t0), r1.max(lines[b - 1].t1))
    } else {
        (r0, r1)
    }
}

/// Lines made from stored segments (t0, t1, text, speaker), joined the way live ones are. The first lines take the
/// ids in `reuse`, in order, so a line that changes in place keeps its place on the screen.
pub(super) fn rebuild_lines(rows: &[(f64, f64, String, Option<i64>)], who: &dyn Fn(Option<i64>) -> String, reuse: &[u64]) -> Vec<Line> {
    let mut out: Vec<Line> = Vec::new();
    for (t0, t1, text, spk) in rows {
        push_merged_as(&mut out, *t0, *t1, text, *spk, &who(*spk));
    }
    for (line, id) in out.iter_mut().zip(reuse) {
        line.id = *id;
    }
    out
}

/// Where an index into the lines (a count seen so far) goes when `removed` lines from `a` are replaced by `added`.
pub(super) fn shift_index(idx: usize, a: usize, removed: usize, added: usize) -> usize {
    if idx <= a {
        idx
    } else if idx >= a + removed {
        idx + added - removed
    } else {
        a + (idx - a).min(added)
    }
}

/// The last `max_chars` characters of the text that comes before `before`, starting at a whole word. None when there is none.
pub(super) fn prompt_tail(lines: &[Line], before: f64, max_chars: usize) -> Option<String> {
    let mut text = String::new();
    for l in lines.iter().filter(|l| l.t1 <= before).rev().take(4).collect::<Vec<_>>().into_iter().rev() {
        if !text.is_empty() {
            text.push(' ');
        }
        text.push_str(l.text.trim());
    }
    let chars: Vec<char> = text.chars().collect();
    if chars.len() > max_chars {
        let tail: String = chars[chars.len() - max_chars..].iter().collect();
        // Drop the word that was cut in half.
        let whole = if chars[chars.len() - max_chars - 1].is_whitespace() { tail.as_str() } else { tail.split_once(char::is_whitespace).map(|x| x.1).unwrap_or("") };
        text = whole.trim().to_string();
    }
    (!text.trim().is_empty()).then(|| text.trim().to_string())
}

// ------------------------------------------------------------------------------------------------ the state

/// Live correction state of the meeting being recorded.
pub(super) struct Correction {
    pub ring: PcmRing,
    /// A node is available and the setting is on, so audio is being kept.
    pub collecting: bool,
    /// The node named on the screen, when one is doing the work.
    pub shown: Option<String>,
    /// Text up to here is corrected (meeting seconds).
    pub upto: f64,
    pub in_flight: bool,
    pub cancel: Arc<AtomicBool>,
    pub retry_at: Option<Instant>,
}

impl Correction {
    /// For a recording that starts at `origin` meeting seconds (0 for a new meeting, where it carries on for a resumed one).
    pub fn new(origin: f64) -> Correction {
        Correction { ring: PcmRing::new(origin, RING_SECS), collecting: false, shown: None, upto: origin, in_flight: false, cancel: Arc::new(AtomicBool::new(false)), retry_at: None }
    }

    /// Notes which node (if any) can do the work now. True when what the screen shows changed.
    pub fn set_node(&mut self, node: Option<String>) -> bool {
        self.collecting = node.is_some();
        if !self.collecting {
            // Nothing will be sent, so hold no audio.
            self.ring.push(&[], false);
        }
        let changed = self.shown != node;
        self.shown = node;
        changed
    }
}

/// A window cut and ready to send.
pub(super) struct CorrectionJob {
    pub meeting_id: i64,
    pub win: Window,
    pub pcm: Vec<u8>,
    pub language: Option<String>,
    pub prompt: Option<String>,
    pub cancel: Arc<AtomicBool>,
}

/// What the node heard in a window, in meeting seconds, and whether it reached the end.
pub(super) struct Heard {
    pub segments: Vec<(f64, f64, String)>,
    pub complete: bool,
    pub node: String,
}

/// Sends one window to a compute node and waits for the answer. Blocks, so never call it on the thread that takes the audio.
pub(super) fn fetch_window(remote: &Arc<Remote>, job: &CorrectionJob) -> Result<Heard> {
    let pcm = Arc::new(job.pcm.clone());
    let total_ms = (pcm.len() / 32) as u64;
    let tj = TranscribeJob {
        total_ms,
        language: job.language.clone(),
        cancel: &job.cancel,
        priority: marginalia_proto::Priority::Interactive,
        prompt: job.prompt.clone(),
    };
    let src = pcm.clone();
    let out = remote.transcribe(&tj, move |start_ms| Ok(frames_from(src.clone(), start_ms)), |_, _, _| {}, |_, _, _| {});
    match out {
        None => Err(anyhow!("no compute node can listen right now")),
        Some(Err(e)) => Err(e),
        Some(Ok(o)) => Ok(Heard {
            segments: o.segments.iter().map(|(a, b, t)| (job.win.start + *a as f64 / 1000.0, job.win.start + *b as f64 / 1000.0, t.clone())).collect(),
            complete: o.complete,
            node: o.node,
        }),
    }
}

/// Swaps the stretch `r0..r1` for `new` in the stored transcript and in `lines`, which are rebuilt for the lines the
/// stretch touches from what is stored (so both always agree). Returns the lines removed, the lines put in their
/// place, and the new lines as JSON with their positions.
pub(super) fn swap_lines(store: &crate::rag::store::Store, mid: i64, lines: &mut Vec<Line>, r0: f64, r1: f64, new: &[(f64, f64, String)]) -> Result<(usize, usize, Vec<Value>)> {
    let (a, b) = touched_lines(lines, r0, r1);
    store.replace_segments_range(mid, r0, r1, new)?;
    let (lo, hi) = span_of(lines, a, b, r0, r1);
    let names: HashMap<i64, String> = store.speaker_names(mid).unwrap_or_default().into_iter().collect();
    let rows: Vec<(f64, f64, String, Option<i64>)> = store
        .segments(mid)?
        .into_iter()
        .filter(|s| s.t0 >= lo - 1e-6 && s.t1 <= hi + 1e-6)
        .map(|s| (s.t0, s.t1, s.text, s.speaker))
        .collect();
    let reuse: Vec<u64> = lines[a..b].iter().map(|x| x.id).collect();
    let fresh = rebuild_lines(&rows, &|s| who_is(&names, s), &reuse);
    let payload: Vec<Value> = fresh.iter().enumerate().map(|(k, x)| line_json(a + k, x)).collect();
    let (removed, added) = (b - a, fresh.len());
    lines.splice(a..b, fresh);
    Ok((removed, added, payload))
}

impl Core {
    /// Sends a window on a thread of its own and puts the answer in place. Never blocks transcription or the screen.
    pub(super) fn spawn_correction(self: &Arc<Self>, job: CorrectionJob) {
        let core = self.clone();
        let spawned = std::thread::Builder::new().name("correct".into()).spawn(move || {
            let heard = match crate::remote::get() {
                Some(r) => fetch_window(r, &job),
                None => Err(anyhow!("compute nodes are not set up")),
            };
            core.apply_correction(&job, heard);
        });
        if spawned.is_err() {
            if let Some(l) = self.live.lock().as_mut() {
                l.corr.in_flight = false;
            }
        }
    }

    /// Puts what a node heard in a window in place of the live text, when it is good enough. Quiet about every problem.
    /// The screen is told how far the text has been checked and how many lines were changed.
    pub(super) fn apply_correction(&self, job: &CorrectionJob, heard: Result<Heard>) {
        if let Some((upto, changed)) = self.apply_correction_inner(job, heard) {
            self.emit(json!({"type": "correction_state", "phase": "idle", "upto": upto, "changed": changed}));
        }
    }

    /// Returns how far the text is checked (meeting seconds) and how many lines were swapped, or None when the meeting is gone.
    fn apply_correction_inner(&self, job: &CorrectionJob, heard: Result<Heard>) -> Option<(f64, usize)> {
        let _gate = self.line_gate.lock();
        let mut g = self.live.lock();
        let l = g.as_mut().filter(|l| l.meeting_id == job.meeting_id)?;
        l.corr.in_flight = false;
        let heard = match heard {
            Ok(h) => h,
            Err(e) => {
                tracing::debug!("live correction: {e:#}");
                l.corr.retry_at = Some(Instant::now() + RETRY_AFTER);
                return Some((l.corr.upto, 0));
            }
        };
        let mid = l.meeting_id;
        let upto = l.corr.upto;
        let accepted = accept(&heard.segments, job.win.accept_from(upto), job.win.end);
        let new = tidy_segments(accepted, crate::engine::whisper::clean_transcript);
        let live_now: Vec<(f64, f64, String)> = self.kb.store.segments(mid).unwrap_or_default().into_iter().map(|s| (s.t0, s.t1, s.text)).collect();
        match decide(&job.win, upto, new, &live_now, heard.complete) {
            Decision::Nothing => {
                l.corr.retry_at = Some(Instant::now() + RETRY_AFTER);
                Some((l.corr.upto, 0))
            }
            Decision::Skip { upto: to } => {
                l.corr.upto = l.corr.upto.max(to);
                Some((l.corr.upto, 0))
            }
            Decision::Swap { r0, r1, new } => {
                let (a, _) = touched_lines(&l.lines, r0, r1);
                if a < l.sec_upto {
                    // Part of this is already condensed into notes, which are written from the old words. Leave it be and move on.
                    let past = l.sections.last().map(|s| s.t1 + 1.0).unwrap_or(r1);
                    l.corr.upto = l.corr.upto.max(past);
                    return Some((l.corr.upto, 0));
                }
                let (removed, added, payload) = match swap_lines(&self.kb.store, mid, &mut l.lines, r0, r1, &new) {
                    Ok(x) => x,
                    Err(e) => {
                        tracing::warn!("live correction could not be saved: {e:#}");
                        l.corr.retry_at = Some(Instant::now() + RETRY_AFTER);
                        return Some((l.corr.upto, 0));
                    }
                };
                l.enhance_lines_seen = shift_index(l.enhance_lines_seen, a, removed, added);
                l.scanned_upto = shift_index(l.scanned_upto, a, removed, added);
                l.corr.upto = l.corr.upto.max(r1);
                l.corr.retry_at = None;
                let upto_now = l.corr.upto;
                tracing::info!("live correction on {}: {:.0}-{:.0}s, {removed} lines became {added}", heard.node, r0, r1);
                drop(g);
                self.emit(json!({"type": "lines_replace", "from": a, "removed": removed, "lines": payload}));
                Some((upto_now, added))
            }
        }
    }

    /// When recording stops: a window that is running gets a few seconds to finish, then is cancelled.
    pub(super) fn finish_correction(&self) {
        let t = Instant::now();
        loop {
            let (running, cancel) = match self.live.lock().as_ref() {
                Some(l) => (l.corr.in_flight, l.corr.cancel.clone()),
                None => return,
            };
            if !running {
                return;
            }
            if t.elapsed() >= FINISH_WAIT {
                cancel.store(true, Ordering::Relaxed);
                return;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn seg(t0: f64, t1: f64, text: &str) -> (f64, f64, String) {
        (t0, t1, text.to_string())
    }

    fn line(t0: f64, t1: f64, text: &str) -> Line {
        Line { id: next_line_id(), t0, t1, text: text.into(), speaker: None, who: String::new() }
    }

    fn plan30() -> Plan {
        plan_for(30, 0)
    }

    #[test]
    fn the_window_follows_the_choice_and_what_the_node_takes() {
        assert_eq!(plan_for(30, 0), Plan { every: 30.0, max_len: 120.0 }, "a node that does not say is taken for 120 seconds");
        assert_eq!(plan_for(20, 120).every, 20.0);
        assert_eq!(plan_for(45, 120).every, 45.0);
        assert_eq!(plan_for(60, 120).every, 60.0);
        assert_eq!(plan_for(7, 120).every, 30.0, "an odd choice falls back to 30");
        // A node that treats only up to 40 seconds as quick: a window is the new talk plus 5 seconds of lead-in.
        let p = plan_for(60, 40);
        assert_eq!((p.every, p.max_len), (35.0, 40.0));
        // Never below 15 seconds, whatever the node says.
        let p = plan_for(20, 10);
        assert_eq!((p.every, p.max_len), (15.0, 15.0));
        assert_eq!(plan_for(30, 1000).max_len, 180.0, "and never more than the backlog allows");
    }

    #[test]
    fn a_window_is_cut_every_so_often_and_only_one_at_a_time() {
        let p = plan30();
        // 20 seconds in: the first window ends 4 seconds before now and 30 seconds of talk have not piled up yet.
        assert_eq!(plan_window(20.0, 0.0, 0.0, 0.0, false, &p), None);
        assert_eq!(plan_window(33.9, 0.0, 0.0, 0.0, false, &p), None);
        let w = plan_window(34.0, 0.0, 0.0, 0.0, false, &p).unwrap();
        assert_eq!((w.start, w.end, w.at_origin), (0.0, 30.0, true));
        // Still out: nothing is cut however long it takes.
        assert_eq!(plan_window(100.0, 0.0, 0.0, 0.0, true, &p), None);
        // The next one starts 5 seconds before the corrected text ends.
        let w = plan_window(64.0, 29.0, 0.0, 0.0, false, &p).unwrap();
        assert_eq!((w.start, w.end, w.at_origin), (24.0, 60.0, false));
        assert!((w.secs() - 36.0).abs() < 1e-9);
    }

    #[test]
    fn a_skipped_window_is_covered_by_the_next_up_to_the_limit() {
        let p = plan30();
        // The window due at 34 s never went out; at 94 s the next one reaches back to 25 s.
        let w = plan_window(94.0, 30.0, 0.0, 0.0, false, &p).unwrap();
        assert_eq!((w.start, w.end), (25.0, 90.0));
        // A node that takes 120 seconds as a quick job: a backlog of 5 minutes is cut to 120.
        let w = plan_window(304.0, 30.0, 0.0, 0.0, false, &p).unwrap();
        assert_eq!((w.start, w.end), (180.0, 300.0));
        assert!(w.secs() <= p.max_len);
        // A node with a short limit gets shorter windows.
        let short = plan_for(60, 45);
        let w = plan_window(304.0, 30.0, 0.0, 0.0, false, &short).unwrap();
        assert_eq!(w.secs(), 45.0);
        assert_eq!(plan_window(300.0 + 4.0, 270.0, 0.0, 0.0, false, &short), None, "30 seconds is less than a 40 second step");
    }

    #[test]
    fn windows_of_different_lengths_come_at_their_own_pace() {
        for (choice, every) in [(20u32, 20.0), (30, 30.0), (45, 45.0), (60, 60.0)] {
            let p = plan_for(choice, 120);
            assert!(plan_window(every + 3.9, 0.0, 0.0, 0.0, false, &p).is_none(), "{choice}: too early");
            let w = plan_window(every + 4.0, 0.0, 0.0, 0.0, false, &p).unwrap();
            assert_eq!(w.secs(), every);
            let next = plan_window(2.0 * every + 4.0, every, 0.0, 0.0, false, &p).unwrap();
            assert_eq!(next.start, every - 5.0);
            assert_eq!(next.secs(), every + 5.0);
        }
    }

    #[test]
    fn a_window_never_reaches_before_the_audio_that_is_held() {
        let p = plan30();
        // The node came back late: only the last 40 seconds were kept.
        let w = plan_window(200.0, 20.0, 160.0, 0.0, false, &p).unwrap();
        assert_eq!((w.start, w.end, w.at_origin), (160.0, 196.0, false));
        // A resumed meeting does not reach into the earlier recording.
        let w = plan_window(1000.0 + 34.0, 1000.0, 1000.0, 1000.0, false, &p).unwrap();
        assert_eq!(w.start, 1000.0);
        assert!(w.at_origin);
        // Hardly any audio held: wait.
        assert_eq!(plan_window(200.0, 20.0, 192.0, 0.0, false, &p), None);
    }

    #[test]
    fn only_what_is_safely_inside_the_window_is_accepted() {
        let win = Window { start: 25.0, end: 60.0, at_origin: false };
        let upto = 30.0;
        let from = win.accept_from(upto);
        assert_eq!(from, 29.0);
        let heard = vec![
            seg(25.0, 28.0, "heard again from the lead-in"),
            seg(28.5, 33.0, "starts too early"),
            seg(29.0, 33.0, "starts exactly at the line"),
            seg(34.0, 45.0, "in the middle"),
            seg(46.0, 59.6, "ends too close to the end"),
            seg(50.0, 55.0, "   "),
            seg(55.0, 59.5, "ends half a second before"),
        ];
        let got = accept(&heard, from, win.end);
        assert_eq!(got.iter().map(|s| s.2.as_str()).collect::<Vec<_>>(), vec!["starts exactly at the line", "in the middle", "ends half a second before"]);
        // A window that begins after the corrected text (a gap) does not take its first second, which may be cut.
        let gap = Window { start: 100.0, end: 130.0, at_origin: false };
        assert_eq!(gap.accept_from(30.0), 101.0);
        // At the very start of the recording nothing was cut.
        let first = Window { start: 0.0, end: 30.0, at_origin: true };
        assert_eq!(first.accept_from(0.0), -1.0);
    }

    #[test]
    fn a_good_answer_swaps_a_failed_or_empty_one_does_not() {
        let win = Window { start: 0.0, end: 30.0, at_origin: true };
        let live = vec![seg(0.0, 10.0, "one two three four five six seven eight nine ten"), seg(10.0, 20.0, "eleven twelve thirteen fourteen fifteen sixteen seventeen")];
        let new = vec![seg(0.5, 9.5, "One two three four five six seven eight nine ten."), seg(10.5, 19.5, "Eleven twelve thirteen fourteen fifteen sixteen seventeen.")];
        match decide(&win, 0.0, new.clone(), &live, true) {
            Decision::Swap { r0, r1, new: n } => {
                assert_eq!((r0, r1), (-1.0, 19.5));
                assert_eq!(n, new);
            }
            other => panic!("{other:?}"),
        }
        // Nothing heard where the phone heard words: keep what is there and ask again with the next window.
        assert_eq!(decide(&win, 0.0, vec![], &live, true), Decision::Nothing);
        assert_eq!(decide(&win, 0.0, vec![], &live, false), Decision::Nothing);
        // Nothing heard by anyone: just move on.
        assert_eq!(decide(&win, 0.0, vec![], &[], true), Decision::Skip { upto: 30.0 });
        assert_eq!(decide(&win, 0.0, vec![], &[], false), Decision::Nothing, "an answer that stopped early says nothing about the rest");
        // Far fewer words than the phone heard: the phone's text stays and the stretch is not asked about again.
        let thin = vec![seg(0.5, 9.5, "One two"), seg(10.5, 19.5, "Eleven")];
        assert_eq!(decide(&win, 0.0, thin, &live, true), Decision::Skip { upto: 19.5 });
    }

    #[test]
    fn the_lines_a_stretch_touches() {
        let lines = vec![line(0.0, 10.0, "a"), line(12.0, 20.0, "b"), line(22.0, 30.0, "c"), line(40.0, 50.0, "d")];
        assert_eq!(touched_lines(&lines, 15.0, 25.0), (1, 3), "a line that is only partly inside is still replaced whole");
        assert_eq!(touched_lines(&lines, 10.0, 12.0), (1, 1), "in the gap between two lines");
        assert_eq!(touched_lines(&lines, 31.0, 39.0), (3, 3), "a gap: text goes between c and d");
        assert_eq!(touched_lines(&lines, 55.0, 60.0), (4, 4), "after the last line");
        assert_eq!(touched_lines(&lines, 0.0, 100.0), (0, 4));
        assert_eq!(touched_lines(&[], 0.0, 10.0), (0, 0));
        let (lo, hi) = span_of(&lines, 1, 3, 15.0, 25.0);
        assert_eq!((lo, hi), (12.0, 30.0));
        assert_eq!(span_of(&lines, 3, 3, 31.0, 39.0), (31.0, 39.0));
    }

    #[test]
    fn replaced_lines_keep_their_places_and_the_others_keep_theirs() {
        let mut lines = vec![line(0.0, 10.0, "keep first"), line(12.0, 20.0, "old b"), line(22.0, 30.0, "old c"), line(40.0, 50.0, "keep last")];
        let ids: Vec<u64> = lines.iter().map(|l| l.id).collect();
        let (a, b) = touched_lines(&lines, 12.0, 30.0);
        assert_eq!((a, b), (1, 3));
        let rows = vec![
            (12.0, 15.0, "New b one.".to_string(), None),
            (15.5, 20.0, "and more.".to_string(), None),
            (21.0, 30.0, "New c.".to_string(), None),
            (60.0, 61.0, "far away".to_string(), None),
        ];
        let reuse: Vec<u64> = lines[a..b].iter().map(|l| l.id).collect();
        let fresh = rebuild_lines(&rows[..3], &|_| String::new(), &reuse);
        assert_eq!(fresh.len(), 1, "close pieces join like live ones do");
        assert_eq!(fresh[0].text, "New b one. and more. New c.");
        assert_eq!(fresh[0].id, ids[1], "the first rebuilt line keeps the id of the first replaced line");
        let added = fresh.len();
        lines.splice(a..b, fresh);
        assert_eq!(lines.iter().map(|l| l.text.as_str()).collect::<Vec<_>>(), vec!["keep first", "New b one. and more. New c.", "keep last"]);
        assert_eq!(lines[0].id, ids[0]);
        assert_eq!(lines[2].id, ids[3], "lines after the stretch keep their ids while their positions shift");
        let all: HashSet<u64> = lines.iter().map(|l| l.id).collect();
        assert_eq!(all.len(), lines.len(), "ids stay unique");
        // More lines than were replaced: the extra ones get new ids.
        let more = rebuild_lines(&[(0.0, 2.0, "x".into(), None), (30.0, 32.0, "y".into(), None), (60.0, 62.0, "z".into(), None)], &|_| String::new(), &[ids[1]]);
        assert_eq!(more.len(), 3);
        assert_eq!(more[0].id, ids[1]);
        assert!(more[1].id != ids[1] && more[2].id != more[1].id);
        assert_eq!(added, 1);
    }

    #[test]
    fn counts_of_lines_seen_follow_the_swap() {
        // Lines 3 and 4 (removed = 2) became 3 lines.
        assert_eq!(shift_index(2, 3, 2, 3), 2, "before the swap: unchanged");
        assert_eq!(shift_index(3, 3, 2, 3), 3);
        assert_eq!(shift_index(5, 3, 2, 3), 6, "after it: moved by the difference");
        assert_eq!(shift_index(9, 3, 2, 1), 8);
        assert_eq!(shift_index(4, 3, 2, 3), 4, "inside it: kept within the new lines");
        assert_eq!(shift_index(4, 3, 2, 0), 3);
        assert_eq!(shift_index(0, 0, 0, 0), 0);
    }

    #[test]
    fn the_prompt_is_the_tail_of_what_came_before() {
        let lines = vec![line(0.0, 10.0, "Welcome to the Orchid launch review."), line(12.0, 20.0, "Priya owns the database migration."), line(25.0, 40.0, "later text")];
        assert_eq!(prompt_tail(&lines, 21.0, 200).as_deref(), Some("Welcome to the Orchid launch review. Priya owns the database migration."));
        let cut = prompt_tail(&lines, 21.0, 40).unwrap();
        assert!(cut.chars().count() <= 40 && "Priya owns the database migration.".ends_with(&cut) || cut.ends_with("migration."), "{cut}");
        assert!(!cut.starts_with(' ') && lines[1].text.contains(cut.split(' ').next().unwrap()), "starts at a whole word: {cut}");
        assert_eq!(prompt_tail(&lines, 5.0, 200), None, "nothing came before");
        assert_eq!(prompt_tail(&[], 50.0, 200), None);
    }

    #[test]
    fn the_ring_holds_the_recent_audio_with_meeting_times() {
        let mut r = PcmRing::new(100.0, 10.0);
        r.push(&vec![0.5; 16_000 * 4], true);
        assert_eq!((r.start_secs(), r.end_secs()), (100.0, 104.0));
        let (t, bytes) = r.slice_bytes(101.0, 103.0).unwrap();
        assert_eq!(t, 101.0);
        assert_eq!(bytes.len(), 2 * 32_000);
        assert_eq!(i16::from_le_bytes([bytes[0], bytes[1]]), 16384);
        // More than it holds: the oldest goes.
        r.push(&vec![-1.0; 16_000 * 8], true);
        assert_eq!(r.end_secs(), 112.0);
        assert_eq!(r.start_secs(), 102.0);
        assert!((r.held_secs() - 10.0).abs() < 1e-9);
        let (t, bytes) = r.slice_bytes(100.0, 105.0).unwrap();
        assert_eq!(t, 102.0, "a start before what is held moves up to it");
        assert_eq!(bytes.len(), 3 * 32_000);
        assert_eq!(i16::from_le_bytes([bytes[0], bytes[1]]), 16384, "102 to 104 s are still the first push");
        assert_eq!(i16::from_le_bytes([bytes[bytes.len() - 2], bytes[bytes.len() - 1]]), -32767, "104 to 105 s are the second");
        assert!(r.slice_bytes(112.0, 120.0).is_none(), "nothing after the end");
        // Not collecting: time moves on, nothing is held.
        r.push(&vec![0.1; 16_000 * 2], false);
        assert_eq!(r.end_secs(), 114.0);
        assert_eq!(r.start_secs(), 114.0);
        assert!(r.slice_bytes(100.0, 114.0).is_none());
        r.push(&vec![0.1; 16_000], true);
        assert_eq!((r.start_secs(), r.end_secs()), (114.0, 115.0));
    }

    #[test]
    fn audio_goes_to_the_node_in_pieces_from_where_it_is_asked() {
        let pcm = Arc::new(vec![7u8; 320_000 * 2 + 64]);
        let all: Vec<Vec<u8>> = frames_from(pcm.clone(), 0).collect();
        assert_eq!(all.iter().map(|p| p.len()).collect::<Vec<_>>(), vec![320_000, 320_000, 64]);
        let rest: usize = frames_from(pcm.clone(), 10_000).map(|p| p.len()).sum();
        assert_eq!(rest, pcm.len() - 320_000);
        assert_eq!(frames_from(pcm.clone(), 10_000_000).count(), 0);
    }

    #[test]
    fn the_state_says_when_a_node_comes_and_goes() {
        let mut c = Correction::new(0.0);
        assert!(c.set_node(Some("Desk".into())) && c.collecting);
        assert!(!c.set_node(Some("Desk".into())), "no change, nothing to announce");
        c.ring.push(&vec![0.1; 16_000], true);
        assert!(c.set_node(None) && !c.collecting);
        assert_eq!(c.ring.held_secs(), 0.0, "audio is let go when no node can use it");
        assert_eq!(c.ring.end_secs(), 1.0, "but time moved on");
        assert_eq!(Correction::new(500.0).upto, 500.0, "a resumed meeting starts where it left off");
    }

    // The whole trip of one window through a node that speaks the protocol (the fake of the remote tests).
    #[test]
    fn one_window_goes_to_a_node_and_comes_back_in_meeting_time() {
        use crate::remote::tests::{remote_with, start_fake, Mode};
        let rt = tokio::runtime::Runtime::new().unwrap();
        let a = start_fake(&rt, 8.0, Mode::Good);
        let r = remote_with(&[("a", &a)], "window");
        r.refresh();
        assert_eq!(r.transcribe_node_window(), Some(("a".to_string(), 90)));
        // 12 seconds of audio that start 100 seconds into the meeting.
        let win = Window { start: 100.0, end: 112.0, at_origin: false };
        let job = CorrectionJob {
            meeting_id: 1,
            win,
            pcm: vec![0u8; 12 * 32_000],
            language: Some("en".into()),
            prompt: Some("Priya owns the migration.".into()),
            cancel: Arc::new(AtomicBool::new(false)),
        };
        let heard = fetch_window(&r, &job).unwrap();
        assert!(heard.complete);
        assert_eq!(heard.node, "a");
        // The fake answers one segment per second of audio, timed from the start of the window.
        assert_eq!(heard.segments.len(), 12);
        assert_eq!(heard.segments[0], (100.0, 101.0, "w0".to_string()));
        assert_eq!(heard.segments[11], (111.0, 112.0, "w11".to_string()));
        assert_eq!(a.jobs.lock().unwrap().as_slice(), &[(12_000, 12 * 32_000)], "total_ms is the real length of the window");
        assert_eq!(a.metas.lock().unwrap().as_slice(), &[(marginalia_proto::Priority::Interactive, Some("Priya owns the migration.".to_string()))]);
        // Accepting: the corrected text ends at 102, so from 101 on, and not the last piece before the end.
        let got = accept(&heard.segments, win.accept_from(102.0), win.end);
        assert_eq!(got.first().map(|s| s.2.as_str()), Some("w1"));
        assert_eq!(got.last().map(|s| s.2.as_str()), Some("w10"), "w11 ends at the window end and may be cut short");
        let tidy = tidy_segments(got, |t| t.to_string());
        assert!(!tidy.is_empty() && tidy.iter().all(|s| s.1 <= 111.5));
        // A cancelled window comes back empty, which changes nothing.
        job.cancel.store(true, Ordering::Relaxed);
        let heard = fetch_window(&r, &job).unwrap();
        assert!(!heard.complete && heard.segments.is_empty());
    }

    #[test]
    fn no_node_means_no_window() {
        use crate::remote::tests::remote_with;
        let r = remote_with(&[], "nowindow");
        let job = CorrectionJob { meeting_id: 1, win: Window { start: 0.0, end: 5.0, at_origin: true }, pcm: vec![0u8; 5 * 32_000], language: None, prompt: None, cancel: Arc::new(AtomicBool::new(false)) };
        assert!(fetch_window(&r, &job).is_err());
        assert_eq!(r.transcribe_node_window(), None);
    }

    #[test]
    fn a_swap_keeps_the_stored_transcript_and_the_lines_in_step() {
        let dir = std::env::temp_dir().join(format!("marginalia-swap-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let store = crate::rag::store::Store::open(&dir.join("t.sqlite3")).unwrap();
        let mid = store.create_meeting("Meeting").unwrap();
        // What the phone heard: three stretches of talk, the middle one misheard.
        let live = [(0.0, 8.0, "Welcome to the meeting."), (20.0, 28.0, "The gamers are coming."), (30.0, 36.0, "To the basement."), (60.0, 66.0, "Later talk.")];
        let mut lines: Vec<Line> = Vec::new();
        for (a, b, t) in live {
            store.add_segment(mid, "mic", a, b, t).unwrap();
            push_merged_as(&mut lines, a, b, t, None, "");
        }
        assert_eq!(lines.len(), 3, "20-36 s is one line");
        let ids: Vec<u64> = lines.iter().map(|l| l.id).collect();
        // The node heard it better, and as three pieces.
        let new = vec![(20.2, 25.0, "The Gamers are".to_string()), (25.1, 28.0, "coming to".to_string()), (30.0, 36.0, "the basement.".to_string())];
        let (removed, added, payload) = swap_lines(&store, mid, &mut lines, 19.0, 36.0, &new).unwrap();
        assert_eq!((removed, added), (1, 1));
        assert_eq!(lines.len(), 3);
        assert_eq!(lines[1].text, "The Gamers are coming to the basement.");
        assert_eq!(lines.iter().map(|l| l.id).collect::<Vec<_>>(), ids, "same lines, same ids");
        assert_eq!(payload.len(), 1);
        assert_eq!(payload[0]["i"], 1);
        assert_eq!(payload[0]["id"], ids[1]);
        assert_eq!(payload[0]["text"], "The Gamers are coming to the basement.");
        let stored: Vec<String> = store.segments(mid).unwrap().into_iter().map(|s| s.text).collect();
        assert_eq!(stored, vec!["Welcome to the meeting.", "The Gamers are", "coming to", "the basement.", "Later talk."]);
        // The lines are what rebuilding the whole transcript from the store gives.
        let mut again: Vec<Line> = Vec::new();
        for s in store.segments(mid).unwrap() {
            push_merged_as(&mut again, s.t0, s.t1, &s.text, s.speaker, "");
        }
        assert_eq!(again.iter().map(|l| (l.t0, l.t1, l.text.clone())).collect::<Vec<_>>(), lines.iter().map(|l| (l.t0, l.t1, l.text.clone())).collect::<Vec<_>>());
        // A window that hears talk the phone missed, between two lines, adds a line there.
        let (removed, added, payload) = swap_lines(&store, mid, &mut lines, 40.0, 50.0, &[(41.0, 49.0, "Nobody caught this.".to_string())]).unwrap();
        assert_eq!((removed, added), (0, 1));
        assert_eq!(payload[0]["i"], 2);
        assert_eq!(lines.iter().map(|l| l.text.as_str()).collect::<Vec<_>>().len(), 4);
        assert_eq!(lines[2].text, "Nobody caught this.");
        assert_eq!(lines[3].text, "Later talk.");
        assert_eq!(lines[3].id, ids[2], "lines after the stretch keep their ids");
        let _ = std::fs::remove_dir_all(dir);
    }
}
