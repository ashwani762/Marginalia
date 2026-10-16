//! Structure- and sentence-aware chunking with real token budgets.
//!
//! Blocks are grouped under their heading path, sentences are packed greedily
//! to a target token budget with a sentence-level overlap, tiny tail chunks are
//! merged backwards, and each chunk keeps its section path and page range so
//! citations can point at an exact place in the source.

use super::extract::{Block, Extracted};
use unicode_segmentation::UnicodeSegmentation;

#[derive(Clone, Debug)]
pub struct Chunk {
    pub ord: i64,
    pub section: String,
    pub page_start: u32,
    pub page_end: u32,
    pub text: String,
    pub token_count: usize,
}

pub struct ChunkConfig {
    pub target_tokens: usize,
    pub max_tokens: usize,
    pub overlap_tokens: usize,
    pub min_tokens: usize,
}

impl Default for ChunkConfig {
    fn default() -> Self {
        Self { target_tokens: 220, max_tokens: 360, overlap_tokens: 40, min_tokens: 48 }
    }
}

impl ChunkConfig {
    pub fn with_target(target: usize) -> Self {
        let target = target.clamp(96, 512);
        Self {
            target_tokens: target,
            max_tokens: (target as f32 * 1.6) as usize,
            overlap_tokens: (target / 5).max(24),
            min_tokens: (target / 4).max(32),
        }
    }
}

struct Unit {
    text: String,
    tokens: usize,
    page: u32,
}

/// `count_tokens` must return one count per input string (batched for speed).
pub fn chunk_document(
    doc: &Extracted,
    cfg: &ChunkConfig,
    count_tokens: &dyn Fn(Vec<String>) -> Vec<usize>,
) -> Vec<Chunk> {
    // 1. Build sections: (heading path, page, paragraphs).
    let mut heading_stack: Vec<(u8, String)> = Vec::new();
    let mut page: u32 = 0;
    struct Section {
        path: String,
        units: Vec<(String, u32)>, // sentence-ish units with page
    }
    let mut sections: Vec<Section> = vec![Section { path: String::new(), units: vec![] }];
    for b in &doc.blocks {
        match b {
            Block::Page(p) => page = *p,
            Block::Heading { level, text } => {
                while heading_stack.last().map(|(l, _)| *l >= *level).unwrap_or(false) {
                    heading_stack.pop();
                }
                heading_stack.push((*level, text.clone()));
                let path = heading_stack.iter().map(|(_, t)| t.as_str()).collect::<Vec<_>>().join(" › ");
                sections.push(Section { path, units: vec![] });
            }
            Block::Para(text) => {
                let sec = sections.last_mut().unwrap();
                for s in sentences(text) {
                    sec.units.push((s, page));
                }
            }
        }
    }

    // 2. Token counts for all units in one batched call.
    let all_texts: Vec<String> = sections.iter().flat_map(|s| s.units.iter().map(|(t, _)| t.clone())).collect();
    if all_texts.is_empty() {
        return vec![];
    }
    let counts = count_tokens(all_texts.clone());
    let counts = if counts.len() == all_texts.len() {
        counts
    } else {
        all_texts.iter().map(|t| t.len() / 4 + 1).collect()
    };

    // 3. Greedy sentence packing per section with overlap.
    let mut chunks: Vec<Chunk> = Vec::new();
    let mut idx = 0usize;
    for sec in &sections {
        let units: Vec<Unit> = sec
            .units
            .iter()
            .map(|(t, p)| {
                let u = Unit { text: t.clone(), tokens: counts[idx].max(1), page: *p };
                idx += 1;
                u
            })
            .collect();
        if units.is_empty() {
            continue;
        }
        let mut i = 0usize;
        while i < units.len() {
            let mut cur: Vec<&Unit> = Vec::new();
            let mut tokens = 0usize;
            let mut j = i;
            while j < units.len() {
                let u = &units[j];
                let fits_target = tokens + u.tokens <= cfg.target_tokens;
                let fits_max = tokens + u.tokens <= cfg.max_tokens;
                if cur.is_empty() || fits_target || (fits_max && tokens < cfg.min_tokens) {
                    cur.push(u);
                    tokens += u.tokens;
                    j += 1;
                    if tokens >= cfg.target_tokens {
                        break;
                    }
                } else {
                    break;
                }
            }
            // Very long single sentence: hard split by words.
            if cur.len() == 1 && cur[0].tokens > cfg.max_tokens {
                let u = cur[0];
                let words: Vec<&str> = u.text.split_whitespace().collect();
                let per = (words.len() * cfg.target_tokens / u.tokens.max(1)).max(20);
                for w in words.chunks(per) {
                    let text = w.join(" ");
                    let t = text.len() / 4 + 1;
                    chunks.push(Chunk {
                        ord: chunks.len() as i64,
                        section: sec.path.clone(),
                        page_start: u.page,
                        page_end: u.page,
                        text,
                        token_count: t,
                    });
                }
                i = j;
                continue;
            }
            let text = cur.iter().map(|u| u.text.as_str()).collect::<Vec<_>>().join(" ");
            chunks.push(Chunk {
                ord: chunks.len() as i64,
                section: sec.path.clone(),
                page_start: cur.first().map(|u| u.page).unwrap_or(0),
                page_end: cur.last().map(|u| u.page).unwrap_or(0),
                text,
                token_count: tokens,
            });
            if j >= units.len() {
                break;
            }
            // Overlap: step back over trailing sentences up to overlap budget.
            let mut back = 0usize;
            let mut k = j;
            while k > i + 1 && back + units[k - 1].tokens <= cfg.overlap_tokens {
                back += units[k - 1].tokens;
                k -= 1;
            }
            i = k.max(i + 1);
        }
    }

    // 4. Merge tiny tails into their predecessor when same section.
    let mut merged: Vec<Chunk> = Vec::with_capacity(chunks.len());
    for c in chunks {
        if let Some(prev) = merged.last_mut() {
            if c.token_count < cfg.min_tokens && prev.section == c.section && prev.token_count + c.token_count <= cfg.max_tokens {
                prev.text.push(' ');
                prev.text.push_str(&c.text);
                prev.token_count += c.token_count;
                prev.page_end = c.page_end.max(prev.page_end);
                continue;
            }
        }
        merged.push(c);
    }
    for (i, c) in merged.iter_mut().enumerate() {
        c.ord = i as i64;
    }
    merged
}

/// Sentence segmentation that keeps code/tables intact and splits long list items.
pub fn sentences(text: &str) -> Vec<String> {
    if text.contains("```") || text.lines().count() > 3 && text.contains(" | ") {
        return vec![text.to_string()];
    }
    let mut out = Vec::new();
    for line in text.split('\n') {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        for s in line.unicode_sentences() {
            let s = s.trim();
            if !s.is_empty() {
                out.push(s.to_string());
            }
        }
    }
    out
}

/// Chunk a meeting transcript by time windows so citations carry timestamps.
pub fn chunk_transcript(segments: &[(String, f64, f64, String)], target_chars: usize) -> Vec<(String, f64, f64)> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut t0 = 0.0;
    let mut t1 = 0.0;
    for (speaker, s0, s1, text) in segments {
        if cur.is_empty() {
            t0 = *s0;
        }
        cur.push_str(&format!("[{}] {}: {}\n", fmt_ts(*s0), speaker, text));
        t1 = *s1;
        if cur.len() >= target_chars {
            out.push((cur.trim().to_string(), t0, t1));
            cur.clear();
        }
    }
    if !cur.trim().is_empty() {
        out.push((cur.trim().to_string(), t0, t1));
    }
    out
}

pub fn fmt_ts(secs: f64) -> String {
    let s = secs.max(0.0) as u64;
    format!("{:02}:{:02}", s / 60, s % 60)
}
