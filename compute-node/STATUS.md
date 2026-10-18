# Status and how to carry on

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

## Measured on Linux later (RTX 4060, 20 core processor, real phone)

- **Live correction on the real phone** (a talk played into its microphone): windows reach the node about 36 s into a
  recording with a 30 s window, the corrected text replaces the draft and the phone logs `live correction on falcon: 1-27s, 2 lines became 1`.
  The live screen says what is happening (will check every 30 s, is checking 00:25 to 00:55, checked up to 00:55) and the fixed lines glow
  for 4 s.
- **A window while Gemma writes on the same card**: a 44 s window takes 0.42 s alone and 0.64 to 0.66 s while four clients keep the chat
  model busy (chat falls from about 100 to 81 tokens a second while they share).
- **Processor only** (`run --cpu`, speech model only, quiet machine, 20 cores): 11 s of audio in 9.7 s, a 44 s window in 19.7 s (2.2 times
  real time), a 121 s recording in 72 s (1.7 times). The earlier 0.5 times figure was taken while the machine was busy. A window of 30 s
  would take about 13 s, so live correction works but runs further behind than on a graphics card.
- **Vulkan** (built with `--features vulkan`, headers from the Khronos repositories because the system had none): on the RTX 4060 a 121 s
  recording runs at 53 times real time and a 44 s window takes 1.0 s (CUDA: about 100 times and 0.42 s), output correct. On an Intel UHD 770
  integrated GPU it is no faster than the processor (1.1 times real time on 121 s, start up takes 50 s while the self measurement runs).
  Vulkan only pays off with a real graphics card.
- **Other languages and noise** (FLEURS test recordings, 12 per language, language detected by the model, word error rate of the large
  turbo model; the reference is the normalised FLEURS text so Hindi is inflated by spelling conventions):

  | Language | Clean | White noise 10 dB | White noise 0 dB | Talk over it, 5 dB |
  | --- | --- | --- | --- | --- |
  | Spanish | 1.7% | 5.5% | 10.7% | 16.5% |
  | German | 4.1% | 10.2% | 35.0% | 16.0% |
  | French | 13.1% | 29.1% | 68.7% | 27.8% |
  | Arabic | 16.7% | 23.5% | 46.2% | 27.7% |
  | Hindi | 35.1% | 56.4% | 76.2% | 81.5% |

  Language detection was right for 59 of 60 clips (one Hindi clip came back as Urdu, so the phone's language lock matters for Hindi).
  Noise at 10 dB roughly doubles the errors and 0 dB is hard for every language except Spanish.
- **Tray with a real mouse** (X11, real clicks sent through the X server): the icon opens its menu, Show details opens the details panel,
  Pause turns the icon orange and the node says paused, Resume undoes it, and Use chat model E4B restarts the node on E4B and back.
  On the GNOME top bar a left click opens the menu, it does not open the panel directly.
- **E4B** next to the speech and search models on the 8 GB card: loads, 59 tokens a second (E2B 105), 5.4 GB of the card in use.

## Not verified yet

- Windows is beta and will be finished later. Still to try by hand: the tray clicks on Windows, a reboot bringing the node and the tray
  back, mDNS announcements seen from a real phone, and a real phone getting an answer.
- Nodes working together: the first slice is built and checked on one machine (see "Cluster slice"). Not checked on two real machines or with a real phone against a cluster.
- Live speech to text as one open stream is not built on purpose. Live correction in windows exists on both sides (phone and node) and works, see "Measured on Linux later". An open stream was decided against: every window is its own request, so failover to another node or to the phone's own captions is free, and a stream would need state per stream on a node that keeps nothing.
- Wayland (the tray icon was only tried under X11) and a Raspberry Pi.
- The phone alone still stalls the screen once for about 5 s when it has to load Gemma on the GPU (see docs/DEVELOPMENT.md).

## Building on Windows

The quick way is `.\scripts\windows\setup.ps1` (see `scripts/windows/README.md`). It was run on Windows 11
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
`core` crate (`scripts/app/build-native.sh` shows the flags and the two CPU variants), then `gradlew assembleRelease`.
Models for the APK are linked by `scripts/app/stage-models.sh` (symlinks, so use WSL or copy the files into
`models-stage/models`). When more than one device is attached, set `ANDROID_SERIAL`.

## Next steps (in order)

1. Done (2026-10-18): the phone and the tray show what state a compute node is in.
   - Phone: `nodes.list` carries `paused`. The dot and words are green running, yellow "Paused on the node", red stopped or not reachable,
     in the Meetings chip and in Settings (`nodeChip`, `HomeComputer.light`, unit tested). A paused node is not offered jobs.
   - Node: pausing, resuming and stopping are announced on the network (`paused=1` in the announcement, a goodbye on stop), the phone
     reports a goodbye with `nodes.gone`, and `nodes.list` asks nodes that answer for a fresh status at most every 4 seconds.
   - Tray: icons running green, busy blue, paused amber, locked violet, not running red, with the state in the tooltip.
   - Checked on the real phone and the real tray with real clicks: pause turns the chip yellow in under 1.5 s, resume green, stop red
     in under 1.5 s, start green in under 6 s (the node loads its models first). The tray shows amber for a moment while a stopping node
     finishes its jobs, then red.
2. Later: take Windows out of beta (tray clicks, a reboot, a real phone). The core features work today on Linux.
3. Cluster, the rest: peers found by mDNS, member lists, standby mode and roles (`DESIGN.md`). The first slice (join, shared phone records, forwarding chat and search) is done.
4. Speech on nodes: done on both sides. The phone replaces its live transcript after a meeting with the `Transcribe` request and corrects the captions during the meeting with short windows (checked on a real phone). No open stream is planned, because windows already fail over for free and a stream would need state per stream on a node.
5. A Windows service wrapper (the tray plus Task Scheduler is the interim answer).

## Several nodes and failures (checked on a real phone, 2026-10-18)

- The phone looks at every paired node and keeps a connection to each one, so Settings shows each node with its own light and speed.
  A chat or search job goes to one node, the one with the shortest expected wait, not to all of them.
- A node that dies halfway through an answer: the words already shown stay, the phone asks the next best node (or itself) to carry on
  and joins the text without repeating the last sentence. Checked by killing the answering node: the answer finished on the other node.
- A node that passes a job to a peer which then dies: before the first word it tries the next peer and then runs the job itself.
  After the first word the node carries on from the words it already sent on (`Forward::Broke` in `cluster.rs`, `resume.rs` in the
  proto crate holds the request and the seam filter that the phone uses too), so the phone sees one answer and does not mark the node
  as failed. Checked by `tests/cluster.sh` (a peer is killed while it writes a forwarded answer, the node logs that it carried on and the
  job comes back whole) and earlier on the real phone with a third node killed mid-answer. If the node itself cannot carry on
  (paused or no chat model) the phone gets the "stopped answering" error and carries on as in the case above.
- Meetings screen: one pill for each allowed compute node, running first, then paused, then not reachable, at most 5, scrolling
  sideways (`nodePills`). Settings lists every compute node. Checked on the real phone with four nodes.

## Cluster slice

Peers, shared phone records and placing chat and search jobs (design in `DESIGN.md`, use in `README.md`, "Working together").
Standby mode, roles, mDNS between nodes, member lists, work stealing and batch requests are not built.

Verified on Linux on one machine, with the debug build, three nodes with their own ports (47701 to 47703), state folders and
`--cpu --lock`, by `compute-node/node/tests/cluster.sh` (a few minutes, all checks passed on the last run):

- `join` between a and b and between c and b (a is a chain, a to b to c). `peers` and `status` list the peers, `peers.json` has mode 0600,
  a wrong code is refused, joining a peer again is refused, a peer is not counted as a phone.
- A phone paired on a with `probe --code` is let in by b and c with `probe --knock` while they are locked and no code was typed there.
  A phone nobody knows is made to wait by b. c gets the record through b.
- `block` on a reaches b and c in under 15 seconds (the immediate push, the 3 second exchange is the fallback) and the phone is turned away
  by both. `unblock` on c reaches a and b, and does not bring the pairing back.
- Chat sent to a, which has only the search model, is answered by b (a counts 1 forwarded job). Search sent to a stays on a.
- A job that comes to a paused b from a peer is not passed on to c (checked in the logs: b "turned a chat job away (paused)"). A chat job sent
  to the paused b is answered by c.
- 4 chat jobs sent to b 0.3 seconds apart: at least one is forwarded to and answered by c, all 4 come back.
- c killed with `kill -9` while b has not yet noticed: the jobs b sends to it get no answer, b waits 10 seconds (the phone gets a heartbeat
  every 2 seconds meanwhile), then runs them itself and all 4 come back.
- `unjoin` on b of a dead c works, `unjoin` on a also removes a from b.
- Unit tests: the merge of phone records (order, ties, block wins, unblock later wins, repeated and reordered merges, 11 tests), the placing
  decision (10), the peer file round trip and its mode (5), the store over restarts, from an older node's `clients.json` and between two
  stores (3), and that the messages old phones know keep their numbers (2). `cargo test -p marginalia-proto -p marginalia-node-tray` and
  `cargo test -p marginalia-node` pass.

Not verified: two real machines (loopback only, so the address a peer sees for the joiner was always 127.0.0.1), a real phone talking to a
cluster, a peer that moved to a new address, a restart of a node with peers (the file round trip is tested, the reconnect is not), clocks
that differ by seconds, the CUDA build (the changed code does not depend on the feature), Windows, nodes with a speech model (transcription is
not forwarded and was not run here).
