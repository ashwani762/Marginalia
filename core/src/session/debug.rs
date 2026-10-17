//! Timing helpers behind the `debug.*` calls. Only the test setup uses them, the app never does.

use super::*;

impl Core {
    /// Times transcription and generation on this device. Used while tuning for phones.
    pub(super) fn bench(&self, path: &str) -> Result<Value> {
        let w = self.engine.whisper().ok_or_else(|| anyhow!("speech model not loaded"))?;
        let (pcm, secs) = crate::audio::decode::decode_to_16k_mono(Path::new(path))?;
        let t = Instant::now();
        let utt = w.transcribe_buffer(pcm)?;
        let whisper_ms = t.elapsed().as_millis();
        let text: String = utt.iter().map(|u| u.text.as_str()).collect::<Vec<_>>().join(" ");
        let llm = self.engine.llm().ok_or_else(|| anyhow!("chat model not loaded"))?;
        let t = Instant::now();
        let (out, st) = {
            let (tx, rx) = crossbeam_channel::unbounded::<GenEvent>();
            llm.generate(GenRequest {
                images: Vec::new(),
                messages: vec![ChatMessage::user(format!("Summarize in two sentences: {text}"))],
                params: GenParams { max_tokens: 64, temperature: 0.2, ..Default::default() },
                priority: Priority::High,
                cancel: Arc::new(AtomicBool::new(false)),
                on_event: Box::new(move |e| { let _ = tx.send(e); }),
            });
            let mut out = String::new();
            let mut stats = GenStats::default();
            while let Ok(e) = rx.recv() {
                match e { GenEvent::Token(t) => out.push_str(&t), GenEvent::Done(s) => { stats = s; break } GenEvent::Error(e) => return Err(anyhow!(e)) }
            }
            (out, stats)
        };
        Ok(json!({
            "audio_secs": secs, "whisper_ms": whisper_ms, "whisper_rtf": whisper_ms as f64 / 1000.0 / secs.max(0.01),
            "text": text, "llm_ms": t.elapsed().as_millis(), "llm_prompt_tokens": st.prompt_tokens, "llm_tokens": st.generated_tokens,
            "llm_tok_per_s": st.generated_tokens as f64 / (st.elapsed_ms.max(1) as f64 / 1000.0), "llm_out": out,
            "threads": std::thread::available_parallelism().map(|n| n.get()).unwrap_or(0),
        }))
    }

    /// One generation with the numbers that matter: time to first token, prefill and decode speed.
    pub(super) fn llm_bench(&self, system: &str, user: &str, max_tokens: usize) -> Result<Value> {
        let llm = self.engine.llm().ok_or_else(|| anyhow!("chat model not loaded"))?;
        let (tx, rx) = crossbeam_channel::unbounded::<(Instant, GenEvent)>();
        let t0 = Instant::now();
        llm.generate(GenRequest {
            images: Vec::new(),
            messages: vec![ChatMessage::system(system), ChatMessage::user(user)],
            params: GenParams { max_tokens, temperature: 0.2, ..Default::default() },
            priority: Priority::High,
            cancel: Arc::new(AtomicBool::new(false)),
            on_event: Box::new(move |e| { let _ = tx.send((Instant::now(), e)); }),
        });
        let mut first: Option<Instant> = None;
        let mut out = String::new();
        let mut stats = GenStats::default();
        let mut end = t0;
        while let Ok((at, e)) = rx.recv() {
            match e {
                GenEvent::Token(t) => { first.get_or_insert(at); out.push_str(&t); }
                GenEvent::Done(s) => { stats = s; end = at; break }
                GenEvent::Error(e) => return Err(anyhow!(e)),
            }
        }
        let ttft = first.map(|f| f.duration_since(t0).as_secs_f64()).unwrap_or(0.0);
        let decode_s = first.map(|f| end.duration_since(f).as_secs_f64()).unwrap_or(0.0).max(1e-3);
        Ok(json!({
            "engine": "litert-lm", "ttft_s": ttft, "total_s": end.duration_since(t0).as_secs_f64(),
            "prefill_tokens": stats.prompt_tokens, "prefill_tok_s": stats.prompt_tokens as f64 / ttft.max(1e-3),
            "decode_tokens": stats.generated_tokens, "decode_tok_s": (stats.generated_tokens.saturating_sub(1)) as f64 / decode_s,
            "text": out,
        }))
    }

    /// Embeds the given texts the way a document would be, writes the vectors to a file in the
    /// app's files folder, and says which kind of path ran. Used to compare computers with the phone.
    pub(super) fn debug_embed(&self, args: &Value) -> Result<Value> {
        let texts: Vec<String> = args.get("texts").and_then(Value::as_array).map(|a| a.iter().filter_map(|v| v.as_str().map(String::from)).collect()).unwrap_or_default();
        let h = self.engine.embed().ok_or_else(|| anyhow!("the search model is not loaded"))?;
        let t0 = Instant::now();
        let v = h.embed(texts, crate::engine::embed::EmbedKind::Raw)?;
        std::fs::write(self.paths.data_dir.join("debug_embed.json"), serde_json::to_vec(&v)?)?;
        Ok(json!({"count": v.len(), "dim": v.first().map(|x| x.len()).unwrap_or(0), "secs": t0.elapsed().as_secs_f64()}))
    }

    /// Loads another speech model next to the running one, times it on a file, and drops it.
    /// args: model, path, lang, threads (default 4), flash (0 or 1), runs (default 1)
    pub(super) fn debug_whisper_bench(&self, args: &Value) -> Result<Value> {
        let s = |k: &str| args.get(k).and_then(Value::as_str).unwrap_or("").to_string();
        let model = self.paths.models_dir.join(s("model"));
        let (pcm, secs) = crate::audio::decode::decode_to_16k_mono(Path::new(&s("path")))?;
        let (tx, _rx) = crossbeam_channel::unbounded::<Transcript>();
        let threads = args.get("threads").and_then(Value::as_i64).unwrap_or(4) as i32;
        std::env::set_var("MARGINALIA_FLASH", if args.get("flash").and_then(Value::as_i64).unwrap_or(0) == 1 { "1" } else { "0" });
        let t = Instant::now();
        let h = crate::engine::whisper::spawn(
            crate::engine::whisper::WhisperConfig { model_path: model, use_gpu: false, language: s("lang"), threads },
            tx,
        )?;
        let load_ms = t.elapsed().as_millis();
        let runs = args.get("runs").and_then(Value::as_i64).unwrap_or(1).max(1);
        let mut best = u128::MAX;
        let mut text = String::new();
        for _ in 0..runs {
            let t = Instant::now();
            let utt = h.transcribe_buffer(pcm.clone())?;
            best = best.min(t.elapsed().as_millis());
            text = utt.iter().map(|u| u.text.as_str()).collect::<Vec<_>>().join(" ");
        }
        h.shutdown();
        Ok(json!({"threads": threads, "audio_secs": secs, "load_ms": load_ms, "decode_ms": best, "rtf": best as f64 / 1000.0 / secs.max(0.01), "text": snippet_of(&text, 80)}))
    }
}
