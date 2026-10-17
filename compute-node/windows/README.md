# Marginalia compute node on Windows

One command sets up the node and its tray icon: it checks the tools, builds, installs to `%LOCALAPPDATA%\MarginaliaNode`, downloads the models, starts both at logon and opens the firewall for phones. These scripts were written without a Windows machine to test on, so expect to report small fixes. Everything is safe to run again.

## Quick start

1. Open PowerShell (the normal one, not an administrator window). Windows PowerShell 5.1 is enough.
2. `git config --global core.longpaths true` (do this BEFORE cloning, the bundled llama.cpp has very long paths).
3. `git clone --depth 1 <repository url> C:\src\foresight` (a short folder name keeps the build paths short).
4. `cd C:\src\foresight`
5. `Set-ExecutionPolicy -Scope Process Bypass` (only for this window, changes nothing permanent).
6. `.\compute-node\windows\setup.ps1` (add `-Auto` to let it run the winget installs for missing tools).
7. If it lists missing tools, install them (it prints the exact `winget install` lines), open a NEW PowerShell window and run step 6 again.
8. Allow the one Windows permission prompt for the firewall rules.
9. Look for the tray icon by the clock (maybe behind the `^` arrow). On the phone: Settings, Compute nodes. The phone finds this machine by itself, tap Connect.
10. `marginalia-node.exe stats` (in a new window) shows what the node has done.

To double click instead: right click `setup.ps1` and choose Run with PowerShell. If the window closes too fast to read, use the steps above.

## The scripts

| Script | What it does |
| --- | --- |
| `setup.ps1` | Everything above. Options: `-Auto`, `-Cpu`, `-Cuda`, `-Vulkan`, `-SkipBuild`, `-SkipModels`, `-ModelsDir D:\models` |
| `build.ps1` | Only the release build, with the same backend detection (`-Cpu`, `-Cuda`, `-Vulkan`) |
| `download-models.ps1` | The three model files (about 3.7 GB: chat, search and speech to text). Uses `marginalia-node.exe models download`, or downloads by itself before the first build. `-Dir`, `-Only chat\|search\|speech`, `-ChatModel e2b\|e4b`, `-SkipSpeech` |
| `status.ps1` | Is the node running, task state, firewall rules, model files, `stats`, addresses, end of the log |
| `uninstall.ps1` | Stops and removes the tasks, firewall rules and installed files. Keeps models and state unless `-Purge` |
| `firewall.ps1` | The administrator part, started by the others with one prompt. You can run it yourself: `-Action Add -Exe <path>` or `-Action Remove` |

Backend choice: CUDA when an NVIDIA card (`nvidia-smi`) and the CUDA toolkit (`nvcc`) are both present, else Vulkan when the Vulkan SDK is installed (`VULKAN_SDK`), else CPU. `CUDAARCHS` comes from `nvidia-smi --query-gpu=compute_cap` (8.9 becomes 89). Force one with `-Cpu`, `-Cuda` or `-Vulkan`.

What gets installed, all under your user (only the firewall rules need administrator permission):

- `%LOCALAPPDATA%\MarginaliaNode\` holds `marginalia-node.exe`, `marginalia-node-tray.exe`, `start-node.vbs` (starts the node with no console window) and `node.log`.
- Scheduled Tasks `Marginalia Node` and `Marginalia Node Tray`, at logon, for you only. The node task restarts after a failure. A clean `marginalia-node.exe stop` is not restarted.
- Firewall rules `Marginalia Node (UDP 47615)` and `Marginalia Node (mDNS UDP 5353)` for that exe, Private networks.
- The install folder is added to your user PATH. `LIBCLANG_PATH` is set for your user if it was not.
- Models in `%APPDATA%\marginalia\models`, node identity and paired phones in `%APPDATA%\marginalia-node`.

## How long each step takes (rough, depends on the machine and the internet)

| Step | Time |
| --- | --- |
| Installing the tools (Visual Studio Build Tools is the big one, about 6 GB) | 10 to 30 minutes, once |
| First build, CPU | 10 to 25 minutes |
| First build, CUDA | 20 to 45 minutes (the GPU kernels take long to compile) |
| Later builds | 1 to 3 minutes |
| Model download (about 3.2 GB) | 5 to 30 minutes, can be stopped and continued |
| Install, tasks, firewall, start | under a minute, plus up to a minute for the node to load the models |

## Troubleshooting

| What you see | Cause and fix |
| --- | --- |
| `Unable to find libclang` or `couldn't find any valid shared libraries matching libclang.dll` | bindgen needs LLVM. `winget install --id LLVM.LLVM -e`, then open a NEW window. `setup.ps1` sets `LIBCLANG_PATH` to `C:\Program Files\LLVM\bin` for you. By hand: `$env:LIBCLANG_PATH = "C:\Program Files\LLVM\bin"` |
| `linker link.exe not found` or `Could not find Visual Studio` or CMake finds no compiler | The C++ tools are missing. Install Visual Studio Build Tools with the "Desktop development with C++" workload, then open a NEW window. Make sure Rust uses the MSVC toolchain: `rustup default stable-x86_64-pc-windows-msvc` |
| CUDA error: `No CUDA toolset found`, or `unsupported Microsoft Visual Studio version`, or `nvcc fatal` | The CUDA toolkit and Visual Studio must fit each other. Install Visual Studio Build Tools FIRST, then the CUDA toolkit (its installer adds the Visual Studio integration only if Visual Studio is already there). CUDA 12.x works with Visual Studio 2022. A very new Visual Studio may be too new for your CUDA version. Reinstall CUDA after Visual Studio, or update CUDA. To skip the GPU: `setup.ps1 -Cpu` |
| `path too long` or files not found deep inside `vendor` | Run `git config --global core.longpaths true` before cloning and keep the repository in a short folder such as `C:\src\foresight`. You can also turn on Windows long paths (administrator): `Set-ItemProperty HKLM:\SYSTEM\CurrentControlSet\Control\FileSystem LongPathsEnabled 1` |
| `cargo` or `winget` is not recognised right after installing it | The window does not know the new PATH yet. Open a NEW PowerShell window. |
| `running scripts is disabled on this system` | `Set-ExecutionPolicy -Scope Process Bypass`, then run the script again in the same window |
| The firewall prompt did not appear or was refused | Run in an administrator window: `.\compute-node\windows\firewall.ps1 -Action Add -Exe "$env:LOCALAPPDATA\MarginaliaNode\marginalia-node.exe"`. `status.ps1` shows whether the rules exist. The rules apply to Private networks, so a network marked Public needs to be switched: Settings, Network and Internet, your network, Network profile type: Private |
| Antivirus or SmartScreen flags or removes the exe | A new unsigned program that opens a network port can trigger this. Windows Security, Virus and threat protection, Protection history, Restore, and add an exclusion for `%LOCALAPPDATA%\MarginaliaNode`. Or build it yourself (it is the code in this repository), which is what these scripts do. Then run `setup.ps1 -SkipBuild` |
| The phone cannot find the node on guest Wi-Fi | Guest networks usually isolate devices from each other, which blocks discovery and direct connections. Put the phone and this machine on the same ordinary network. `marginalia-node.exe status` shows the addresses, and in the app you can add the node by address. If isolation is on, the address will not work either |
| The model download says access denied | Create a read token at https://huggingface.co/settings/tokens, accept the licence on the model page, then `$env:HF_TOKEN = "hf_..."` and run the download again. The token is only sent to Hugging Face, never printed |
| The node does not start after setup | Read `%LOCALAPPDATA%\MarginaliaNode\node.log`. Missing models: `marginalia-node.exe models download`. Or run it in the foreground to watch: `marginalia-node.exe run` |
| Phones find the node but jobs never start | Sleep mode. The node is unreachable while this machine sleeps. Settings, System, Power, set Sleep to Never while plugged in |

## Useful commands

```powershell
marginalia-node.exe models list           # which model files are present
marginalia-node.exe models download       # get what is missing
marginalia-node.exe status                # JSON: models, phones, addresses
marginalia-node.exe stats                 # jobs, speed, errors, no content
marginalia-node.exe pair                  # a one-time code for the phone
marginalia-node.exe stop                  # stop cleanly (the logon task starts it again next time)
Start-ScheduledTask -TaskName "Marginalia Node"        # start it again
```
