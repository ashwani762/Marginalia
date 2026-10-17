# Shared helpers for the Windows scripts in this folder. It is dot-sourced by them and not run on its own.
# Written for Windows PowerShell 5.1 (and works in PowerShell 7). ASCII only on purpose.

$ErrorActionPreference = 'Continue'

$MN_NodePort  = 47615
$MN_AdminPort = 47616
$MN_InstallDir = Join-Path $env:LOCALAPPDATA 'MarginaliaNode'
$MN_NodeTask  = 'Marginalia Node'
$MN_TrayTask  = 'Marginalia Node Tray'
$MN_FwNode    = 'Marginalia Node (UDP 47615)'
$MN_FwMdns    = 'Marginalia Node (mDNS UDP 5353)'
$MN_RepoRoot  = (Resolve-Path (Join-Path $PSScriptRoot '..\..')).Path
$MN_DefaultModelsDir = Join-Path $env:APPDATA 'marginalia\models'
$MN_StateDir  = Join-Path $env:APPDATA 'marginalia-node'
$MN_BuiltNode = Join-Path $MN_RepoRoot 'target\release\marginalia-node.exe'
$MN_BuiltTray = Join-Path $MN_RepoRoot 'target\release\marginalia-node-tray.exe'
$MN_BuildStamp = Join-Path $MN_RepoRoot 'target\release\mn-backend.txt'
$MN_InstNode  = Join-Path $MN_InstallDir 'marginalia-node.exe'
$MN_InstTray  = Join-Path $MN_InstallDir 'marginalia-node-tray.exe'
$MN_Launcher  = Join-Path $MN_InstallDir 'start-node.vbs'
$MN_LogFile   = Join-Path $MN_InstallDir 'node.log'
# The model files the node serves and their exact sizes (same as compute-node/node/src/models_dl.rs).
$MN_Models = @(
    @{ Role = 'chat';   File = 'gemma-4-E2B-it-Q4_0.gguf';   Size = 2841481184 },
    @{ Role = 'search'; File = 'embeddinggemma-2-Q8_0.gguf'; Size = 309855456 },
    @{ Role = 'speech'; File = 'ggml-large-v3-turbo-q5_0.bin'; Size = 574041195 }
)
# The larger chat model. Setup downloads it so the tray menu can switch to it. Not part of verify.ps1.
$MN_ChatLarge = @{ Role = 'chat'; File = 'gemma-4-E4B-it-Q4_0.gguf'; Size = 4590807392 }

# ---- Output: one line per step ------------------------------------------------------------------
function Write-Tag([string]$tag, [string]$text, [string]$color) {
    Write-Host ('[{0}] {1}' -f $tag, $text) -ForegroundColor $color
}
function Step-Ok([string]$t)   { Write-Tag ' OK ' $t 'Green' }
function Step-Warn([string]$t) { Write-Tag 'WARN' $t 'Yellow' }
function Step-Fail([string]$t) { Write-Tag 'FAIL' $t 'Red' }
function Step-Skip([string]$t) { Write-Tag 'SKIP' $t 'DarkGray' }
function Step-Info([string]$t) { Write-Host ('       ' + $t) }
function Write-Head([string]$t) { Write-Host ''; Write-Host ('== ' + $t + ' ==') -ForegroundColor Cyan }

# ---- Small helpers ------------------------------------------------------------------------------
function Test-IsAdmin {
    $id = [Security.Principal.WindowsIdentity]::GetCurrent()
    return (New-Object Security.Principal.WindowsPrincipal($id)).IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)
}

# Reads PATH again from the registry, so programs winget just installed are found without a new window.
function Update-SessionPath {
    $m = [Environment]::GetEnvironmentVariable('Path', 'Machine')
    $u = [Environment]::GetEnvironmentVariable('Path', 'User')
    $env:Path = $m + ';' + $u
    $cargoBin = Join-Path $env:USERPROFILE '.cargo\bin'
    if (($env:Path -split ';') -notcontains $cargoBin) { $env:Path = $env:Path + ';' + $cargoBin }
}

function Find-Command([string]$name) {
    $c = Get-Command $name -CommandType Application -ErrorAction SilentlyContinue | Select-Object -First 1
    if ($c) { return $c.Source }
    return $null
}

function Test-LocalPort([int]$port, [int]$ms = 500) {
    $c = New-Object Net.Sockets.TcpClient
    try {
        $iar = $c.BeginConnect('127.0.0.1', $port, $null, $null)
        if ($iar.AsyncWaitHandle.WaitOne($ms, $false) -and $c.Connected) {
            $c.EndConnect($iar)
            return $true
        }
        return $false
    } catch {
        return $false
    } finally {
        $c.Close()
    }
}

# The node program: installed copy first, then a build in this repository, then PATH.
function Find-NodeExe {
    $a = Join-Path $MN_InstallDir 'marginalia-node.exe'
    if (Test-Path $a) { return $a }
    $b = Join-Path $MN_RepoRoot 'target\release\marginalia-node.exe'
    if (Test-Path $b) { return $b }
    return (Find-Command 'marginalia-node.exe')
}

function Format-Bytes([double]$b) {
    if ($b -ge 1e9) { return ('{0:N2} GB' -f ($b / 1e9)) }
    if ($b -ge 1e6) { return ('{0:N1} MB' -f ($b / 1e6)) }
    return ('{0:N0} KB' -f ($b / 1e3))
}

# ---- GPU detection ------------------------------------------------------------------------------
function Get-NvidiaSmi {
    $p = Find-Command 'nvidia-smi.exe'
    if ($p) { return $p }
    $q = Join-Path $env:SystemRoot 'System32\nvidia-smi.exe'
    if (Test-Path $q) { return $q }
    return $null
}

# Returns $null without an NVIDIA card, else an object with the card name and the CUDA architectures
# (compute capability without the dot, for example 89), joined with a semicolon when there are several.
function Get-NvidiaInfo {
    $smi = Get-NvidiaSmi
    if (-not $smi) { return $null }
    $names = $null
    try { $names = & $smi --query-gpu=name --format=csv,noheader 2>$null } catch { }
    if ($LASTEXITCODE -ne 0 -or -not $names) { return $null }
    $archs = @()
    try {
        $caps = & $smi --query-gpu=compute_cap --format=csv,noheader 2>$null
        if ($LASTEXITCODE -eq 0) {
            foreach ($line in @($caps)) {
                $t = ([string]$line).Trim()
                if ($t -match '^\d+\.\d+$') { $archs += $t.Replace('.', '') }
            }
        }
    } catch { }
    $archs = @($archs | Select-Object -Unique)
    return New-Object PSObject -Property @{ Name = (@($names)[0]).Trim(); Archs = ($archs -join ';') }
}

function Find-Nvcc {
    $p = Find-Command 'nvcc.exe'
    if ($p) { return $p }
    if ($env:CUDA_PATH) {
        $q = Join-Path $env:CUDA_PATH 'bin\nvcc.exe'
        if (Test-Path $q) { return $q }
    }
    $root = Join-Path $env:ProgramFiles 'NVIDIA GPU Computing Toolkit\CUDA'
    if (Test-Path $root) {
        $f = Get-ChildItem $root -Directory -ErrorAction SilentlyContinue | Sort-Object Name -Descending | ForEach-Object { Join-Path $_.FullName 'bin\nvcc.exe' } | Where-Object { Test-Path $_ } | Select-Object -First 1
        if ($f) { return $f }
    }
    return $null
}

function Test-VulkanSdk {
    return ($env:VULKAN_SDK -and (Test-Path $env:VULKAN_SDK))
}

# Decides which llama.cpp backend to build. Returns an object with Feature (cuda, vulkan or empty for CPU),
# Archs (for CUDAARCHS) and Reason (one sentence).
function Resolve-Gpu([bool]$cpu, [bool]$vulkan, [bool]$cuda) {
    $nv = Get-NvidiaInfo
    $nvcc = Find-Nvcc
    $r = @{ Feature = ''; Archs = ''; Reason = '' }
    if ($cpu) {
        $r.Reason = 'CPU only, as asked'
    } elseif ($cuda) {
        if (-not $nvcc) { throw 'You asked for -Cuda but nvcc.exe (the CUDA toolkit) was not found. Install it with: winget install --id Nvidia.CUDA -e' }
        $r.Feature = 'cuda'
        $r.Reason = 'CUDA, as asked'
    } elseif ($vulkan) {
        if (-not (Test-VulkanSdk)) { throw 'You asked for -Vulkan but the Vulkan SDK was not found (the VULKAN_SDK variable is not set). Install it from https://vulkan.lunarg.com/ and open a new window.' }
        $r.Feature = 'vulkan'
        $r.Reason = 'Vulkan, as asked'
    } elseif ($nv -and $nvcc) {
        $r.Feature = 'cuda'
        $r.Reason = 'CUDA, found ' + $nv.Name + ' and the CUDA toolkit'
    } elseif (Test-VulkanSdk) {
        $r.Feature = 'vulkan'
        $r.Reason = 'Vulkan, found the Vulkan SDK'
    } else {
        $r.Reason = 'CPU only, no CUDA toolkit and no Vulkan SDK found'
    }
    if ($r.Feature -eq 'cuda') {
        if ($env:CUDAARCHS) { $r.Archs = $env:CUDAARCHS }
        elseif ($nv -and $nv.Archs) { $r.Archs = $nv.Archs }
    }
    return New-Object PSObject -Property $r
}

# ---- Prerequisite checks (each returns $true when present) ---------------------------------------
function Test-Git   { return [bool](Find-Command 'git.exe') }
function Test-Cargo {
    if (-not (Find-Command 'cargo.exe')) { return $false }
    return $true
}
function Test-RustMsvc {
    $v = $null
    try { $v = (& rustc -vV 2>$null) -join "`n" } catch { }
    return ($v -match 'host:\s*\S*windows-msvc')
}
# The tray needs rustc 1.95 or newer (egui 0.37). Older toolchains stop with "requires rustc 1.95".
function Test-RustNewEnough {
    $v = $null
    try { $v = (& rustc --version 2>$null) -join ' ' } catch { }
    if ($v -notmatch 'rustc (\d+)\.(\d+)') { return $false }
    return ([int]$Matches[1] -gt 1) -or ([int]$Matches[2] -ge 95)
}
function Get-VsInstall {
    $vw = Join-Path ${env:ProgramFiles(x86)} 'Microsoft Visual Studio\Installer\vswhere.exe'
    if (-not (Test-Path $vw)) { return $null }
    $p = & $vw -latest -products '*' -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -property installationPath 2>$null
    if ($p) { return (@($p)[0]) }
    return $null
}
function Test-CmakeNewEnough {
    # Visual Studio 2026 (folder "18") needs CMake 4.2 or newer, older ones fail with "Could not create named generator".
    $vs = Get-VsInstall
    if (-not $vs -or $vs -notmatch '\\18\\') { return $true }
    $out = (& cmake.exe --version 2>$null | Select-Object -First 1)
    if ($out -notmatch '(\d+)\.(\d+)') { return $true }
    return ([int]$Matches[1] -gt 4) -or ([int]$Matches[1] -eq 4 -and [int]$Matches[2] -ge 2)
}
function Test-Cmake {
    if (Find-Command 'cmake.exe') { return (Test-CmakeNewEnough) }
    $c = Join-Path $env:ProgramFiles 'CMake\bin'
    if (Test-Path (Join-Path $c 'cmake.exe')) { $env:Path = $env:Path + ';' + $c; return (Test-CmakeNewEnough) }
    $vs = Get-VsInstall
    if ($vs) {
        $d = Join-Path $vs 'Common7\IDE\CommonExtensions\Microsoft\CMake\CMake\bin'
        if (Test-Path (Join-Path $d 'cmake.exe')) { $env:Path = $env:Path + ';' + $d; return $true }
    }
    return $false
}
# bindgen needs libclang.dll. Sets LIBCLANG_PATH for this window (and for your user, so later builds find it).
function Test-Libclang {
    $dirs = @()
    if ($env:LIBCLANG_PATH) { $dirs += $env:LIBCLANG_PATH }
    $dirs += (Join-Path $env:ProgramFiles 'LLVM\bin')
    $dirs += (Join-Path $env:ProgramFiles 'LLVM\lib')
    foreach ($d in $dirs) {
        if (Test-Path (Join-Path $d 'libclang.dll')) {
            if ($env:LIBCLANG_PATH -ne $d) {
                $env:LIBCLANG_PATH = $d
                [Environment]::SetEnvironmentVariable('LIBCLANG_PATH', $d, 'User')
            }
            return $true
        }
    }
    return $false
}

# ---- Firewall (needs administrator, see firewall.ps1) --------------------------------------------
function Test-FirewallRules([string]$exe) {
    foreach ($n in @($MN_FwNode, $MN_FwMdns)) {
        $r = Get-NetFirewallRule -DisplayName $n -ErrorAction SilentlyContinue
        if (-not $r) { return $false }
        $app = $r | Get-NetFirewallApplicationFilter -ErrorAction SilentlyContinue | Select-Object -First 1
        if (-not $app -or $app.Program -ne $exe) { return $false }
    }
    return $true
}

# Runs firewall.ps1 as administrator (one Windows prompt). Returns $true when the prompt was accepted.
function Invoke-FirewallElevated([string]$action, [string]$exe) {
    $ps = Join-Path $PSHOME 'powershell.exe'
    $script = Join-Path $PSScriptRoot 'firewall.ps1'
    $argList = @('-NoProfile', '-ExecutionPolicy', 'Bypass', '-File', ('"' + $script + '"'), '-Action', $action, '-Exe', ('"' + $exe + '"'))
    try {
        Start-Process -FilePath $ps -ArgumentList $argList -Verb RunAs -Wait -ErrorAction Stop
        return $true
    } catch {
        return $false
    }
}

# ---- Scheduled tasks -----------------------------------------------------------------------------
function Register-MnTask([string]$name, [string]$exe, [string]$arguments, [string]$workDir, [string]$description) {
    $user = $env:USERDOMAIN + '\' + $env:USERNAME
    if ($arguments) {
        $action = New-ScheduledTaskAction -Execute $exe -Argument $arguments -WorkingDirectory $workDir
    } else {
        $action = New-ScheduledTaskAction -Execute $exe -WorkingDirectory $workDir
    }
    $trigger = New-ScheduledTaskTrigger -AtLogOn -User $user
    $settings = New-ScheduledTaskSettingsSet -AllowStartIfOnBatteries -DontStopIfGoingOnBatteries -StartWhenAvailable -RestartCount 3 -RestartInterval (New-TimeSpan -Minutes 1) -ExecutionTimeLimit ([TimeSpan]::Zero) -MultipleInstances IgnoreNew
    $principal = New-ScheduledTaskPrincipal -UserId $user -LogonType Interactive -RunLevel Limited
    Register-ScheduledTask -TaskName $name -Action $action -Trigger $trigger -Settings $settings -Principal $principal -Description $description -Force -ErrorAction Stop | Out-Null
}

function Get-TaskState([string]$name) {
    $t = Get-ScheduledTask -TaskName $name -ErrorAction SilentlyContinue
    if (-not $t) { return 'not installed' }
    return [string]$t.State
}

# Starts a program, waits for it, and returns what it printed. Used where a hung program must not hang the script.
# Returns an object with ExitCode, Out, Err (strings) and TimedOut.
function Invoke-MnCapture([string]$exe, [string[]]$arguments, [int]$timeoutSec = 30) {
    $so = [IO.Path]::GetTempFileName()
    $se = [IO.Path]::GetTempFileName()
    $parts = @()
    foreach ($a in $arguments) {
        if ($a -match '[\s"]') { $parts += ('"' + ($a -replace '"', '\"') + '"') } else { $parts += $a }
    }
    $r = New-Object PSObject -Property @{ ExitCode = -1; Out = ''; Err = ''; TimedOut = $false }
    try {
        $p = Start-Process -FilePath $exe -ArgumentList ($parts -join ' ') -NoNewWindow -PassThru -RedirectStandardOutput $so -RedirectStandardError $se -ErrorAction Stop
        $null = $p.Handle
        if (-not $p.WaitForExit($timeoutSec * 1000)) {
            $r.TimedOut = $true
            try { $p.Kill() } catch { }
            Start-Sleep -Milliseconds 300
        } else {
            $p.WaitForExit()
            $r.ExitCode = $p.ExitCode
        }
        $o = Get-Content -Path $so -Raw -ErrorAction SilentlyContinue
        $e = Get-Content -Path $se -Raw -ErrorAction SilentlyContinue
        if ($o) { $r.Out = [string]$o }
        if ($e) { $r.Err = [string]$e }
    } catch {
        $r.Err = $_.Exception.Message
    } finally {
        Remove-Item -Path $so, $se -Force -ErrorAction SilentlyContinue
    }
    return $r
}

# The JSON of  marginalia-node.exe status,  or $null when the node does not answer.
function Get-MnStatus([string]$exe) {
    if (-not $exe -or -not (Test-Path $exe)) { return $null }
    $r = Invoke-MnCapture $exe @('status') 15
    if ($r.TimedOut -or $r.ExitCode -ne 0) { return $null }
    try { return ($r.Out | ConvertFrom-Json) } catch { return $null }
}

# ---- Is the work already done? (setup.ps1 and verify.ps1 use these to skip and to explain) -------------
# The newest write time of the code that goes into the node and the tray.
# $which is node, tray or all. The tray is not rebuilt when only node code changed, so each program is compared with its own sources.
function Get-NewestSourceTime([string]$which = 'all') {
    $newest = [DateTime]::MinValue
    $dirs = @('compute-node\node\src', 'compute-node\tray\src', 'compute-node\proto\src')
    if ($which -eq 'node') { $dirs = @('compute-node\node\src', 'compute-node\proto\src') }
    if ($which -eq 'tray') { $dirs = @('compute-node\tray\src') }
    foreach ($d in $dirs) {
        $p = Join-Path $MN_RepoRoot $d
        if (-not (Test-Path $p)) { continue }
        foreach ($f in (Get-ChildItem -Path $p -Recurse -File -ErrorAction SilentlyContinue)) {
            if ($f.LastWriteTime -gt $newest) { $newest = $f.LastWriteTime }
        }
    }
    $files = @('Cargo.toml', 'Cargo.lock', 'compute-node\node\Cargo.toml', 'compute-node\node\build.rs', 'compute-node\tray\Cargo.toml', 'compute-node\proto\Cargo.toml', 'compute-node\vendor\llama-cpp-sys-2\build.rs', 'compute-node\vendor\llama-cpp-sys-2\Cargo.toml')
    if ($which -eq 'node') { $files = @('Cargo.toml', 'Cargo.lock', 'compute-node\node\Cargo.toml', 'compute-node\node\build.rs', 'compute-node\proto\Cargo.toml', 'compute-node\vendor\llama-cpp-sys-2\build.rs', 'compute-node\vendor\llama-cpp-sys-2\Cargo.toml') }
    if ($which -eq 'tray') { $files = @('Cargo.toml', 'Cargo.lock', 'compute-node\tray\Cargo.toml') }
    foreach ($rel in $files) {
        $p = Join-Path $MN_RepoRoot $rel
        if (Test-Path $p) {
            $t = (Get-Item $p).LastWriteTime
            if ($t -gt $newest) { $newest = $t }
        }
    }
    return $newest
}

# What backend a build would use, as text (for example "cuda|89" or "cpu|"). Used to notice a changed backend.
function Get-BackendStamp([bool]$cpu, [bool]$vulkan, [bool]$cuda) {
    try { $g = Resolve-Gpu $cpu $vulkan $cuda } catch { return $null }
    $f = $g.Feature
    if (-not $f) { $f = 'cpu' }
    return ($f + '|' + $g.Archs)
}

# $true when both built programs exist, are newer than all the sources, and were built with the same backend.
function Test-BuildCurrent([bool]$cpu, [bool]$vulkan, [bool]$cuda) {
    if (-not (Test-Path $MN_BuiltNode) -or -not (Test-Path $MN_BuiltTray)) { return $false }
    if ((Get-NewestSourceTime 'node') -gt (Get-Item $MN_BuiltNode).LastWriteTime) { return $false }
    if ((Get-NewestSourceTime 'tray') -gt (Get-Item $MN_BuiltTray).LastWriteTime) { return $false }
    if (-not (Test-Path $MN_BuildStamp)) { return $false }
    $want = Get-BackendStamp $cpu $vulkan $cuda
    if (-not $want) { return $false }
    $have = ((Get-Content -Path $MN_BuildStamp -Raw -ErrorAction SilentlyContinue) + '').Trim()
    return ($have -eq $want)
}

# $true when the installed program is the same file as the built one (same size, not older).
function Test-InstalledCurrent([string]$built, [string]$installed) {
    if (-not (Test-Path $built) -or -not (Test-Path $installed)) { return $false }
    $b = Get-Item $built
    $i = Get-Item $installed
    if ($b.Length -ne $i.Length) { return $false }
    return ($i.LastWriteTimeUtc -ge $b.LastWriteTimeUtc.AddSeconds(-2))
}

# The file names of required model files that are missing or have the wrong size in $dir.
function Get-MissingModels([string]$dir, [bool]$skipSpeech) {
    $miss = @()
    foreach ($m in ($MN_Models + @($MN_ChatLarge))) {
        if ($skipSpeech -and $m.Role -eq 'speech') { continue }
        $f = Join-Path $dir $m.File
        if (-not (Test-Path $f) -or (Get-Item $f).Length -ne $m.Size) { $miss += $m.File }
    }
    return $miss
}

function Test-TaskRegistered([string]$name, [string]$exe, [string]$argPart) {
    $t = Get-ScheduledTask -TaskName $name -ErrorAction SilentlyContinue
    if (-not $t) { return $false }
    $a = @($t.Actions)[0]
    if (-not $a -or $a.Execute -ne $exe) { return $false }
    if ($argPart -and -not ([string]$a.Arguments).Contains($argPart)) { return $false }
    return $true
}

# ---- Stop, copy, start ---------------------------------------------------------------------------
# Asks a running node (installed copy) to stop, then makes sure its processes are gone.
# $which is both, node or tray.
function Stop-MnProcesses([string]$which = 'both') {
    $doNode = ($which -ne 'tray')
    $doTray = ($which -ne 'node')
    if ($doNode) { Stop-ScheduledTask -TaskName $MN_NodeTask -ErrorAction SilentlyContinue }
    if ($doTray) { Stop-ScheduledTask -TaskName $MN_TrayTask -ErrorAction SilentlyContinue }
    if ($doNode) {
        $exe = Join-Path $MN_InstallDir 'marginalia-node.exe'
        if ((Test-Path $exe) -and (Test-LocalPort $MN_AdminPort)) {
            try { & $exe stop 2>$null | Out-Null } catch { }
            for ($i = 0; $i -lt 30; $i++) {
                if (-not (Test-LocalPort $MN_AdminPort 200)) { break }
                Start-Sleep -Milliseconds 500
            }
        }
    }
    $names = @()
    if ($doTray) { $names += 'marginalia-node-tray' }
    if ($doNode) { $names += 'marginalia-node' }
    foreach ($n in $names) {
        Get-Process -Name $n -ErrorAction SilentlyContinue | Where-Object { $_.Path -and $_.Path.StartsWith($MN_InstallDir, [StringComparison]::OrdinalIgnoreCase) } | Stop-Process -Force -ErrorAction SilentlyContinue
    }
    Start-Sleep -Milliseconds 500
}

# Stops what is running, then copies the built programs into the install folder. $which is both, node or tray.
function Install-MnPrograms([string]$which = 'both') {
    try {
        Stop-MnProcesses $which
        New-Item -ItemType Directory -Force -Path $MN_InstallDir | Out-Null
        if ($which -ne 'tray') { Copy-Item -Force $MN_BuiltNode $MN_InstNode -ErrorAction Stop }
        if ($which -ne 'node') { Copy-Item -Force $MN_BuiltTray $MN_InstTray -ErrorAction Stop }
        return $true
    } catch {
        Step-Fail ('Could not copy the programs: ' + $_.Exception.Message + '  Close any window running marginalia-node and try again.')
        return $false
    }
}

# Starts the node and/or the tray the same way logon does (the scheduled task, or the launcher when there is no task).
function Start-MnProcesses([string]$which = 'both') {
    if ($which -ne 'tray' -and -not (Test-LocalPort $MN_AdminPort)) {
        if (Get-ScheduledTask -TaskName $MN_NodeTask -ErrorAction SilentlyContinue) {
            Start-ScheduledTask -TaskName $MN_NodeTask -ErrorAction SilentlyContinue
        } elseif (Test-Path $MN_Launcher) {
            Start-Process -FilePath 'wscript.exe' -ArgumentList ('//B //Nologo "' + $MN_Launcher + '"') | Out-Null
        }
    }
    if ($which -ne 'node' -and -not (Get-Process -Name 'marginalia-node-tray' -ErrorAction SilentlyContinue)) {
        if (Get-ScheduledTask -TaskName $MN_TrayTask -ErrorAction SilentlyContinue) {
            Start-ScheduledTask -TaskName $MN_TrayTask -ErrorAction SilentlyContinue
        } elseif (Test-Path $MN_InstTray) {
            Start-Process -FilePath $MN_InstTray -WorkingDirectory $MN_InstallDir | Out-Null
        }
    }
}

# Waits until the node answers on its local control port. Returns $true when it does.
function Wait-MnNode([int]$seconds = 120) {
    for ($i = 0; $i -lt $seconds; $i++) {
        if (Test-LocalPort $MN_AdminPort 300) { return $true }
        Start-Sleep -Seconds 1
    }
    return $false
}

# ---- Build ---------------------------------------------------------------------------------------
# $which is both, node or tray.
function Invoke-MnBuild {
    param([bool]$cpu, [bool]$vulkan, [bool]$cuda, [string]$which = 'both')
    $buildNode = ($which -ne 'tray')
    $buildTray = ($which -ne 'node')
    if (-not (Find-Command 'cargo.exe')) {
        Step-Fail 'cargo was not found. Install Rust with: winget install --id Rustlang.Rustup -e   then open a new PowerShell window.'
        return $false
    }
    $gpu = $null
    if ($buildNode) {
        try {
            $gpu = Resolve-Gpu $cpu $vulkan $cuda
        } catch {
            Step-Fail $_.Exception.Message
            return $false
        }
        Step-Ok ('Backend: ' + $gpu.Reason)
        if (-not (Test-Libclang)) {
            Step-Fail 'libclang.dll was not found (bindgen needs it). Install LLVM with: winget install --id LLVM.LLVM -e   then run this again.'
            return $false
        }
        $env:CMAKE_BUILD_PARALLEL_LEVEL = [string][Environment]::ProcessorCount
        if ($gpu.Feature -eq 'cuda' -and $gpu.Archs) {
            $env:CUDAARCHS = $gpu.Archs
            Step-Info ('CUDAARCHS is ' + ($gpu.Archs -replace ';', ' and '))
        }
    }
    Push-Location $MN_RepoRoot
    try {
        if ($buildNode) {
            $nodeArgs = @('build', '--release', '-p', 'marginalia-node')
            if ($gpu.Feature) { $nodeArgs += @('--features', $gpu.Feature) }
            Step-Info ('cargo ' + ($nodeArgs -join ' ') + '   (the first time this takes a while)')
            & cargo @nodeArgs | Out-Host
            if ($LASTEXITCODE -ne 0) {
                Step-Fail 'Building marginalia-node failed. docs\WINDOWS-PLAN.md lists the likely causes with the file to look at. Fix it and run this again, finished parts are kept.'
                return $false
            }
            Step-Ok 'Built marginalia-node.exe'
            $f = $gpu.Feature
            if (-not $f) { $f = 'cpu' }
            try { Set-Content -Path $MN_BuildStamp -Value ($f + '|' + $gpu.Archs) -Encoding ASCII } catch { }
        }
        if ($buildTray) {
            Step-Info 'cargo build --release -p marginalia-node-tray'
            & cargo build --release -p marginalia-node-tray | Out-Host
            if ($LASTEXITCODE -ne 0) {
                Step-Fail 'Building marginalia-node-tray failed. docs\WINDOWS-PLAN.md lists the likely causes (compute-node\tray\src\windows.rs was never compiled before).'
                return $false
            }
            Step-Ok 'Built marginalia-node-tray.exe'
        }
    } finally {
        Pop-Location
    }
    return $true
}
