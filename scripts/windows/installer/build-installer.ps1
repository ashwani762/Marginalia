# Builds the setup wizard: target\setup\Marginalia-Node-Setup-<version>.exe
#   powershell -ExecutionPolicy Bypass -File scripts\windows\installer\build-installer.ps1 [-NoBuild]
# Needs Inno Setup 6 (winget install JRSoftware.InnoSetup). It builds the node, the speaker helper and the tray first,
# unless -NoBuild says the ones in target\release are the ones to use. PowerShell 5.1, ASCII only.
param(
    [switch]$NoBuild
)
$ErrorActionPreference = 'Stop'
$root = (Resolve-Path (Join-Path $PSScriptRoot '..\..\..')).Path

$iscc = @(
    (Join-Path $env:LOCALAPPDATA 'Programs\Inno Setup 6\ISCC.exe'),
    (Join-Path ${env:ProgramFiles(x86)} 'Inno Setup 6\ISCC.exe'),
    (Join-Path $env:ProgramFiles 'Inno Setup 6\ISCC.exe')
) | Where-Object { Test-Path $_ } | Select-Object -First 1
if (-not $iscc) { throw 'Inno Setup 6 was not found. Install it with: winget install JRSoftware.InnoSetup' }

$line = Select-String -Path (Join-Path $root 'compute-node\node\Cargo.toml') -Pattern '^version\s*=\s*"([^"]+)"' | Select-Object -First 1
$version = $line.Matches[0].Groups[1].Value

if (-not $NoBuild) {
    # The same build setup.ps1 and make-all.ps1 do, with the graphics card backend this machine has.
    & (Join-Path $PSScriptRoot '..\make-all.ps1')
    if ($LASTEXITCODE) { throw 'The build failed' }
}
foreach ($f in 'marginalia-node.exe', 'marginalia-node-tray.exe') {
    if (-not (Test-Path (Join-Path $root "target\release\$f"))) { throw "target\release\$f is missing, build first" }
}

& $iscc "/DAppVersion=$version" "/DSourceDir=$root\target\release" "/DOutputDir=$root\target\setup" (Join-Path $PSScriptRoot 'MarginaliaNode.iss')
if ($LASTEXITCODE) { throw 'Inno Setup failed' }
Write-Host "setup: $root\target\setup\Marginalia-Node-Setup-$version.exe"
