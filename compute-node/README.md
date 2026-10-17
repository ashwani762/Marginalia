# Marginalia compute node

A small program you run on a machine on your network. It does the heavy AI work for the Marginalia phone app (writing up notes, answering questions, turning text into search vectors) so the phone does not have to. Phones you have paired with it send a job over the local network, the node runs it and sends the answer back.

It is optional. The phone app works without it. If a node is reachable, the phone can use it for the jobs it is faster at.

## What it keeps, and what it does not

The node is stateless with respect to your work. It keeps exactly two things on disk:

- its own identity (a self-signed certificate and key, made on first run), and
- the list of paired phones (a fingerprint and a name for each).

Nothing about a job is stored: not the text sent in, not the answer, not the search vectors, not a log of what was asked. A request and its answer exist in memory while the job runs and are gone when the connection stream ends. The node never reports anywhere, has no account, and talks to nothing outside your network. Its normal log lines say things like "paired with X" and "ready", not what anyone asked.

One caveat to be straight about: the process necessarily sees the text while it is working on it, so anyone who can run code as the same user on that machine could in principle read its memory while a job runs.

## How it works

- **Transport.** QUIC over UDP (the `quinn` library), TLS 1.3, ALPN `marginalia/1`. One connection per phone, one stream per request.
- **Identity.** Every device, node and phone, makes itself a self-signed certificate once. There are no certificate authorities and no names to check. Each side knows the other by the fingerprint (BLAKE3 hash) of its certificate.
- **Mutual pinning.** After pairing, the phone only talks to a node whose fingerprint it saved, and the node only does work for phones whose fingerprint it saved. A phone that is not on the list gets as far as asking to pair and nothing else.
- **Discovery.** The node announces itself with mDNS as `_marginalia._udp` so phones on the same network find it without typing an address. The announcement carries the protocol version, the first 16 characters of the node fingerprint and the node name. Nothing in it grants access.
- **Connecting (the normal way).** The phone finds the node by itself. In the app, Settings, Compute nodes, it lists the compute nodes it found and asks "Trust this compute node?". When the person taps Trust, the phone saves the node's fingerprint and the node lets the phone in, with no code and nothing to do at the node. If several compute nodes are found the phone asks once for all of them. The node keeps nothing about jobs, so a phone that gets in can only borrow the node's speed, and the question that matters, whether to send your text to this compute node, is answered on the phone.
- **Locking.** `marginalia-node lock` (or `run --lock`) makes new phones wait instead. A waiting phone is allowed by a phone that is already trusted (the app shows "Let X use this compute node?") or with `marginalia-node approve`. `unlock` goes back to the default.
- **Pairing with a code (strict).** Run `marginalia-node pair`. It prints a one-time code such as `K7QM-4TDX`, valid for 5 minutes and for five wrong guesses. On the phone you type the code under "Pair with a code instead". The phone connects without trusting anything yet and sends a proof that it knows the code. The proof is a keyed hash of the code (stretched with scrypt), the secret of that exact TLS session and both certificate fingerprints. The code itself is never sent. The node checks the proof and replies with its own proof. Only when the phone has checked the node's proof does it save the node's fingerprint. Because the proof is tied to the TLS session, a device sitting in the middle and relaying messages cannot make pairing succeed: its two sessions have different secrets. This is covered by the tests in `proto/tests/handshake.rs`. Use this route if you do not want to trust whatever answers on the network the first time.
- **Jobs.** Chat (streamed tokens), embeddings (search vectors) and a status request. Jobs have a priority: interactive (someone is waiting), background (notes, write-ups) and bulk (indexing). The node serves them in that order and tells the phone how many jobs are ahead of it.

## Requirements

- A machine on the same network as the phones. Linux x86_64 and ARM64 are the tested targets. Windows is expected to work, macOS is untested.
- Rust 1.85 or newer, CMake and a C++ compiler (the llama.cpp engine is built from source).
- For the GPU build: an NVIDIA card and the CUDA toolkit. Without them use the CPU-only build.
- The two model files (see below). Enough disk and memory for both models (a few GB; check the file sizes).

## Build

```sh
compute-node/build.sh              # CUDA build; CUDAARCHS defaults to 89 (RTX 40 series)
CUDAARCHS=86 compute-node/build.sh # another card, e.g. RTX 30 series
compute-node/build.sh --cpu-only   # no CUDA; also what a Raspberry Pi needs
```

The binary ends up at `target/release/marginalia-node` in the repository root. The first build takes a long time because it compiles llama.cpp. Build only the node with `cargo build --release -p marginalia-node`; do not build the whole workspace, the phone core in `core/` is built separately for Android.

## Models

Put these two files in the models folder:

| File | Used for |
| --- | --- |
| `gemma-4-E2B-it-Q4_0.gguf` | chat (write-ups, answers) |
| `embeddinggemma-2-Q8_0.gguf` | search (embeddings) |

Get them with `marginalia-node models download` (works the same on Linux and Windows, see below). The default folder is `~/.local/share/marginalia/models/` on Linux (`%APPDATA%\marginalia\models` on Windows). Use another folder with `--models DIR`, or point at single files with `--chat-model FILE` and `--embed-model FILE`. If only one of the two files is present the node serves only that job type and says so in its status. If neither is found it refuses to start.

At start the node runs a short measurement and reports tokens per second for chat and passages per second for search. Phones use those numbers to decide whether this node is worth using for a given job.

## Commands

```sh
marginalia-node models list
marginalia-node models download [--dir DIR] [--only chat|search] [--chat-model e2b|e4b]
marginalia-node run [--port 47615] [--name NAME] [--models DIR] [--cpu]
marginalia-node pair
marginalia-node status
marginalia-node pause | resume
marginalia-node stop
marginalia-node revoke <id or name>
marginalia-node probe <addr:port> [--code CODE]
```

- `models list` shows each expected model file, whether it is present, its size and the folder. `models download` fetches what is missing from Hugging Face: it continues a stopped download (`.part` file, HTTP Range), shows MB, percent, speed and time left on one updating line, checks the size and only then renames the file, skips files that are complete, and retries with growing pauses when the network drops. If a repository asks for a login (HTTP 401 or 403) set the `HF_TOKEN` environment variable, it is sent only to Hugging Face and never printed. `--chat-model e4b` fetches the larger chat model, which the node only uses when started with `run --chat-model FILE`. `run` tells you about this command when the models are missing.
- `run` starts the node in the foreground and stops on Ctrl-C. `--cpu` keeps the models off the graphics card at run time (the build still has to match). `--name` is what phones show; it defaults to the machine's name.
- `pair` asks the running node for a one-time code and prints it with the addresses the node can be reached at. On the phone: Settings, Compute nodes, Add a compute node.
- `status` prints what the running node serves, its measured speed and the paired phones with their ids.
- `stats` prints how much work the node has done: jobs, failures, tokens and passages, speed, per phone totals and the size and duration of recent jobs. Numbers only, never any content. The counters live in memory and restart from zero with the node.
- `approve` / `deny` answer a phone that is waiting (when locked). `lock` / `unlock` switch between the two modes.
- `pause` makes the node turn new chat and search jobs away (the phone gets "paused" and uses its own speed instead). The models stay loaded, jobs that are already running finish, and `status` shows `"paused": true`. `resume` takes jobs again. A pause lives in memory only, so a restarted node is never paused.
- `stop` stops the running node cleanly: it stops accepting phones and new jobs, lets running jobs finish for up to 10 seconds, leaves the network (mDNS) and exits with status 0. Ctrl-C and a system stop request (SIGTERM) do the same. With the systemd unit a clean exit is not restarted, use `systemctl --user start marginalia-node` to bring it back.
- `revoke` forgets a phone. Use the id from `status` or the phone's name. That phone is a stranger again and has to pair anew.
- `probe` behaves like a phone: connects to an address, optionally pairs with `--code`, and tries each job type. It is for checking that a node works, for example from another machine.

`pair`, `status`, `pause`, `stop`, `revoke` and the tray icon talk to the running node through a control channel on `127.0.0.1:47616` (change with `--admin-port`). It only listens on this machine, so these commands must be run on the same machine as `run`.

`status` prints one JSON object with the node name, whether it is paused or locked, `running_jobs`, `waiting_jobs`, `served` (chat and search jobs since start), `uptime_s`, the number of `phones`, the models and measured speed, the paired phones and the addresses the node can be reached at. `stats` adds failures and per phone totals. Both hold counts, names and timings only.

State (identity and paired list) lives in `~/.config/marginalia-node` on Linux. Change with `--state-dir`. Delete that folder to make the node a new device that no phone knows.

## Tray icon

`marginalia-node-tray` is a small separate program that puts the node in the Ubuntu top bar (or the Windows system tray). It talks only to the node's local control channel (`127.0.0.1:47616`, change with `--admin-port`) and shows counts, names and timings. It never sees anything a phone asked.

```sh
cargo build --release -p marginalia-node-tray   # pure Rust, no system packages
compute-node/packaging/install-linux.sh         # installs it and adds it to ~/.config/autostart
marginalia-node-tray --print                    # prints the menu as text and exits, to check the setup
```

- **Icon.** A ring with a dot: green when the node is running, amber when it is paused, grey when it is not running. Hover for a line such as "falcon: running, 12 jobs served, 2 phones". The icon is polled every 5 seconds.
- **Left click: the details panel.** A small dark window (about 340 by 540 pixels, no title bar, always on top) opens near the icon. It closes with Escape, with the x, when you click anywhere else, or with a second click on the icon. It shows, from top to bottom:
  - the node name with a state pill (Running, Paused, Not running) and a line saying what pause and lock mean right now
  - phones that are asking to join, with Allow and Deny, when there are any
  - big numbers: jobs served, phones, uptime
  - speed bars for chat (tokens a second) and search (passages a second), each with its model name
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
- On Windows it uses the `tray-icon` crate. That part could not be built or tried where it was written. See `packaging/windows.md` for starting it at login.
- It is built on its own and is not part of the default workspace build.

## Firewall and network

- **UDP 47615 inbound** must be allowed for phones to connect (QUIC runs on UDP, so a TCP rule does nothing). If you change `--port`, open that port instead.
- **UDP 5353** (mDNS multicast) must not be blocked on the local network for discovery to work. On the node it is outbound/multicast traffic, which most firewalls permit by default.
- **Windows** shows a firewall prompt the first time the node runs. Allow it on private networks. If you dismissed it, add an inbound rule for UDP 47615. `windows\setup.ps1` adds the rules for UDP 47615 and 5353 for you.
- **Linux with ufw:** `sudo ufw allow 47615/udp`. With firewalld: `sudo firewall-cmd --add-port=47615/udp --permanent && sudo firewall-cmd --reload`.
- **Guest Wi-Fi, client isolation and some mesh systems** stop devices from seeing each other, which breaks mDNS discovery and sometimes direct connections. If the phone cannot find the node, check that both are on the same ordinary network, then add the compute node by address (`marginalia-node pair` prints it, for example `192.168.1.20:47615`). If isolation is on, the address will not work either; turn it off or put both on the same network.
- The node does not open any port to the internet and does not need a router rule. Do not forward the port.

## Windows

`windows/` has PowerShell scripts for a one command setup (`.\compute-node\windows\setup.ps1`): prerequisites check with the exact winget commands, GPU backend detection (CUDA, Vulkan or CPU), build, install to `%LOCALAPPDATA%\MarginaliaNode`, model download (`marginalia-node.exe models download`), Task Scheduler entries for the node and the tray at logon, and Windows Firewall rules. See `windows/README.md` for the quick start and a troubleshooting table, and `packaging/windows.md` for the manual steps. These scripts are not tested on a real Windows machine yet.

## Raspberry Pi

A Pi 4 or 5 (64-bit OS) can run the node, CPU only (`compute-node/build.sh --cpu-only`, or the Dockerfile in `packaging/`). Be honest about what to expect: on a Pi, chat is slower than a recent phone's GPU, so the phone's speed comparison will normally skip the Pi for chat and keep doing it locally. Search (embeddings) is cheaper and the Pi is still useful there, especially for indexing a lot of documents while the phone is on charge. Building on the Pi itself takes a long while and needs swap or at least 4 GB of RAM; cross-building or using the Dockerfile on a faster machine is easier.

## Protocol versioning

`PROTO_VERSION` (currently 1) and the ALPN string `marginalia/1` identify the wire format. The version is part of the TLS handshake through ALPN, so a phone and a node that disagree refuse each other cleanly instead of misreading messages. The version also appears in status and in the mDNS announcement. Changes that old peers can read keep the number; changes they cannot read bump it, and the phone and node need to be updated together. There is no negotiation of older versions yet.

## Not built yet

These are plans, not features:

- Nodes that find, join and share work with each other (hot-hot, primary and standby, load balancing). The plan is in `DESIGN.md`.
- Live speech (streaming transcription) on the node.
- Fan-out of one write-up across several nodes.
- Per-client priority and fairness (today the order is by job priority, not by who asked).
- A true Windows service wrapper (for now `windows/setup.ps1` makes login tasks).
- Access from outside your network (the node is for the local network only).

## Files

- `proto/` shared wire protocol, identity and pairing (crate `marginalia-proto`), with tests in `proto/tests/`.
- `node/` the node program.
- `tray/` the tray icon program (crate `marginalia-node-tray`).
- `vendor/` a patched `llama-cpp-sys-2` that includes support for EmbeddingGemma 2.
- `windows/` PowerShell setup, build, model download, status and uninstall scripts.
- `packaging/` systemd unit, tray autostart entry, Linux install script, Windows notes, CPU-only Dockerfile.
