//! The meeting that is being recorded now: start and resume, audio and transcript intake, the timer loop and the live note enhancement.

use super::*;

#[derive(Default)]
pub(super) struct Work {
    jot: Option<(i64, String, String, String)>,
    refine: Option<(i64, String, String, String)>,
    section: Option<(usize, usize, f64, f64, String)>,
    tick: Option<u64>,
    enhance: bool,
    scan: bool,
    question: Option<String>,
}

impl Core {
    pub(super) fn meeting_start(self: &Arc<Self>, folder: Option<i64>) -> Result<Value> {
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
    pub(super) fn blank_live(meeting_id: i64, folder: Option<i64>) -> Live {
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
            language: None,
        }
    }

    /// Carries on recording an earlier meeting: the same entry, the same transcript, the same notes.
    /// The write-up is redone from everything once the recording ends.
    pub(super) fn meeting_resume(self: &Arc<Self>, id: i64) -> Result<Value> {
        if self.live.lock().is_some() {
            return Err(anyhow!("a meeting is already in progress"));
        }
        if self.finishing_ids.lock().contains(&id) {
            return Err(anyhow!("The notes for this meeting are still being written. Try again in a moment."));
        }
        if self.engine.whisper().is_none() {
            return Err(anyhow!("The speech model is still loading."));
        }
        // Carrying on a meeting whose language is known: start from it instead of working it out again.
        let remembered = self.kb.store.meeting_language(id);
        if let Some(w) = self.engine.whisper() {
            let setting = self.settings().language.clone();
            w.set_language(if setting == "auto" { remembered.clone().unwrap_or(setting) } else { setting });
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
        live.language = remembered;
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
        // More audio is coming, so the transcript can be improved again when the meeting ends.
        let _ = self.kb.store.set_flag(&format!("improved_{id}"), false);
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

    pub(super) fn live_snapshot(&self) -> Value {
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
        // Once the speech model has settled on a language, the meeting remembers it: a compute node that listens
        // again later, and a recording that is carried on, need to know which one it was.
        if let Some(lang) = self.engine.whisper().and_then(|w| w.language_lock()) {
            if l.language.as_deref() != Some(lang.as_str()) {
                let _ = self.kb.store.set_meeting_language(l.meeting_id, &lang);
                l.language = Some(lang);
            }
        }
        if is_question(&t.text) {
            l.pending_questions.push(t.text.clone());
        }
        let idx = push_merged(&mut l.lines, t.t0, t.t1, &t.text);
        let line = line_json(idx, &l.lines[idx]);
        drop(g);
        self.emit(json!({"type": "partial", "text": ""}));
        self.emit(json!({"type": "line", "line": line}));
    }

    pub(super) fn meeting_notes(&self, text: &str) -> Result<Value> {
        if let Some(l) = self.live.lock().as_mut() {
            if l.notes != text {
                l.notes = text.to_string();
                l.notes_dirty = true;
                l.last_note_edit = Instant::now();
            }
        }
        Ok(json!({}))
    }

    pub(super) fn meeting_style(&self, style: &str) -> Result<Value> {
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

    pub(super) fn tick_loop(self: Arc<Self>, meeting_id: i64) {
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
                        work.refine = Some((j.id, j.raw.clone(), j.polished.clone(), ctx));
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
            if let Some((id, raw, current, ctx)) = work.refine.take() {
                self.spawn_refine(id, raw, current, ctx);
            }
            if let Some((from, to, t0, t1, text)) = work.section.take() {
                self.spawn_section(from, to, t0, t1, text);
            }
            if let Some(q) = work.question {
                self.create_card(q);
            }
        }
    }

    pub(super) fn run_enhance(self: &Arc<Self>) {
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

    pub(super) fn on_enhance_event(&self, gen: u64, ev: GenEvent) {
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
    pub(super) fn spawn_section(self: &Arc<Self>, from: usize, to: usize, t0: f64, t1: f64, text: String) {
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
}
