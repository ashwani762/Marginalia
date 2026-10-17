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

    pub(super) fn describe_photo(&self, path: &str) -> String {
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
}
