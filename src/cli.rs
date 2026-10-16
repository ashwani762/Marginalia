//! Headless mode: `marginalia <command> …` exercises the same engine without the UI.
//! Useful for scripting and for verifying a build.

use crate::app::Services;
use crate::audio::{AudioCapture, CaptureConfig};
use crate::engine::{ChatMessage, GenParams, Priority, Transcript};
use crate::rag::{IngestInput, Scope};
use anyhow::{anyhow, Result};
use std::sync::Arc;
use std::time::{Duration, Instant};

pub fn usage() -> &'static str {
    "usage:\n  marginalia                      launch the app\n  marginalia ingest <paths|urls…> add to the knowledge base\n  marginalia search <query>       hybrid retrieval\n  marginalia ask <question>       retrieval-augmented answer\n  marginalia caption <image>      describe an image with Gemma 4\n  marginalia transcribe <audio>   transcribe an audio file\n  marginalia listen <seconds>     capture mic + system audio and transcribe live\n  marginalia docs                 list indexed documents"
}

pub fn run(services: Arc<Services>, args: Vec<String>) -> Result<()> {
    let cmd = args.first().map(|s| s.as_str()).unwrap_or("");
    let rest: Vec<String> = args.iter().skip(1).cloned().collect();
    match cmd {
        "docs" => {
            for d in services.kb.documents()? {
                println!("{:>4}  {:<8} {:>5} passages  {}  [{}]", d.id, d.kind.as_str(), d.chunk_count, d.title, d.source);
            }
            Ok(())
        }
        "ingest" | "search" | "ask" | "caption" | "transcribe" | "listen" => {
            boot(&services)?;
            match cmd {
                "ingest" => ingest(&services, &rest),
                "search" => search(&services, &rest.join(" ")),
                "ask" => ask(&services, &rest.join(" ")),
                "caption" => caption(&services, &rest),
                "transcribe" => transcribe(&services, &rest),
                "listen" => listen(&services, rest.first().and_then(|s| s.parse().ok()).unwrap_or(15)),
                _ => unreachable!(),
            }
        }
        _ => {
            println!("{}", usage());
            Ok(())
        }
    }
}

fn boot(services: &Arc<Services>) -> Result<()> {
    let missing = services.missing_models();
    if !missing.is_empty() {
        for spec in missing {
            eprintln!("downloading {}…", spec.label);
            let mut last = Instant::now();
            crate::models::download(&services.paths, &spec, services.download_cancel.clone(), |p| {
                if last.elapsed() > Duration::from_secs(2) {
                    last = Instant::now();
                    eprintln!("  {} / {}", crate::models::human_bytes(p.downloaded), crate::models::human_bytes(p.total));
                }
            })?;
        }
    }
    let settings = services.settings();
    let t = Instant::now();
    services.engine.load_embedder(&settings)?;
    let (tx, rx) = crossbeam_channel::unbounded();
    services.kb.attach_embedder(services.engine.embed().unwrap(), tx);
    std::thread::spawn(move || while rx.recv().is_ok() {});
    services.engine.load_llm(&settings)?;
    services.kb.attach_llm(services.engine.llm());
    let _ = crate::app::load_whisper(services, &settings)?;
    services.kb.attach_whisper(services.engine.whisper());
    eprintln!("models ready in {:.1}s", t.elapsed().as_secs_f64());
    Ok(())
}

fn ingest(services: &Arc<Services>, items: &[String]) -> Result<()> {
    let (tx, rx) = crossbeam_channel::unbounded::<crate::rag::IngestEvent>();
    // Re-attach so we observe events from this process.
    services.kb.attach_embedder(services.engine.embed().unwrap(), tx);
    let mut pending = 0;
    for it in items {
        let input = if it.starts_with("http://") || it.starts_with("https://") {
            IngestInput::Url(it.clone())
        } else {
            IngestInput::File(std::path::PathBuf::from(it))
        };
        if services.kb.enqueue(input).is_some() {
            pending += 1;
        }
    }
    let started = Instant::now();
    while pending > 0 {
        let ev = rx.recv_timeout(Duration::from_secs(600)).map_err(|_| anyhow!("ingest timed out"))?;
        use crate::rag::IngestStage::*;
        match ev.stage {
            Queued | Extracting | Chunking => eprintln!("[{}] {:?}", ev.label, ev.stage),
            Embedding { done, total } => eprintln!("[{}] embedding {done}/{total}", ev.label),
            Done { title, chunks, .. } => {
                println!("✓ {title} — {chunks} passages");
                pending -= 1;
            }
            Skipped { reason } => {
                println!("· {} — skipped: {reason}", ev.label);
                pending -= 1;
            }
            Failed { error } => {
                println!("✗ {} — {error}", ev.label);
                pending -= 1;
            }
        }
    }
    eprintln!("done in {:.1}s", started.elapsed().as_secs_f64());
    Ok(())
}

fn search(services: &Arc<Services>, q: &str) -> Result<()> {
    let t = Instant::now();
    let hits = services.kb.search(q, 6, Scope::All)?;
    eprintln!("{} hits in {:.1} ms", hits.len(), t.elapsed().as_secs_f64() * 1000.0);
    for (i, h) in hits.iter().enumerate() {
        println!(
            "[{}] {}  ({})  dense={:.3} bm25={:.2} fused={:.4}\n    {}",
            i + 1,
            h.title,
            h.locator(),
            h.dense,
            h.lexical,
            h.score,
            crate::rag::snippet_of(&h.text, 200)
        );
    }
    Ok(())
}

fn ask(services: &Arc<Services>, q: &str) -> Result<()> {
    let hits = services.kb.search(q, 6, Scope::All)?;
    let passages = hits
        .iter()
        .enumerate()
        .map(|(i, h)| format!("[{}] {} — {}\n{}", i + 1, h.title, h.locator(), h.context))
        .collect::<Vec<_>>()
        .join("\n\n");
    let llm = services.engine.llm().ok_or_else(|| anyhow!("llm not loaded"))?;
    let t = Instant::now();
    let answer = llm.complete(
        vec![
            ChatMessage::system("Answer using only the passages; cite them like [1]. Be concise."),
            ChatMessage::user(format!("Passages:\n{passages}\n\nQuestion: {q}")),
        ],
        GenParams { max_tokens: 300, temperature: 0.3, ..Default::default() },
        Priority::High,
    )?;
    println!("{answer}\n");
    for (i, h) in hits.iter().enumerate() {
        println!("[{}] {} — {}", i + 1, h.title, h.locator());
    }
    eprintln!("({:.1}s)", t.elapsed().as_secs_f64());
    Ok(())
}

fn caption(services: &Arc<Services>, paths: &[String]) -> Result<()> {
    let llm = services.engine.llm().ok_or_else(|| anyhow!("llm not loaded"))?;
    let embed = services.engine.embed().ok_or_else(|| anyhow!("embedder not loaded"))?;
    for p in paths {
        let t = Instant::now();
        let text = llm.complete_with_images(
            vec![ChatMessage::user(format!("{}\nDescribe this image in 2-3 sentences, including any visible text or numbers.", crate::engine::llm::image_marker()))],
            vec![std::path::PathBuf::from(p)],
            GenParams { max_tokens: 160, temperature: 0.2, ..Default::default() },
            Priority::High,
        )?;
        println!("{p}: {text}   ({:.1}s)", t.elapsed().as_secs_f64());
        // Cross-modal sanity check.
        let img = embed.embed_image(std::path::Path::new(p))?;
        for q in ["a bar chart of quarterly revenue", "an invoice with a total amount due", "a photo of a cat on grass"] {
            let qv = embed.embed_one(q, crate::engine::embed::EmbedKind::Query)?;
            println!("   sim({q:?}) = {:.3}", crate::engine::embed::cosine(&img, &qv));
        }
    }
    Ok(())
}

fn transcribe(services: &Arc<Services>, paths: &[String]) -> Result<()> {
    let w = services.engine.whisper().ok_or_else(|| anyhow!("whisper not loaded"))?;
    for p in paths {
        let t = Instant::now();
        let (pcm, dur) = crate::audio::decode::decode_to_16k_mono(std::path::Path::new(p))?;
        let utts = w.transcribe_buffer(pcm)?;
        eprintln!("{p}: {dur:.1}s audio in {:.1}s", t.elapsed().as_secs_f64());
        for u in utts {
            println!("[{} → {}] {}", crate::rag::chunk::fmt_ts(u.t0), crate::rag::chunk::fmt_ts(u.t1), u.text);
        }
    }
    Ok(())
}

fn listen(services: &Arc<Services>, seconds: u64) -> Result<()> {
    let w = services.engine.whisper().ok_or_else(|| anyhow!("whisper not loaded"))?;
    let (tx, rx) = crossbeam_channel::unbounded::<Transcript>();
    // Route transcripts to our channel for this session.
    let settings = services.settings();
    services.engine.load_whisper(&settings, tx)?;
    let w2 = services.engine.whisper().unwrap_or(w);
    let cfg = CaptureConfig { mic_device: settings.mic_device.clone(), capture_system: settings.capture_system_audio };
    let cap = AudioCapture::start(&cfg, w2.clone())?;
    for warn in &cap.warnings {
        eprintln!("warning: {warn}");
    }
    eprintln!("listening for {seconds}s on {:?}…", cap.sources);
    let deadline = Instant::now() + Duration::from_secs(seconds);
    let meters = cap.meters.clone();
    let mut last_level = Instant::now();
    while Instant::now() < deadline {
        match rx.recv_timeout(Duration::from_millis(200)) {
            Ok(t) if !t.partial => println!("[{} {:?}] {}", crate::rag::chunk::fmt_ts(t.t0), t.source, t.text),
            Ok(_) => {}
            Err(_) => {}
        }
        if last_level.elapsed() > Duration::from_secs(2) {
            last_level = Instant::now();
            eprintln!(
                "  levels: mic={:.3} system={:.3}",
                meters.level(crate::engine::AudioSource::Microphone),
                meters.level(crate::engine::AudioSource::System)
            );
        }
    }
    cap.stop(&w2);
    // Drain trailing transcripts.
    let drain_until = Instant::now() + Duration::from_secs(8);
    while Instant::now() < drain_until {
        if let Ok(t) = rx.recv_timeout(Duration::from_millis(300)) {
            if !t.partial {
                println!("[{} {:?}] {}", crate::rag::chunk::fmt_ts(t.t0), t.source, t.text);
            }
        }
    }
    Ok(())
}
