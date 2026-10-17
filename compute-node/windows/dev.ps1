# The inner loop of development: build, replace the running programs, start them again, follow the node log.
# One command per try. Needs setup.ps1 to have been run once.
#   .\compute-node\windows\dev.ps1            build node and tray, restart both, follow node.log
#   .\compute-node\windows\dev.ps1 node       only the node (the tray keeps running)
#   .\compute-node\windows\dev.ps1 tray       only the tray (the node keeps running, so the models stay loaded)
#   .\compute-node\windows\dev.ps1 node -NoLog   do not follow the log afterwards
# Press Ctrl+C to stop following the log. The node and the tray keep running.
# The backend (CUDA, Vulkan or CPU) is picked like setup.ps1 does. -Cpu, -Cuda and -Vulkan force one.
param(
    [Parameter(Position = 0)][ValidateSet('both', 'node', 'tray')][string]$What = 'both',
    [switch]$Cpu,
    [switch]$Vulkan,
    [switch]$Cuda,
    [switch]$NoLog,
    [int]$LogLines = 25
)
. (Join-Path $PSScriptRoot 'common.ps1')
Update-SessionPath
$clock = [Diagnostics.Stopwatch]::StartNew()

Write-Host ''
Write-Host ('Marginalia compute node: dev loop (' + $What + ')') -ForegroundColor Cyan

if (-not (Test-Path $MN_Launcher) -or -not (Test-Path $MN_InstNode)) {
    Step-Fail 'The compute node is not installed yet.'
    Step-Info ('Run once:  & "' + (Join-Path $PSScriptRoot 'setup.ps1') + '"')
    exit 1
}

# 1. Build
Write-Head '1. Build'
$ok = Invoke-MnBuild $Cpu.IsPresent $Vulkan.IsPresent $Cuda.IsPresent $What
if (-not $ok) {
    Step-Info 'The running programs were left alone. Fix the error above and run this again.'
    exit 1
}
$buildSec = [int]$clock.Elapsed.TotalSeconds

# 2. Replace
Write-Head '2. Replace the programs'
if (-not (Install-MnPrograms $What)) { exit 1 }
Step-Ok ('Copied to ' + $MN_InstallDir)

# 3. Start
Write-Head '3. Start'
Start-MnProcesses $What
if ($What -ne 'tray') {
    Step-Info 'Loading the models and measuring the speed takes up to a minute.'
    if (Wait-MnNode 150) {
        Step-Ok 'The node answers'
        $st = Get-MnStatus $MN_InstNode
        if ($st) {
            if ($null -ne $st.chat) { Step-Info ('chat: ' + $st.chat) } else { Step-Warn 'chat model not loaded' }
            if ($null -ne $st.embed) { Step-Info ('search: ' + $st.embed) } else { Step-Warn 'search model not loaded' }
            if ($null -ne $st.speech) { Step-Info ('speech: ' + $st.speech) } else { Step-Warn 'speech model not loaded' }
        }
    } else {
        Step-Fail ('The node does not answer. Read the log below, or run it in a window to see the error:  & "' + $MN_InstNode + '" run')
    }
}
if ($What -ne 'node') {
    Start-Sleep -Seconds 2
    if (Get-Process -Name 'marginalia-node-tray' -ErrorAction SilentlyContinue) {
        Step-Ok 'The tray program is running'
    } else {
        Step-Fail ('The tray program did not stay running. Run it in a window to see the error:  & "' + $MN_InstTray + '"')
    }
}
Write-Host ''
Write-Host ('Done in ' + [int]$clock.Elapsed.TotalSeconds + ' seconds (build ' + $buildSec + '). Full check:  & "' + (Join-Path $PSScriptRoot 'verify.ps1') + '"') -ForegroundColor Green

# 4. Follow the log
if (-not $NoLog.IsPresent) {
    Write-Head ('4. node.log (last ' + $LogLines + ' lines, Ctrl+C to stop)')
    if (Test-Path $MN_LogFile) {
        Get-Content -Path $MN_LogFile -Tail $LogLines -Wait
    } else {
        Step-Warn ('No log yet at ' + $MN_LogFile)
    }
}
