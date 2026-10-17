# Marginalia

**Record a meeting, jot as it happens, and get the notes, a summary and answers from your own documents. Everything runs on the phone.**
Optional *compute nodes* on your network make the AI faster while you are near them, and the phone carries on by itself when they are not.

<p align="center">
  <img src="docs/images/meetings.png" width="24%" alt="Meetings list with a compute node banner">
  <img src="docs/images/live.png" width="24%" alt="Live recording with captions and jots">
  <img src="docs/images/review.png" width="24%" alt="Written notes and transcript">
  <img src="docs/images/ask.png" width="24%" alt="Ask with sources, answered on a compute node">
</p>

## What it does

- **Live captions** in many languages, on the phone (whisper.cpp). Nothing is uploaded.
- **Jots**: type a few words while people talk. The AI finds what was said around it and writes it up properly. Four kinds: note, action, decision, question.
- **Notes after the meeting**: a summary and clean notes, also for meetings of two hours. Notes are written once the meeting ends, so recording stays light.
- **Ask** your meetings and documents. Answers carry their sources. Add PDFs, pictures, web pages, text.
- **Photos** of a whiteboard are read and added to the meeting.
- **Speaker separation** after the meeting, with rename and merge. Tap a transcript line to hear it.
- **Compute nodes** (optional): a small daemon on a PC does the heavy AI work for the phone, found automatically on the Wi-Fi, trusted with one tap.
  Notes, answers and search run about six times faster. If a node is lost mid-answer, another node or the phone finishes it.
- **Private by design**: no accounts, no cloud, no analytics. Nodes keep nothing and show only counts.

<p align="center">
  <img src="docs/images/nodes.png" width="24%" alt="Compute nodes in Settings">
  <img src="docs/images/knowledge.png" width="24%" alt="Knowledge">
  <img src="docs/images/tray.png" width="24%" alt="Tray icon details panel on the compute node">
  <img src="docs/images/topbar.png" width="24%" alt="Tray icon in the Ubuntu top bar">
</p>

## How it is built

| Part | Technology |
| --- | --- |
| App and screens | Kotlin, Jetpack Compose (`app/`) |
| Meetings, notes, search, chat logic | Rust, SQLite with FTS5, a vector index (`core/`) |
| AI on the phone | Gemma 4 E2B and EmbeddingGemma 2 on Google LiteRT-LM (GPU first), whisper.cpp for speech, sherpa-onnx for speakers |
| Compute node | Rust, tokio, QUIC with pinned certificates, llama.cpp with CUDA or Vulkan (`compute-node/`) |
| Tray icon | Rust, `ksni` on Linux, `tray-icon` on Windows, `egui` details panel |

See [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) for the diagrams and how a job is routed and recovered.

## Getting started

**The phone app** (Android 14 or newer, 8 GB of memory recommended): see [docs/DEVELOPMENT.md](docs/DEVELOPMENT.md). In short
`scripts/build-native.sh arm64`, `scripts/stage-models.sh`, `./gradlew assembleRelease`. The models are not in the repository.

**A compute node on Linux**

```bash
compute-node/build.sh                      # CUDA build, or --cpu-only
target/release/marginalia-node models download
compute-node/packaging/install-linux.sh    # user service and tray icon at login
```

**A compute node on Windows**: `compute-node\windows\setup.ps1` (PowerShell, no bash). See [docs/WINDOWS-PLAN.md](docs/WINDOWS-PLAN.md).

Open the app, **Settings, Compute nodes** (or the banner on the main screen), tap **Connect**. That is all.

## Repository map

```
app/            Android app
core/           Rust core for the phone
compute-node/   protocol, node daemon, tray icon, Linux and Windows packaging
scripts/        build and device helpers for the app
vendor/         ggml and whisper.cpp for the phone core
docs/           architecture, measurements, plans
```

## Tests

`cd core && cargo test --lib` (phone core), `cargo test -p marginalia-proto` (pairing, pinning, framing), `cargo test -p marginalia-node-tray`,
`./gradlew :app:testDebugUnitTest`. Measurements on a real phone and the bugs found are in [docs/AUDIT.md](docs/AUDIT.md).

## Status

Working and tested on an Android phone and Ubuntu: everything above. The compute node on Windows and the tray icon on Windows are written but
not yet verified (see [compute-node/STATUS.md](compute-node/STATUS.md)). Planned: nodes that find and share work with each other,
speech to text on nodes.

## License

Apache-2.0. Third-party code under `vendor/` and `compute-node/vendor/` keeps its own licenses.
