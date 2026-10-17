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
function Get-VsInstall {
    $vw = Join-Path ${env:ProgramFiles(x86)} 'Microsoft Visual Studio\Installer\vswhere.exe'
    if (-not (Test-Path $vw)) { return $null }
    $p = & $vw -latest -products '*' -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -property installationPath 2>$null
    if ($p) { return (@($p)[0]) }
    return $null
}
function Test-Cmake {
    if (Find-Command 'cmake.exe') { return $true }
    $c = Join-Path $env:ProgramFiles 'CMake\bin'
    if (Test-Path (Join-Path $c 'cmake.exe')) { $env:Path = $env:Path + ';' + $c; return $true }
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

# Asks a running node (installed copy) to stop, then makes sure its processes are gone.
function Stop-MnProcesses {
    Stop-ScheduledTask -TaskName $MN_NodeTask -ErrorAction SilentlyContinue
    Stop-ScheduledTask -TaskName $MN_TrayTask -ErrorAction SilentlyContinue
    $exe = Join-Path $MN_InstallDir 'marginalia-node.exe'
    if ((Test-Path $exe) -and (Test-LocalPort $MN_AdminPort)) {
        try { & $exe stop 2>$null | Out-Null } catch { }
        for ($i = 0; $i -lt 30; $i++) {
            if (-not (Test-LocalPort $MN_AdminPort 200)) { break }
            Start-Sleep -Milliseconds 500
        }
    }
    foreach ($n in @('marginalia-node-tray', 'marginalia-node')) {
        Get-Process -Name $n -ErrorAction SilentlyContinue | Where-Object { $_.Path -and $_.Path.StartsWith($MN_InstallDir, [StringComparison]::OrdinalIgnoreCase) } | Stop-Process -Force -ErrorAction SilentlyContinue
    }
    Start-Sleep -Milliseconds 500
}

# ---- Build ---------------------------------------------------------------------------------------
function Invoke-MnBuild {
    param([bool]$cpu, [bool]$vulkan, [bool]$cuda)
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
    if (-not (Find-Command 'cargo.exe')) {
        Step-Fail 'cargo was not found. Install Rust with: winget install --id Rustlang.Rustup -e   then open a new PowerShell window.'
        return $false
    }
    $env:CMAKE_BUILD_PARALLEL_LEVEL = [string][Environment]::ProcessorCount
    if ($gpu.Feature -eq 'cuda' -and $gpu.Archs) {
        $env:CUDAARCHS = $gpu.Archs
        Step-Info ('CUDAARCHS is ' + ($gpu.Archs -replace ';', ' and '))
    }
    Push-Location $MN_RepoRoot
    try {
        $nodeArgs = @('build', '--release', '-p', 'marginalia-node')
        if ($gpu.Feature) { $nodeArgs += @('--features', $gpu.Feature) }
        Step-Info ('cargo ' + ($nodeArgs -join ' ') + '   (the first time this takes a while)')
        & cargo @nodeArgs | Out-Host
        if ($LASTEXITCODE -ne 0) {
            Step-Fail 'Building marginalia-node failed. See the table in compute-node\windows\README.md for common causes. Fix it and run this again, finished parts are kept.'
            return $false
        }
        Step-Ok 'Built marginalia-node.exe'
        Step-Info 'cargo build --release -p marginalia-node-tray'
        & cargo build --release -p marginalia-node-tray | Out-Host
        if ($LASTEXITCODE -ne 0) {
            Step-Fail 'Building marginalia-node-tray failed. See compute-node\windows\README.md.'
            return $false
        }
        Step-Ok 'Built marginalia-node-tray.exe'
    } finally {
        Pop-Location
    }
    return $true
}
