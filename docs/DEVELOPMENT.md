# Development

## Layout

```
app/            Android app (Kotlin, Jetpack Compose)
core/           Rust core for the phone (JNI library), whisper.cpp and RAG
compute-node/   proto/ (wire protocol), node/ (daemon), tray/ (tray icon), packaging/, vendor/ (llama.cpp)
scripts/        app/ (phone build helpers), node/ (Linux build and install), windows/ (PowerShell setup for the node)
vendor/         ggml and whisper.cpp used by core/
docs/           Architecture, audit notes, plans
```

The Cargo workspace root is the repository root. `cargo build` and `cargo test` at the root cover the protocol and the node. The
phone core is built for Android by `scripts/app/build-native.sh`.

## The phone app

Needs: Android SDK and NDK, Rust with the Android targets, `cargo-ndk`, a phone with USB debugging (Android 14 or newer, 8 GB
recommended).

```bash
scripts/app/fetch-sherpa.sh        # speaker separation library, fetched once
scripts/app/build-native.sh arm64  # Rust core for the phone (baseline and i8mm variants)
scripts/app/stage-models.sh        # link the models so Gradle can embed them (release build)
./gradlew assembleRelease      # about 3.2 GB with the models inside
# development: small APK, models pushed over USB
./gradlew assembleDebug -Pmarginalia.embedModels=false && scripts/app/push-models.sh
```

Models (never committed, see `.gitignore`): `gemma-4-E2B-it.litertlm`, `embeddinggemma-2-text-270m.litertlm` (LiteRT-LM) and
`ggml-small-q8_0.bin` (whisper.cpp). `scripts/app/stage-models.sh` says where it looks for each.

Always set `ANDROID_SERIAL` before `adb` or `gradlew` when more than one device is attached.

Photos during a recording are never read on the phone's GPU (it froze the screen for about 4.5 seconds and lagged the live transcript): a compute node
reads them when one can, otherwise they wait for the end of the meeting. Check a change to this with `adb shell dumpsys gfxinfo app.marginalia reset`, a real
photo, and `dumpsys gfxinfo` and `logcat | grep Davey` afterwards.

Logs: the phone's core logs to `adb logcat` only. A node writes `logs/node.log` in its state folder (`marginalia-node logs`).

Tests: `cd core && cargo test --lib`, `./gradlew :app:testDebugUnitTest`. Debug builds accept development intents
(`DEBUG_CALL`, `DEBUG_FEED`) that run any core command or feed an audio file as if it were the microphone, even while the phone is locked.

One command for everything on Linux: `make` (node, tray and the optional speaker helper), `make cpu`, `make test`, `make install`, `make app`. `make help` lists the
targets. On Windows the same is `.\scripts\windows\make-all.ps1` (add `-Test`, `-Install`, `-Models speakers`).

## The compute node

```bash
scripts/node/build.sh                 # CUDA build (or --cpu-only)
target/release/marginalia-node run
cargo test -p marginalia-proto        # pairing, pinning, man in the middle, framing
cargo test -p marginalia-node-tray    # tray icon logic
scripts/node/install-linux.sh   # user service and tray autostart
```

On Windows use `scripts/windows/setup.ps1` (see `docs/WINDOWS-PLAN.md`). Node models are GGUF files, download them with
`marginalia-node models download`.

## Loading models on the phone

Gemma on the GPU (LiteRT-LM, OpenCL) takes the phone's graphics chip for several seconds while it sets up, and the screen is drawn by the
same chip. Measured on the test phone: frames waited 3 s or more for the GPU (`Fence waitForever ... didn't signal in 3000 ms` in
`adb logcat`), the app skipped up to 600 frames in a row and Android killed other apps for memory. The stall is the same on a still
screen and on an animated one (5.05 s against 5.17 s), so calming the screen does not help. What helps is choosing when it happens:

- **With a compute node in reach nothing heavy loads.** Gemma loads only when the phone itself has to write something (`AppVm.ensureChat`,
  through the stand in generator `lazyChat`). A cold start then has no stalls and keeps about 2 GB more memory free.
- **With no compute node in reach, everything is warmed up behind a loading screen** (`AppVm.loadChatIfAlone`, `AiWarmScreen`): the search
  model and its first embedding, the speech model run once the way live captions run it (`engine.warm_speech`), and Gemma with its image
  reader (`LiteRtGenerator.warmUp`, the engine sets the reader up on the first conversation and that stalls the screen). The screen is a
  still bar with a "Keep going" button. LiteRT does not report progress, so the bar follows how long the last load took. After this only
  real work is left, which shows as caption lag of 0.6 to 1.4 s and no stalls during a meeting.
- The loading screen is up from the first frame when the last start had no compute node (`last_alone` in the engine preferences), so the main
  screen never shows first and then jumps away. After a start with a node in reach it is not shown first.
- Setting "Keep the AI ready on this phone" treats the phone as alone even with a node nearby.
- Gemma stays loaded for ten minutes after the app leaves the screen with nothing running, so background notes can be written on the phone.
- Nothing may start a job at launch. The old vision warm-up did, and it loaded Gemma at every start.
- Starting a recording with no node in reach loads Gemma then only if it is not loaded yet (a fallback).

To check a change: `adb logcat -v time | grep -E "Davey|Skipped [0-9]+ frames|didn't signal"` during a cold start and a recording. The caption
lag is in the core's own log (`session::live: final t=... lag=...`). With a node in reach there should be no stalls.

**Release builds rename methods.** The Rust core calls `LlmGenerator.start`, `TextEmbedder.embed` and the listener by name through JNI. A
keep rule for each is in `app/proguard-rules.pro`, and `ProguardRulesTest` fails when one is missing. Without the `TextEmbedder` rule the
release build renamed `embed`, every search failed with "embedder threw" whenever no compute node answered (a debug build and the unit
tests do not show it) and the phone alone could not search or index.

## Conventions

- **User-visible text** (app, node, tray, scripts): plain wording, no semicolons and no em dashes. The chat model is "the AI".
  The feature is called "compute node", never "home" or "computer".
- **Commit messages:** `[area] short subject`, then a short body saying why.
- **Nothing private is committed:** no models, no databases, no tokens. Hugging Face tokens are read from `HF_TOKEN` and never printed.
- **Stateless nodes:** never write job content to disk or to a log on a node.
