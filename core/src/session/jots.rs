//! Jots, the short notes a person types in a meeting, and the way they are reworded by the chat model.

use super::*;

pub(super) fn jot_json(id: i64, t: Option<f64>, kind: &str, raw: &str, polished: &str, ready: bool) -> Value {
    json!({"id": id, "t": t, "kind": kind, "raw": raw, "polished": polished, "state": if ready { "ready" } else { "pending" }})
}

pub(super) fn live_jot_json(j: &LiveJot) -> Value {
    let mut v = jot_json(j.id, Some(j.t), &j.kind, &j.raw, &j.polished, j.ready);
    v["updated"] = json!(j.updated);
    v
}

pub(super) fn stored_jot_json(j: &crate::rag::store::Jot) -> Value {
    // A stored jot with no polish is either a plain note or one that never finished: show it as it is.
    jot_json(j.id, j.t, &j.kind, &j.raw, &j.polished, true)
}

/// The jots as the old two-part notes: what was typed, and one tidy Markdown document.
pub(super) fn compile_jots(jots: &[(String, String, String)]) -> (String, String) {
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

/// The words spoken around `t` (a little before, a little after) plus the notes on earlier parts.
pub(super) fn jot_context(lines: &[Line], sections: &[Section], t: f64, raw: &str) -> String {
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
pub(super) fn lines_about(lines: &[Line], query: &str, skip_from: f64, skip_to: f64, take: usize) -> String {
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

pub(super) const STOP_WORDS: [&str; 24] = [
    "that", "this", "with", "from", "have", "what", "when", "where", "which", "about", "there", "their", "would", "could",
    "should", "into", "than", "then", "them", "they", "will", "your", "were", "been",
];

/// A rewrite is only kept if it is still about the jot: it must keep at least one of the jot's own words.
/// Otherwise the jot is used as the person wrote it, tidied.
pub(super) fn faithful_jot(raw: &str, rewritten: &str) -> String {
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
pub(super) fn tidy_jot(raw: &str) -> String {
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
pub(super) fn one_line(raw: &str) -> String {
    let t = raw.trim().trim_start_matches(['-', '*', '•', ' ']).trim();
    let t = t.strip_prefix("Note:").unwrap_or(t).trim().trim_matches(['"', '“', '”']);
    t.lines().map(str::trim).filter(|l| !l.is_empty()).collect::<Vec<_>>().join(" ")
}

impl Core {
    /// A jot: one quick note, saved at once, polished in the background.
    pub(super) fn meeting_jot(self: &Arc<Self>, args: &Value) -> Result<Value> {
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

    pub(super) fn jot_edit(self: &Arc<Self>, id: i64, text: &str) -> Result<Value> {
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
    pub(super) fn stored_context(&self, meeting_id: i64, t: Option<f64>) -> String {
        let Some(t) = t else { return String::new() };
        let segs = self.kb.store.segments(meeting_id).unwrap_or_default();
        let names: HashMap<i64, String> = self.kb.store.speaker_names(meeting_id).unwrap_or_default().into_iter().collect();
        let mut text = String::new();
        for s in segs.iter().filter(|s| s.t1 >= t - 90.0 && s.t0 <= t + 30.0) {
            text.push_str(&format!("[{}] {}{}\n", fmt_ts(s.t0), spk(&who_is(&names, s.speaker)), s.text));
        }
        format!("Transcript around that moment:\n{}", tail_tokens(&text, 650))
    }

    pub(super) fn polish_prompt(kind: &str, raw: &str, ctx: &str) -> Vec<ChatMessage> {
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
    pub(super) fn spawn_polish(self: &Arc<Self>, id: Option<i64>, kind: String, raw: String, ctx: String) {
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
    pub(super) fn spawn_refine(self: &Arc<Self>, id: i64, raw: String, current: String, ctx: String) {
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
    pub(super) fn spawn_polish_stored(self: &Arc<Self>, meeting_id: i64, id: i64, kind: String, raw: String, ctx: String) {
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
}
