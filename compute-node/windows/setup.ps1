# One command setup of the compute node on Windows. Safe to run again at any time, and fast when little changed.
#
#   Set-ExecutionPolicy -Scope Process Bypass
#   .\compute-node\windows\setup.ps1
#
# What it does: checks the tools it needs (and prints the exact winget command for each missing one),
# builds the node and the tray icon, installs them to %LOCALAPPDATA%\MarginaliaNode, downloads the
# models (chat, search and speech to text), makes both start at logon (Task Scheduler), opens the Windows Firewall for phones,
# starts them and runs verify.ps1. Each step that is already done is skipped, and the list of skipped steps is printed first:
#   build        skipped when the programs in target\release are newer than the sources and use the same backend
#   install      skipped when the installed programs are the built ones
#   models       skipped when every model file is complete
#   tasks, firewall rules   skipped when they exist
#
# Options:
#   -Quick       only: build (if needed), copy the two programs, restart node and tray, run verify.ps1. For after a git pull.
#   -Force       do every step again, even when it looks done
#   -Auto        run the winget installs for missing tools itself (Windows asks for permission on some)
#   -Cpu         build without a GPU backend      -Cuda / -Vulkan  force that backend
#   -SkipBuild   use the programs already built in target\release
#   -SkipModels  do not download the models
#   -SkipSpeech  download the chat and search models but not the speech model (570 MB)
#   -ModelsDir   keep the models in this folder (default %APPDATA%\marginalia\models)
#   -NoVerify    do not run verify.ps1 at the end
param(
    [switch]$Cpu,
    [switch]$Vulkan,
    [switch]$Cuda,
    [switch]$SkipBuild,
    [switch]$SkipModels,
    [switch]$SkipSpeech,
    [string]$ModelsDir = '',
    [switch]$Auto,
    [switch]$Force,
    [switch]$Quick,
    [switch]$NoVerify
)
. (Join-Path $PSScriptRoot 'common.ps1')

$problems = 0
$started = Get-Date
Write-Host ''
Write-Host 'Marginalia compute node: Windows setup' -ForegroundColor Cyan
Write-Host ('Repository: ' + $MN_RepoRoot)
Update-SessionPath

if ($ModelsDir) { $ModelsDir = [IO.Path]::GetFullPath($ModelsDir) }
$modelsPath = $MN_DefaultModelsDir
if ($ModelsDir) { $modelsPath = $ModelsDir }
$verifyScript = Join-Path $PSScriptRoot 'verify.ps1'
$verifyArgs = @{}
if ($SkipSpeech.IsPresent) { $verifyArgs['SkipSpeech'] = $true }
if ($ModelsDir) { $verifyArgs['ModelsDir'] = $ModelsDir }

# Names used below
$nodeExe = $MN_InstNode
$trayExe = $MN_InstTray
$launcher = $MN_Launcher
$logFile = $MN_LogFile
$exeBuilt = $MN_BuiltNode
$trayBuilt = $MN_BuiltTray

# ------------------------------------------------------------------------------------------------
# -Quick: build if needed, copy, restart, verify. Nothing else.
if ($Quick.IsPresent) {
    Write-Head 'Quick: build, copy, restart, verify'
    if (-not (Test-Path $launcher) -or -not (Test-Path $nodeExe)) {
        Step-Fail 'The compute node is not set up yet. Run this once without -Quick.'
        exit 1
    }
    if ($SkipBuild.IsPresent) {
        Step-Skip 'Build skipped (-SkipBuild)'
    } elseif ((-not $Force.IsPresent) -and (Test-BuildCurrent $Cpu.IsPresent $Vulkan.IsPresent $Cuda.IsPresent)) {
        Step-Skip 'Build skipped, the programs in target\release are newer than the sources'
    } elseif (-not (Invoke-MnBuild $Cpu.IsPresent $Vulkan.IsPresent $Cuda.IsPresent)) {
        exit 1
    }
    if (-not (Test-Path $exeBuilt) -or -not (Test-Path $trayBuilt)) {
        Step-Fail 'target\release does not hold both marginalia-node.exe and marginalia-node-tray.exe. Run without -SkipBuild.'
        exit 1
    }
    $running = Test-LocalPort $MN_AdminPort
    if ((-not $Force.IsPresent) -and (Test-InstalledCurrent $exeBuilt $nodeExe) -and (Test-InstalledCurrent $trayBuilt $trayExe)) {
        Step-Skip 'Copy skipped, the installed programs are the built ones'
        if ($running) { Step-Skip 'No restart needed, the node is running' }
    } else {
        if (-not (Install-MnPrograms 'both')) { exit 1 }
        Step-Ok ('Programs copied to ' + $MN_InstallDir + ' and stopped')
    }
    Start-MnProcesses 'both'
    Step-Info 'Starting the node if it is not running. Loading the models takes up to a minute.'
    if (Wait-MnNode 150) { Step-Ok 'The node answers' } else { Step-Warn ('The node did not answer yet. verify.ps1 will say why. The log is ' + $logFile) }
    Write-Head 'Verify'
    & $verifyScript @verifyArgs
    exit $LASTEXITCODE
}

# ------------------------------------------------------------------------------------------------
# What is already done? Print it first, so you know what this run will skip.
Write-Head 'Plan'
$buildCurrent = $false
if (-not $Force.IsPresent -and -not $SkipBuild.IsPresent) { $buildCurrent = Test-BuildCurrent $Cpu.IsPresent $Vulkan.IsPresent $Cuda.IsPresent }
$doBuild = (-not $SkipBuild.IsPresent) -and (-not $buildCurrent)
$installCurrent = (-not $Force.IsPresent) -and (-not $doBuild) -and (Test-InstalledCurrent $exeBuilt $nodeExe) -and (Test-InstalledCurrent $trayBuilt $trayExe)
$missingModels = @(Get-MissingModels $modelsPath $SkipSpeech.IsPresent)
$nodeTaskOk = (-not $Force.IsPresent) -and (Test-TaskRegistered $MN_NodeTask 'wscript.exe' $launcher)
$trayTaskOk = (-not $Force.IsPresent) -and (Test-TaskRegistered $MN_TrayTask $trayExe '')
$firewallOk = (-not $Force.IsPresent) -and (Test-Path $nodeExe) -and (Test-FirewallRules $nodeExe)

if ($SkipBuild.IsPresent) { Step-Skip 'Build: -SkipBuild' }
elseif ($buildCurrent) { Step-Skip 'Build: the programs in target\release are newer than the sources (-Force builds anyway)' }
else { Step-Info 'Will do:  build (the first time takes a long while, later builds a few minutes)' }
if ($SkipBuild.IsPresent -and -not (Test-Path $exeBuilt)) { Step-Warn 'There is no build in target\release, -SkipBuild will stop later.' }
if ($installCurrent) { Step-Skip 'Install: the installed programs are the built ones' } else { Step-Info 'Will do:  copy the programs to the install folder' }
if ($SkipModels.IsPresent) { Step-Skip 'Models: -SkipModels' }
elseif ($Force.IsPresent -eq $false -and $missingModels.Count -eq 0) { Step-Skip ('Models: all files are complete in ' + $modelsPath) }
else { Step-Info ('Will do:  models (' + $missingModels.Count + ' file(s) missing or incomplete in ' + $modelsPath + ')') }
if ($nodeTaskOk -and $trayTaskOk) { Step-Skip 'Tasks: both scheduled tasks exist' } else { Step-Info 'Will do:  register the scheduled tasks' }
if ($firewallOk) { Step-Skip 'Firewall: both rules exist' } else { Step-Info 'Will do:  firewall rules (one Windows permission prompt)' }
if (-not $doBuild) { Step-Skip 'Prerequisites: not needed, nothing is built' }

# ------------------------------------------------------------------------------------------------
Write-Head '1. Prerequisites'
$needCuda = $false
function New-Prereq([string]$name, [scriptblock]$check, [string]$file, [string[]]$wargs, [string]$display, [string]$note, [bool]$optional) {
    return New-Object PSObject -Property @{ Name = $name; Check = $check; File = $file; Args = $wargs; Display = $display; Note = $note; Optional = $optional }
}

function Get-Prereqs {
    $list = @()
    $list += New-Prereq 'Git' { Test-Git } 'winget' @('install', '--id', 'Git.Git', '-e', '--source', 'winget') 'winget install --id Git.Git -e --source winget' 'Not needed to build, only to update the code with git pull.' $true
    if ((Test-Cargo) -and -not (Test-RustNewEnough)) {
        $list += New-Prereq 'Rust 1.95 or newer' { Test-RustNewEnough } 'rustup' @('update', 'stable') 'rustup update stable' 'The tray libraries need a recent compiler.' $false
    }
    if (Test-Cargo) {
        $list += New-Prereq 'Rust MSVC toolchain' { Test-RustMsvc } 'rustup' @('default', 'stable-x86_64-pc-windows-msvc') 'rustup default stable-x86_64-pc-windows-msvc' 'The GNU toolchain cannot build llama.cpp here.' $false
    } else {
        $list += New-Prereq 'Rust (rustup)' { Test-Cargo } 'winget' @('install', '--id', 'Rustlang.Rustup', '-e', '--source', 'winget') 'winget install --id Rustlang.Rustup -e --source winget' 'Then open a NEW PowerShell window. If asked, choose the default (MSVC) toolchain.' $false
    }
    $list += New-Prereq 'Visual Studio Build Tools (C++)' { [bool](Get-VsInstall) } 'winget' @('install', '--id', 'Microsoft.VisualStudio.2022.BuildTools', '-e', '--source', 'winget', '--override', '--passive --wait --add Microsoft.VisualStudio.Workload.VCTools --includeRecommended') 'winget install --id Microsoft.VisualStudio.2022.BuildTools -e --source winget --override "--passive --wait --add Microsoft.VisualStudio.Workload.VCTools --includeRecommended"' 'About 6 GB and several minutes. Needs the "Desktop development with C++" workload. If you install it by hand, tick that workload.' $false
    $cmakeVerb = 'install'
    if (Find-Command 'cmake.exe') { $cmakeVerb = 'upgrade' }
    $list += New-Prereq 'CMake (4.2 or newer for Visual Studio 2026)' { Test-Cmake } 'winget' @($cmakeVerb, '--id', 'Kitware.CMake', '-e', '--source', 'winget') "winget $cmakeVerb --id Kitware.CMake -e --source winget" 'Open a NEW PowerShell window afterwards if it is still not found.' $false
    $list += New-Prereq 'LLVM (libclang, for bindgen)' { Test-Libclang } 'winget' @('install', '--id', 'LLVM.LLVM', '-e', '--source', 'winget') 'winget install --id LLVM.LLVM -e --source winget' 'This script sets LIBCLANG_PATH to C:\Program Files\LLVM\bin for you.' $false
    if ($needCuda) {
        $list += New-Prereq 'CUDA toolkit (nvcc)' { [bool](Find-Nvcc) } 'winget' @('install', '--id', 'Nvidia.CUDA', '-e', '--source', 'winget') 'winget install --id Nvidia.CUDA -e --source winget' 'Install it AFTER Visual Studio Build Tools. To skip the GPU for now run setup.ps1 -Cpu.' $false
    }
    return $list
}

function Get-Missing {
    $miss = @()
    foreach ($p in (Get-Prereqs)) {
        if (-not (& $p.Check)) { $miss += $p }
    }
    return $miss
}

if (-not $doBuild) {
    Step-Skip 'Nothing to build, the build tools are not needed'
} else {
    $nv = Get-NvidiaInfo
    $needCuda = $Cuda.IsPresent -or ($nv -and -not $Cpu.IsPresent -and -not $Vulkan.IsPresent)
    if ($nv) { Step-Ok ('NVIDIA graphics card found: ' + $nv.Name) } else { Step-Info 'No NVIDIA graphics card found (nvidia-smi is not available).' }
    $wingetOk = [bool](Find-Command 'winget.exe')
    $attempted = @{}
    $missing = @()
    for ($round = 1; $round -le 3; $round++) {
        $missing = @(Get-Missing)
        if ($missing.Count -eq 0 -or -not $Auto.IsPresent) { break }
        if (-not $wingetOk) { break }
        $todo = @($missing | Where-Object { -not $attempted.ContainsKey($_.Name) })
        if ($todo.Count -eq 0) { break }
        foreach ($p in $todo) {
            $attempted[$p.Name] = $true
            Step-Info ('Installing ' + $p.Name + ': ' + $p.Display)
            try {
                $installArgs = $p.Args
                & $p.File @installArgs | Out-Host
                if ($LASTEXITCODE -ne 0) { Step-Warn ($p.Name + ': the installer reported exit code ' + $LASTEXITCODE + '. Checking again.') }
            } catch {
                Step-Warn ($p.Name + ': could not run the installer (' + $_.Exception.Message + ')')
            }
            Update-SessionPath
        }
    }

    foreach ($p in (Get-Prereqs)) {
        if (& $p.Check) {
            Step-Ok $p.Name
        } elseif ($p.Optional) {
            Step-Warn ($p.Name + ' is missing (optional). ' + $p.Note)
            Step-Info ('Install: ' + $p.Display)
        } else {
            Step-Fail ($p.Name + ' is missing.')
            Step-Info ('Install: ' + $p.Display)
            if ($p.Note) { Step-Info $p.Note }
            $problems++
        }
    }
    if ($problems -gt 0) {
        Write-Host ''
        if (-not $wingetOk) {
            Step-Info 'winget was not found. Install "App Installer" from the Microsoft Store, or install the tools above by hand.'
        }
        Step-Info 'To continue: install the tools above, open a NEW PowerShell window and run this script again.'
        Step-Info 'Or run it with -Auto and it will run the winget commands itself.'
        exit 1
    }

    $longPaths = (Get-ItemProperty 'HKLM:\SYSTEM\CurrentControlSet\Control\FileSystem' -ErrorAction SilentlyContinue).LongPathsEnabled
    if ($MN_RepoRoot.Length -gt 50 -and $longPaths -ne 1) {
        Step-Warn ('The repository path is long (' + $MN_RepoRoot.Length + ' characters) and Windows long paths are off. If the build fails with "path too long", move the repository to a short folder such as C:\src\foresight.')
    }

}

# ------------------------------------------------------------------------------------------------
Write-Head '2. Build'
if ($SkipBuild.IsPresent) {
    if ((Test-Path $exeBuilt) -and (Test-Path $trayBuilt)) {
        Step-Skip ('Build skipped, using ' + (Split-Path $exeBuilt))
    } else {
        Step-Fail 'You asked for -SkipBuild but target\release does not hold both marginalia-node.exe and marginalia-node-tray.exe. Run without -SkipBuild.'
        exit 1
    }
} elseif (-not $doBuild) {
    Step-Skip 'Build skipped, the programs in target\release are newer than the sources. Use -Force to build anyway.'
} else {
    $ok = Invoke-MnBuild $Cpu.IsPresent $Vulkan.IsPresent $Cuda.IsPresent
    if (-not $ok) {
        Step-Info 'Nothing was installed. Fix the problem above and run this script again, the build continues where it stopped.'
        exit 1
    }
}

# ------------------------------------------------------------------------------------------------
Write-Head '3. Install'
$copied = $false
if ((-not $Force.IsPresent) -and (Test-InstalledCurrent $exeBuilt $nodeExe) -and (Test-InstalledCurrent $trayBuilt $trayExe)) {
    Step-Skip ('The installed programs are the built ones (' + $MN_InstallDir + ')')
} else {
    if (-not (Install-MnPrograms 'both')) { exit 1 }
    $copied = $true
    Step-Ok ('Programs copied to ' + $MN_InstallDir)
}

# A tiny launcher so the node runs without a console window and keeps a log (node.log, trimmed at 2 MB).
$modelsArg = ''
if ($ModelsDir) { $modelsArg = ' --models " & q & "' + $ModelsDir + '" & q & "' }
$vbs = @"
' Starts marginalia-node without a console window and writes its messages to node.log. Made by setup.ps1.
Set sh = CreateObject("WScript.Shell")
Set fso = CreateObject("Scripting.FileSystemObject")
q = Chr(34)
dirPath = "$MN_InstallDir"
logPath = dirPath & "\node.log"
If fso.FileExists(logPath) Then
  If fso.GetFile(logPath).Size > 2000000 Then fso.DeleteFile logPath, True
End If
sh.Environment("PROCESS")("NO_COLOR") = "1"
cmd = "cmd.exe /c " & q & q & dirPath & "\marginalia-node.exe" & q & " run$modelsArg >> " & q & logPath & q & " 2>&1" & q
WScript.Quit sh.Run(cmd, 0, True)
"@
$vbs = $vbs -replace "`r?`n", "`r`n"
$oldVbs = ''
if (Test-Path $launcher) { $oldVbs = [string](Get-Content -Path $launcher -Raw -ErrorAction SilentlyContinue) }
$launcherChanged = $false
if ($Force.IsPresent -or ($oldVbs.TrimEnd() -ne $vbs.TrimEnd())) {
    Set-Content -Path $launcher -Value $vbs -Encoding ASCII
    $launcherChanged = $true
    Step-Ok 'Launcher written (start-node.vbs, hides the console window and keeps node.log)'
} else {
    Step-Skip 'The launcher (start-node.vbs) is already right'
}

$userPath = [Environment]::GetEnvironmentVariable('Path', 'User')
if (($userPath -split ';') -notcontains $MN_InstallDir) {
    [Environment]::SetEnvironmentVariable('Path', ($userPath.TrimEnd(';') + ';' + $MN_InstallDir), 'User')
    Step-Ok 'Added the install folder to your PATH (new windows can run marginalia-node.exe)'
} else {
    Step-Skip 'The install folder is already in your PATH'
}

# ------------------------------------------------------------------------------------------------
Write-Head '4. Models'
if ($SkipModels.IsPresent) {
    Step-Skip 'Models skipped (-SkipModels). Later: marginalia-node.exe models download'
} elseif ((-not $Force.IsPresent) -and (@(Get-MissingModels $modelsPath $SkipSpeech.IsPresent).Count -eq 0)) {
    Step-Skip ('All model files are complete in ' + $modelsPath)
} else {
    $dlArgs = @('models', 'download')
    if ($ModelsDir) { $dlArgs += @('--dir', $ModelsDir) }
    if ($SkipSpeech.IsPresent) { $dlArgs += '--skip-speech' }
    & $nodeExe @dlArgs
    $dlCode = $LASTEXITCODE
    # The larger chat model too, so the tray menu can switch to it. It is skipped when it is already there.
    if ($dlCode -eq 0) {
        $largeArgs = @('models', 'download', '--only', 'chat', '--chat-model', 'e4b')
        if ($ModelsDir) { $largeArgs += @('--dir', $ModelsDir) }
        & $nodeExe @largeArgs
        $dlCode = $LASTEXITCODE
    }
    if ($dlCode -eq 0) {
        Step-Ok 'Models are in place'
    } else {
        Step-Warn 'The model download did not finish. Run  marginalia-node.exe models download  to continue where it stopped. If it asks for a login, set HF_TOKEN first.'
        $problems++
    }
}

# ------------------------------------------------------------------------------------------------
Write-Head '5. Start at logon (Task Scheduler)'
try {
    if ((-not $Force.IsPresent) -and (Test-TaskRegistered $MN_NodeTask 'wscript.exe' $launcher)) {
        Step-Skip ('Task "' + $MN_NodeTask + '" already exists')
    } else {
        Register-MnTask $MN_NodeTask 'wscript.exe' ('//B //Nologo "' + $launcher + '"') $MN_InstallDir 'Runs the Marginalia compute node. Restarts it if it fails.'
        Step-Ok ('Task "' + $MN_NodeTask + '" runs the node at logon (hidden, restarts after a failure)')
    }
    if ((-not $Force.IsPresent) -and (Test-TaskRegistered $MN_TrayTask $trayExe '')) {
        Step-Skip ('Task "' + $MN_TrayTask + '" already exists')
    } else {
        Register-MnTask $MN_TrayTask $trayExe '' $MN_InstallDir 'Shows the Marginalia compute node icon in the system tray.'
        Step-Ok ('Task "' + $MN_TrayTask + '" shows the tray icon at logon')
    }
} catch {
    Step-Fail ('Could not create the scheduled tasks: ' + $_.Exception.Message)
    $problems++
}

# ------------------------------------------------------------------------------------------------
Write-Head '6. Windows Firewall'
if ((-not $Force.IsPresent) -and (Test-FirewallRules $nodeExe)) {
    Step-Skip 'Firewall rules for UDP 47615 and UDP 5353 already exist'
} else {
    $go = $true
    if (-not (Test-IsAdmin) -and -not $Auto.IsPresent) {
        Write-Host 'Phones can only reach this machine if the firewall allows it. Adding the two rules needs administrator permission.'
        Write-Host 'Windows will show one permission prompt.'
        $answer = Read-Host 'Add the firewall rules now? [Y/n]'
        if ($answer -match '^\s*n') { $go = $false }
    }
    if (-not $go) {
        Step-Warn ('Firewall rules skipped. Phones will not find this node until they exist. Run later in an administrator window:  .\compute-node\windows\firewall.ps1 -Action Add -Exe "' + $nodeExe + '"')
        $problems++
    } else {
        if (Test-IsAdmin) {
            & (Join-Path $PSScriptRoot 'firewall.ps1') -Action Add -Exe $nodeExe | Out-Null
        } else {
            [void](Invoke-FirewallElevated 'Add' $nodeExe)
        }
        if (Test-FirewallRules $nodeExe) {
            Step-Ok 'Firewall rules added for UDP 47615 and UDP 5353 on Private networks'
        } else {
            Step-Warn ('The firewall rules were not added (permission refused?). Run in an administrator window:  .\compute-node\windows\firewall.ps1 -Action Add -Exe "' + $nodeExe + '"')
            $problems++
        }
    }
}
$public = @(Get-NetConnectionProfile -ErrorAction SilentlyContinue | Where-Object { $_.NetworkCategory -eq 'Public' })
if ($public.Count -gt 0) {
    Step-Warn ('The network "' + $public[0].Name + '" is set to Public, and the rules only apply to Private networks. Settings, Network and Internet, open your network, Network profile type: Private.')
}

# ------------------------------------------------------------------------------------------------
Write-Head '7. Start now'
if ($launcherChanged -and -not $copied) { Stop-MnProcesses 'node' }
if (Test-LocalPort $MN_AdminPort) {
    Step-Skip 'The node is already running'
} else {
    Start-MnProcesses 'node'
    Step-Info 'Starting the node. Loading the models and measuring the speed takes up to a minute.'
    if (Wait-MnNode 120) {
        Step-Ok 'The node is running'
    } else {
        Step-Warn ('The node did not answer yet. Look at the end of ' + $logFile + ' for the reason (for example missing models).')
        $problems++
    }
}
if (Get-Process -Name 'marginalia-node-tray' -ErrorAction SilentlyContinue) {
    Step-Skip 'The tray icon is already running'
} else {
    Start-MnProcesses 'tray'
    Start-Sleep -Seconds 2
    if (Get-Process -Name 'marginalia-node-tray' -ErrorAction SilentlyContinue) { Step-Ok 'The tray icon is running' } else { Step-Warn 'The tray icon did not start. Try starting the task "Marginalia Node Tray" in Task Scheduler.'; $problems++ }
}

# ------------------------------------------------------------------------------------------------
$verifyFailed = $false
if ($NoVerify.IsPresent) {
    Step-Skip 'verify.ps1 not run (-NoVerify)'
} else {
    Write-Head 'Verify'
    & $verifyScript @verifyArgs
    if ($LASTEXITCODE -ne 0) { $verifyFailed = $true }
}

# ------------------------------------------------------------------------------------------------
Write-Head 'Next steps'
$mins = [int]((Get-Date) - $started).TotalMinutes
if ($problems -eq 0 -and -not $verifyFailed) { Step-Ok ('Setup finished in about ' + $mins + ' minutes') } else { Step-Warn ('Setup finished with things to look at, see the lines above. Check again at any time with  .\compute-node\windows\verify.ps1') }
$addresses = @()
if (Test-LocalPort $MN_AdminPort) {
    try { $addresses = @((& $nodeExe status | ConvertFrom-Json).addresses) } catch { }
}
Write-Host ''
Write-Host '  Tray icon : look at the bottom right near the clock, maybe behind the ^ arrow.'
Write-Host '              Drag it onto the taskbar to keep it visible. Left click opens the details panel.'
Write-Host '  Phone     : open the app, Settings, Compute nodes. It lists this machine by itself when both are'
Write-Host '              on the same network, then tap Connect. Or run  marginalia-node.exe pair  for a code.'
if ($addresses.Count -gt 0) { Write-Host ('  Addresses : ' + ($addresses -join '   ')) }
Write-Host '  Check     : .\compute-node\windows\verify.ps1      (one line per check)'
Write-Host '  After code changes : .\compute-node\windows\dev.ps1     (or  setup.ps1 -Quick  after a git pull)'
Write-Host ('  Log       : ' + $logFile)
Write-Host '  Remove    : .\compute-node\windows\uninstall.ps1'
Write-Host ''
if ($problems -gt 0 -or $verifyFailed) { exit 1 }
