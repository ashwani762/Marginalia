//! The Ask page: suggested questions and answers that cite the library.

use super::*;

pub(super) fn scope_of(s: &str) -> Scope {
    match s {
        "documents" => Scope::Documents,
        "meetings" => Scope::Meetings,
        _ => Scope::All,
    }
}

impl Core {
    pub(super) fn suggestions(&self, folder: Option<i64>) -> Result<Value> {
        // A title cut to fit, at a whole word and without a trailing ellipsis: the suggestion is also the question that gets
        // searched, and a literal "…" in it matches nothing.
        let short = |t: &str| {
            let t = t.trim();
            if t.chars().count() <= 48 {
                return t.to_string();
            }
            let cut: String = t.chars().take(48).collect();
            cut.rsplit_once(' ').map(|x| x.0.to_string()).unwrap_or(cut)
        };
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

    pub(super) fn chat_ask(self: &Arc<Self>, args: &Value) -> Result<Value> {
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

    pub(super) fn answer(&self, turn: u64, question: String, scope: Scope, folder: Option<i64>, history: Vec<(String, String)>, cancel: Arc<AtomicBool>) {
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
                    format!("[{}] {} - {} ({})\n{}", i + 1, h.title, h.locator(), kind, snippet_of(&h.context, per))
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
                let ms = if stats.remote_ms > 0 { stats.remote_ms as u128 } else { stats.elapsed_ms };
                let tps = if ms > 0 { stats.generated_tokens as f64 / (ms as f64 / 1000.0) } else { 0.0 };
                let place = match &stats.served_by { Some(n) => format!("on {n}"), None => "on device".to_string() };
                let _ = started;
                self.emit(json!({
                    "type": "chat", "turn": turn, "state": "done", "text": text,
                    "stats": format!("{} tokens · {:.1} tok/s · {}", stats.generated_tokens, tps, place),
                }));
            }
            Err(e) => self.emit(json!({"type": "chat", "turn": turn, "state": "error", "error": e.to_string()})),
        }
    }
}
