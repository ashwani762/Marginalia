# Plan for the Windows session

You are continuing the work on a Windows machine (the Linux machine is dual booted with it). Work **only on the compute node
and its tray icon**. Do not edit the Android source (`app/`, `core/`). Everything below was built and verified on Ubuntu, nothing was
ever run on Windows.

## What the project is

Marginalia is an Android meeting notes app whose AI runs on the phone. `compute-node/` is an optional daemon on a PC that does the
AI work faster for phones on the same network: chat (Gemma 4 E2B Q4_0 via llama.cpp) and search embeddings (EmbeddingGemma 2 Q8_0).
It is stateless (keeps no job data), is found by the phone through mDNS (`_marginalia._udp`), speaks QUIC with pinned self-signed
certificates, and has a tray icon that shows status and opens a small details panel. Read `docs/ARCHITECTURE.md`,
`compute-node/README.md`, `compute-node/DESIGN.md` and `compute-node/STATUS.md` first.

## Goals

1. `marginalia-node.exe` builds, starts, loads both models, answers a phone.
2. `marginalia-node-tray.exe` shows an icon in the Windows tray, a left click opens the details panel, the right-click menu works
   (pause, resume, lock, stop, start node).
3. One command sets everything up: `compute-node\windows\setup.ps1` (build, download models, install, scheduled tasks at logon, firewall rules).
4. Documented, repeatable, no bash needed.

## Steps

1. **Clone** with `git config --global core.longpaths true` first (the vendored llama.cpp has long paths).
2. **Prerequisites:** Rust (MSVC toolchain), Visual Studio Build Tools (C++ workload), CMake, LLVM (`LIBCLANG_PATH`, needed by bindgen),
   CUDA toolkit if the GPU is NVIDIA. `setup.ps1` checks these and prints the `winget` commands.
3. **Build the node:** `cargo build --release -p marginalia-node --features cuda` (or `vulkan`, or no feature for CPU). Set `CUDAARCHS` to the
   card's architecture. The likely trouble spots are in `compute-node/vendor/llama-cpp-sys-2/build.rs` (MSVC flags, OpenMP, CUDA with
   a Visual Studio version the toolkit does not support). Fix them there, keeping Linux behavior.
4. **Models:** `target\release\marginalia-node.exe models download`. They go to `%APPDATA%\marginalia\models`. Never commit them.
5. **Run:** `marginalia-node.exe run`, then in another shell `status`, `stats`, `pair`, `pause`, `resume`, `stop`, and
   `marginalia-node.exe --state-dir %TEMP%\probe probe 127.0.0.1:47615 --knock` (acts like a phone: knocks, embeds, chats). Expected: let in,
   embedding of 48 texts in well under a second, a chat answer streaming.
6. **Network:** allow UDP 47615 and mDNS 5353 on Private networks. Check that the node announces (`dns-sd -B _marginalia._udp` or look at the phone).
   If mDNS does not work on Windows, fix `compute-node/node/src/discovery.rs` (interface selection, firewall) or document the address route.
7. **Tray:** `cargo build --release -p marginalia-node-tray`. `compute-node/tray/src/windows.rs` was written without ever being
   compiled: expect small API fixes (`tray-icon`, `winit`). Check: icon colors (green running, amber paused, grey stopped), left
   click toggles the panel (`marginalia-node-tray.exe --panel` is the egui window), right click menu, "Start node" spawns the node without a console window.
8. **Start at logon:** scheduled tasks for the node and the tray (done by `setup.ps1`), verify after a reboot.
9. **Phone test:** with a real Android phone on the same Wi-Fi the Marginalia app (release build) shows a banner "1 compute node found on your network",
   tap Connect, Trust. Then Ask something and look for "on <node name>" in the answer footer, and `marginalia-node.exe stats` counting the job.
   Kill the node mid-answer: the phone must finish the answer by itself.
10. **Package:** produce a zip with the two exes and the `windows/` scripts if useful. Update `compute-node/STATUS.md` and `README.md` with what is now verified.

## Acceptance checklist

- [ ] clean clone to running node in one script run, on a machine with only Git installed (the script may install the rest with winget)
- [ ] node serves chat and search to a phone, stats count them, no content in any log or stats
- [ ] pause makes jobs fail over, resume restores them, stop ends the process cleanly
- [ ] tray icon visible, left click panel, menu actions work, survives the node being stopped and started
- [ ] runs at logon with no console window
- [ ] firewall rules added once, removed by `uninstall.ps1`
- [ ] docs updated

## Rules

- User-visible text: plain wording, no semicolons, no em dashes, say "compute node" or "node" (never "home" or "computer").
- Commit messages `[area] subject` plus a short why. Do not commit models, databases or tokens.
- Keep Linux working: `cargo test -p marginalia-proto` and `cargo test -p marginalia-node-tray` should still pass.
- The test phone is not available on Windows unless you connect one. Without it use the `probe` command.
