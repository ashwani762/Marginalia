# Removes what setup.ps1 made: the two scheduled tasks, the firewall rules, the installed programs.
# The models and the node's identity and paired phones are kept unless you add -Purge.
#   .\scripts\windows\uninstall.ps1
#   .\scripts\windows\uninstall.ps1 -Purge     also delete the models and the node's state
param([switch]$Purge)
. (Join-Path $PSScriptRoot 'common.ps1')

Write-Head 'Stopping'
Stop-MnProcesses
Step-Ok 'Node and tray stopped'

Write-Head 'Removing'
foreach ($t in @($MN_NodeTask, $MN_TrayTask)) {
    if (Get-ScheduledTask -TaskName $t -ErrorAction SilentlyContinue) {
        Unregister-ScheduledTask -TaskName $t -Confirm:$false -ErrorAction SilentlyContinue
        Step-Ok ('Task "' + $t + '" removed')
    } else {
        Step-Skip ('Task "' + $t + '" was not there')
    }
}

$haveRules = $false
foreach ($n in @($MN_FwNode, $MN_FwMdns)) { if (Get-NetFirewallRule -DisplayName $n -ErrorAction SilentlyContinue) { $haveRules = $true } }
if ($haveRules) {
    if (Test-IsAdmin) {
        & (Join-Path $PSScriptRoot 'firewall.ps1') -Action Remove | Out-Null
    } else {
        Step-Info 'Removing the firewall rules needs administrator permission. Windows will ask once.'
        [void](Invoke-FirewallElevated 'Remove' '')
    }
    $left = $false
    foreach ($n in @($MN_FwNode, $MN_FwMdns)) { if (Get-NetFirewallRule -DisplayName $n -ErrorAction SilentlyContinue) { $left = $true } }
    if ($left) { Step-Warn 'The firewall rules are still there (permission refused?). Run firewall.ps1 -Action Remove in an administrator window.' } else { Step-Ok 'Firewall rules removed' }
} else {
    Step-Skip 'No firewall rules were there'
}

$userPath = [Environment]::GetEnvironmentVariable('Path', 'User')
if ($userPath) {
    $keep = @($userPath -split ';' | Where-Object { $_ -and $_ -ne $MN_InstallDir })
    [Environment]::SetEnvironmentVariable('Path', ($keep -join ';'), 'User')
}
if (Test-Path $MN_InstallDir) {
    Remove-Item -Recurse -Force $MN_InstallDir -ErrorAction SilentlyContinue
    if (Test-Path $MN_InstallDir) { Step-Warn ($MN_InstallDir + ' could not be fully removed (a file is in use). Delete it by hand after a restart.') } else { Step-Ok ('Removed ' + $MN_InstallDir) }
} else {
    Step-Skip 'The install folder was not there'
}

if ($Purge.IsPresent) {
    $marginalia = Join-Path $env:APPDATA 'marginalia'
    foreach ($d in @($marginalia, $MN_StateDir)) {
        if (Test-Path $d) {
            Remove-Item -Recurse -Force $d -ErrorAction SilentlyContinue
            Step-Ok ('Deleted ' + $d)
        }
    }
    Step-Info 'Phones that were paired with this node have to pair again if you set it up anew.'
} else {
    Step-Info ('Kept the models (' + $MN_DefaultModelsDir + ') and the node state (' + $MN_StateDir + '). Add -Purge to delete them too.')
}
