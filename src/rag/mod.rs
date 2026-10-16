//! Knowledge base: ingestion pipeline and hybrid retrieval.
//!
//! Retrieval = dense (EmbeddingGemma 2, exact cosine with Matryoshka prefilter)
//! ∪ lexical (SQLite FTS5 BM25) → Reciprocal Rank Fusion → MMR diversification
//! → neighbor expansion for context. Every hit carries an exact citation.

pub mod chunk;
pub mod extract;
pub mod store;
pub mod vector;

use crate::engine::embed::{format_document, EmbedHandle, EmbedKind};
use crate::engine::llm::{image_marker, ChatMessage, GenParams, LlmHandle, Priority};
use crate::engine::WhisperHandle;
use anyhow::{anyhow, Context, Result};
use crossbeam_channel::{unbounded, Sender};
use parking_lot::RwLock;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;
use store::{DocKind, Document, NewChunk, Store};
use vector::VectorIndex;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Citation {
    pub quote: String,
    pub chunk_id: i64,
    pub doc_id: i64,
    pub title: String,
    pub kind: String,
    pub section: String,
    pub page_start: i64,
    pub page_end: i64,
    pub source: String,
    pub snippet: String,
}

#[derive(Clone, Debug)]
pub struct Hit {
    pub chunk_id: i64,
    pub doc_id: i64,
    pub title: String,
    pub kind: DocKind,
    pub section: String,
    pub page_start: i64,
    pub page_end: i64,
    pub source: String,
    pub text: String,
    /// Expanded text with neighboring chunks for the LLM.
    pub context: String,
    /// The single sentence that best matches the query (exact citation).
    pub quote: String,
    pub score: f32,
    pub dense: f32,
    pub lexical: f32,
}

impl Hit {
    pub fn citation(&self) -> Citation {
        Citation {
            quote: self.quote.clone(),
            chunk_id: self.chunk_id,
            doc_id: self.doc_id,
            title: self.title.clone(),
            kind: self.kind.as_str().to_string(),
            section: self.section.clone(),
            page_start: self.page_start,
            page_end: self.page_end,
            source: self.source.clone(),
            snippet: snippet_of(&self.text, 220),
        }
    }
    pub fn locator(&self) -> String {
        let mut s = String::new();
        if self.kind == DocKind::Meeting {
            if !self.section.is_empty() {
                s.push_str(&self.section);
            }
        } else {
            if self.page_start > 0 {
                if self.page_end > self.page_start {
                    s.push_str(&format!("p. {}–{}", self.page_start, self.page_end));
                } else {
                    s.push_str(&format!("p. {}", self.page_start));
                }
            }
            if !self.section.is_empty() {
                if !s.is_empty() {
                    s.push_str(" · ");
                }
                s.push_str(&self.section);
            }
        }
        s
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Scope {
    All,
    Documents,
    Meetings,
}

#[derive(Clone, Debug)]
pub enum IngestStage {
    Queued,
    Extracting,
    Chunking,
    Embedding { done: usize, total: usize },
    Done { doc_id: i64, title: String, chunks: usize },
    Skipped { reason: String },
    Failed { error: String },
}

#[derive(Clone, Debug)]
pub struct IngestEvent {
    pub job_id: u64,
    pub label: String,
    pub stage: IngestStage,
}

#[derive(Clone, Debug)]
pub enum IngestInput {
    File(PathBuf),
    Url(String),
}

struct IngestJob {
    id: u64,
    input: IngestInput,
}

pub struct Kb {
    pub store: Arc<Store>,
    index: RwLock<VectorIndex>,
    embed: RwLock<Option<EmbedHandle>>,
    llm: RwLock<Option<LlmHandle>>,
    whisper: RwLock<Option<WhisperHandle>>,
    ingest_tx: RwLock<Option<Sender<IngestJob>>>,
    chunk_tokens: RwLock<usize>,
}

impl Kb {
    pub fn open(db_path: &Path) -> Result<Arc<Self>> {
        let store = Arc::new(Store::open(db_path)?);
        let mut index = VectorIndex::new();
        let started = Instant::now();
        for (id, doc, section, v) in store.all_embeddings()? {
            index.add_grouped(id, &v, section_group(doc, &section));
        }
        tracing::info!("vector index loaded: {} chunks, {} sections in {:?}", index.len(), index.group_count(), started.elapsed());
        Ok(Arc::new(Self {
            store,
            index: RwLock::new(index),
            embed: RwLock::new(None),
            llm: RwLock::new(None),
            whisper: RwLock::new(None),
            ingest_tx: RwLock::new(None),
            chunk_tokens: RwLock::new(220),
        }))
    }

    pub fn attach_llm(&self, llm: Option<LlmHandle>) {
        *self.llm.write() = llm;
    }

    pub fn attach_whisper(&self, w: Option<WhisperHandle>) {
        *self.whisper.write() = w;
    }

    pub fn set_chunk_tokens(&self, n: usize) {
        *self.chunk_tokens.write() = n;
    }

    pub fn chunk_count(&self) -> usize {
        self.index.read().len()
    }

    /// Attach the embedding worker and start the ingest thread.
    pub fn attach_embedder(self: &Arc<Self>, embed: EmbedHandle, events: Sender<IngestEvent>) {
        *self.embed.write() = Some(embed);
        let (tx, rx) = unbounded::<IngestJob>();
        *self.ingest_tx.write() = Some(tx);
        let kb = self.clone();
        std::thread::Builder::new()
            .name("ingest".into())
            .spawn(move || {
                while let Ok(job) = rx.recv() {
                    let label = match &job.input {
                        IngestInput::File(p) => p.file_name().and_then(|s| s.to_str()).unwrap_or("file").to_string(),
                        IngestInput::Url(u) => u.clone(),
                    };
                    let emit = |stage: IngestStage| {
                        let _ = events.send(IngestEvent { job_id: job.id, label: label.clone(), stage });
                    };
                    emit(IngestStage::Extracting);
                    let res = kb.ingest_blocking(&job.input, &emit);
                    match res {
                        Ok(Some((doc_id, title, chunks))) => emit(IngestStage::Done { doc_id, title, chunks }),
                        Ok(None) => emit(IngestStage::Skipped { reason: "already in the knowledge base".into() }),
                        Err(e) => emit(IngestStage::Failed { error: format!("{e:#}") }),
                    }
                }
            })
            .expect("spawn ingest thread");
    }

    pub fn enqueue(&self, input: IngestInput) -> Option<u64> {
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
        let id = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let tx = self.ingest_tx.read().clone()?;
        tx.send(IngestJob { id, input }).ok()?;
        Some(id)
    }

    fn embedder(&self) -> Result<EmbedHandle> {
        self.embed.read().clone().ok_or_else(|| anyhow!("embedding model not loaded yet"))
    }

    fn ingest_blocking(
        &self,
        input: &IngestInput,
        emit: &dyn Fn(IngestStage),
    ) -> Result<Option<(i64, String, usize)>> {
        let embed = self.embedder()?;
        let (extracted, source, hash, bytes) = match input {
            IngestInput::File(path) => {
                let bytes = std::fs::read(path).with_context(|| format!("reading {}", path.display()))?;
                let hash = blake3::hash(&bytes).to_hex().to_string();
                if self.store.find_document_by_hash(&hash)?.is_some() {
                    return Ok(None);
                }
                let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
                if !extract::supported_extension(ext) {
                    anyhow::bail!("unsupported file type .{ext}");
                }
                if extract::is_image_extension(ext) {
                    return self.ingest_image(path, &hash, bytes.len() as u64, emit).map(Some);
                }
                let ex = if crate::audio::decode::supported_extension(ext) {
                    self.extract_audio(path, emit)?
                } else {
                    extract::extract_path(path)?
                };
                (ex, path.display().to_string(), hash, bytes.len() as u64)
            }
            IngestInput::Url(url) => {
                if let Some(existing) = self.store.find_document_by_source(url)? {
                    let _ = existing;
                    return Ok(None);
                }
                let ex = extract::fetch_url(url)?;
                let text: String = ex
                    .blocks
                    .iter()
                    .filter_map(|b| match b {
                        extract::Block::Para(t) | extract::Block::Heading { text: t, .. } => Some(t.as_str()),
                        _ => None,
                    })
                    .collect::<Vec<_>>()
                    .join("\n");
                let hash = blake3::hash(text.as_bytes()).to_hex().to_string();
                if self.store.find_document_by_hash(&hash)?.is_some() {
                    return Ok(None);
                }
                (ex, url.clone(), hash, text.len() as u64)
            }
        };
        if extracted.blocks.is_empty() {
            anyhow::bail!("no readable text found");
        }
        emit(IngestStage::Chunking);
        let cfg = chunk::ChunkConfig::with_target(*self.chunk_tokens.read());
        let embed_for_count = embed.clone();
        let chunks = chunk::chunk_document(&extracted, &cfg, &move |texts| embed_for_count.count_tokens(texts));
        if chunks.is_empty() {
            anyhow::bail!("no chunks produced");
        }
        let total = chunks.len();
        emit(IngestStage::Embedding { done: 0, total });
        let title = extracted.title.clone();
        let word_count = extracted.word_count() as i64;
        let doc_id = self.store.insert_document(&title, &source, extracted.kind, &hash, bytes, word_count)?;

        // Contextual embeddings: title + section path travel with every chunk.
        let mut done = 0usize;
        for batch in chunks.chunks(32) {
            let texts: Vec<String> = batch
                .iter()
                .map(|c| {
                    let ctx_title = if c.section.is_empty() { title.clone() } else { format!("{} › {}", title, c.section) };
                    format_document(&ctx_title, &c.text)
                })
                .collect();
            let vectors = embed.embed(texts, EmbedKind::Raw)?;
            let new: Vec<NewChunk> = batch
                .iter()
                .zip(vectors)
                .map(|(c, v)| NewChunk {
                    ord: c.ord,
                    section: c.section.clone(),
                    page_start: c.page_start as i64,
                    page_end: c.page_end as i64,
                    text: c.text.clone(),
                    token_count: c.token_count as i64,
                    embedding: v,
                })
                .collect();
            let ids = self.store.insert_chunks(doc_id, &title, &new)?;
            {
                let mut idx = self.index.write();
                for (id, c) in ids.iter().zip(&new) {
                    idx.add_grouped(*id, &c.embedding, section_group(doc_id, &c.section));
                }
            }
            done += batch.len();
            emit(IngestStage::Embedding { done, total });
        }
        Ok(Some((doc_id, title, total)))
    }

    /// Images: captioned by Gemma 4 (vision projector) for lexical search and
    /// LLM context, embedded by EmbeddingGemma 2's vision tower so text queries
    /// retrieve them cross-modally.
    fn ingest_image(
        &self,
        path: &Path,
        hash: &str,
        bytes: u64,
        emit: &dyn Fn(IngestStage),
    ) -> Result<(i64, String, usize)> {
        let embed = self.embedder()?;
        let title = path.file_stem().and_then(|s| s.to_str()).unwrap_or("image").to_string();
        emit(IngestStage::Chunking);
        let caption = match self.llm.read().clone() {
            Some(llm) => {
                let prompt = format!(
                    "{}\nDescribe this image for a search index in 2-4 sentences: what it shows, any visible text, numbers, labels, names or diagram structure. Plain prose, no preamble.",
                    image_marker()
                );
                match llm.complete_with_images(
                    vec![ChatMessage::user(prompt)],
                    vec![path.to_path_buf()],
                    GenParams { max_tokens: 160, temperature: 0.2, ..Default::default() },
                    Priority::High,
                ) {
                    Ok(c) if !c.trim().is_empty() => c.trim().to_string(),
                    Ok(_) => String::new(),
                    Err(e) => {
                        tracing::warn!("caption failed for {}: {e:#}", path.display());
                        String::new()
                    }
                }
            }
            None => String::new(),
        };
        emit(IngestStage::Embedding { done: 0, total: 1 });
        let text = if caption.is_empty() { format!("Image: {title}") } else { caption.clone() };
        // The caption lives in the text embedding space, which is what queries use.
        // (A raw projector embedding is available via EmbedHandle::embed_image, but
        // mean-pooling only the vision tokens is not aligned with text queries.)
        let vector = embed.embed_one(&format_document(&title, &text), EmbedKind::Raw)?;
        let doc_id = self.store.insert_document(&title, &path.display().to_string(), DocKind::Image, hash, bytes, text.split_whitespace().count() as i64)?;
        let new = vec![NewChunk {
            ord: 0,
            section: String::new(),
            page_start: 0,
            page_end: 0,
            text,
            token_count: 0,
            embedding: vector,
        }];
        let ids = self.store.insert_chunks(doc_id, &title, &new)?;
        self.index.write().add_grouped(ids[0], &new[0].embedding, section_group(doc_id, ""));
        emit(IngestStage::Embedding { done: 1, total: 1 });
        Ok((doc_id, title, 1))
    }

    /// Audio files become timestamped transcripts (voice memos, recordings).
    fn extract_audio(&self, path: &Path, emit: &dyn Fn(IngestStage)) -> Result<extract::Extracted> {
        let whisper = self.whisper.read().clone().ok_or_else(|| anyhow!("transcription model not loaded yet"))?;
        emit(IngestStage::Extracting);
        let (pcm, duration) = crate::audio::decode::decode_to_16k_mono(path)?;
        if duration < 0.5 {
            anyhow::bail!("audio is too short");
        }
        // Transcribe in ~5 minute windows to bound memory; whisper internally chunks by 30 s.
        let window = 16_000 * 300;
        let mut utterances = Vec::new();
        for (i, part) in pcm.chunks(window).enumerate() {
            let offset = (i * window) as f64 / 16_000.0;
            for mut u in whisper.transcribe_buffer(part.to_vec())? {
                u.t0 += offset;
                u.t1 += offset;
                utterances.push(u);
            }
        }
        if utterances.is_empty() {
            anyhow::bail!("no speech recognized");
        }
        let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("audio").to_string();
        let mut blocks = Vec::new();
        let mut cur = String::new();
        let mut start = utterances[0].t0;
        for u in &utterances {
            if cur.is_empty() {
                start = u.t0;
            }
            cur.push_str(&u.text);
            cur.push(' ');
            if cur.len() > 700 || u.t0 - start > 90.0 {
                blocks.push(extract::Block::Heading { level: 3, text: format!("[{}]", chunk::fmt_ts(start)) });
                blocks.push(extract::Block::Para(cur.trim().to_string()));
                cur.clear();
            }
        }
        if !cur.trim().is_empty() {
            blocks.push(extract::Block::Heading { level: 3, text: format!("[{}]", chunk::fmt_ts(start)) });
            blocks.push(extract::Block::Para(cur.trim().to_string()));
        }
        Ok(extract::Extracted { title: stem, kind: DocKind::Audio, blocks })
    }

    /// Index a finished meeting (transcript windows + enhanced notes + summary).
    pub fn ingest_meeting(
        &self,
        meeting_id: i64,
        title: &str,
        segments: &[(String, f64, f64, String)],
        notes: &str,
        summary: &str,
    ) -> Result<usize> {
        let embed = self.embedder()?;
        let source = format!("meeting://{meeting_id}");
        if let Some(existing) = self.store.find_document_by_source(&source)? {
            let removed = self.store.delete_document(existing.id)?;
            self.index.write().remove_many(&removed);
        }
        let mut items: Vec<(String, String, i64, i64)> = Vec::new(); // (section, text, page_start, page_end)
        for (text, t0, t1) in chunk::chunk_transcript(segments, 900) {
            items.push((format!("{}–{}", chunk::fmt_ts(t0), chunk::fmt_ts(t1)), text, t0 as i64, t1 as i64));
        }
        if !summary.trim().is_empty() {
            items.push(("Summary".into(), summary.trim().to_string(), 0, 0));
        }
        if !notes.trim().is_empty() {
            items.push(("Notes".into(), notes.trim().to_string(), 0, 0));
        }
        if items.is_empty() {
            return Ok(0);
        }
        let hash = blake3::hash(format!("{meeting_id}:{}", items.len()).as_bytes()).to_hex().to_string();
        let words: i64 = items.iter().map(|(_, t, _, _)| t.split_whitespace().count() as i64).sum();
        let doc_id = self.store.insert_document(title, &source, DocKind::Meeting, &hash, 0, words)?;
        let texts: Vec<String> = items.iter().map(|(sec, t, _, _)| format_document(&format!("{title} › {sec}"), t)).collect();
        let vectors = embed.embed(texts, EmbedKind::Raw)?;
        let new: Vec<NewChunk> = items
            .iter()
            .zip(vectors)
            .enumerate()
            .map(|(i, ((sec, t, p0, p1), v))| NewChunk {
                ord: i as i64,
                section: sec.clone(),
                page_start: *p0,
                page_end: *p1,
                text: t.clone(),
                token_count: (t.len() / 4) as i64,
                embedding: v,
            })
            .collect();
        let ids = self.store.insert_chunks(doc_id, title, &new)?;
        let mut idx = self.index.write();
        for (id, c) in ids.iter().zip(&new) {
            idx.add_grouped(*id, &c.embedding, section_group(doc_id, &c.section));
        }
        Ok(new.len())
    }

    pub fn remove_document(&self, doc_id: i64) -> Result<()> {
        let removed = self.store.delete_document(doc_id)?;
        self.index.write().remove_many(&removed);
        Ok(())
    }

    pub fn documents(&self) -> Result<Vec<Document>> {
        self.store.list_documents()
    }

    /// Hybrid search. `k` final results; returns hits best-first.
    pub fn search(&self, query: &str, k: usize, scope: Scope) -> Result<Vec<Hit>> {
        let query = query.trim();
        if query.is_empty() || self.index.read().is_empty() {
            return Ok(vec![]);
        }
        let started = Instant::now();
        let embed = self.embedder()?;
        let qv = embed.embed_one(query, EmbedKind::Query)?;
        let pool = (k * 10).max(40);
        let dense = self.index.read().search_routed(&qv, pool, 12);
        let lexical = self.store.bm25(query, pool).unwrap_or_default();

        // Reciprocal rank fusion (dense weighted a little higher).
        const RRF_K: f32 = 60.0;
        let mut fused: HashMap<i64, (f32, f32, f32)> = HashMap::new(); // id -> (score, dense, lex)
        for (rank, (id, sim)) in dense.iter().enumerate() {
            let e = fused.entry(*id).or_default();
            e.0 += 1.1 / (RRF_K + rank as f32 + 1.0);
            e.1 = *sim;
        }
        let lex_max = lexical.first().map(|(_, s)| *s).unwrap_or(1.0).max(1e-3);
        for (rank, (id, s)) in lexical.iter().enumerate() {
            let e = fused.entry(*id).or_default();
            e.0 += 0.9 / (RRF_K + rank as f32 + 1.0);
            e.2 = s / lex_max;
        }
        // Beyond ranks, reward strong absolute evidence so a decisive keyword hit or a
        // very close vector is not buried by weak agreement from the other channel.
        for (_, (score, d, l)) in fused.iter_mut() {
            *score += 0.006 * *l + 0.006 * (*d - 0.45).max(0.0) * 2.5;
        }
        let mut ranked: Vec<(i64, f32, f32, f32)> = fused.into_iter().map(|(id, (s, d, l))| (id, s, d, l)).collect();
        ranked.sort_by(|a, b| b.1.total_cmp(&a.1));
        ranked.truncate(pool);

        let rows = self.store.chunks_by_ids(&ranked.iter().map(|r| r.0).collect::<Vec<_>>())?;
        let by_id: HashMap<i64, _> = rows.into_iter().map(|(c, d)| (c.id, (c, d))).collect();
        let mut cands: Vec<Hit> = Vec::new();
        for (id, score, d, l) in ranked {
            let Some((c, doc)) = by_id.get(&id) else { continue };
            let in_scope = match scope {
                Scope::All => true,
                Scope::Documents => doc.kind != DocKind::Meeting,
                Scope::Meetings => doc.kind == DocKind::Meeting,
            };
            if !in_scope {
                continue;
            }
            cands.push(Hit {
                chunk_id: c.id,
                doc_id: doc.id,
                title: doc.title.clone(),
                kind: doc.kind,
                section: c.section.clone(),
                page_start: c.page_start,
                page_end: c.page_end,
                source: doc.source.clone(),
                text: c.text.clone(),
                context: String::new(),
                quote: String::new(),
                score,
                dense: d,
                lexical: l,
            });
        }

        // Late sentence rescoring (MaxSim-style): embed the sentences of the top
        // candidates in one batch, let the best sentence sharpen the chunk score,
        // and keep it as the exact quote for the citation.
        let rerank_n = cands.len().min(k * 2 + 2);
        if rerank_n > 0 {
            let mut owners: Vec<(usize, String)> = Vec::new();
            let mut texts: Vec<String> = Vec::new();
            for (ci, h) in cands.iter().take(rerank_n).enumerate() {
                let mut sents = chunk::sentences(&h.text);
                sents.retain(|s| s.split_whitespace().count() >= 4);
                sents.truncate(6);
                for sent in sents {
                    texts.push(format_document(&h.title, &sent));
                    owners.push((ci, sent));
                }
            }
            if !texts.is_empty() {
                if let Ok(vecs) = embed.embed(texts, EmbedKind::Raw) {
                    let mut best: Vec<(f32, String)> = vec![(0.0, String::new()); rerank_n];
                    for ((ci, sent), v) in owners.into_iter().zip(vecs) {
                        let sim = vector::dot(&v, &qv);
                        if sim > best[ci].0 {
                            best[ci] = (sim, sent);
                        }
                    }
                    let max_score = cands.iter().take(rerank_n).map(|h| h.score).fold(0.0f32, f32::max).max(1e-6);
                    for (ci, (sim, sent)) in best.into_iter().enumerate() {
                        let h = &mut cands[ci];
                        if sim > 0.0 {
                            let rel = h.score / max_score;
                            let sharp = ((sim - 0.30) / 0.55).clamp(0.0, 1.0);
                            h.score = max_score * (0.6 * rel + 0.4 * sharp);
                            if sim >= 0.45 && sent.len() + 12 < h.text.len() {
                                h.quote = sent;
                            }
                        }
                    }
                    cands[..rerank_n].sort_by(|a, b| b.score.total_cmp(&a.score));
                }
            }
        }

        // MMR: trade relevance for diversity so we don't return 5 near-duplicates.
        let index = self.index.read();
        let mut selected: Vec<Hit> = Vec::new();
        let mut remaining = cands;
        let lambda = 0.72f32;
        let max_score = remaining.first().map(|h| h.score).unwrap_or(1.0).max(1e-6);
        while selected.len() < k && !remaining.is_empty() {
            let mut best_i = 0;
            let mut best = f32::MIN;
            for (i, h) in remaining.iter().enumerate() {
                let rel = h.score / max_score;
                let mut red = 0.0f32;
                if let Some(hv) = index.get(h.chunk_id) {
                    for s in &selected {
                        if let Some(sv) = index.get(s.chunk_id) {
                            red = red.max(vector::dot(hv, sv));
                        }
                    }
                }
                let same_doc_penalty = if selected.iter().filter(|s| s.doc_id == h.doc_id).count() >= 3 { 0.15 } else { 0.0 };
                let mmr = lambda * rel - (1.0 - lambda) * red - same_doc_penalty;
                if mmr > best {
                    best = mmr;
                    best_i = i;
                }
            }
            selected.push(remaining.remove(best_i));
        }
        drop(index);

        // Neighbor expansion for richer LLM context (not shown as the citation itself).
        let seen: HashSet<i64> = selected.iter().map(|h| h.chunk_id).collect();
        for h in &mut selected {
            let ord = by_id.get(&h.chunk_id).map(|(c, _)| c.ord).unwrap_or(0);
            let (prev, next) = self.store.neighbor_text(h.doc_id, ord).unwrap_or((None, None));
            let mut ctx = String::new();
            if let Some(p) = prev {
                if h.kind != DocKind::Meeting {
                    ctx.push_str(&tail_of(&p, 300));
                    ctx.push(' ');
                }
            }
            ctx.push_str(&h.text);
            if let Some(n) = next {
                if h.kind != DocKind::Meeting && !seen.contains(&(h.chunk_id + 1)) {
                    ctx.push(' ');
                    ctx.push_str(&head_of(&n, 300));
                }
            }
            h.context = ctx;
        }
        tracing::debug!("search '{}' -> {} hits in {:?}", snippet_of(query, 40), selected.len(), started.elapsed());
        Ok(selected)
    }

    pub fn embed_query(&self, text: &str) -> Result<Vec<f32>> {
        self.embedder()?.embed_one(text, EmbedKind::Query)
    }
}

/// Stable id for a (document, heading path) section.
pub fn section_group(doc_id: i64, section: &str) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    doc_id.hash(&mut h);
    section.hash(&mut h);
    h.finish() | 1
}

pub fn snippet_of(text: &str, max: usize) -> String {
    let t = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if t.chars().count() <= max {
        return t;
    }
    let mut s: String = t.chars().take(max).collect();
    if let Some(i) = s.rfind(' ') {
        s.truncate(i);
    }
    s.push('…');
    s
}

fn tail_of(text: &str, max: usize) -> String {
    let n = text.chars().count();
    if n <= max {
        return text.to_string();
    }
    let s: String = text.chars().skip(n - max).collect();
    match s.find(' ') {
        Some(i) => format!("…{}", &s[i + 1..]),
        None => s,
    }
}

fn head_of(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_string();
    }
    let mut s: String = text.chars().take(max).collect();
    if let Some(i) = s.rfind(' ') {
        s.truncate(i);
    }
    s.push('…');
    s
}
