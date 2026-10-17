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

1. Install Rust (MSVC toolchain), Visual Studio Build Tools (C++), CMake, LLVM (for bindgen, set `LIBCLANG_PATH`)
   and the CUDA toolkit if you have an NVIDIA card. `git config core.longpaths true` before cloning, the vendored
   llama.cpp has long paths.
2. From the repo root in a Developer PowerShell: `cargo build --release -p marginalia-node --features cuda`
   (leave `--features cuda` out for CPU only, use `--features vulkan` for AMD or Intel). Set `CUDAARCHS` to your
   card's architecture (89 for the RTX 40 series, 86 for the 30 series).
3. Models go in `%APPDATA%\marginalia\models` (`gemma-4-E2B-it-Q4_0.gguf`, `embeddinggemma-2-Q8_0.gguf`) or pass
   `--models DIR`. State lives in `%APPDATA%\marginalia-node`.
4. `target\release\marginalia-node.exe run`, then allow it through the Windows firewall (private networks, UDP 47615
   and mDNS 5353).
5. Tray: `cargo build --release -p marginalia-node-tray`, run `marginalia-node-tray.exe` (see `packaging/windows.md`).
   Expect small fixes in `windows.rs`.

## The Android app on Windows

The scripts are bash. Use Git Bash or WSL for `scripts/*.sh`, or run the steps by hand: `cargo ndk` for the
`core` crate (`scripts/build-native.sh` shows the flags and the two CPU variants), then `gradlew assembleRelease`.
Models for the APK are linked by `scripts/stage-models.sh` (symlinks, so use WSL or copy the files into
`models-stage/models`). When more than one device is attached, set `ANDROID_SERIAL`.

## Next steps (in order)

1. Run the node and the tray on Windows and fix what breaks.
2. Cluster: peers pair and gossip, entry node forwards jobs, block records spread, hot-hot and standby (`DESIGN.md`).
3. Speech on nodes: re-transcribe the kept audio with large-v3 after a meeting, then live streaming with failover.
4. A Windows service wrapper (the tray plus Task Scheduler is the interim answer).
