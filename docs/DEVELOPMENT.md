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

Tests: `cd core && cargo test --lib`, `./gradlew :app:testDebugUnitTest`. Debug builds accept development intents
(`DEBUG_CALL`, `DEBUG_FEED`) that run any core command or feed an audio file as if it were the microphone, even while the phone is locked.

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
`adb logcat`), the app skipped up to 600 frames in a row and Android killed other apps for memory. So:

- The search model (processor, quick) loads at start. Gemma loads only when the phone itself has to write something (`AppVm.ensureChat`,
  through the stand in generator `lazyChat`). With a compute node in reach that never happens.
- Starting a recording with no compute node in reach loads Gemma at that moment instead of in the middle of the meeting.
- Nothing may start a job at launch. The old vision warm-up did, and it loaded Gemma at every start.
- To check a change: `adb logcat -v time | grep -E "Davey|Skipped [0-9]+ frames|didn't signal"` during a cold start. With a node in reach
  there should be none. Phone alone, one stall of about 5 s remains when Gemma loads.

## Conventions

- **User-visible text** (app, node, tray, scripts): plain wording, no semicolons and no em dashes. The chat model is "the AI".
  The feature is called "compute node", never "home" or "computer".
- **Commit messages:** `[area] short subject`, then a short body saying why.
- **Nothing private is committed:** no models, no databases, no tokens. Hugging Face tokens are read from `HF_TOKEN` and never printed.
- **Stateless nodes:** never write job content to disk or to a log on a node.
