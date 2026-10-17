//! Plain helpers for transcript lines: merging, speaker labels, windows that fit a prompt.

use super::*;

/// Saved section notes that still match the transcript: unbroken from the start, and the lines agree.
pub(super) fn valid_sections(saved: Vec<(usize, usize, f64, f64, String)>, lines: &[Line]) -> (Vec<Section>, usize) {
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

/// Passages overlap on purpose so search never cuts a thought in half. To read them in a row, drop the words
/// a passage repeats from the end of the one before it (only runs of five words or more count).
pub(super) fn strip_overlap(prev: &str, cur: &str) -> String {
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
pub(super) fn without_block(text: &str, block: &str) -> String {
    if block.trim().is_empty() {
        return text.to_string();
    }
    text.replace(block.trim(), "").trim().to_string()
}

static NEXT_LINE_ID: AtomicU64 = AtomicU64::new(1);

/// A fresh number for a line. Ids only go up, so one is never used twice in a run of the app.
pub(super) fn next_line_id() -> u64 {
    NEXT_LINE_ID.fetch_add(1, Ordering::Relaxed)
}

pub(super) fn line_json(i: usize, l: &Line) -> Value {
    json!({ "i": i, "id": l.id, "t0": l.t0, "t1": l.t1, "text": l.text, "speaker": l.speaker })
}

/// "Priya: " (or "Speaker 2: ") in front of a line in a prompt, when speakers have been told apart.
pub(super) fn spk(who: &str) -> String {
    if who.is_empty() { String::new() } else { format!("{who}: ") }
}

/// What a speaker is called: the name the person gave, or "Speaker N".
pub(super) fn who_is(names: &HashMap<i64, String>, speaker: Option<i64>) -> String {
    match speaker {
        Some(s) => names.get(&s).cloned().unwrap_or_else(|| format!("Speaker {}", s + 1)),
        None => String::new(),
    }
}

/// Give each transcript segment the speaker who talks for most of it. `segments` is (id, t0, t1) and
/// `diar` is (start, end, speaker) from the diarizer. A segment nobody overlaps takes the nearest
/// speaker within 1.5 s, else the one before it.
pub(super) fn assign_speakers(segments: &[(i64, f64, f64)], diar: &[(f64, f64, i64)]) -> Vec<(i64, i64)> {
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
pub(super) fn push_merged(lines: &mut Vec<Line>, t0: f64, t1: f64, text: &str) -> usize {
    push_merged_as(lines, t0, t1, text, None, "")
}

/// Like [`push_merged`], for a segment whose speaker is known: lines never run two speakers together.
pub(super) fn push_merged_as(lines: &mut Vec<Line>, t0: f64, t1: f64, text: &str, speaker: Option<i64>, who: &str) -> usize {
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
    lines.push(Line { id: next_line_id(), t0, t1, text: text.trim().to_string(), speaker, who: who.to_string() });
    lines.len() - 1
}

pub(super) fn transcript_text(lines: &[Line], from: usize, max_chars: usize) -> String {
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
pub(super) fn lines_tokens(lines: &[Line]) -> usize {
    lines.iter().map(|l| est_tokens(&l.text) + 6).sum()
}

/// Largest run of lines starting at `from` that fits in `budget` tokens (at least one line).
pub(super) fn window_end(lines: &[Line], from: usize, budget: usize) -> usize {
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
pub(super) fn window_text(lines: &[Line], from: usize, to: usize, budget: usize) -> String {
    let mut out = String::new();
    for l in &lines[from.min(lines.len())..to.min(lines.len())] {
        out.push_str(&format!("[{}] {}{}\n", fmt_ts(l.t0), spk(&l.who), l.text));
    }
    head_tokens(&out, budget)
}

/// Context budget for prompts, in tokens, leaving room for the reply and the chat template.
pub(super) fn prompt_budget(ctx: u32, reply: usize) -> usize {
    (ctx as usize).saturating_sub(reply + 350).max(1200)
}

pub(super) fn is_default_title(t: &str) -> bool {
    let t = t.trim();
    t.is_empty() || t.starts_with("Untitled") || t.starts_with("Meeting ·")
}

