# Development

## Layout

```
app/            Android app (Kotlin, Jetpack Compose)
core/           Rust core for the phone (JNI library), whisper.cpp and RAG
compute-node/   proto/ (wire protocol), node/ (daemon), tray/ (tray icon), packaging/, windows/, vendor/ (llama.cpp)
scripts/        Build and device helpers for the phone app (bash)
vendor/         ggml and whisper.cpp used by core/
docs/           Architecture, audit notes, plans
```

The Cargo workspace root is the repository root. `cargo build` and `cargo test` at the root cover the protocol and the node. The
phone core is built for Android by `scripts/build-native.sh`.

## The phone app

Needs: Android SDK and NDK, Rust with the Android targets, `cargo-ndk`, a phone with USB debugging (Android 14 or newer, 8 GB
recommended).

```bash
scripts/fetch-sherpa.sh        # speaker separation library, fetched once
scripts/build-native.sh arm64  # Rust core for the phone (baseline and i8mm variants)
scripts/stage-models.sh        # link the models so Gradle can embed them (release build)
./gradlew assembleRelease      # about 3.2 GB with the models inside
# development: small APK, models pushed over USB
./gradlew assembleDebug -Pmarginalia.embedModels=false && scripts/push-models.sh
```

Models (never committed, see `.gitignore`): `gemma-4-E2B-it.litertlm`, `embeddinggemma-2-text-270m.litertlm` (LiteRT-LM) and
`ggml-small-q8_0.bin` (whisper.cpp). `scripts/stage-models.sh` says where it looks for each.

Always set `ANDROID_SERIAL` before `adb` or `gradlew` when more than one device is attached.

Tests: `cd core && cargo test --lib`, `./gradlew :app:testDebugUnitTest`. Debug builds accept development intents
(`DEBUG_CALL`, `DEBUG_FEED`) that run any core command or feed an audio file as if it were the microphone, even while the phone is locked.

## The compute node

```bash
compute-node/build.sh                 # CUDA build (or --cpu-only)
target/release/marginalia-node run
cargo test -p marginalia-proto        # 22 tests: pairing, pinning, man in the middle, framing
cargo test -p marginalia-node-tray    # 25 tests
compute-node/packaging/install-linux.sh   # user service and tray autostart
```

On Windows use `compute-node/windows/setup.ps1` (see `docs/WINDOWS-PLAN.md`). Node models are GGUF files, download them with
`marginalia-node models download`.

## Conventions

- **User-visible text** (app, node, tray, scripts): plain wording, no semicolons and no em dashes. The chat model is "the AI".
  The feature is called "compute node", never "home" or "computer".
- **Commit messages:** `[area] short subject`, then a short body saying why.
- **Nothing private is committed:** no models, no databases, no tokens. Hugging Face tokens are read from `HF_TOKEN` and never printed.
- **Stateless nodes:** never write job content to disk or to a log on a node.
