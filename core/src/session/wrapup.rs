//! Everything after a meeting stops: the write-up job (summary, title, search index), recovery of unfinished meetings, and reading or deleting a saved meeting.

use super::*;

/// Everything needed to write a finished meeting up, owned by the background job.
pub(super) struct FinishJob {
    id: i64,
    secs: u64,
    raw: String,
    enhanced: String,
    lines: Vec<Line>,
    jots: Vec<JobJot>,
    sections: Vec<Section>,
    covered: usize,
    /// Tell the person when it is done (not for a quiet recovery at launch).
    notify: bool,
    /// Photos taken during the meeting that still need reading: (id, path, seconds in).
    unread_photos: Vec<(i64, String, Option<f64>)>,
    /// First let a compute node listen to the kept recording again and put its transcript in place of the live one.
    improve: bool,
    /// The person asked for the notes to be written again from a better transcript: the meeting keeps its
    /// finishing time and the app says the transcript was improved.
    manual: bool,
}

/// What listening to the recording again came to.
pub(super) enum Improved {
    /// The transcript was replaced (in part when the node stopped early).
    Done { segments: usize, node: String, partial: bool },
    /// Nothing was changed, and why.
    Skipped(String),
}

#[derive(Clone)]
pub(super) struct JobJot {
    id: i64,
    t: Option<f64>,
    kind: String,
    raw: String,
    polished: String,
}

impl Core {
    /// Progress of a meeting being written up in the background.
    pub(super) fn wrap(&self, meeting_id: i64, step: &str, label: &str, progress: f32) {
        self.emit(json!({"type": "wrap", "meeting_id": meeting_id, "step": step, "label": label, "progress": progress}));
    }

    /// Stop recording. Returns at once; the rest happens in the background and ends with a
    /// `wrap_done` event, so the person can leave, start another meeting, or close the app.
    pub(super) fn meeting_stop(self: &Arc<Self>) -> Result<Value> {
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
    pub(super) fn finish_after_stop(self: &Arc<Self>, meeting_id: i64) {
        self.wrap(meeting_id, "finalizing", "Finalizing the transcript", 8.0);
        let t0 = Instant::now();
        std::thread::sleep(Duration::from_millis(1500));
        while self.engine.whisper().map(|w| w.queue_len() > 0).unwrap_or(false) && t0.elapsed() < Duration::from_secs(25) {
            std::thread::sleep(Duration::from_millis(250));
        }
        std::thread::sleep(Duration::from_millis(1500));
        // A window a compute node is still listening to gets a few seconds more, so its fixes are in the transcript.
        self.finish_correction();

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
            lines: l.lines.clone(), jots, sections: l.sections.clone(), covered: l.sec_upto,
            notify: true,
            unread_photos: l.queued_photos.iter().map(|(id, path)| (*id, path.clone(), l.photos.iter().find(|p| p.0 == *id).map(|p| p.1))).collect(),
            improve: true,
            manual: false,
        };
        self.run_finish(job);
    }

    /// Write up a meeting: polish any jots still waiting, wait for photo descriptions, then
    /// summary, title and search index. Runs one at a time, at low priority so a meeting being
    /// recorded right now always has the model first.
    pub(super) fn run_finish(self: &Arc<Self>, mut job: FinishJob) {
        let id = job.id;
        if !self.finishing_ids.lock().insert(id) {
            return;
        }
        self.finishing_jobs.fetch_add(1, Ordering::Relaxed);
        let result = (|| -> Result<(String, String)> {
            // 0. a compute node listens to the kept recording again, so the notes are written from the better transcript
            let improved = job.improve && self.improve_in_finish(&mut job);
            // The bar has already come this far when the transcript was improved.
            let floor = if improved { 30.0f32 } else { 0.0 };
            // 1. jots that were still waiting for their polish
            let waiting: Vec<usize> = job.jots.iter().enumerate().filter(|(_, j)| j.polished.trim().is_empty() && j.kind != "keep").map(|(i, _)| i).collect();
            for (n, i) in waiting.iter().enumerate() {
                self.wrap(id, "jots", &format!("Polishing note {} of {}", n + 1, waiting.len()), (12.0 + 12.0 * n as f32 / waiting.len().max(1) as f32).max(floor));
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
                self.wrap(id, "photos", &format!("Reading photo {} of {}", n + 1, unread.len()), (24.0 + 4.0 * n as f32 / unread.len().max(1) as f32).max(floor));
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
                self.wrap(id, "photos", "Reading your photos", 26.0f32.max(floor));
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
            if !job.manual {
                let _ = self.kb.store.finish_meeting(id, job.secs as i64);
            }
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
                self.emit(json!({"type": "wrap_done", "meeting_id": id, "title": title, "preview": snippet_of(&summary.lines().find(|l| !l.trim().is_empty() && !l.starts_with('#')).unwrap_or(""), 140), "notify": job.notify, "improve": job.manual, "improved": job.manual}));
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
    pub(super) fn gen_job(&self, messages: Vec<ChatMessage>, params: GenParams, on_text: impl FnMut(&str)) -> Result<(String, GenStats)> {
        self.gen_job_with(true, messages, params, on_text)
    }

    /// [`Self::gen_job`], optionally without taking the write-up turn. Parts written side by side on
    /// several home computers hold the turn once for all of them instead.
    pub(super) fn gen_job_with(&self, gated: bool, messages: Vec<ChatMessage>, params: GenParams, mut on_text: impl FnMut(&str)) -> Result<(String, GenStats)> {
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
            let _turn = gated.then(|| self.finish_gate.lock());
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
    pub(super) fn summarize_meeting(
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
        // With several home computers up, the parts are written side by side, one per computer.
        let workers = crate::remote::get().map(|r| r.chat_parallelism()).unwrap_or(0).min(4);
        if workers >= 2 && remaining_parts >= 2 {
            let mut todo: Vec<(usize, usize)> = Vec::new();
            let mut i = covered.min(lines.len());
            while i < lines.len() {
                let to = window_end(lines, i, SECTION_WINDOW_TOKENS);
                todo.push((i, to));
                i = to;
            }
            let workers = workers.min(todo.len());
            tracing::info!("writing {} parts on {} computers at once", todo.len(), workers);
            let results: parking_lot::Mutex<(Vec<Option<Section>>, usize)> = parking_lot::Mutex::new((vec![None; todo.len()], 0));
            let next = AtomicUsize::new(0);
            let finished = AtomicUsize::new(done_parts);
            let failure: parking_lot::Mutex<Option<anyhow::Error>> = parking_lot::Mutex::new(None);
            let _turn = self.finish_gate.lock();
            std::thread::scope(|sc| {
                for _ in 0..workers {
                    sc.spawn(|| loop {
                        if failure.lock().is_some() {
                            break;
                        }
                        let k = next.fetch_add(1, Ordering::Relaxed);
                        let Some(&(from, to)) = todo.get(k) else { break };
                        let text = window_text(lines, from, to, SECTION_WINDOW_TOKENS + 200);
                        let r = self.gen_job_with(false, vec![ChatMessage::system(SECTION_SYSTEM), ChatMessage::user(format!("Part of the meeting:\n{text}\n\nWrite the notes for this part."))], GenParams { max_tokens: 260, temperature: 0.2, ..Default::default() }, |_| {});
                        match r {
                            Ok((t, _)) => {
                                let sec = Section { from, to, t0: lines[from].t0, t1: lines[to - 1].t1, text: t.trim().to_string() };
                                let mut g = results.lock();
                                g.0[k] = Some(sec);
                                // Parts are saved in order, so a restart picks up right after the last one saved.
                                while g.1 < g.0.len() {
                                    let Some(s) = g.0[g.1].clone() else { break };
                                    let _ = self.kb.store.save_section(mid, s.from, s.to, s.t0, s.t1, &s.text);
                                    g.1 += 1;
                                }
                                drop(g);
                                let n = finished.fetch_add(1, Ordering::Relaxed) + 1;
                                self.wrap(mid, "summarizing", &format!("Summarizing part {n} of {total_parts}"), 30.0 + 50.0 * n as f32 / total_parts.max(1) as f32);
                            }
                            Err(e) => {
                                *failure.lock() = Some(e);
                                break;
                            }
                        }
                    });
                }
            });
            drop(_turn);
            let (written, saved) = results.into_inner();
            // Whatever was saved in order is kept even if a later part failed.
            for sec in written.into_iter().take(saved.max(0)).flatten() {
                covered = sec.to;
                sections.push(sec);
            }
            if let Some(e) = failure.into_inner() {
                return Err(e);
            }
            done_parts = sections.len();
        }
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
    pub(super) fn reembed_if_needed(self: &Arc<Self>) {
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

    /// Finds meetings whose notes were never completed and finishes them. Safe to call at any time:
    /// it skips the live recording and anything already being written. Returns how many it started.
    pub(super) fn rescan_unfinished(self: &Arc<Self>, grace_secs: u64) -> usize {
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

    pub(super) fn recover_one(self: &Arc<Self>, m: &Meeting) -> Result<()> {
        let Some(job) = self.finish_job_for(m)? else { return Ok(()) };
        self.toast("info", format!("Finishing the notes for “{}”", snippet_of(&m.title, 40)));
        self.run_finish(job);
        Ok(())
    }

    /// Everything the write-up needs for a meeting that is already saved. `None` when nothing was captured.
    fn finish_job_for(&self, m: &Meeting) -> Result<Option<FinishJob>> {
        let id = m.id;
        let segs = self.kb.store.segments(id)?;
        let jots = self.kb.store.jots(id)?;
        let photos = self.kb.store.attachments(id)?;
        if segs.is_empty() && jots.is_empty() && photos.is_empty() {
            return Ok(None);
        }
        let lines = self.lines_of(id)?;
        let (secs_saved, covered) = valid_sections(self.kb.store.sections(id).unwrap_or_default(), &lines);
        let secs = if m.ended_at.is_none() { segs.last().map(|s| s.t1).unwrap_or(0.0) as u64 } else { m.duration_secs.max(0) as u64 };
        Ok(Some(FinishJob {
            id, secs, raw: m.notes_raw.clone(), enhanced: m.notes_enhanced.clone(), lines,
            jots: jots.into_iter().map(|j| JobJot { id: j.id, t: j.t, kind: j.kind, raw: j.raw, polished: j.polished }).collect(),
            sections: secs_saved, covered, notify: true,
            unread_photos: photos.iter().filter(|a| a.caption.trim().is_empty()).map(|a| (a.id, a.path.clone(), a.t)).collect(),
            improve: true,
            manual: false,
        }))
    }

    /// The transcript of a saved meeting as readable lines.
    fn lines_of(&self, id: i64) -> Result<Vec<Line>> {
        let names: HashMap<i64, String> = self.kb.store.speaker_names(id).unwrap_or_default().into_iter().collect();
        let mut lines: Vec<Line> = Vec::new();
        for s in self.kb.store.segments(id)? {
            push_merged_as(&mut lines, s.t0, s.t1, &s.text, s.speaker, &who_is(&names, s.speaker));
        }
        Ok(lines)
    }

    // ---------------------------------------------------------------- a better transcript from a compute node

    /// Part of the finishing flow: when a compute node can listen again and the recording was kept, its transcript
    /// replaces the live one before the notes are written. Anything that goes wrong is only logged, and the write-up
    /// then uses the live transcript. Returns true when the transcript was replaced.
    fn improve_in_finish(&self, job: &mut FinishJob) -> bool {
        let id = job.id;
        let flag = format!("improved_{id}");
        if !self.settings().improve_transcript || self.kb.store.flag(&flag) {
            return false;
        }
        let Some(node) = crate::remote::get().and_then(|r| r.transcribe_node_available()) else { return false };
        if !self.kb.store.audios(id).map(|a| a.iter().any(|(p, _)| Path::new(p).is_file())).unwrap_or(false) {
            return false;
        }
        self.wrap(id, "improving", &format!("Improving the transcript on {node}"), 9.0);
        match self.improve_transcript(id, &|node, frac| self.wrap(id, "improving", &improving_label(node, frac), 9.0 + 21.0 * frac)) {
            Ok(Improved::Done { segments, node, partial }) => {
                tracing::info!("meeting {id}: transcript improved on {node} ({segments} segments, partial={partial})");
                match self.lines_of(id) {
                    Ok(lines) if !lines.is_empty() => {
                        job.lines = lines;
                        // Notes condensed from the old lines no longer match.
                        job.sections.clear();
                        job.covered = 0;
                        true
                    }
                    _ => false,
                }
            }
            Ok(Improved::Skipped(why)) => {
                tracing::info!("meeting {id}: transcript kept as it was: {why}");
                false
            }
            Err(e) => {
                tracing::warn!("meeting {id}: could not improve the transcript: {e:#}");
                false
            }
        }
    }

    /// Sends the kept recording of a meeting to a compute node with a large speech model and puts what comes back in
    /// place of the live transcript. Only done when what comes back is plausible (see `judge`): a failed or empty job
    /// never touches the live transcript. When the node stops early, the part it did cover is swapped and the live text
    /// is kept for the rest. `progress(node, 0..1)` reports how far it is.
    pub(super) fn improve_transcript(&self, mid: i64, progress: &dyn Fn(&str, f32)) -> Result<Improved> {
        let skip = |why: &str| Ok(Improved::Skipped(why.to_string()));
        let Some(remote) = crate::remote::get() else { return skip("no compute node") };
        if remote.transcribe_node_available().is_none() {
            return skip("no compute node can listen to the recording right now");
        }
        let meeting = self.kb.store.list_meetings()?.into_iter().find(|m| m.id == mid).ok_or_else(|| anyhow!("meeting not found"))?;
        let old: Vec<(f64, f64, String)> = self.kb.store.segments(mid)?.into_iter().map(|s| (s.t0, s.t1, s.text)).collect();
        let last_spoken = old.iter().map(|s| s.1).fold(0.0f64, f64::max);
        // The kept files, with their lengths: from the file itself, else up to where the next one starts, else up to the end of the meeting.
        let kept: Vec<(String, f64)> = self.kb.store.audios(mid)?.into_iter().filter(|(p, _)| Path::new(p).is_file()).collect();
        if kept.is_empty() {
            return skip("the recording was not kept");
        }
        let meeting_end = (meeting.duration_secs.max(0) as f64).max(last_spoken);
        let files: Vec<(String, f64, f64)> = kept
            .iter()
            .enumerate()
            .map(|(i, (path, offset))| {
                let secs = crate::audio::decode::probe_duration_secs(Path::new(path))
                    .or_else(|| kept.get(i + 1).map(|n| n.1 - offset))
                    .unwrap_or(meeting_end - offset)
                    .max(1.0);
                (path.clone(), *offset, secs)
            })
            .collect();
        let plan = AudioPlan::new(&files);
        if plan.is_empty() {
            return skip("the recording is empty");
        }
        // A recording being made has the phone to itself.
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
        let language = language_for_node(self.kb.store.meeting_language(mid), &self.settings().language);
        let cancel = AtomicBool::new(false);
        let job = crate::remote::TranscribeJob { total_ms: plan.total_ms(), language, cancel: &cancel, priority: marginalia_proto::Priority::Background, prompt: None };
        let started = Instant::now();
        let outcome = remote.transcribe(
            &job,
            |start_ms| Ok(open_frames(&plan, start_ms)),
            |done, total, node| progress(node, (done as f32 / total.max(1) as f32).clamp(0.0, 1.0)),
            |_, _, _| {},
        );
        let outcome = match outcome {
            None => return skip("no compute node can listen to the recording right now"),
            Some(r) => r?,
        };
        let heard: Vec<(f64, f64, String)> = outcome
            .segments
            .iter()
            .map(|(a, b, t)| (plan.to_meeting(*a as f64 / 1000.0), plan.to_meeting(*b as f64 / 1000.0), t.clone()))
            .collect();
        let new = tidy_segments(heard, crate::engine::whisper::clean_transcript);
        let (from, to) = match judge(&new, &old, plan.meeting_span(), outcome.complete) {
            Verdict::Replace { from, to } => (from, to),
            Verdict::Reject(why) => return skip(why),
        };
        let composed = compose(&old, &new, from, to);
        let stored = self.kb.store.replace_segments(mid, &composed)?;
        let _ = self.kb.store.set_flag(&format!("improved_{mid}"), true);
        if self.kb.store.meeting_language(mid).is_none() {
            if let Some(l) = language_for_node(outcome.language.clone(), "auto") {
                let _ = self.kb.store.set_meeting_language(mid, &l);
            }
        }
        tracing::info!("transcript of meeting {mid}: {} new segments from {} in {:.0}s, {} stored", new.len(), outcome.node, started.elapsed().as_secs_f32(), stored);
        Ok(Improved::Done { segments: new.len(), node: outcome.node.clone(), partial: !outcome.complete })
    }

    /// The Review screen's "Improve transcript": listens again on a compute node, then writes the notes and the summary
    /// again from the new transcript. Returns at once. The rest runs in the background and ends with a `wrap_done` event
    /// (with `improve: true`, and `improved` telling whether the transcript changed).
    pub(super) fn meeting_improve(self: &Arc<Self>, id: i64) -> Result<Value> {
        if self.live.lock().as_ref().map(|l| l.meeting_id == id).unwrap_or(false) {
            return Err(anyhow!("This meeting is still being recorded."));
        }
        let meeting = self.kb.store.list_meetings()?.into_iter().find(|m| m.id == id).ok_or_else(|| anyhow!("meeting not found"))?;
        if meeting.ended_at.is_none() {
            return Err(anyhow!("The notes for this meeting are still being written. Try again in a moment."));
        }
        if !self.kb.store.audios(id)?.iter().any(|(p, _)| Path::new(p).is_file()) {
            return Err(anyhow!("This meeting has no kept recording."));
        }
        let Some(node) = crate::remote::get().and_then(|r| r.transcribe_node_available()) else {
            return Err(anyhow!("No compute node that can listen to a recording is available right now."));
        };
        if !self.finishing_ids.lock().insert(id) {
            return Err(anyhow!("The notes for this meeting are still being written. Try again in a moment."));
        }
        self.wrap(id, "improving", &format!("Improving the transcript on {node}"), 2.0);
        let core = self.clone();
        std::thread::Builder::new()
            .name("improve".into())
            .spawn(move || {
                let r = core.improve_transcript(id, &|node, frac| core.wrap(id, "improving", &improving_label(node, frac), 3.0 + 27.0 * frac));
                core.finishing_ids.lock().remove(&id);
                let note = match r {
                    Ok(Improved::Done { .. }) => {
                        // Write the notes and the summary again from the new transcript.
                        let job = core.kb.store.list_meetings().ok().and_then(|ms| ms.into_iter().find(|m| m.id == id)).and_then(|m| core.finish_job_for(&m).ok().flatten());
                        match job {
                            Some(mut job) => {
                                job.improve = false;
                                job.manual = true;
                                job.notify = false;
                                job.sections.clear();
                                job.covered = 0;
                                core.run_finish(job);
                                return;
                            }
                            None => "The transcript changed, but the notes could not be written again.".to_string(),
                        }
                    }
                    Ok(Improved::Skipped(why)) => format!("The transcript was left as it was, because {}.", why.trim_end_matches('.')),
                    Err(e) => {
                        tracing::warn!("improving the transcript of meeting {id} failed: {e:#}");
                        "The transcript was left as it was, because the compute node could not finish.".to_string()
                    }
                };
                core.emit(json!({"type": "wrap_done", "meeting_id": id, "title": "", "preview": "", "notify": false, "improve": true, "improved": false, "note": note}));
                core.emit(json!({"type": "meetings_changed"}));
            })
            .expect("spawn improve");
        Ok(json!({}))
    }

    pub(super) fn meeting_get(&self, id: i64) -> Result<Value> {
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

    /// Rebuild a finished meeting's passages (after a late photo, for example).
    pub(super) fn reindex_meeting(&self, id: i64) {
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

    pub(super) fn delete_meeting(&self, id: i64) {
        for a in self.kb.store.attachments(id).unwrap_or_default() {
            let _ = std::fs::remove_file(a.path);
        }
        let _ = self.kb.store.delete_meeting(id);
        if let Ok(Some(doc)) = self.kb.store.find_document_by_source(&format!("meeting://{id}")) {
            let _ = self.kb.remove_document(doc.id);
        }
    }
}

/// "Improving the transcript on Desk PC (63%)".
fn improving_label(node: &str, frac: f32) -> String {
    format!("Improving the transcript on {node} ({}%)", (frac * 100.0).floor().clamp(0.0, 100.0) as u32)
}
