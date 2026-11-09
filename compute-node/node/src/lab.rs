//! `marginalia-node lab`: runs a fixed list of tasks against one model on this machine and writes down what came back and how long it
//! took. It is how a candidate model is compared with the ones in use on the same input, with no phone and nothing on the network.
//!
//! The tasks are a JSON Lines file, one task to a line:
//! `{"id": "photo-1", "system": "...", "user": "...", "media": ["board.png"], "max_tokens": 300, "temperature": 0.2}`
//! `media` takes photos, audio files and, for a model that can watch them with `ffmpeg` in PATH, videos.

use crate::engine::llm::{prompt_style, render_prompt, ChatMessage, PromptStyle, Role};
use anyhow::{anyhow, Context, Result};
use llama_cpp_2::context::params::LlamaContextParams;
use llama_cpp_2::llama_backend::LlamaBackend;
use llama_cpp_2::llama_batch::LlamaBatch;
use llama_cpp_2::model::params::LlamaModelParams;
use llama_cpp_2::model::LlamaModel;
use llama_cpp_2::mtmd::{mtmd_default_marker, MtmdBitmap, MtmdContext, MtmdContextParams, MtmdInputText};
use llama_cpp_2::sampling::LlamaSampler;
use serde::Deserialize;
use std::io::Write;
use std::num::NonZeroU32;
use std::path::{Path, PathBuf};
use std::time::Instant;

#[derive(Deserialize)]
struct Task {
    id: String,
    #[serde(default)]
    system: Option<String>,
    user: String,
    #[serde(default)]
    media: Vec<PathBuf>,
    #[serde(default = "default_max_tokens")]
    max_tokens: usize,
    #[serde(default = "default_temperature")]
    temperature: f32,
}

fn default_max_tokens() -> usize {
    512
}

fn default_temperature() -> f32 {
    0.2
}

/// The user's words with one media marker for each file that has none written in them. Markers come first, as the models expect.
fn with_markers(user: &str, media: usize) -> String {
    let marker = mtmd_default_marker();
    let missing = media.saturating_sub(user.matches(marker).count());
    format!("{}{user}", marker.repeat(missing))
}

pub fn run(model_path: &Path, mmproj: Option<&Path>, tasks_path: &Path, out_path: &Path, n_ctx: u32, gpu_layers: u32) -> Result<()> {
    let backend = LlamaBackend::init().map_err(|e| anyhow!("llama backend: {e}"))?;
    let started = Instant::now();
    let params = LlamaModelParams::default().with_n_gpu_layers(gpu_layers);
    let model = LlamaModel::load_from_file(&backend, model_path, &params).with_context(|| format!("loading {}", model_path.display()))?;
    let architecture = model.meta_val_str("general.architecture").unwrap_or_default();
    let style = prompt_style(&architecture);
    let threads = std::thread::available_parallelism().map(|n| n.get() as i32).unwrap_or(4).clamp(1, 8);
    let ctx_params = LlamaContextParams::default().with_n_ctx(NonZeroU32::new(n_ctx)).with_n_batch(1024).with_n_ubatch(1024).with_n_threads(threads).with_n_threads_batch(threads);
    let mut ctx = model.new_context(&backend, ctx_params).map_err(|e| anyhow!("context: {e}"))?;
    let mut mtmd = match mmproj {
        Some(p) => {
            let mut m = MtmdContextParams::default();
            m.use_gpu = gpu_layers > 0;
            m.n_threads = threads;
            Some(MtmdContext::init_from_file(&p.to_string_lossy(), &model, &m).map_err(|e| anyhow!("loading the projector: {e}"))?)
        }
        None => None,
    };
    println!("ready: {architecture} in {:.1} s", started.elapsed().as_secs_f32());
    let text = std::fs::read_to_string(tasks_path).with_context(|| format!("reading {}", tasks_path.display()))?;
    let mut out = std::fs::File::create(out_path).with_context(|| format!("writing {}", out_path.display()))?;
    for line in text.lines().filter(|l| !l.trim().is_empty()) {
        let task: Task = serde_json::from_str(line).with_context(|| format!("not a task: {line}"))?;
        let t0 = Instant::now();
        let result = one(&model, &mut ctx, mtmd.as_mut(), style, &task);
        let v = match result {
            Ok(r) => serde_json::json!({"id": task.id, "text": r.text, "tokens": r.tokens, "prompt_tokens": r.prompt_tokens, "prompt_s": r.prompt_s, "gen_s": r.gen_s, "tokens_per_s": r.tokens as f32 / r.gen_s.max(0.001)}),
            Err(e) => serde_json::json!({"id": task.id, "error": format!("{e:#}"), "seconds": t0.elapsed().as_secs_f32()}),
        };
        writeln!(out, "{v}")?;
        out.flush()?;
        println!("{} {:.1} s", task.id, t0.elapsed().as_secs_f32());
    }
    Ok(())
}

struct Reply {
    text: String,
    tokens: usize,
    prompt_tokens: usize,
    prompt_s: f32,
    gen_s: f32,
}

fn one(model: &LlamaModel, ctx: &mut llama_cpp_2::context::LlamaContext<'_>, mtmd: Option<&mut MtmdContext>, style: PromptStyle, task: &Task) -> Result<Reply> {
    let mut messages = Vec::new();
    if let Some(s) = &task.system {
        messages.push(ChatMessage { role: Role::System, content: s.clone() });
    }
    messages.push(ChatMessage { role: Role::User, content: with_markers(&task.user, task.media.len()) });
    let prompt = render_prompt(style, &messages);
    ctx.clear_kv_cache();
    let vocab = model.vocab();
    let n_batch = ctx.n_batch() as usize;
    let mut batch = LlamaBatch::new(n_batch.max(512), 1);
    let t0 = Instant::now();
    let mut pos: i32 = 0;
    let prompt_tokens;
    if task.media.is_empty() {
        let tokens = vocab.tokenize(prompt.as_bytes(), false, true);
        prompt_tokens = tokens.len();
        if tokens.len() + 16 > ctx.n_ctx() as usize {
            return Err(anyhow!("the prompt is {} tokens, longer than the context", tokens.len()));
        }
        for chunk in tokens.chunks(n_batch) {
            batch.clear();
            for (k, t) in chunk.iter().enumerate() {
                let last = pos as usize + k == tokens.len() - 1;
                batch.add(*t, pos + k as i32, &[0], last).map_err(|e| anyhow!("batch: {e}"))?;
            }
            ctx.decode(&mut batch).map_err(|e| anyhow!("decode: {e}"))?;
            pos += chunk.len() as i32;
        }
    } else {
        let mtmd = mtmd.ok_or_else(|| anyhow!("this task has media and no projector was given"))?;
        let mut bitmaps = Vec::with_capacity(task.media.len());
        for m in &task.media {
            bitmaps.push(MtmdBitmap::from_file(mtmd, &m.to_string_lossy(), false).map_err(|e| anyhow!("reading {}: {e}", m.display()))?);
        }
        let refs: Vec<&MtmdBitmap> = bitmaps.iter().collect();
        let chunks = mtmd.tokenize(MtmdInputText { text: prompt.clone(), add_special: false, parse_special: true }, &refs).map_err(|e| anyhow!("tokenize: {e}"))?;
        pos = chunks.eval_chunks(mtmd, ctx, 0, 0, n_batch as i32, true).map_err(|e| anyhow!("eval: {e}"))?;
        prompt_tokens = pos as usize;
    }
    let prompt_s = t0.elapsed().as_secs_f32();
    let t1 = Instant::now();
    let mut sampler = LlamaSampler::chain_simple([
        LlamaSampler::penalties(vocab.n_tokens(), 64, 1.05, 0.0, 0.0),
        LlamaSampler::top_k(64),
        LlamaSampler::top_p(0.95, 1),
        LlamaSampler::temp(task.temperature.max(0.0)),
        if task.temperature <= 0.01 { LlamaSampler::greedy() } else { LlamaSampler::dist(1234) },
    ]);
    let (mut bytes, mut n): (Vec<u8>, usize) = (Vec::new(), 0);
    while n < task.max_tokens && (pos as usize) < ctx.n_ctx() as usize - 1 {
        let token = sampler.sample(ctx, -1);
        sampler.accept(token);
        if vocab.is_eog(token) {
            break;
        }
        let piece = vocab.token_to_piece(token, false, None);
        if piece == b"<turn|>" || piece == b"<|turn>" {
            break;
        }
        bytes.extend_from_slice(&piece);
        n += 1;
        batch.clear();
        batch.add(token, pos, &[0], true).map_err(|e| anyhow!("batch: {e}"))?;
        pos += 1;
        ctx.decode(&mut batch).map_err(|e| anyhow!("decode: {e}"))?;
    }
    Ok(Reply { text: String::from_utf8_lossy(&bytes).into_owned(), tokens: n, prompt_tokens, prompt_s, gen_s: t1.elapsed().as_secs_f32() })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_file_gets_a_marker_unless_the_words_already_place_it() {
        let m = mtmd_default_marker();
        assert_eq!(with_markers("Read this.", 1), format!("{m}Read this."));
        assert_eq!(with_markers("Read this.", 2), format!("{m}{m}Read this."));
        assert_eq!(with_markers(&format!("Before {m} after"), 1), format!("Before {m} after"));
        assert_eq!(with_markers("Plain words.", 0), "Plain words.");
    }
}
