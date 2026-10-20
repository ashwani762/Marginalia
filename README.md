# Marginalia

### Notes in the margin of your meetings.

Marginalia sits in your pocket while people talk. It listens, you jot a few words, and when the meeting ends it hands you clean notes,
a summary, who said what, and answers from everything you have ever kept. Photos of the whiteboard included.

**It all runs on your phone. No accounts, no cloud, no analytics.** And when a PC on your Wi-Fi is willing to help, the AI simply gets
a lot faster. Walk out of the house and the phone carries on alone. Nothing breaks.

<p align="center">
  <img src="docs/images/meetings.png" width="24%" alt="Meetings list with a compute node banner">
  <img src="docs/images/live.png" width="24%" alt="Live recording with captions and jots">
  <img src="docs/images/review.png" width="24%" alt="Written notes and transcript">
  <img src="docs/images/ask.png" width="24%" alt="Ask with sources, answered on a compute node">
</p>

## Contents

- [A meeting in 60 seconds](#a-meeting-in-60-seconds)
- [What it can do](#what-it-can-do)
- [Meet the compute node](#meet-the-compute-node)
- [How it fits together](#how-it-fits-together)
- [Quick start](#quick-start)
- [Everyday commands](#everyday-commands)
- [Build everything with one command](#build-everything-with-one-command)
- [Repository map](#repository-map)
- [Tests](#tests)
- [House rules](#house-rules)
- [Documentation](#documentation)
- [Status](#status)
- [License](#license)

## A meeting in 60 seconds

1. **Tap the mic.** Captions appear as people talk, in many languages, made on the phone itself.
2. **Jot.** Type a few words: a note, an action, a decision, a question. The AI finds what was said around it and writes it up properly.
3. **Snap the whiteboard.** The photo is read and joins the meeting. If a compute node is around it reads the photo on the spot, if not
   the phone waits until the meeting is over so the live captions never stutter.
4. **Glance at the cards.** When someone asks a question out loud, a card appears with an answer from your own documents and what was
   said earlier, with its sources. Close it with the X if you do not want it.
5. **Stop.** The summary, the notes and the search index are ready in moments, even for a two hour meeting.
6. **Ask anything later.** "What did we decide about the deadline?" Answers carry their sources, and they include what the photos showed.

## What it can do

- **Live captions** in many languages (whisper.cpp). Nothing is uploaded.
- **Jots** in four kinds, polished against the transcript.
- **Notes and a summary** once the meeting ends, so recording stays light.
- **Ask** your meetings and documents. Add PDFs, pictures, web pages and text.
- **Photos** read and searchable, so the whiteboard is part of the answer.
- **Who said what**: speaker separation after the meeting, with rename and merge. Tap a line to hear it. It is opt-in, a button you press
  only when you want it. A compute node can do it about 24 times faster than before (a 19 minute talk in 23 seconds), the phone can do it alone.
- **Better transcripts**: the phone's small model gives instant captions. After the meeting a compute node listens again with a large
  Whisper model and the improved text replaces the live one before the notes are written. An optional switch even corrects the captions
  while you record, about 12 seconds behind the speech.
- **Private by design**: nodes keep nothing and show only counts, never what was said. Their logs hold events and numbers only.

<p align="center">
  <img src="docs/images/nodes.png" width="24%" alt="Compute nodes in Settings">
  &nbsp;
  <img src="docs/images/knowledge.png" width="24%" alt="Knowledge">
</p>

## Meet the compute node

A compute node is a small program on a PC, a mini server or a Raspberry Pi. Marginalia calls it a *compute node* and so does everything below.
It is optional, it is stateless and it never decides anything. It borrows its speed to the phone.

- **Found by itself.** It announces itself on the Wi-Fi and the phone offers a **Connect** button. One tap and you are paired.
- **Traffic lights.** On the phone each node is a small pill on the Meetings screen: **green** running, **yellow** paused, **red** off or out of reach.
  Up to five pills, scrolling sideways, and Settings lists every node. Pause it from the tray and the pill turns yellow in about a second.
- **Never leaves you hanging.** Every AI job goes to the node with the shortest wait. If a node drops out in the middle of an answer, the next node
  or the phone carries on from the words already shown. If a node that passed a job to a friend loses that friend, it finishes the answer itself.
- **Nodes make friends.** They see each other on the network, you join them with a code, and from then on they share the list of allowed phones
  and pass work to whichever of them is less busy.
- **A tray icon that tells the truth.** One icon per state: green ready, blue busy, amber paused, violet locked, red not running. Click it for the details window.

<p align="center">
  <img src="docs/images/tray.png" width="200" alt="Details panel of the compute node tray icon">
</p>

- **Live graphs.** The details window draws the last two minutes of processor, memory, graphics card and network use, gliding along with the busy
  stretches tinted in, and a line turns amber and then red as the thing fills up. Hover a chart to read that moment.
- **A log you can send.** `marginalia-node logs` shows a rotating log (5 MB at a time, 5 kept) with a load line every minute, for debugging. Never the text of a note, a question, a recording or a photo.
- **Opt-in extras.** Reading photos needs one extra model file and telling speakers apart needs a small helper program and two more files.
  Both are a single command (`marginalia-node models download --only vision` and `--only speakers`) and a node that has not got them simply does not offer the work.

<p align="center">
  <img src="docs/images/topbar.png" width="45%" alt="The compute node tray icon in the Ubuntu top bar">
</p>

## How it fits together

```text
             Phone                     Compute node (optional, a PC)
┌──────────────────────────────┐      ┌─────────────────────────────┐
│ Kotlin + Compose UI   (app/) │      │ marginalia-node             │
│       |  JSON over JNI       │      │ (compute-node/)             │
│ Rust core   (core/)          │      │   Rust, tokio, QUIC         │
│   SQLite, vector index,      │◄────►│   llama.cpp (CUDA, Vulkan)  │
│   write-up pipeline          │      │   whisper.cpp               │
│       |                      │      │   speaker helper (optional) │
│ LiteRT-LM (Gemma)            │      │       ^                     │
│ whisper.cpp                  │      │       | local admin channel │
│ sherpa-onnx (speakers)       │      │ tray icon, details window   │
└──────────────────────────────┘      └─────────────────────────────┘
                     QUIC, pinned certificates
```

The one rule: **every AI job is offered to the compute nodes first and runs on the phone only when no node can take it.**
Jobs are ranked by expected wait. A job that fails moves to the next node. [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) has the diagrams and the full story.

| Part | Technology | Folder |
| --- | --- | --- |
| App and screens | Kotlin, Jetpack Compose | `app/` |
| Meetings, notes, search, chat logic | Rust, SQLite with FTS5, a vector index | `core/` |
| AI on the phone | Gemma 4 E2B and EmbeddingGemma 2 on Google LiteRT-LM (GPU first), whisper.cpp for speech, sherpa-onnx for speakers | `app/`, `core/`, `vendor/` |
| Wire protocol and trust | Rust, QUIC (quinn), pinned certificates, pairing codes | `compute-node/proto/` |
| Compute node | Rust, tokio, llama.cpp with CUDA or Vulkan, whisper.cpp | `compute-node/node/` |
| Speaker helper (optional) | Rust, sherpa-onnx with pyannote segmentation and WeSpeaker ResNet34 | `compute-node/speakers/` |
| Tray icon and window | Rust, `ksni` on Linux, `tray-icon` on Windows (beta), `egui` for the window | `compute-node/tray/` |

## Quick start

| Platform | Support |
| --- | --- |
| Phone app, Android 14 or newer | Supported |
| Compute node on Linux (Ubuntu tested) | Supported |
| Compute node on Windows 11 | **Beta**, builds and runs but is not fully tested yet |

### The phone app

You need the Android SDK and NDK, Rust with the Android targets, `cargo-ndk` and a phone with USB debugging (Android 14 or newer,
8 GB of memory recommended). The models are not in the repository, [docs/DEVELOPMENT.md](docs/DEVELOPMENT.md) names the three files and `scripts/app/stage-models.sh` says where it looks for each.

```bash
scripts/app/fetch-sherpa.sh        # speaker separation library, fetched once
scripts/app/build-native.sh arm64  # Rust core for the phone
scripts/app/stage-models.sh        # link the models so Gradle can embed them
./gradlew assembleRelease          # about 3.2 GB with the models inside
```

For daily work build a small debug APK and push the models over USB instead:

```bash
export ANDROID_SERIAL=<your phone>   # needed when more than one device is attached
./gradlew assembleDebug -Pmarginalia.embedModels=false
scripts/app/push-models.sh
```

### A compute node on Linux

You need Rust, CMake and a C++ toolchain. For an NVIDIA card also the CUDA toolkit.

```bash
make                                          # node, tray icon and speaker helper (make cpu for no CUDA)
target/release/marginalia-node models download
make install                                  # user service and tray icon at login
```

Then check it:

```bash
marginalia-node status     # what the node serves and which phones are paired
marginalia-node peers      # friends you joined and nodes seen on the network
marginalia-node logs       # where the log lives and its last lines
```

### A compute node on Windows (beta)

Windows support is in beta. It builds and runs, but it is not as well tested as Linux and some parts still need work. Run `scripts\windows\setup.ps1 -Auto` in PowerShell. It needs no bash. It installs missing tools, builds, downloads the models, installs the node and the tray icon, starts them at login and checks everything. Run `scripts\windows\verify.ps1` any time to check again. See [docs/WINDOWS-PLAN.md](docs/WINDOWS-PLAN.md).

### Connect the phone

Open the app, then **Settings, Compute nodes** (or the banner on the main screen) and tap **Connect**. That is all.
Use `marginalia-node lock` if you want to approve every new phone yourself, or `marginalia-node pair` for a one-time code.

## Everyday commands

| Task | Command |
| --- | --- |
| Run the node in the foreground | `target/release/marginalia-node run` |
| Run on the processor only | `marginalia-node run-on cpu`, then restart the node (or `run --cpu` once) |
| Split the models between the graphics card and the processor | `marginalia-node run-on auto` (the node decides) or `marginalia-node run-on mixed --chat 60 --speech cpu`. The tray's Settings page does the same |
| Build with Vulkan (AMD, Intel) | `cargo build --release -p marginalia-node --features vulkan` |
| Show or fetch model files | `marginalia-node models list` and `marginalia-node models download` |
| Let the node read photos | `marginalia-node models download --only vision` |
| Let the node tell speakers apart | `marginalia-node models download --only speakers` (the helper program must sit next to the node) |
| Choose the chat model (E2B is smaller and faster, E4B answers better) | `marginalia-node chat-model e4b`, then restart the node. The tray's Settings page does the same |
| Change a model file or the memory estimates | Edit `models.toml` (see [compute-node/COMMANDS.md](compute-node/COMMANDS.md#the-models-file)) |
| Every command with examples | [compute-node/COMMANDS.md](compute-node/COMMANDS.md) |
| Pause or resume new jobs | `marginalia-node pause` and `marginalia-node resume` |
| Let a waiting phone in | `marginalia-node approve` |
| Block or forget a phone | `marginalia-node block <id or name>` and `marginalia-node revoke <id or name>` |
| Join another compute node (it shows a code with `pair`) | `marginalia-node join <name or address> <code>`, list with `marginalia-node peers`, leave with `marginalia-node unjoin <name>` |
| Try every job type like a phone would | `marginalia-node probe <address>` (add `--speakers FILE.wav` to try speaker separation) |
| Read the log | `marginalia-node logs --follow` |

`marginalia-node --help` lists everything. The node listens on UDP 47615 and its local admin channel on TCP 127.0.0.1:47616.
It keeps its identity, paired phones and logs in `~/.config/marginalia-node` and looks for models in `~/.local/share/marginalia/models`.
Set `HF_TOKEN` if a Hugging Face repository asks for a login. The token is never printed or stored.

## Build everything with one command

| System | Command | What it does |
| --- | --- | --- |
| Linux | `make` | node, tray icon and speaker helper |
| Linux | `make cpu` | the same without CUDA |
| Linux | `make test` | every test that needs no phone |
| Linux | `make install` | build, install for this user and start |
| Linux | `make app` | the phone app (needs the Android tools) |
| Linux | `make help` | all the targets |
| Windows | `.\scripts\windows\make-all.ps1` | node, tray icon and speaker helper (add `-Test`, `-Install`, `-Models speakers`) |

## Repository map

```
app/            Android app (Kotlin, Jetpack Compose)
core/           Rust core for the phone (JNI library), whisper.cpp and RAG
compute-node/
  proto/        wire protocol, certificates, pairing, framing
  node/         the daemon
  speakers/     the optional helper that tells speakers apart (a separate program on purpose)
  tray/         tray icon and details window
  packaging/    systemd unit, tray autostart entry, Docker file for a processor build, Windows notes
  vendor/       patched llama.cpp bindings
scripts/        every script, grouped by use
  app/          build the phone core, fetch libraries, stage and push models (bash)
  node/         build and install the compute node on Linux (bash)
  windows/      setup, verify, dev, build, make-all, status and uninstall for the compute node (PowerShell, beta)
Makefile        build everything on Linux with one command
vendor/         ggml and whisper.cpp used by core/
docs/           architecture, development, plans, screenshots
```

The Cargo workspace root is the repository root, so `cargo build` and `cargo test` there cover the protocol and the node.
The phone core is built for Android by `scripts/app/build-native.sh`.

## Tests

```bash
make test                               # all of it
cargo test -p marginalia-proto          # pairing, pinning, man in the middle, framing
cargo test -p marginalia-node           # the node, including the log files and speaker helper plumbing
cargo test -p marginalia-node-tray      # tray icon and chart logic
(cd core && cargo test --lib)           # phone core
./gradlew :app:testDebugUnitTest        # Kotlin unit tests
compute-node/node/tests/cluster.sh      # three real nodes on one machine, joining, forwarding, failing (a few minutes)
```

## House rules

- **Commit messages** look like `[area] short subject`, then a short body saying why. Areas in use: `repo`, `app`, `core`, `compute-node`.
- **Words on screen** (app, node, tray, scripts) are plain, with no semicolons and no em dashes.
  The chat model is "the AI" and the feature is always called "compute node".
- **Nothing private is committed**: no models, no databases, no tokens. `.gitignore` covers `*.gguf`, `*.litertlm`, `*.bin`, `*.onnx`, `*.sqlite3*` and `*.log`.
- **Nodes are stateless**: never write job content anywhere on a node except a temporary file that is removed when the job ends.
- **Every feature must work on the phone alone.** A compute node only makes it faster.
- **Opt-in beats always-on.** Anything heavy, like telling speakers apart, runs only when someone asks for it.

## Documentation

| File | What is in it |
| --- | --- |
| [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) | Pieces, job routing, failover, diagrams |
| [docs/DEVELOPMENT.md](docs/DEVELOPMENT.md) | Building the app and the node, debug intents, conventions |
| [docs/WINDOWS-PLAN.md](docs/WINDOWS-PLAN.md) | Start here when working on Windows (beta) |
| [compute-node/README.md](compute-node/README.md) | Using and running a node |
| [compute-node/DESIGN.md](compute-node/DESIGN.md) | Protocol and the cluster of nodes (what exists, what is planned) |
| [compute-node/STATUS.md](compute-node/STATUS.md) | What is checked, what is not, measurements, next steps |

## Status

Working and tested on an Android phone and Ubuntu: everything above. The compute node on Windows is **beta**. It builds, runs and passes its script checks on Windows 11, and the rest of the Windows testing comes later (see [compute-node/STATUS.md](compute-node/STATUS.md)). The new Windows pieces (the speaker helper, the log file, the one-tray rule and `make-all.ps1`) type check for Windows but have not been run there.
Compute nodes can find and join each other, share allowed and blocked phones and hand work to a less busy node, checked on one machine.
Planned: nodes sharing their member lists, standby mode and roles.

## License

MIT, see [LICENSE](LICENSE). Third-party code under `vendor/` and `compute-node/vendor/` keeps its own licenses
(llama.cpp, whisper.cpp and ggml are MIT, sherpa-onnx is Apache-2.0).
