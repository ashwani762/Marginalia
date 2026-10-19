# Marginalia compute node

A small program you run on a machine on your network. It does the heavy AI work for the Marginalia phone app (writing up notes, answering questions, turning text into search vectors) so the phone does not have to. Phones you have paired with it send a job over the local network, the node runs it and sends the answer back.

It is optional. The phone app works without it. If a node is reachable, the phone can use it for the jobs it is faster at.

## What it keeps, and what it does not

The node is stateless with respect to your work. It keeps exactly three things on disk:

- its own identity (a self-signed certificate and key, made on first run),
- the phones it knows: paired phones, blocked phones and the records of phones that were removed or unblocked (a fingerprint, a name and a time for each, in `records.json`, with `clients.json` and `blocked.json` written beside it as plain lists), and
- the other compute nodes it has joined (`peers.json`: a fingerprint, a name and an address for each).

Nothing about a job is stored: not the text sent in, not the answer, not the search vectors, not a log of what was asked. A request and its answer exist in memory while the job runs and are gone when the connection stream ends. The node never reports anywhere, has no account, and talks to nothing outside your network. Its normal log lines say things like "paired with X" and "ready", not what anyone asked.

One caveat to be straight about: the process necessarily sees the text while it is working on it, so anyone who can run code as the same user on that machine could in principle read its memory while a job runs.

## How it works

- **Transport.** QUIC over UDP (the `quinn` library), TLS 1.3, ALPN `marginalia/1`. One connection per phone, one stream per request.
- **Identity.** Every device, node and phone, makes itself a self-signed certificate once. There are no certificate authorities and no names to check. Each side knows the other by the fingerprint (BLAKE3 hash) of its certificate.
- **Mutual pinning.** After pairing, the phone only talks to a node whose fingerprint it saved, and the node only does work for phones whose fingerprint it saved. A phone that is not on the list gets as far as asking to pair and nothing else.
- **Discovery.** The node announces itself with mDNS as `_marginalia._udp` so phones on the same network find it without typing an address. The announcement carries the protocol version, the first 16 characters of the node fingerprint and the node name. Nothing in it grants access.
- **Connecting (the normal way).** The phone finds the node by itself. In the app, Settings, Compute nodes, it lists the compute nodes it found and asks "Connect to this compute node?". When the person taps Trust, the phone saves the node's fingerprint and the node lets the phone in, with no code and nothing to do at the node. If several compute nodes are found the phone asks once for all of them. The node keeps nothing about jobs, so a phone that gets in can only borrow the node's speed, and the question that matters, whether to send your text to this compute node, is answered on the phone.
- **Locking.** `marginalia-node lock` (or `run --lock`) makes new phones wait instead. A waiting phone is allowed by a phone that is already trusted (the app shows "Let X use this compute node?") or with `marginalia-node approve`. `unlock` goes back to the default.
- **Pairing with a code (strict).** Run `marginalia-node pair`. It prints a one-time code such as `K7QM-4TDX`, valid for 5 minutes and for five wrong guesses. On the phone you type the code under "Pair with a code instead". The phone connects without trusting anything yet and sends a proof that it knows the code. The proof is a keyed hash of the code (stretched with scrypt), the secret of that exact TLS session and both certificate fingerprints. The code itself is never sent. The node checks the proof and replies with its own proof. Only when the phone has checked the node's proof does it save the node's fingerprint. Because the proof is tied to the TLS session, a device sitting in the middle and relaying messages cannot make pairing succeed: its two sessions have different secrets. This is covered by the tests in `proto/tests/handshake.rs`. Use this route if you do not want to trust whatever answers on the network the first time.
- **Jobs.** Chat (streamed tokens), embeddings (search vectors), transcription of recorded audio (see below) and a status request. Jobs have a priority: interactive (someone is waiting), background (notes, write-ups) and bulk (indexing). The node serves them in that order and tells the phone how many jobs are ahead of it.

## Requirements

- A machine on the same network as the phones. Linux x86_64 and ARM64 are the tested targets. Windows 11 works (see the Windows section), macOS is untested.
- Rust 1.85 or newer, CMake and a C++ compiler (the llama.cpp engine is built from source).
- For the GPU build: an NVIDIA card and the CUDA toolkit. Without them use the CPU-only build.
- The model files (see below). Enough disk and memory for the models (about 3.7 GB on disk; chat and search need a few GB of memory, the speech model about 0.8 GB more while it is loaded and 1 GB extra for a 75 minute recording).

## Build

```sh
scripts/node/build.sh              # CUDA build; CUDAARCHS defaults to 89 (RTX 40 series)
CUDAARCHS=86 scripts/node/build.sh # another card, e.g. RTX 30 series
scripts/node/build.sh --cpu-only   # no CUDA; also what a Raspberry Pi needs
```

The binary ends up at `target/release/marginalia-node` in the repository root. The first build takes a long time because it compiles llama.cpp. Build only the node with `cargo build --release -p marginalia-node`; do not build the whole workspace, the phone core in `core/` is built separately for Android.

## Models

Put these files in the models folder:

| File | Used for |
| --- | --- |
| `gemma-4-E2B-it-Q4_0.gguf` | chat (write-ups, answers), the default. The larger `gemma-4-E4B-it-Q4_0.gguf` (`--chat-model e4b`) is used when chosen from the tray menu or with `chat-model e4b` |
| `embeddinggemma-2-Q8_0.gguf` | search (embeddings) |
| `ggml-large-v3-turbo-q5_0.bin` | speech to text on recorded audio (about 570 MB, from the `ggerganov/whisper.cpp` repository on Hugging Face) |

Get them with `marginalia-node models download` (works the same on Linux and Windows, see below). The default folder is `~/.local/share/marginalia/models/` on Linux (`%APPDATA%\marginalia\models` on Windows). Use another folder with `--models DIR`, or point at single files with `--chat-model FILE`, `--embed-model FILE` and `--speech-model FILE`. Each job type is served only when its file is present, and the status says which. A node with no speech model works as before and does not offer transcription. `run --no-speech` leaves the speech model out even when the file is there. If none of the three is found the node refuses to start.

At start the node runs a short measurement and reports tokens per second for chat and passages per second for search. Phones use those numbers to decide whether this node is worth using for a given job.

## Commands

```sh
marginalia-node models list
marginalia-node models download [--dir DIR] [--only chat|search|speech] [--chat-model e4b|e2b] [--skip-speech]
marginalia-node chat-model [e2b|e4b]   # show or choose the chat model, restart the node to apply. The tray menu has the same choice and restarts the node itself
marginalia-node run --chat e2b|e4b      # choose it for one run
marginalia-node run [--port 47615] [--name NAME] [--models DIR] [--cpu] [--speech-model FILE] [--no-speech] [--window-max-seconds 120]
marginalia-node pair
marginalia-node join <addr:port> <code>
marginalia-node peers
marginalia-node unjoin <id or name>
marginalia-node status
marginalia-node pause | resume
marginalia-node stop
marginalia-node revoke <id or name>
marginalia-node probe <addr:port> [--code CODE] [--knock] [--only status|embed|chat] [--transcribe FILE.wav] [--language en] [--window] [--prompt TEXT]
```

- `models list` shows each expected model file, whether it is present, its size and the folder. `models download` fetches what is missing from Hugging Face: it continues a stopped download (`.part` file, HTTP Range), shows MB, percent, speed and time left on one updating line, checks the size and only then renames the file, skips files that are complete, and retries with growing pauses when the network drops. If a repository asks for a login (HTTP 401 or 403) set the `HF_TOKEN` environment variable, it is sent only to Hugging Face and never printed. `--chat-model e4b` fetches the larger chat model, which the node uses instead of E2B whenever it is in the models folder (or when started with `run --chat-model FILE`). The Windows scripts fetch E4B. `models download` fetches all three files by default, `--skip-speech` leaves out the speech model and `--only speech` fetches just that one. `run` tells you about this command when the models are missing.
- `run` starts the node in the foreground and stops on Ctrl-C. `--cpu` keeps the models off the graphics card at run time (the build still has to match). `--name` is what phones show; it defaults to the machine's name.
- `pair` asks the running node for a one-time code and prints it with the addresses the node can be reached at. On the phone: Settings, Compute nodes, Add a compute node.
- `status` prints what the running node serves, its measured speed, the paired phones with their ids and the peers (see "Working together").
- `join` makes this node a peer of another compute node, `peers` lists the peers and whether they answer, `unjoin` forgets one. See "Working together".
- `stats` prints how much work the node has done: jobs, failures, tokens, passages and seconds of audio transcribed, speed, per phone totals and the size and duration of recent jobs. Numbers only, never any content. The counters live in memory and restart from zero with the node.
- `approve` / `deny` answer a phone that is waiting (when locked). `lock` / `unlock` switch between the two modes.
- `pause` makes the node turn new jobs (chat, search and transcription) away (the phone gets "paused" and uses its own speed instead). The models stay loaded, jobs that are already running finish, and `status` shows `"paused": true`. `resume` takes jobs again. A pause lives in memory only, so a restarted node is never paused.
- `stop` stops the running node cleanly: it stops accepting phones and new jobs, lets running jobs finish for up to 10 seconds, leaves the network (mDNS) and exits with status 0. Ctrl-C and a system stop request (SIGTERM) do the same. With the systemd unit a clean exit is not restarted, use `systemctl --user start marginalia-node` to bring it back.
- `revoke` forgets a phone. Use the id from `status` or the phone's name. That phone is a stranger again and has to pair anew.
- `probe` behaves like a phone: connects to an address, optionally pairs with `--code` (or asks to be let in with `--knock`), and tries each job type (`--only status`, `embed` or `chat` sends just one). It is for checking that a node works, for example from another machine. `probe ADDR --transcribe FILE.wav` instead sends a recording (16 kHz, mono, 16 bit `.wav`, or a raw `.pcm`) like the phone does, and prints the progress, each segment with its times, and the speed in times faster than real time. `--language` sets the language, otherwise it is detected. `--window` sends it as a short live window (interactive priority) and prints the time from the last audio frame to `Done`, `--prompt TEXT` sends what was said just before.

`pair`, `status`, `pause`, `stop`, `revoke` and the tray icon talk to the running node through a control channel on `127.0.0.1:47616` (change with `--admin-port`). It only listens on this machine, so these commands must be run on the same machine as `run`.

`status` prints one JSON object with the node name, whether it is paused or locked, `running_jobs`, `waiting_jobs`, `served` (chat and search jobs since start), `uptime_s`, the number of `phones`, the models and measured speed, the paired phones, the blocked phones, the `peers` (name, id, address, whether they answer, their load and what they serve), `forwarded` (jobs a peer answered for this node since it started) and the addresses the node can be reached at. `stats` adds failures and per phone totals. Both hold counts, names and timings only.

State (identity and paired list) lives in `~/.config/marginalia-node` on Linux. Change with `--state-dir`. Delete that folder to make the node a new device that no phone knows.

## Working together

Several compute nodes on one network can work as a group. The phone still talks to one node at a time and does not need to know about the others. This is built for phone records and for chat and search jobs. Standby mode, roles and splitting one model over several machines are only designed (see `DESIGN.md`).

- **Joining.** On the node that is already there run `marginalia-node pair`. It prints a code and its address. On the other node run `marginalia-node join ADDRESS CODE`, for example `marginalia-node join 192.168.1.20:47615 K7QM-4TDX`. The joining node pairs the way a phone does, then tells the other that it is a compute node and where it listens. Both save the other's certificate fingerprint, name and address in `peers.json` (readable by your user only) and trust that certificate in both directions. `marginalia-node peers` lists them, `status` includes them, `marginalia-node unjoin NAME_OR_ID` forgets one on this node and tells it to forget this node too. Nodes see each other on the network through mDNS: `peers` and `status` also list the nodes that were seen and not joined, the tray window shows them, and `join` takes the name of one of them, for example `marginalia-node join "Kitchen node" K7QM-4TDX`. Seeing a node never joins it, the code from `pair` is still needed. Nodes on another subnet or a VPN are joined by address.
- **Phones.** Allowed phones, blocked phones and the notes that a phone was removed or unblocked are copied between peers, every 3 seconds and at once when one changes here (pair, approve, block, unblock, revoke). The newest note about a phone wins. A block beats an allow for the same phone until a later unblock. So a phone paired with one node is accepted by its peers, and a phone blocked on one node is blocked on all of them. An unblock does not bring a pairing back, the phone asks again. Nodes should have roughly the same time. Peers never share job content.
- **Jobs.** Each node asks its peers for their load and speed every 2 seconds. When a chat or search job arrives from a phone and this node is clearly the worse choice (its expected wait is more than 1.5 times a peer's plus half a second, or it is paused, or it has no model for that job) the job is run on the best peer and the answer is passed back through the same stream. If the peer cannot be reached, turns the job away or says nothing for 10 seconds before the first token, the next peer is tried and then this node. If a peer stops after the first words, this node carries on from the words already sent, so the phone sees one answer. A job that arrives from a peer is never passed on again. Transcription is never passed on. A node's load counts all its jobs, including transcription.
- **Check it.** `compute-node/node/tests/cluster.sh` starts three nodes on one machine (own ports and state folders, chat on the processor) and checks all of the above. See `STATUS.md`.

## Transcribing recordings

A node with the speech model can write out a recording with a large Whisper model (`large-v3-turbo`, 5 bit), so the phone can replace the transcript its small live model made with a better one after the meeting.

- **Protocol.** The phone sends a `Transcribe` request (language or none to detect it, length, priority, and an optional prompt), then the audio as raw PCM (16 bit little endian, mono, 16 kHz) in frames of up to about 1 MB on the same stream, then an end marker. The node answers with `Queued`, `Working` (a heartbeat every 2 seconds), `Progress`, `Segment` (start and end in milliseconds from the start of the audio, sent as soon as it is final), `Done` and `Error`.
- **Limits.** Up to 3 hours of audio per job (about 350 MB in memory), longer is refused with a clear message. One whole recording is transcribed at a time. Others wait their turn (the phone is told how many are ahead), and a node with more than 3 waiting turns further ones away as busy.
- **How it works.** Whisper's own 30 second windows run over the whole recording, so any length works, each window knowing what came before. It decodes with a beam of 5 on the graphics card (greedy on the processor), turns on flash attention on the graphics card, falls back to a higher temperature when a stretch looks like a loop or has low confidence, and drops text that is made up over silence (sound tags, one character repeated, a stuck repeated line). When the language is left open on a long recording it is chosen from the three loudest stretches, so a quiet start does not decide it.
- **Live windows.** While a meeting is recorded the phone can send short windows of audio (20 to 60 seconds) to correct its live transcript. A request with interactive priority and a length of at most `window_max_s` seconds (`run --window-max-seconds N`, default 120, shown in `status` and told to phones in the capabilities) is a window. Windows have their own lane: one window is decoded at a time, next to one long recording, so a window never waits behind an hour of audio. They wait for each other in the order they came, a phone may have 2 windows in flight and the node 4 waiting, more are answered "busy", and a window that has waited 20 seconds is answered "busy" too because its words have gone stale. Anything longer than `window_max_s` is not an error, it goes through the long lane like a recording, and windows shorter than 15 seconds are fine. They use their own settings: greedy decoding (the final pass over the whole recording uses the beam of 5 and replaces them), no restart at a hotter temperature, and the language given by the phone. Both lanes use the same loaded speech model, each job with its own decoder state. On a processor there is one lane only, because two decodes at once would only slow each other down.
- **Prompt.** `prompt` is what was said just before the window. It is given to the model as its previous text (the last 800 characters, at most 200 tokens, cut at a word) so names and spelling carry over. It is never returned as a segment, never logged and not kept. Segment times are from the start of the window whether or not there is a prompt. A window with nothing but a quiet room in it returns no segments at once without using the model. Segments with no sound under them are dropped, as are a scrap of up to 3 words the model was unsure of (what it makes up over noise), and a short scrap that only reads the prompt back.
- **Processor only.** With `run --cpu`, or a node without a usable graphics card, the large model is slow: about half of real time was measured on a busy 12 thread machine (an 11 second clip took 20 seconds), so an hour of meeting would take hours. Use a graphics card for this job. The node does not measure the speed at start, so `transcribe_x` stays 0 until the first recording.
- **Memory.** The speech model takes about 0.8 GB of graphics memory while the node runs. A job adds a decoder state (about 0.3 GB on the card) that is freed when the job ends, so a window running next to a long recording adds a second one. In memory a recording takes 2 bytes per sample while it arrives and 4 bytes per sample (about 230 MB per hour) while it is transcribed, and the node hands the memory back when the job is over.
- **Stopping.** If the phone closes the connection (or a heartbeat cannot be sent) the node stops whisper at once and frees the audio.
- **Privacy.** The audio and the text exist only in memory while the job runs. Nothing is written to disk or to the log. `stats` shows the number of jobs, seconds of audio and speed only, with windows counted apart from whole recordings.
- **Speed.** `status` and the phone see `transcribe_x`, how many times faster than real time the node is. It is 0 until the first recording of 20 seconds or more has been done, after which it follows a moving average of real jobs. See `STATUS.md` for the measured figures.

## Tray icon

`marginalia-node-tray` is a small separate program that puts the node in the Ubuntu top bar (or the Windows system tray). It talks only to the node's local control channel (`127.0.0.1:47616`, change with `--admin-port`) and shows counts, names and timings. It never sees anything a phone asked.

```sh
cargo build --release -p marginalia-node-tray   # pure Rust, no system packages
scripts/node/install-linux.sh         # installs it and adds it to ~/.config/autostart
marginalia-node-tray --print                    # prints the menu as text and exits, to check the setup
```

- **Icon.** A ring with a dot: green when the node is running, amber when it is paused, grey when it is not running. Hover for a line such as "falcon: running, 12 jobs served, 2 phones". The icon is polled every 5 seconds.
- **Left click: the details panel.** A small dark window (about 340 by 540 pixels, no title bar, always on top) opens near the icon. It closes with Escape, with the x, when you click anywhere else, or with a second click on the icon. It shows, from top to bottom:
  - the node name with a state pill (Running, Paused, Not running) and a line saying what pause and lock mean right now
  - phones that are asking to join, with Allow and Deny, when there are any
  - big numbers: jobs served, phones, uptime
  - speed bars for chat (tokens a second), search (passages a second) and, when the node has the speech model, speech (times faster than real time), each with its model name
  - live activity: one pulsing dot per running or waiting job, a strip of the last 2 minutes (kept in memory in the panel only), and the failure count
  - the paired phones with their job counts and when they were last seen, each with Block and Remove (both ask first)
  - the addresses the node can be reached at
  - buttons: Pause or Resume, Show pairing code, Lock new phones or Unlock, Stop node (asks first)
- **Pairing code.** It appears inside the panel in large type with a 5 minute countdown, and is hidden otherwise. It is never in the menu and nothing is copied.
- **Not running.** The panel shows a friendly empty state with a Start node button.
- **Right click: a short menu.** The status line, Pause or Resume, Show details (opens the panel), Stop node and Quit tray. When the node is not running: Start node and Quit tray.
- **Start node.** On Linux it starts the `marginalia-node` user service if that is installed, otherwise it runs `marginalia-node run` from next to the tray or from PATH.
- **How it is built.** The tray and the panel are the same program. The tray starts `marginalia-node-tray --panel` as a second process when the panel is wanted and stops it when it is not, so the tray stays small. The panel polls the node every 1.5 seconds while it is open. On Windows the panel appears bottom right above the taskbar, on X11 top right, and on Wayland the compositor chooses the place because windows cannot position themselves there.
- **Ubuntu.** The top bar shows the icon through the AppIndicator extension, which ships and is on by default. On a plain GNOME without it: `sudo apt install gnome-shell-extension-appindicator` and log in again. The icon uses the StatusNotifierItem protocol over D-Bus (the `ksni` crate) and the panel uses `egui` with OpenGL, so no GTK or other development packages are needed. The OpenGL, X11 and Wayland libraries are loaded at run time and are on any desktop.
- Short messages (for example "Could not pause the node") use `notify-send`, so the `libnotify-bin` package must be installed (Ubuntu has it).
- On Windows it uses the `tray-icon` crate. It builds and runs, and `scripts\windows\setup.ps1` starts it at login. The right-click menu also lets you switch the chat model between E2B and E4B, which restarts the node.
- It is built on its own and is not part of the default workspace build.

## Firewall and network

- **UDP 47615 inbound** must be allowed for phones to connect (QUIC runs on UDP, so a TCP rule does nothing). If you change `--port`, open that port instead.
- **UDP 5353** (mDNS multicast) must not be blocked on the local network for discovery to work. On the node it is outbound/multicast traffic, which most firewalls permit by default.
- **Windows** shows a firewall prompt the first time the node runs. Allow it on private networks. If you dismissed it, add an inbound rule for UDP 47615. `scripts\windows\setup.ps1` adds the rules for UDP 47615 and 5353 for you.
- **Linux with ufw:** `sudo ufw allow 47615/udp`. With firewalld: `sudo firewall-cmd --add-port=47615/udp --permanent && sudo firewall-cmd --reload`.
- **Guest Wi-Fi, client isolation and some mesh systems** stop devices from seeing each other, which breaks mDNS discovery and sometimes direct connections. If the phone cannot find the node, check that both are on the same ordinary network, then add the compute node by address (`marginalia-node pair` prints it, for example `192.168.1.20:47615`). If isolation is on, the address will not work either; turn it off or put both on the same network.
- The node does not open any port to the internet and does not need a router rule. Do not forward the port.

## Windows (beta)

Windows support is in beta. The core features work on Linux and are the supported path for now. Windows will be finished later.

`scripts/windows/` has PowerShell scripts for a one command setup (`.\scripts\windows\setup.ps1`): prerequisites check with the exact winget commands, GPU backend detection (CUDA, Vulkan or CPU), build, install to `%LOCALAPPDATA%\MarginaliaNode`, model download (`marginalia-node.exe models download`), Task Scheduler entries for the node and the tray at logon, and Windows Firewall rules. See `scripts/windows/README.md` for the quick start and a troubleshooting table, and `packaging/windows.md` for the manual steps. The scripts and the node were run on Windows 11 with an NVIDIA RTX 4060 (CUDA). `verify.ps1` passes all 19 checks there, including the self tests for search, chat and speech. Still to try by hand are the tray clicks, a reboot and a real phone on the same Wi-Fi.

Needs: Visual Studio Build Tools (C++), CMake 4.2 or newer when Visual Studio 2026 is installed, Rust 1.95 or newer, LLVM, and the CUDA toolkit for the graphics card. `setup.ps1` checks each one and upgrades CMake and Rust when they are too old. The chat model is E2B unless you switch it with `marginalia-node chat-model e4b` or from the tray menu. `setup.ps1` downloads both.

## Raspberry Pi

A Pi 4 or 5 (64-bit OS) can run the node, CPU only (`scripts/node/build.sh --cpu-only`, or the Dockerfile in `packaging/`). Be honest about what to expect: on a Pi, chat is slower than a recent phone's GPU, so the phone's speed comparison will normally skip the Pi for chat and keep doing it locally. Search (embeddings) is cheaper and the Pi is still useful there, especially for indexing a lot of documents while the phone is on charge. Building on the Pi itself takes a long while and needs swap or at least 4 GB of RAM; cross-building or using the Dockerfile on a faster machine is easier.

## Protocol versioning

`PROTO_VERSION` (currently 1) and the ALPN string `marginalia/1` identify the wire format. The version is part of the TLS handshake through ALPN, so a phone and a node that disagree refuse each other cleanly instead of misreading messages. The version also appears in status and in the mDNS announcement. Changes that old peers can read keep the number; changes they cannot read bump it, and the phone and node need to be updated together. There is no negotiation of older versions yet.

## Graphs and logs

- The tray window draws the last two minutes of processor, memory, graphics card and network use. The node reads them once a second (`resources.rs`,
  the graphics card through `nvidia-smi` when it is installed, so there is no graphics line on other cards) and keeps 15 minutes in memory. The admin
  channel answers `{"cmd": "resources", "n": 120}`.
- Everything the node says is also written to `logs/node.log` in the state folder (`~/.config/marginalia-node/logs` on Linux). The file is replaced at
  5 MB and the last 5 are kept. It holds events and numbers only, never the text of notes, questions, recordings or photos. A line with the machine's
  load is added every minute. `marginalia-node logs` shows the folder and the last lines, `--follow` keeps going, `RUST_LOG=marginalia_node=debug`
  gives more detail. When something goes wrong, send the last `node.log`.
- A node with the vision projector next to its chat model also reads photos for phones (`vision` in `status`). Download it with `marginalia-node models download --only vision`.
- **Telling speakers apart (opt-in).** After a meeting a phone can ask a node to tell the speakers apart on the kept recording. The node needs the helper program
  `marginalia-node-speakers` next to it (`scripts/node/build.sh` and `make` build it, the install scripts copy it) and the two models
  (`marginalia-node models download --only speakers`, about 33 MB). Without either the node does not offer it and the phone does it itself. The audio is a temporary
  file that is removed when the job ends. `marginalia-node probe ADDRESS --speakers FILE.wav` tries it.

## Not built yet

These are plans, not features:

- Peers sharing their member lists, standby mode, node roles, taking work from a busy peer and splitting one model over several nodes. The plan is in `DESIGN.md`. Joining, shared phone records and forwarding chat and search jobs are built (see "Working together").
- Live speech as one open audio stream. Not planned: short windows of a recording in progress are served (see above) and the phone cuts them, so a lost node costs one window and nothing else.
- Fan-out of one write-up across several nodes.
- Per-client priority and fairness (today the order is by job priority, not by who asked).
- A true Windows service wrapper (for now `scripts/windows/setup.ps1` makes login tasks).
- Access from outside your network (the node is for the local network only).

## Files

- `proto/` shared wire protocol, identity and pairing (crate `marginalia-proto`), with tests in `proto/tests/`.
- `node/` the node program, with `tests/cluster.sh` that starts three nodes on one machine.
- `tray/` the tray icon program (crate `marginalia-node-tray`).
- `vendor/` a patched `llama-cpp-sys-2` that includes support for EmbeddingGemma 2.
- The scripts live in the top level `scripts/` folder: `scripts/node/` for Linux and `scripts/windows/` for Windows.
- `packaging/` systemd unit, tray autostart entry, Windows notes, CPU-only Dockerfile.
