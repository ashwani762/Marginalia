# Audit

What was checked on the real phone (Android 16, Snapdragon 7s Gen 3, 7.6 GB), what broke,
and what was done about it. Anything not listed here has not been tested on the phone.

## Engines

| Part | Choice | Why |
| --- | --- | --- |
| Chat, notes, photos | Gemma 4 E2B on LiteRT-LM, GPU first and CPU as a fallback | Prefill 223 tokens a second on the GPU against 26 for llama.cpp on the CPU, about 1.5 GB of memory against 4 GB |
| Search | EmbeddingGemma 2 (270M) on LiteRT-LM, CPU | 1.1 passages a second on the CPU, 2.1 on the GPU. It is compute bound, so a large document takes minutes. It stays on the CPU because next to Gemma's GPU engine it takes the memory the image reader needs and every photo then fails to read |
| Speech | whisper.cpp small, 8-bit, CPU | LiteRT Whisper exports use a fixed 30 second window and cannot do live captions. Trimmed `audio_ctx` gives about 0.7 s lag |
| Speaker separation | sherpa-onnx (pyannote segmentation 3.0 and WeSpeaker CAM++), CPU | Runs after the meeting on the kept audio, about 36 MB of models fetched once. 73 s of audio took about 25 s |

llama.cpp was tried first and has been removed.

## Measured on the phone

- Whisper small q8: batch RTF 0.33. Live decode 0.6 to 1.2 s for 2 to 3 s of speech, lag about 0.7 s, and the lag
  stayed at 1.2 to 1.4 s while a photo was being read on the GPU at the same time.
- Gemma E2B on the GPU: decode 8.7 to 9.7 tokens a second, 12.0 to 12.6 with speculative decoding (MTP). MTP is on by
  default on the GPU and is a switch in Settings.
- Gemma E2B on the CPU: prefill 103, decode 10.2. First GPU start takes 17 to 35 s, about 10 s once the program cache exists.
- Speech accuracy (small q8, FLEURS, 20 clips each, computer CPU): English 7.0% word error, German 7.8%, French 13.9%, Spanish 5.4%, Hindi 56.5%. Auto-detect picked the right language on 98 of 100 clips (two Hindi clips came out as Urdu), so the language is now settled from the first few sentences of each recording. The larger large-v3-turbo model reads Hindi clearly better but ran at 4.5 times real time on this phone, so it is not shipped.
- Memory: 1.2 to 1.5 GB PSS with both models loaded. The models are unloaded a minute after all work ends.
- Photo read while recording: about 12 s, no queueing.
- A 120 minute test meeting was written up with the app closed and the screen off (about 12 minutes, no crash).

## Bugs found and fixed

| Found | Cause | Fix |
| --- | --- | --- |
| Process killed by Android during a long write-up (LOW_MEMORY) | No foreground service while the test ran | The write-up runs under `WorkService` |
| App crashed on every service start after heavy testing | Android 15+ limits `dataSync` foreground services to about 6 hours a day | Service type is now `specialUse`, which has no limit, and a refusal is caught instead of crashing |
| A meeting killed mid-write-up | Recovery only ran when the app was opened | Sticky service restart, a persisted job that restarts the app, and recovery of anything unfinished. Killed with SIGKILL mid-summary the service came back in about 6 s and finished the same meeting |
| Short meetings were "recovered" on every launch | No marker for a finished write-up | `wrapped_<id>` flag, set only after the summary, title and index are done |
| Notes written twice after recovery | Compiled jots were appended each time | Replaced instead of appended |
| Summary failed once after a restart | One model call failed (model reloading) | Retried up to 3 times, resuming from the saved parts |
| Recording killed halfway, then recording again made a second entry | There was no way to carry on | `meeting.resume`: same entry, timestamps continue, notes rewritten in place. Verified on the phone: 12 to 27 lines in the same meeting, no new row |
| Crash "Key was already used" in the live transcript | Lines were added by list position, not line number | Replaced by line number |
| Live screen stayed open after a stop from outside the screen | State was only cleared by the screen's own stop | Cleared when the write-up of that meeting starts |
| Toast in the wrong place and bullets misaligned | Fixed offsets, no hanging indent | Toast is a bubble that drops from under the status bar. Bullets use a marker box with a hanging indent |
| Settings section labels indented more than their text | `SectionLabel` added its own margin inside a sheet | `flush` option |
| Tapping a document in Knowledge did nothing | The row's click handler was empty and there was no viewer | Document screen: details, the text in reading order (overlap between passages removed), ask, move, delete. PDFs and pictures are kept and shown as they were, PDF pages drawn in the app |
| Photos and picture documents failed to read ("Status Code 13") | The search model was running on the GPU next to Gemma's GPU engine | Search model back on the CPU |
| Native crash when separating speakers | The library calls back into Kotlin by exact method signature and a lambda does not have it | A real callback class and keep rules |
| Ask scope chips (Everything, Documents, Meetings) were unclear and the clear button floated | Two unrelated controls and a trailing icon with nothing to line up with | One pill that opens a plain chooser (what to search, which folder), and a New chat button in the same row |
| Files placed in the app inbox were never tidied away | /data/data and /data/user/0 compared as different paths | Real paths compared |

## Stress test on the phone (release build, fresh install, a CppCon talk playing in Chrome on this machine)

- Fresh install: 3 GB unpacked and the app opened in about 20 s with 0.45% janky frames (99th percentile 24 ms).
- A 13 minute recording with 9 jots across the four kinds, backgrounding for 45 s, rotation, light and dark theme,
  a force-stop and relaunch, continuing the recording, and a stop: memory stayed between 1.34 and 1.72 GB, the
  battery temperature between 38 and 41 degrees, and the write-up took 40 s. The transcript caught up at once after
  the app came back from the background.
- A crash during a recording (below) lost nothing: the 2 minutes already captured were written up on the next launch.
- Over the whole run 8% of frames were janky (99th percentile 65 ms) while the CPU was busy transcribing.

Bugs this found, all fixed:

| Found | Cause | Fix |
| --- | --- | --- |
| Release build said the models were missing | The two LiteRT model files were stored compressed in the APK and the app opens them as raw files | `noCompress` for `litertlm` (the APK is now 3.18 GB) |
| Crash while the keyboard opened on the Ask tab behind the live screen | A spring-animated spacing overshot below zero ("Padding must be non-negative") | Clamped at zero |
| Ask input hidden behind the recording card | Nothing reserved room for the card | `MiniBar` clearance |
| "Jump to live" see-through, waveform washed out on the light theme | Translucent chip, thin low-contrast bars | Solid dark pill, heavier bars, deeper spark colour |
| Status bar icons white on cream in sheets | A sheet is its own window | `SheetHandle` sets the icon colour |
| Short jots rewritten into something else | The longer context let the model drift | Stricter instruction, and a rewrite that loses the jot's words is replaced by the jot, tidied |
| Garbled first line from a silent intro | Whisper hallucination | Filter for repeated characters and rare scripts |
| Camera sometimes failed to capture | The camera app can report done before the file is complete, or cancelled after saving | Wait for the file to settle, retry, accept a saved picture |
| "Condensing earlier minutes" during a short meeting | Live section notes started after about 12 minutes | Only after about 45 minutes. The write-up covers the rest |

## Speed

| Change | Result |
| --- | --- |
| Whisper flash attention on the phone's CPU | 8.7 s against 10.0 s for 37 s of audio (13% faster, same output). 4 threads stay best: 3 gave 11.3 s, 5 gave 15.1 s, 6 gave 16.9 s, 8 gave 27.6 s |
| Meeting list from a saved copy | On screen about 1 s after launch (about 4 s before). The list never waits for the AI, which loads behind it in about 22 s |
| Model loading on a low-priority thread | First-run frames stay smooth while the models load |
| Database cache, temp store and mmap settings | Small, no change in behaviour |
| Search model pinned to 4 CPU threads | 1.06 passages a second against 0.91 with the library default |

## Added since the first audit

- Continue recording on any meeting, and a prompt after an interrupted recording.
- Kept audio (AAC, about 11 MB an hour, playable even after a kill), tap a transcript line to hear it.
- Backup and restore to a file you choose, nothing sent anywhere.
- Section notes of long meetings are saved, so a restart resumes at the part it reached.
- File and link ingests are saved in a queue and resumed.
- Search is rebuilt once when the embedding model changes.
- Living jots: a polished jot is reworded once when later talk adds a detail, and marked Updated.
- Larger Gemma (E4B) download and switch with a fall back to E2B if it does not fit. Not yet loaded on the phone.
- Speaker separation after the meeting, with rename and merge. On a test file of three voices it got the first half right and split the third voice in the second half, so over-split voices are merged by hand.
- Landscape, tablet and foldable layouts (verified on the emulator only).
- Document viewer with the original PDF or picture.
- Notes use the names given to speakers.

## Home computers

- Phone to node over the real Wi-Fi: pairing, status (7 to 14 ms round trip), search and chat jobs, discovery with
  Android NSD (found by itself in about a second), trust, lock and approve, block.
- Same prompt: 0.86 s on the node (88 tokens a second) against 5.2 s on the phone (15 tokens a second). Three search
  passages: 0.06 s against 0.68 s.
- The node's search vectors and the phone's own agree with a cosine of 0.98 to 0.99 for the same text (different
  quantisation), so documents embedded in either place still search together.
- The phone could not reach the PC once because its network neighbour entry had failed until the PC sent it a packet.
  The phone retries every 20 seconds and does the work itself in the meantime.
- A block follows the phone's certificate, so a phone that clears the app's data gets a new identity. Lock the node to stop that.

## Tests

- Rust: 15 unit tests run on the host (`cd core && cargo test --lib`), 22 protocol tests (`cargo test -p marginalia-proto`).
- Kotlin: JSON model parsing (`./gradlew :app:testDebugUnitTest`).
- Everything else was tried by hand on the phone and the emulator.

## Known gaps

- E4B has not been loaded on this 7.6 GB phone and may not fit.
- Speaker separation: long meetings (over 2 hours) are untested and may need block-wise processing. Accuracy on real meeting audio is unmeasured.
- Ingest of a large document is slow (about 2 passages a second on the GPU).
- Hindi accuracy with the small speech model is poor (about 57% word error).
- Landscape, tablets and foldables have only been checked on the emulator.
- Living jots (a jot reworded when later talk adds a detail) and the E4B switch have not been exercised on the phone.
- The release build is signed with the debug key and is about 3.2 GB with models.
- The database and photos are not encrypted and there is no app lock. That was a decision for a single user.
