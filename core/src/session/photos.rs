//! Photos taken during a meeting and the plain description the chat model writes for each.

use super::*;

pub(super) fn photo_json(a: &crate::rag::store::Attachment) -> Value {
    json!({
        "id": a.id, "path": a.path, "t": a.t, "caption": a.caption,
        // "queued" until it is actually being read; the live screen overrides it while reading.
        "state": if a.caption.is_empty() { "queued" } else { "ready" },
    })
}

/// "Photos shared" block for prompts: `[mm:ss] description` per photo.
pub(super) fn photos_text(photos: &[(i64, f64, String)]) -> String {
    photos
        .iter()
        .filter(|p| !p.2.trim().is_empty())
        .map(|(_, t, c)| format!("[{}] {}", fmt_ts(*t), snippet_of(c, 400)))
        .collect::<Vec<_>>()
        .join("\n")
}

impl Core {
    /// A photo taken (or picked) for a meeting. Stored at once; Gemma describes it in the
    /// background and the description joins the meeting's searchable text.
    pub(super) fn meeting_photo(self: &Arc<Self>, args: &Value) -> Result<Value> {
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
        if live && recording {
            if let Some(l) = self.live.lock().as_mut() {
                l.photos.push((aid, t.unwrap_or(0.0), String::new()));
            }
            // A compute node reads it at once when one can: the phone stays free for the live transcript.
            if crate::remote::get().map(|r| r.vision_node_available()).unwrap_or(false) {
                self.emit(json!({"type": "photo", "meeting_id": meeting_id, "photo": {"id": aid, "path": path, "t": t, "caption": "", "state": "describing"}}));
                self.describe_during_recording_on_node(meeting_id, aid, path, t);
                return Ok(json!({"id": aid}));
            }
            // Otherwise it is saved and read after the meeting (or now, if the person asks with "Read now"). Reading it on the
            // phone during the recording keeps the graphics chip busy for several seconds: the screen freezes and the live
            // transcript lags, on the GPU as much as on the processor.
            if let Some(l) = self.live.lock().as_mut() {
                l.queued_photos.push((aid, path.clone()));
            }
            self.emit(json!({"type": "photo", "meeting_id": meeting_id, "photo": {"id": aid, "path": path, "t": t, "caption": "", "state": "queued"}}));
            return Ok(json!({"id": aid, "queued": true}));
        }
        self.emit(json!({"type": "photo", "meeting_id": meeting_id, "photo": {"id": aid, "path": path, "t": t, "caption": "", "state": "describing"}}));
        self.start_describing(meeting_id, aid, path, t, live);
        Ok(json!({"id": aid}))
    }

    /// Have Gemma read a photo that was waiting. Used by "Read now" on the card.
    pub(super) fn photo_read_now(self: &Arc<Self>, id: i64) -> Result<Value> {
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

    pub(super) fn start_describing(self: &Arc<Self>, meeting_id: i64, aid: i64, path: String, t: Option<f64>, live: bool) {
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
    pub(super) fn finish_photo(self: &Arc<Self>, meeting_id: i64, aid: i64, path: &str, t: Option<f64>, caption: &str, live: bool) {
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

    /// What the model is asked about a photo (the picture itself goes in front of it).
    const PHOTO_ASK: &'static str = "This photo was taken during a meeting. In 2 to 4 sentences describe what it shows. If it contains writing (whiteboard, slide, document, screen, sticky notes), transcribe the important text exactly. Plain prose, no preamble.";

    /// A compute node reads the photo when one can. None when none can or none did.
    fn describe_photo_on_node(&self, path: &str) -> Option<String> {
        let r = crate::remote::get()?;
        if !r.vision_node_available() {
            return None;
        }
        let jpeg = std::fs::read(path).ok()?;
        r.describe(jpeg, Self::PHOTO_ASK, 170)
    }

    /// A compute node reads it when one can, else this phone does.
    pub(super) fn describe_photo(&self, path: &str) -> String {
        self.describe_photo_on_node(path).unwrap_or_else(|| self.describe_photo_here(path))
    }

    /// Reads the photo with the model on this phone. During a recording this is never done on its own: it keeps the graphics
    /// chip busy for seconds, the screen freezes and the live transcript lags.
    fn describe_photo_here(&self, path: &str) -> String {
        let Some(llm) = self.engine.llm() else { return String::new() };
        let prompt = format!("{}\n{}", image_marker(), Self::PHOTO_ASK);
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

    /// During a recording: a compute node reads the photo now. If none does, the photo waits for the end of the meeting like
    /// the others (or is read on the phone at once when the meeting has already ended).
    fn describe_during_recording_on_node(self: &Arc<Self>, meeting_id: i64, aid: i64, path: String, t: Option<f64>) {
        self.describing.fetch_add(1, Ordering::Relaxed);
        *self.describing_by.lock().entry(meeting_id).or_insert(0) += 1;
        let core = self.clone();
        std::thread::Builder::new()
            .name("photo".into())
            .spawn(move || match core.describe_photo_on_node(&path) {
                Some(caption) => core.finish_photo(meeting_id, aid, &path, t, &caption, true),
                None => {
                    core.describing.fetch_sub(1, Ordering::Relaxed);
                    if let Some(n) = core.describing_by.lock().get_mut(&meeting_id) {
                        *n = n.saturating_sub(1);
                    }
                    let still_recording = core.live.lock().as_mut().map(|l| {
                        let rec = l.recording && l.meeting_id == meeting_id;
                        if rec {
                            l.queued_photos.push((aid, path.clone()));
                        }
                        rec
                    });
                    if still_recording == Some(true) {
                        core.emit(json!({"type": "photo", "meeting_id": meeting_id, "photo": {"id": aid, "path": path, "t": t, "caption": "", "state": "queued"}}));
                    } else {
                        // The meeting ended while the node was trying: nothing is recording any more, so the phone can read it.
                        core.describing.fetch_add(1, Ordering::Relaxed);
                        *core.describing_by.lock().entry(meeting_id).or_insert(0) += 1;
                        let caption = core.describe_photo_here(&path);
                        core.finish_photo(meeting_id, aid, &path, t, &caption, false);
                    }
                }
            })
            .expect("spawn photo");
    }
}
