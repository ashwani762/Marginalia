# Marginalia for Android

The phone version of Marginalia: record a meeting, jot as it happens, and get notes, a summary and
answers from your own documents. **Everything runs on the phone.** Nothing is sent anywhere.

## How it is built

| Part | What runs it |
| --- | --- |
| Screens, gestures, notifications | Kotlin and Jetpack Compose (`app/`) |
| Gemma 4 E2B: notes, summaries, answers, reading photos | Google **LiteRT-LM** on the GPU (CPU as a fallback), through a Kotlin bridge |
| EmbeddingGemma 2 (270M): search | Google **LiteRT-LM** `EmbeddingEngine`, on the CPU |
| Speech to text | **whisper.cpp** (small, multilingual) on the CPU |
| Search, documents, meetings, folders, long-meeting logic | Rust (`core/`), the same retrieval code as the desktop app |

The Kotlin and Rust halves talk through four JNI calls (`app.marginalia.Native`) that carry JSON and raw
audio. llama.cpp is not part of this build.

### Why Whisper is not on LiteRT

The LiteRT Whisper exports use a fixed 30 second window and a decoder that re-runs the whole sequence
at every step. Live captions need the opposite: short snippets, decoded fast. whisper.cpp with a window
trimmed to the snippet decodes 2 to 3 seconds of speech in about 0.7 seconds on the test phone (about
0.3x real time). The two speech models built for LiteRT-LM (Qwen3-ASR 1.7B and Fun-ASR-Nano) are far
larger and give nothing back in speed. If a streaming LiteRT speech model appears, it plugs in behind the
same `WhisperHandle`.

## Build

```bash
# 1. Native libraries (needs the Android NDK and the Rust Android targets; cargo-ndk)
scripts/build-native.sh            # arm64 (baseline + i8mm) and x86_64 (emulator)

# 2. Models
scripts/stage-models.sh            # link the models so Gradle embeds them in the APK
./gradlew assembleRelease          # embedded models (about 3.1 GB)

# or, for development: a small APK and models pushed over USB
./gradlew assembleDebug -Pmarginalia.embedModels=false
scripts/push-models.sh
```

Models used: `gemma-4-E2B-it.litertlm` (litert-community/gemma-4-E2B-it-litert-lm),
`embeddinggemma-2-text-270m.litertlm` (litert-community/embeddinggemma-2-text-270m-litert-lm),
`ggml-small-q8_0.bin` (whisper.cpp).

## Things worth knowing

- **Real time.** Live speech gets the processor first. Whisper decodes each snippet in well under its
  own duration, so captions follow speech by under a second. Gemma runs on the GPU, so it does not
  compete for the CPU cores speech needs.
- **Closing the app.** Notes are written by a foreground service. Swipe the app away and the work
  continues, and a notification says when it is done. If Android kills the process anyway, the next launch
  finishes any meeting that was left half done.
- **Long meetings.** Notes on finished stretches are written while recording, merged at the end, and
  the summary is built from those, so nothing earlier is lost to the context window.
- **Folders.** Meetings and documents can be filed in folders; Ask and Live Assistance can be limited to
  one folder.

See `AUDIT.md` for the design and testing notes.

## Tools

```bash
scripts/fetch-sherpa.sh     # the speaker separation library (about 50 MB, not committed)
cd core && cargo test --lib # Rust tests, on the host
./gradlew :app:testDebugUnitTest
```

Development switches (debug builds): a `tune.properties` file in the app's files folder accepts `mtp=true|false|auto`,
`maxTokens=2048` and `embedGpu=true`. Debug intents are listed in `MainActivity.kt`.
