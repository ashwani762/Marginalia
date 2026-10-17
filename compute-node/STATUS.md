# Status and how to carry on (written before moving development to Windows)

## What works (checked on Linux with a real phone)

- Phone finds nodes by itself (Android NSD), a banner on the main screen offers "Connect", one tap connects to one or all of them,
  nodes let phones in without a code (lock mode and approve exist, pairing with a code stays as the strict route), block list per node.
- Notes, answers and search vectors run on nodes: about 100 tokens a second and 1000 passages a second on an
  RTX 4060 against about 15 and 1 on the phone. If no node answers, the phone does the work itself.
- Nodes are ranked by expected wait (jobs ahead over speed, using the lower of the advertised and the really observed speed).
  A job that fails moves to the next node. A node lost or turned slow halfway through an answer is replaced by another node,
  or by the phone, which carries on from the words already shown (a seam filter removes a repeated sentence).
- A node folds the speed of real jobs into what it reports and measures itself again every 5 minutes while idle.
- Several jobs run side by side so every node is busy, and a meeting write-up writes its parts on several nodes (tested with
  three nodes on one PC, each loaded with its own state, names and ports).
- Wi-Fi off and on during a recording, the app in the background, a node killed with SIGKILL and brought back by systemd: the
  recording and the jots carried on. Pause, resume and stop work on the running service. The tray icon shows in the Ubuntu
  top bar and its panel shows the live numbers.
- The node reports counts and timings only (`marginalia-node stats`).
- Transcription of recordings with `ggml-large-v3-turbo-q5_0` (whisper.cpp built against the same ggml as llama.cpp, so one ggml
  in the process, CUDA). Checked on the RTX 4060 with `marginalia-node probe ADDR --transcribe FILE.wav` against a second node
  instance (ports 47690 and 47691, own state folder, the service on 47615 was not touched):
  - The public domain JFK sample (11 s) came back word for word right ("And so, my fellow Americans, ask not what your country can
    do for you, ask what you can do for your country."), language detected as English, 0.3 to 0.4 s of work (27 to 38 times faster than real time).
  - The same sample repeated with 3 s of faint noise between, 770 s (13 min): 55 of 55 segments, segments arrive while the job runs,
    progress after every percent, 110 times faster than real time with a beam of 5 (122 times greedy).
  - 4620 s (77 min, 147 MB of audio sent in about 0.3 s over loopback): 329 of 330 repeats came back (two neighbouring repeats were
    merged by the model, no pipeline loss), 94 times faster than real time with a beam of 5 (34 s of work, 137 times with greedy decoding). Heartbeats
    arrived every 2 seconds. Node process memory peaked near 1.3 GB (344 MB before, about 490 MB after: the node hands memory back),
    graphics memory went from 3.76 GB to 4.13 GB while working and back to 3.81 GB afterwards (this includes the other work on the card).
  - Hanging up halfway: the node stopped working and freed everything within a second (`running_jobs` 0, memory back). A phone that
    vanishes without closing the connection is noticed at the 30 second idle timeout.
  - `pause` turns a transcription away with "paused". A second recording sent during the first waits and is told "queued behind 1"
    every 2 seconds, then runs after it. `stats` and `status` show the speech model, jobs, seconds of audio and speed.
  - A node started with only the speech model file (no Gemma, no embedder) serves transcription and says so in its status.

## Short windows for live correction (checked on the RTX 4060)

Second node instance on ports 47690 and 47691, speech model only, `probe --transcribe FILE --window`. Time from the last audio frame
sent to `Done`, the 11 s JFK sample repeated to 40 s, English, greedy decoding:

- Alone: 0.35 to 0.45 s (about 100 times real time). 40 s of denser speech (0.3 s gaps): 0.39 to 0.46 s. A 120 s window: 1.2 s.
- While a 10 minute recording ran on the other lane (it took 9.2 s instead of 7.6 s alone): 0.47 to 0.56 s, 8 windows in a row.
- Beam of 5 for windows, for comparison: 0.44 to 0.55 s alone, 0.55 to 0.76 s next to the recording. Greedy was chosen anyway: the windows
  are replaced by the final pass over the whole recording (beam 5), and it leaves more room for the unmeasured case of Gemma writing
  on the same card. `MARGINALIA_SPEECH_WINDOW_BEAM=5` brings the beam back.
- 40 s of digital silence: no segments in 5 ms (the model is not used). 2 s of silence: none. 5 s of noise: none in 0.12 s. 40 s of loud
  white noise: none in 0.3 s (before the checks it returned "Thank you." and "So,"). A 1 s clip: "Ask not." in 0.11 s.
- Starting in the middle of "ask not what your country...": segment times are from the start of the window and the same (within 20 ms) with
  and without a prompt. With a prompt the text follows its style (no comma in "And so my fellow Americans"). A prompt ending in "Thank you all
  for joining" made the model read "Thank you." back over loud noise, which the node now drops.
- Lanes: two windows of 120 s at once: the second said "queued behind 1" and finished after the first. Four at once from one phone: two ran,
  two got "busy" (limit of 2 per phone). A window hung up on after 0.2 s stopped at once (`running_jobs` 0). With `--window-max-seconds 30`
  a 40 s interactive request queued behind the long recording, no error. Windows are counted apart in `stats` (jobs, failed, seconds of audio).
- Graphics memory next to the other service (3.1 GB of its own): about 4.28 GB idle with the speech model loaded, peak 4.57 GB with a
  10 minute recording and windows running together.
- whisper.cpp's `no_speech_prob` reads 0.00 for this model even over silence, so the old "thank you over silence" check never fired.
  The node now looks at the sound under each segment instead (loudness of 50 ms frames) and at the model's own word confidence.

## Verified on Windows 11 (RTX 4060, CUDA)

- `setup.ps1 -Auto` builds the node and the tray, downloads the models, installs to `%LOCALAPPDATA%\MarginaliaNode`, adds the two
  login tasks and the firewall rules, starts both programs and runs `verify.ps1`, which passes 19 of 19 checks (the mDNS line is
  a warning, because Windows cannot see its own announcement).
- Fixed on the way: CMake older than 4.2 cannot make a Visual Studio 2026 project, and Rust older than 1.95 cannot build the
  tray. `setup.ps1` now upgrades both.
- The node serves chat (E2B, about 65 tokens a second), search and speech, and the self tests for let in, search, chat and speech pass.
- Chat model choice: E2B is the default. `marginalia-node chat-model e4b` and the tray menu entry switch to E4B. Stopping the
  node frees the graphics memory (5141 MiB used, 2026 MiB after stop, 7166 MiB with E4B loaded), then the new model loads.
- `cargo test` for proto (23), node (22) and tray (29) passes on Windows.

## Not verified yet

- On Windows, still to try by hand: the tray clicks (left click panel, right click menu, pause, resume, stop, start), a reboot
  bringing the node and the tray back, mDNS announcements seen from a real phone, and a real phone getting an answer.
- Nodes finding and sharing work with each other (cluster) is designed in `DESIGN.md`, not built.
- Live (streaming) speech to text on nodes is not built as one open stream. The node side of "live correction" exists (short windows with a prompt, own lane, see below) but no phone sends them yet.
- The window lane was not tried with Gemma generating at the same time (the 8 GB card is shared with the running service, so a second Gemma did not fit). Windows were measured next to a long recording only.
- The window lane on a processor (one shared lane) and prompts in other languages were not tried.
- Transcription on the processor only (`run --cpu`) was tried once while the machine was busy: the 11 s sample took about 20 s (0.5 times real time, same correct text), so the large model is only practical on a graphics card. Vulkan and Windows were not tried.
- A phone sending a real recording: only the `probe --transcribe` client (same protocol, written like the phone) was used, not the Android app.
- Real meeting audio in other languages: only the English JFK sample was available, so accuracy on other languages and noisy rooms is not measured.
- A left click on the tray icon opening the panel was only triggered over D-Bus, not with a real mouse click.

## Building on Windows

The quick way is `.\compute-node\windows\setup.ps1` (see `compute-node/windows/README.md`). It was run on Windows 11
(see "Verified on Windows" below). What the scripts do:

- `setup.ps1`: checks Git, Rust (MSVC), Visual Studio Build Tools (C++), CMake, LLVM (libclang, sets `LIBCLANG_PATH`) and the
  CUDA toolkit when an NVIDIA card is found, and prints the exact `winget install` line for each missing one (`-Auto` runs them).
  Picks `cuda` (nvcc and an NVIDIA card, `CUDAARCHS` from `nvidia-smi --query-gpu=compute_cap`), else `vulkan` (`VULKAN_SDK`),
  else CPU. Builds node and tray in release, installs to `%LOCALAPPDATA%\MarginaliaNode`, runs `marginalia-node.exe models
  download`, registers Scheduled Tasks `Marginalia Node` (through `start-node.vbs`, hidden, log in `node.log`, restart on failure)
  and `Marginalia Node Tray` at logon, adds firewall rules for UDP 47615 and 5353 (Private profile, one admin prompt through
  `firewall.ps1`), starts both. Options `-Cpu -Vulkan -Cuda -SkipBuild -SkipModels -SkipSpeech -ModelsDir -Auto`.
- `build.ps1`, `download-models.ps1` (falls back to its own resumable download before the node is built), `status.ps1`,
  `uninstall.ps1` (`-Purge` also deletes models and state).
- Things most likely to need a fix on the first real run: the winget package ids, the Task Scheduler registration, the VBS launcher
  quoting, CUDA with the installed Visual Studio version, `windows.rs` in the tray (never compiled), mDNS from Windows.
- Checked on Linux only: `cargo check -p marginalia-node` and `-p marginalia-node-tray`, the download logic against the real
  Hugging Face server (resume, skip), the pure helpers of `models_dl.rs` in a scratch crate. The Windows target was not installed.

By hand: `git config core.longpaths true` before cloning, then in a Developer PowerShell
`cargo build --release -p marginalia-node --features cuda` (nothing for CPU only, `--features vulkan` for AMD or Intel), set
`CUDAARCHS` to the card's architecture (89 for the RTX 40 series, 86 for the 30 series), `marginalia-node.exe models download`,
`target\release\marginalia-node.exe run`, allow it through the firewall (private networks, UDP 47615 and 5353), then the tray:
`cargo build --release -p marginalia-node-tray` (see `packaging/windows.md`). State lives in `%APPDATA%\marginalia-node`,
models in `%APPDATA%\marginalia\models`.

## The Android app on Windows

The scripts are bash. Use Git Bash or WSL for `scripts/*.sh`, or run the steps by hand: `cargo ndk` for the
`core` crate (`scripts/build-native.sh` shows the flags and the two CPU variants), then `gradlew assembleRelease`.
Models for the APK are linked by `scripts/stage-models.sh` (symlinks, so use WSL or copy the files into
`models-stage/models`). When more than one device is attached, set `ANDROID_SERIAL`.

## Next steps (in order)

1. Run `windows\setup.ps1` on Windows, run the node and the tray there and fix what breaks.
2. Cluster: peers pair and gossip, entry node forwards jobs, block records spread, hot-hot and standby (`DESIGN.md`).
3. Speech on nodes: the phone uses the `Transcribe` request to replace its live transcript after a meeting (the node side is done and checked), and sends short windows with a prompt during the meeting (the node side is done and checked, see above), then live streaming with failover.
4. A Windows service wrapper (the tray plus Task Scheduler is the interim answer).
