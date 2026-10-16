//! Flat, exact vector index with a Matryoshka coarse pre-filter.
//!
//! For a personal knowledge base (tens of thousands of chunks) an exact scan
//! with a 256-d prefix pass plus a 768-d rerank is both faster to build and
//! more accurate than an approximate graph index, and it needs no tuning.

use crate::engine::embed::{COARSE_DIM, EMBED_DIM};
use rayon::prelude::*;
use std::collections::HashMap;

pub struct VectorIndex {
    ids: Vec<i64>,
    full: Vec<f32>,
    coarse: Vec<f32>,
    pos: HashMap<i64, usize>,
    /// Section group of each row (hash of document + heading path).
    group_of: Vec<u64>,
    /// Running centroid per section: (sum vector, count).
    groups: HashMap<u64, (Vec<f32>, usize)>,
}

impl Default for VectorIndex {
    fn default() -> Self {
        Self::new()
    }
}

impl VectorIndex {
    pub fn new() -> Self {
        Self { ids: Vec::new(), full: Vec::new(), coarse: Vec::new(), pos: HashMap::new(), group_of: Vec::new(), groups: HashMap::new() }
    }

    pub fn group_count(&self) -> usize {
        self.groups.len()
    }

    pub fn len(&self) -> usize {
        self.ids.len()
    }
    pub fn is_empty(&self) -> bool {
        self.ids.is_empty()
    }

    pub fn add(&mut self, id: i64, v: &[f32]) {
        self.add_grouped(id, v, 0);
    }

    /// Add a vector that belongs to section `group` (0 = ungrouped).
    pub fn add_grouped(&mut self, id: i64, v: &[f32], group: u64) {
        if v.len() != EMBED_DIM {
            return;
        }
        if let Some(&row) = self.pos.get(&id) {
            self.full[row * EMBED_DIM..(row + 1) * EMBED_DIM].copy_from_slice(v);
            let c = coarse_of(v);
            self.coarse[row * COARSE_DIM..(row + 1) * COARSE_DIM].copy_from_slice(&c);
            return;
        }
        self.pos.insert(id, self.ids.len());
        self.ids.push(id);
        self.full.extend_from_slice(v);
        self.coarse.extend_from_slice(&coarse_of(v));
        self.group_of.push(group);
        if group != 0 {
            let e = self.groups.entry(group).or_insert_with(|| (vec![0.0; EMBED_DIM], 0));
            for (a, b) in e.0.iter_mut().zip(v) {
                *a += b;
            }
            e.1 += 1;
        }
    }

    pub fn remove_many(&mut self, ids: &[i64]) {
        if ids.is_empty() {
            return;
        }
        let drop: std::collections::HashSet<i64> = ids.iter().copied().collect();
        let mut new = VectorIndex::new();
        for (row, id) in self.ids.iter().enumerate() {
            if drop.contains(id) {
                continue;
            }
            new.add_grouped(*id, &self.full[row * EMBED_DIM..(row + 1) * EMBED_DIM], self.group_of[row]);
        }
        *self = new;
    }

    pub fn get(&self, id: i64) -> Option<&[f32]> {
        self.pos.get(&id).map(|&row| &self.full[row * EMBED_DIM..(row + 1) * EMBED_DIM])
    }

    /// Hierarchical search: route through section centroids first, then score
    /// every chunk of the best sections exactly, and union with the global
    /// top-k so nothing strong is missed. Scales to very large documents because
    /// the fine pass only touches a handful of sections.
    pub fn search_routed(&self, query: &[f32], k: usize, top_sections: usize) -> Vec<(i64, f32)> {
        let mut best = self.search(query, k);
        if self.groups.len() < 4 {
            return best;
        }
        let mut sec: Vec<(u64, f32)> = self
            .groups
            .iter()
            .map(|(g, (sum, n))| {
                let inv = 1.0 / (*n as f32).max(1.0);
                let mut d = 0.0f32;
                let mut norm = 0.0f32;
                for (a, q) in sum.iter().zip(query) {
                    let c = a * inv;
                    d += c * q;
                    norm += c * c;
                }
                (*g, d / norm.sqrt().max(1e-6))
            })
            .collect();
        sec.sort_unstable_by(|a, b| b.1.total_cmp(&a.1));
        sec.truncate(top_sections);
        let wanted: std::collections::HashSet<u64> = sec.into_iter().map(|(g, _)| g).collect();
        let seen: std::collections::HashSet<i64> = best.iter().map(|(id, _)| *id).collect();
        let extra: Vec<(i64, f32)> = (0..self.ids.len())
            .into_par_iter()
            .filter(|row| wanted.contains(&self.group_of[*row]) && !seen.contains(&self.ids[*row]))
            .map(|row| (self.ids[row], dot(&self.full[row * EMBED_DIM..(row + 1) * EMBED_DIM], query)))
            .collect();
        best.extend(extra);
        best.sort_unstable_by(|a, b| b.1.total_cmp(&a.1));
        best.truncate(k);
        best
    }

    /// Top-k by cosine similarity (inputs are unit vectors).
    pub fn search(&self, query: &[f32], k: usize) -> Vec<(i64, f32)> {
        let n = self.ids.len();
        if n == 0 || query.len() != EMBED_DIM || k == 0 {
            return vec![];
        }
        let candidates: Vec<usize> = if n > 4096 {
            // Coarse pass on the 256-d Matryoshka prefix.
            let qc = coarse_of(query);
            let mut scored: Vec<(usize, f32)> = (0..n)
                .into_par_iter()
                .map(|row| (row, dot(&self.coarse[row * COARSE_DIM..(row + 1) * COARSE_DIM], &qc)))
                .collect();
            let keep = (k * 8).max(256).min(n);
            scored.select_nth_unstable_by(keep - 1, |a, b| b.1.total_cmp(&a.1));
            scored.truncate(keep);
            scored.into_iter().map(|(r, _)| r).collect()
        } else {
            (0..n).collect()
        };
        let mut exact: Vec<(i64, f32)> = candidates
            .into_par_iter()
            .map(|row| (self.ids[row], dot(&self.full[row * EMBED_DIM..(row + 1) * EMBED_DIM], query)))
            .collect();
        exact.sort_unstable_by(|a, b| b.1.total_cmp(&a.1));
        exact.truncate(k);
        exact
    }
}

fn coarse_of(v: &[f32]) -> Vec<f32> {
    let prefix = &v[..COARSE_DIM.min(v.len())];
    let norm = prefix.iter().map(|x| x * x).sum::<f32>().sqrt().max(f32::EPSILON);
    prefix.iter().map(|x| x / norm).collect()
}

#[inline]
pub fn dot(a: &[f32], b: &[f32]) -> f32 {
    let mut acc = [0.0f32; 8];
    let chunks = a.len() / 8;
    for i in 0..chunks {
        let o = i * 8;
        for j in 0..8 {
            acc[j] += a[o + j] * b[o + j];
        }
    }
    let mut s: f32 = acc.iter().sum();
    for i in chunks * 8..a.len() {
        s += a[i] * b[i];
    }
    s
}
