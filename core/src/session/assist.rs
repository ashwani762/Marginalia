//! Live assistance during a meeting: question cards that look things up in the library, typed questions and ideas to ask the speaker.

use super::*;

impl Core {
    pub(super) fn run_scan(self: &Arc<Self>) {
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

    pub(super) fn create_card(self: &Arc<Self>, question: String) {
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

    pub(super) fn on_card_hits(self: &Arc<Self>, id: u64, hits: Vec<Hit>) {
        let strong = hits.iter().any(|h| h.dense >= 0.40 || h.lexical >= 0.55);
        let request;
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

    pub(super) fn generate_card(self: &Arc<Self>, card: u64, gen: u64, user: String) {
        self.generate_card_with(card, gen, user, CARD_SYSTEM, 140, Priority::Low);
    }

    pub(super) fn generate_card_with(self: &Arc<Self>, card: u64, gen: u64, user: String, system: &'static str, max_tokens: usize, priority: Priority) {
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

    /// Adds a card to the live timeline in the "answering" state and returns what the model should read:
    /// the meeting so far, the person's own notes and (when the folder allows) a few library passages.
    pub(super) fn live_card(self: &Arc<Self>, kind: &'static str, question: &str, with_library: bool) -> Result<(u64, u64, String)> {
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
    pub(super) fn meeting_ask(self: &Arc<Self>, question: &str) -> Result<Value> {
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
    pub(super) fn meeting_suggest(self: &Arc<Self>) -> Result<Value> {
        let (id, gen, context) = self.live_card("ideas", "Questions you could ask", false)?;
        let user = format!("{context}\n\nThe 3 questions:");
        self.generate_card_with(id, gen, user, IDEAS_SYSTEM, 170, Priority::High);
        Ok(json!({"id": id}))
    }
}
