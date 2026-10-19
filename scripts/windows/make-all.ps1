# One command to build everything on Windows: the node, the tray icon and the optional speaker helper.
#   .\scripts\windows\make-all.ps1                 build all three (CUDA, Vulkan or CPU is picked by itself)
#   .\scripts\windows\make-all.ps1 -Cpu            force the CPU only build
#   .\scripts\windows\make-all.ps1 -Test           also run the tests that need no phone
#   .\scripts\windows\make-all.ps1 -Install        also copy the programs into the install folder and restart them
#   .\scripts\windows\make-all.ps1 -Models speakers  also download optional models (speakers or vision, or all for the needed ones)
# The phone app is built on Linux or macOS, see docs\DEVELOPMENT.md. The programs end up in target\release.
param(
    [switch]$Cpu,
    [switch]$Vulkan,
    [switch]$Cuda,
    [switch]$Test,
    [switch]$Install,
    [ValidateSet('', 'all', 'speakers', 'vision')][string]$Models = ''
)
. (Join-Path $PSScriptRoot 'common.ps1')
Update-SessionPath

Write-Head '1. Build the node, the tray icon and the speaker helper'
if (-not (Invoke-MnBuild $Cpu.IsPresent $Vulkan.IsPresent $Cuda.IsPresent)) { exit 1 }

if ($Test) {
    Write-Head '2. Tests'
    Push-Location $MN_RepoRoot
    try {
        & cargo test -p marginalia-proto -p marginalia-node -p marginalia-node-tray -p marginalia-node-speakers | Out-Host
        if ($LASTEXITCODE -ne 0) { Step-Fail 'Some tests failed.'; exit 1 }
        Step-Ok 'All tests passed'
    } finally { Pop-Location }
}

if ($Models -ne '') {
    Write-Head '3. Models'
    $args = @('models', 'download')
    if ($Models -ne 'all') { $args += @('--only', $Models) }
    & $MN_BuiltNode @args | Out-Host
    if ($LASTEXITCODE -ne 0) { Step-Warn 'The download did not finish. Run the same command again to continue where it stopped.' }
}

if ($Install) {
    Write-Head '4. Install and restart'
    if (-not (Install-MnPrograms 'both')) { exit 1 }
    Step-Ok ('Copied to ' + $MN_InstallDir + '. Start them with: Start-ScheduledTask -TaskName "' + $MN_NodeTask + '" (or run .\scripts\windows\setup.ps1 -Quick)')
}

Step-Ok ('Done. The programs are in ' + (Join-Path $MN_RepoRoot 'target\release'))
exit 0
