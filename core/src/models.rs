//! Local model catalog and an in-process resumable downloader (no external tools).

use crate::config::{ChatModel, Paths, WhisperModel};
use anyhow::{bail, Context, Result};
use std::io::{Read, Write};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ModelKind {
    Embedding,
    Chat,
    Whisper,
}

#[derive(Clone, Copy, Debug)]
pub struct ModelSpec {
    pub id: &'static str,
    pub kind: ModelKind,
    pub label: &'static str,
    pub description: &'static str,
    pub repo: &'static str,
    pub file: &'static str,
    /// Expected size in bytes (0 = unknown; trust Content-Length).
    pub bytes: u64,
}

pub const EMBEDDING_GEMMA_2: ModelSpec = ModelSpec {
    id: "embeddinggemma-2",
    kind: ModelKind::Embedding,
    label: "EmbeddingGemma 2 (Q8_0)",
    description: "768-d retrieval embeddings, Matryoshka capable. Powers the knowledge base.",
    repo: "ggml-org/embeddinggemma-2-GGUF",
    file: "embeddinggemma-2-Q8_0.gguf",
    bytes: 309_855_456,
};

pub const GEMMA_4_E2B: ModelSpec = ModelSpec {
    id: "gemma-4-e2b",
    kind: ModelKind::Chat,
    label: "Gemma 4 E2B (Q4_0)",
    description: "Expands notes, answers questions, writes summaries.",
    repo: "ggml-org/gemma-4-E2B-it-GGUF",
    file: "gemma-4-E2B-it-Q4_0.gguf",
    bytes: 2_841_481_184,
};

pub const GEMMA_4_E4B: ModelSpec = ModelSpec {
    id: "gemma-4-e4b",
    kind: ModelKind::Chat,
    label: "Gemma 4 E4B (Q4_0)",
    description: "Stronger reasoning; best with GPU offload.",
    repo: "ggml-org/gemma-4-E4B-it-GGUF",
    file: "gemma-4-E4B-it-Q4_0.gguf",
    bytes: 0,
};

pub const WHISPER_SMALL: ModelSpec = ModelSpec {
    id: "whisper-small",
    kind: ModelKind::Whisper,
    label: "Whisper small (Q8_0)",
    description: "Light multilingual speech recognition, comfortable on CPU.",
    repo: "ggerganov/whisper.cpp",
    file: "ggml-small-q8_0.bin",
    bytes: 264_464_607,
};

pub const WHISPER_LARGE_TURBO: ModelSpec = ModelSpec {
    id: "whisper-large-v3-turbo",
    kind: ModelKind::Whisper,
    label: "Whisper large-v3-turbo (Q5_0)",
    description: "Best transcription quality; GPU recommended.",
    repo: "ggerganov/whisper.cpp",
    file: "ggml-large-v3-turbo-q5_0.bin",
    bytes: 574_041_195,
};

pub const EMBEDDING_GEMMA_2_MMPROJ: ModelSpec = ModelSpec {
    id: "embeddinggemma-2-mmproj",
    kind: ModelKind::Embedding,
    label: "EmbeddingGemma 2 vision projector",
    description: "Lets images live in the same embedding space as text, so a query can find a diagram.",
    repo: "ggml-org/embeddinggemma-2-GGUF",
    file: "mmproj-embeddinggemma-2-Q8_0.gguf",
    bytes: 554_821_024,
};

pub const GEMMA_4_E2B_MMPROJ: ModelSpec = ModelSpec {
    id: "gemma-4-e2b-mmproj",
    kind: ModelKind::Chat,
    label: "Gemma 4 E2B vision projector",
    description: "Captions imported images so answers can cite what is in them.",
    repo: "ggml-org/gemma-4-E2B-it-GGUF",
    file: "mmproj-gemma-4-E2B-it-Q8_0.gguf",
    bytes: 557_368_064,
};

pub const GEMMA_4_E4B_MMPROJ: ModelSpec = ModelSpec {
    id: "gemma-4-e4b-mmproj",
    kind: ModelKind::Chat,
    label: "Gemma 4 E4B vision projector",
    description: "Captions imported images so answers can cite what is in them.",
    repo: "ggml-org/gemma-4-E4B-it-GGUF",
    file: "mmproj-gemma-4-E4B-it-Q8_0.gguf",
    bytes: 559_874_816,
};

pub fn chat_mmproj_spec(choice: ChatModel) -> ModelSpec {
    match choice {
        ChatModel::GemmaE2B => GEMMA_4_E2B_MMPROJ,
        ChatModel::GemmaE4B => GEMMA_4_E4B_MMPROJ,
    }
}

pub const ALL_MODELS: &[ModelSpec] = &[
    EMBEDDING_GEMMA_2,
    GEMMA_4_E2B,
    GEMMA_4_E4B,
    WHISPER_SMALL,
    WHISPER_LARGE_TURBO,
    EMBEDDING_GEMMA_2_MMPROJ,
    GEMMA_4_E2B_MMPROJ,
    GEMMA_4_E4B_MMPROJ,
];

pub fn chat_spec(choice: ChatModel) -> ModelSpec {
    match choice {
        ChatModel::GemmaE2B => GEMMA_4_E2B,
        ChatModel::GemmaE4B => GEMMA_4_E4B,
    }
}

pub fn whisper_spec(choice: WhisperModel) -> ModelSpec {
    match choice {
        WhisperModel::Small => WHISPER_SMALL,
        WhisperModel::LargeTurbo => WHISPER_LARGE_TURBO,
    }
}

pub fn model_path(paths: &Paths, spec: &ModelSpec) -> PathBuf {
    paths.models_dir.join(spec.file)
}

pub fn is_present(paths: &Paths, spec: &ModelSpec) -> bool {
    let p = model_path(paths, spec);
    match std::fs::metadata(&p) {
        Ok(m) if m.is_file() => spec.bytes == 0 || m.len() == spec.bytes,
        _ => false,
    }
}

pub fn url_for(spec: &ModelSpec) -> String {
    format!(
        "https://huggingface.co/{}/resolve/main/{}",
        spec.repo, spec.file
    )
}

#[derive(Clone, Debug)]
pub struct DownloadProgress {
    pub id: &'static str,
    pub downloaded: u64,
    pub total: u64,
    pub bytes_per_sec: f64,
}

/// Download `spec` into the models dir, resuming a partial `.part` file if present.
/// Honors `HF_TOKEN` for gated repositories.
pub fn download(
    paths: &Paths,
    spec: &ModelSpec,
    cancel: Arc<AtomicBool>,
    mut on_progress: impl FnMut(DownloadProgress),
) -> Result<PathBuf> {
    let final_path = model_path(paths, spec);
    if is_present(paths, spec) {
        return Ok(final_path);
    }
    let part_path = final_path.with_extension(format!(
        "{}.part",
        final_path
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("bin")
    ));
    let have = std::fs::metadata(&part_path).map(|m| m.len()).unwrap_or(0);

    let url = url_for(spec);
    let agent = ureq::Agent::config_builder()
        .timeout_connect(Some(Duration::from_secs(30)))
        .timeout_recv_body(Some(Duration::from_secs(120)))
        .http_status_as_error(false)
        .build()
        .new_agent();
    let mut req = agent.get(&url).header("User-Agent", "marginalia/0.1 (rust)");
    if let Ok(tok) = std::env::var("HF_TOKEN") {
        if !tok.trim().is_empty() {
            req = req.header("Authorization", &format!("Bearer {}", tok.trim()));
        }
    }
    if have > 0 {
        req = req.header("Range", &format!("bytes={have}-"));
    }
    let mut resp = req.call().context("starting download")?;
    let status = resp.status().as_u16();
    let (mut downloaded, append) = match status {
        206 => (have, true),
        200 => (0, false),
        401 | 403 => bail!("access denied for {} (set HF_TOKEN for gated repos)", spec.repo),
        416 => {
            // Range not satisfiable: the partial file is already complete or corrupt.
            let _ = std::fs::remove_file(&part_path);
            bail!("resume failed; partial file removed, retry");
        }
        s => bail!("unexpected HTTP status {s} for {url}"),
    };
    let content_len: u64 = resp
        .headers()
        .get("content-length")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);
    let total = if spec.bytes > 0 {
        spec.bytes
    } else if append {
        have + content_len
    } else {
        content_len
    };

    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .append(append)
        .truncate(!append)
        .open(&part_path)
        .with_context(|| format!("opening {}", part_path.display()))?;

    let mut reader = resp.body_mut().as_reader();
    let mut buf = vec![0u8; 1 << 20];
    let started = Instant::now();
    let mut last_emit = Instant::now() - Duration::from_secs(1);
    let mut window_start = Instant::now();
    let mut window_bytes = 0u64;
    let mut speed = 0.0;
    loop {
        if cancel.load(Ordering::Relaxed) {
            bail!("download cancelled");
        }
        let n = reader.read(&mut buf).context("reading download body")?;
        if n == 0 {
            break;
        }
        file.write_all(&buf[..n])?;
        downloaded += n as u64;
        window_bytes += n as u64;
        if window_start.elapsed() >= Duration::from_millis(800) {
            speed = window_bytes as f64 / window_start.elapsed().as_secs_f64();
            window_start = Instant::now();
            window_bytes = 0;
        }
        if last_emit.elapsed() >= Duration::from_millis(120) {
            last_emit = Instant::now();
            on_progress(DownloadProgress {
                id: spec.id,
                downloaded,
                total,
                bytes_per_sec: speed,
            });
        }
    }
    file.flush()?;
    drop(file);
    if spec.bytes > 0 && downloaded != spec.bytes {
        bail!(
            "size mismatch for {}: got {downloaded}, expected {}",
            spec.file,
            spec.bytes
        );
    }
    std::fs::rename(&part_path, &final_path)?;
    on_progress(DownloadProgress {
        id: spec.id,
        downloaded,
        total: downloaded.max(total),
        bytes_per_sec: downloaded as f64 / started.elapsed().as_secs_f64().max(0.001),
    });
    Ok(final_path)
}

pub fn human_bytes(b: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut v = b as f64;
    let mut i = 0;
    while v >= 1024.0 && i < UNITS.len() - 1 {
        v /= 1024.0;
        i += 1;
    }
    if i == 0 {
        format!("{b} B")
    } else {
        format!("{v:.1} {}", UNITS[i])
    }
}
