# Windows plan: fix only what Windows needs

You work on Windows on the compute node only (`compute-node/node`, `compute-node/tray`, `scripts/windows`). Do not read the
whole project. Everything in the section "Already verified" was checked on Linux and must not be re-checked or re-designed.
Nothing in `scripts/windows/*.ps1` and nothing Windows specific in the Rust code was ever run on Windows.

## Start here

```powershell
git config --global core.longpaths true
git clone <repository url> C:\src\foresight     # a short path keeps the build paths short
cd C:\src\foresight
Set-ExecutionPolicy -Scope Process Bypass
.\scripts\windows\setup.ps1 -Auto      # tools, build, models, install, tasks, firewall, start, verify. Add -Cpu first if CUDA gives trouble
.\scripts\windows\verify.ps1           # 20 numbered lines, only failures explain themselves, the last lines give the command for the first failure
.\scripts\windows\dev.ps1 [node|tray]  # after each code change: build, replace, restart, follow node.log
```

The loop: run `verify.ps1`, take the FIRST failing line, find its row in the table below, fix that file, run `dev.ps1`, run `verify.ps1`
again. Stop when it says `20 of 20 checks passed`. A build that fails stops `setup.ps1` before anything is installed. Fix it, run it again.
It skips what is already done (it prints the skipped steps first) and `-Force` redoes everything.

After a `git pull`: `.\scripts\windows\setup.ps1 -Quick` (incremental build, copy, restart, verify).

Added on the Linux side and type checked for Windows only (never run there): the node writes its own `logs\node.log` under
`%APPDATA%\marginalia-node` (replaced at 5 MB, the last 5 kept, also `marginalia-node.exe logs`), next to the launcher's `node.log` that
`verify.ps1` reads. The tray refuses to start twice (it holds port admin port plus 1, 47617). The tray window shows live charts and a "Logs"
row (`explorer`). The node reads graphics card use through `nvidia-smi` (no console window), and reads photos when `mmproj-*.gguf` is next to the chat model.

The optional speaker helper `marginalia-node-speakers.exe` is built by `build.ps1` and `make-all.ps1` and copied next to the node by the install step (`$MN_BuiltSpeakers`).
It links sherpa-onnx statically, which downloads a prebuilt library for `x86_64-pc-windows-msvc` while building. If it does not build, the node works without it. Not run on Windows.
`.\scripts\windows\make-all.ps1` builds everything in one go.

Features are being added on the Linux side right now (live correction windows and more). Your only duty for those is that
the node and the tray still build and `verify.ps1` still passes after you pull. Do not implement or review them.

## Known likely Windows problems, most probable first

| # | Symptom | Look at | Fix hint |
| --- | --- | --- | --- |
| 1 | Build stops with `Unable to find libclang` or `'stddef.h' file not found` | `compute-node/node/build.rs` (bindgen for whisper.h) | Install LLVM, `LIBCLANG_PATH=C:\Program Files\LLVM\bin` (setup sets it). For missing headers build from a Developer PowerShell for VS 2022 or set `BINDGEN_EXTRA_CLANG_ARGS` to the MSVC include folders |
| 2 | llama.cpp (CMake) fails to configure or compile under MSVC | `compute-node/vendor/llama-cpp-sys-2/build.rs` | MSVC flags and the CMake generator. Keep Linux behaviour: put Windows changes behind `cfg!(target_os = "windows")` or the target triple. Run `setup.ps1 -Cpu` to separate this from CUDA |
| 3 | Link errors about OpenMP (`vcomp`, `omp.lib`) | `compute-node/node/Cargo.toml` (feature `openmp` of llama-cpp-2) and the OpenMP part of `llama-cpp-sys-2/build.rs` | Link `vcomp` on MSVC, or drop the `openmp` feature for the `windows` target only (`[target.'cfg(windows)'.dependencies]`) |
| 4 | CUDA: `No CUDA toolset found`, `unsupported Microsoft Visual Studio version`, `nvcc fatal` | `compute-node/vendor/llama-cpp-sys-2/build.rs` (CUDA section), the installed toolkit | Install Visual Studio Build Tools first, then the CUDA toolkit. A Visual Studio newer than the toolkit supports needs a newer toolkit. Set `CUDAARCHS` (89 RTX 40, 86 RTX 30). Ship CPU first, GPU second |
| 5 | whisper.cpp fails to compile with MSVC | `compute-node/node/build.rs` (`cc::Build` for `vendor/whisper.cpp`, compiled against the llama ggml headers) | MSVC wants `/utf-8`, `/bigobj`, `_USE_MATH_DEFINES`, `_CRT_SECURE_NO_WARNINGS`. Add them with `build.flag_if_supported(...)` or `.define(...)` so Linux is unchanged. `-fPIC` is already optional |
| 6 | Duplicate symbol or unresolved `ggml_*` when linking whisper and llama together | same file, and the ggml include path `compute-node/vendor/llama-cpp-sys-2/llama.cpp/ggml` | One ggml only. The whisper object must not compile its own ggml. Check the CUDA and Vulkan defines match the llama build |
| 7 | `cargo build -p marginalia-node-tray` fails | `compute-node/tray/src/windows.rs` (never compiled), `compute-node/tray/Cargo.toml` | Expect small API changes in `tray-icon` 0.26 and `winit` 0.30: the `TrayIconEvent::Click` fields, `MenuEvent::receiver`, `Icon::from_rgba`, `ApplicationHandler`, `EventLoop::run_app` |
| 8 | Tray icon shows but behaves wrongly | `compute-node/tray/src/windows.rs`, `compute-node/tray/src/control.rs` | Left click must open the panel (`marginalia-node-tray.exe --panel`, an egui window) and right click must show the menu (`with_menu_on_left_click(false)` is already set). Spawned processes use `CREATE_NO_WINDOW` (`0x08000000`, `control.rs`), so no console flashes. The panel needs OpenGL: on a virtual machine or old drivers it may not open |
| 9 | Node does not start at logon, or `node.log` is empty | `scripts/windows/setup.ps1` (the `start-node.vbs` text and `Register-MnTask`) | Run `wscript //B //Nologo "%LOCALAPPDATA%\MarginaliaNode\start-node.vbs"` and read `node.log`. Quoting of the exe path and of `--models` is the usual cause. Then `Start-ScheduledTask -TaskName "Marginalia Node"`. The node must run hidden (no console window) |
| 10 | Phone does not list the node, but `status` shows addresses | `compute-node/node/src/discovery.rs` (`lan_addresses` adapter filter, `advertise`) | Firewall rules for UDP 47615 and 5353 and the network profile must be Private. Check that the right adapter is announced (VPN, Hyper-V, WSL adapters are filtered by name). Windows has its own mDNS service on UDP 5353, so check the `mdns-sd` socket options. Fallback that must keep working: add the node by address in the app |
| 11 | Probe fails on loopback although the node runs | `compute-node/proto` transport, `compute-node/node/src/server.rs` | Only if check 17 fails. QUIC over UDP on 127.0.0.1. Windows Firewall should allow loopback. Look at the `node.log` line when the knock arrives |
| 12 | `status`, `stop`, `pause` cannot reach the node | `compute-node/node/src/admin.rs` | The admin port 47616 must listen on 127.0.0.1 only. `Test-NetConnection 127.0.0.1 -Port 47616` |
| 13 | Models not found by the node | `compute-node/node/src/models_dl.rs` (`default_dir`), `main.rs` (`state_dir`) | Both use `dirs`: models in `%APPDATA%\marginalia\models`, state in `%APPDATA%\marginalia-node`. Check path separators if you build paths from strings. `--models "D:\x"` is passed by the launcher |
| 14 | `path too long`, files missing in `vendor` | the clone | `git config --global core.longpaths true` BEFORE cloning, clone to `C:\src\foresight`, optionally turn on Windows long paths |
| 15 | winget id wrong, script refuses to run, antivirus removes the exe | `scripts/windows/*.ps1`, `scripts/windows/README.md` | The script prints the exact install line. `Set-ExecutionPolicy -Scope Process Bypass`. Add an exclusion for `%LOCALAPPDATA%\MarginaliaNode` |

Fix the script when the script is wrong (keep it PowerShell 5.1, ASCII only, simple). Fix the Rust when Windows needs it, and keep
Linux behaviour (`cargo test -p marginalia-proto` and `cargo test -p marginalia-node-tray` must still pass on Linux, so use
`cfg(windows)` for Windows only code).

## Already verified on Linux: do not re-check, do not re-design

- The wire protocol, QUIC with pinned self-signed certificates, knock, pairing with a code, lock and approve, block list.
- Node ranking, failover between nodes, the phone finishing an answer by itself when a node dies, side by side jobs.
- Serving chat (Gemma 4 E2B Q4_0), search (EmbeddingGemma 2 Q8_0) and speech (whisper large-v3-turbo q5_0) including long recordings.
- `stats` and `status` content, the model download logic with resume (`models download`), the tray logic (`model.rs`, `control.rs`, the panel).
- Everything under `app/`, `core/` and `docs/ARCHITECTURE.md`. Read nothing of it unless a Windows symptom points there.

## Done checklist (the same 20 lines as `verify.ps1`)

- [ ] 01 programs installed (node and tray in `%LOCALAPPDATA%\MarginaliaNode`)
- [ ] 02 installed copy is current (the installed programs are the last build, the build is newer than the code)
- [ ] 03 chat model, 04 search model, 05 speech model (files complete, sizes shown)
- [ ] 06 task "Marginalia Node", 07 task "Marginalia Node Tray" (exist, not disabled)
- [ ] 08 firewall rules (UDP 47615 and 5353, Private)
- [ ] 09 node running (answers on port 47616), 10 tray running
- [ ] 11 status (valid JSON, not paused)
- [ ] 12 chat loaded (tokens a second), 13 search loaded (passages a second), 14 speech loaded (times real time)
- [ ] 15 addresses (at least one a phone can reach)
- [ ] 16 mDNS announcement (may only say "check from the phone", then check it by hand)
- [ ] 17 self test let in, 18 self test search, 19 self test chat, 20 self test speech (3 seconds of silence)

Then by hand, once, because a script cannot see them: the tray icon is visible and green, left click opens the panel, right click shows the
menu, pause and resume work from it, "Stop node" ends the process and "Start node" brings it back with no console window, a reboot
brings both back, and a real Android phone on the same Wi-Fi lists the node and gets an answer with "on <node name>" in the footer.
Kill the node during an answer: the phone must finish by itself.

When it passes, write what is now verified into `compute-node/STATUS.md` (a short list) and stop.

## Rules

- Wording in anything a user sees: plain, no semicolons, no em dashes, say "compute node" or "node", never "home" or "computer".
- Commit messages are `[area] subject` plus a short why, for example `[compute-node] Fix tray build on Windows`. Never commit models, databases or tokens.
- Do not touch the Android source (`app/`, `core/`). Do not change the protocol.
- No bash in the Windows scripts. PowerShell 5.1, ASCII only.
- The test phone is not available on Windows unless you connect one. `verify.ps1` runs `probe` for you.
