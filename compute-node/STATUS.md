# Status and how to carry on (written before moving development to Windows)

## What works (checked on Linux with a real phone)

- Phone finds nodes by itself (Android NSD), one tap trusts one or all of them, nodes let phones in without a code
  (lock mode and approve exist, pairing with a code stays as the strict route), block list per node.
- Notes, answers and search vectors run on nodes: about 100 tokens a second and 1000 passages a second on an
  RTX 4060 against about 15 and 1 on the phone. If no node answers, the phone does the work itself.
- Nodes are ranked by expected wait. A job that fails moves to the next node. A node lost halfway through an
  answer is replaced by another node, or by the phone, which carries on from the words already shown.
- Several jobs run side by side so every node is busy, and a meeting write-up writes its parts on several nodes.
- The node reports counts and timings only. `marginalia-node stats`, and a tray icon (Linux verified over D-Bus).

## Not verified yet

- Everything on Windows: the node build, the tray (`compute-node/tray/src/windows.rs` was written without being
  compiled), the Task Scheduler start at login, the firewall prompt, mDNS announcements from Windows.
- Pause, resume and stop on a running node were only compile checked (the tray agent did not run the node).
- Nodes finding and sharing work with each other (cluster) is designed in `DESIGN.md`, not built.
- Speech to text on nodes (post meeting large-v3 and live streaming) is not built, transcription is phone only.

## Building on Windows

The quick way is `.\compute-node\windows\setup.ps1` (see `compute-node/windows/README.md`). It is UNTESTED: the scripts and
the new `models` command were written on Linux, nothing here ran on Windows. What the scripts do:

- `setup.ps1`: checks Git, Rust (MSVC), Visual Studio Build Tools (C++), CMake, LLVM (libclang, sets `LIBCLANG_PATH`) and the
  CUDA toolkit when an NVIDIA card is found, and prints the exact `winget install` line for each missing one (`-Auto` runs them).
  Picks `cuda` (nvcc and an NVIDIA card, `CUDAARCHS` from `nvidia-smi --query-gpu=compute_cap`), else `vulkan` (`VULKAN_SDK`),
  else CPU. Builds node and tray in release, installs to `%LOCALAPPDATA%\MarginaliaNode`, runs `marginalia-node.exe models
  download`, registers Scheduled Tasks `Marginalia Node` (through `start-node.vbs`, hidden, log in `node.log`, restart on failure)
  and `Marginalia Node Tray` at logon, adds firewall rules for UDP 47615 and 5353 (Private profile, one admin prompt through
  `firewall.ps1`), starts both. Options `-Cpu -Vulkan -Cuda -SkipBuild -SkipModels -ModelsDir -Auto`.
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
3. Speech on nodes: re-transcribe the kept audio with large-v3 after a meeting, then live streaming with failover.
4. A Windows service wrapper (the tray plus Task Scheduler is the interim answer).
