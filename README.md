# Marginalia

Notes in the margins of your meetings. A local-first meeting companion for the desktop, written in
Rust with [GPUI](https://www.gpui.rs/). It records your meetings, expands shorthand notes while people
are still talking, surfaces answers from your own documents with exact citations, and lets you ask
questions across everything afterwards. Nothing leaves your machine: no Python, no helper processes,
no cloud.

## What it does

- **Transcription** — microphone *and* system audio (PipeWire monitor through the ALSA plugin, so any
  meeting app or browser tab works), voice-activity segmented, transcribed in-process by whisper.cpp
  (`large-v3-turbo` on GPU, `small` on CPU), tagged *You* / *Others*, merged into readable sentences.
- **Live Enhance** — type fragments; Gemma 4 rewrites them into complete notes from the live transcript,
  in *Standard* or *Concise* style, and keeps them current.
- **Live Assistance** — questions raised in the conversation are detected (heuristics + Gemma), answered
  from your knowledge base, and shown as context cards with the exact passage and locator.
- **Knowledge base** — PDF, Word, PowerPoint, Excel, OpenDocument, EPUB, RTF, Markdown, text, HTML,
  subtitles, code, web links, **images** (captioned by Gemma 4 vision) and **audio files** (transcribed).
  Everything is parsed in Rust.
- **Ask** — retrieval-augmented chat over documents and past meetings with numbered citations and
  expandable sources. Prompts are token-budgeted so long chats never overflow the context window.
- **Summaries** — when a recording stops: Summary / Key decisions / Action items / Open questions, and
  the meeting itself becomes searchable.
- **Two themes** — Graphite (dark) and Paper (warm gallery neutrals, serif display face).

## Retrieval

Built to stay fast and precise as the corpus grows:

| Stage | What happens |
| --- | --- |
| Ingest | structure-aware extraction → heading path per block → sentence packing to ~220 tokens with sentence overlap → `title › section \| text` contextual embeddings, batched 32 sequences per llama batch |
| Routing | every section keeps a running centroid; a query scores sections first, then scores every chunk of the best sections exactly, unioned with the global top-k (Matryoshka 256-d prefilter above 4k chunks) |
| Lexical | SQLite FTS5 (porter + unicode61) BM25 over text, section and title |
| Fusion | Reciprocal Rank Fusion plus an evidence bonus for a decisive keyword hit or a very close vector |
| Late rescoring | the sentences of the top candidates are embedded in one batch; the best sentence sharpens the chunk score (MaxSim-style) and becomes the **quoted citation** |
| Diversity | MMR with a same-document penalty, then neighbour chunks are appended for the LLM |

A query over a few thousand passages takes about 50 ms end to end on a desktop GPU.

## Models (downloaded on first launch into `~/.local/share/marginalia/models`)

| Role | Model | Size |
| --- | --- | --- |
| Embeddings | `ggml-org/embeddinggemma-2-GGUF` · Q8_0 (+ vision projector) | 310 MB + 555 MB |
| Chat / notes / captions | `ggml-org/gemma-4-E2B-it-GGUF` · Q4_0 (+ vision projector), or E4B | 2.8 GB + 557 MB |
| Speech | `ggerganov/whisper.cpp` · `large-v3-turbo` q5_0 (default) or `small` q8_0 | 574 MB / 264 MB |

All repositories are public. `HF_TOKEN` in the environment is honoured for gated repos.

Models load with fallback tiers (full GPU offload → partial → CPU) and the chat model is moved to CPU
automatically if a GPU generation ever fails at runtime; a notification tells you when that happens.

## Build & run

Requirements: Rust 1.85+, a C/C++ toolchain, CMake, and for the default GPU build the CUDA toolkit
(tested with an RTX 4060, CUDA 13.2). On Linux: `libxkbcommon-dev`, `libwayland-dev`, `libvulkan-dev`,
`libfontconfig-dev`, `libasound2-dev`, PipeWire with `pipewire-alsa`, and GTK 3 dev headers for the file
picker. Fonts used if installed: Inter, Inter Display, EB Garamond, DejaVu Sans Mono.

```bash
cargo run --release                        # CUDA build (default feature)
cargo run --release --no-default-features  # CPU-only
```

`.cargo/config.toml` pins `CUDAARCHS=89` (Ada); change it for other GPUs.

### Headless mode

```bash
marginalia ingest <paths|urls…>   # add to the knowledge base
marginalia search <query>         # hybrid retrieval with scores and quotes
marginalia ask <question>         # cited answer
marginalia caption <image>        # Gemma 4 vision description
marginalia transcribe <audio>     # transcribe a file
marginalia listen <seconds>       # live mic + system capture
marginalia docs                   # list indexed documents
```

## How the native pieces fit

- `vendor/llama-cpp-sys-2` is the published crate with a current llama.cpp checkout (the crates.io
  bundle predates the `gemma-embedding2` architecture) and `tools/mtmd` for vision.
- `vendor/whisper.cpp` is compiled by `build.rs` against the same ggml that llama.cpp uses, so the
  process carries exactly one ggml and one CUDA context.
- `vendor/syslibs` holds a symlink used only when `libxkbcommon-x11-dev` is not installed.

## Layout

```
src/
  main.rs / cli.rs      app bootstrap, window, headless commands
  app.rs                services, event bus, model bootstrap, GPU fallbacks
  config.rs / models.rs settings, model catalog, resumable downloader
  engine/               llama.cpp backend: embed.rs (EmbeddingGemma 2 + vision), llm.rs (Gemma 4,
                        streaming, priority queues, images), whisper.rs (+ whisper_sys.rs bindings)
  audio/                cpal capture (mic + PipeWire sink monitor), decode.rs (symphonia), VAD
  rag/                  extract.rs, chunk.rs, store.rs (SQLite + FTS5), vector.rs (routed index),
                        mod.rs (ingest, hybrid search, late rescoring)
  ui/                   GPUI views: meeting (list + live + review), knowledge, chat, stats, settings;
                        theme.rs (Graphite / Paper), components.rs, anim.rs
```

Data lives in `~/.local/share/marginalia/` (`marginalia.sqlite3`, `settings.json`, `models/`);
override the root with `MARGINALIA_DATA=/path`.
