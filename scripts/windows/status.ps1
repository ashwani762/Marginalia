# Shows whether the node is running, the state of its tasks and its numbers.
#   .\scripts\windows\status.ps1
. (Join-Path $PSScriptRoot 'common.ps1')
Update-SessionPath

$exe = Find-NodeExe
$nodeProc = @(Get-Process -Name 'marginalia-node' -ErrorAction SilentlyContinue)
$trayProc = @(Get-Process -Name 'marginalia-node-tray' -ErrorAction SilentlyContinue)
$answers = Test-LocalPort $MN_AdminPort

Write-Head 'Node'
if ($answers) { Step-Ok ('The node answers on 127.0.0.1:' + $MN_AdminPort) }
elseif ($nodeProc.Count -gt 0) { Step-Warn 'The node process exists but does not answer yet. It may still be loading the models (up to a minute).' }
else { Step-Fail 'The node is not running. Start it with:  Start-ScheduledTask -TaskName "Marginalia Node"' }
if ($trayProc.Count -gt 0) { Step-Ok 'The tray icon program is running' } else { Step-Warn 'The tray icon program is not running. Start it with:  Start-ScheduledTask -TaskName "Marginalia Node Tray"' }

Write-Head 'Tasks and firewall'
Step-Info ('Task "' + $MN_NodeTask + '": ' + (Get-TaskState $MN_NodeTask))
Step-Info ('Task "' + $MN_TrayTask + '": ' + (Get-TaskState $MN_TrayTask))
$installed = Join-Path $MN_InstallDir 'marginalia-node.exe'
if (Test-Path $installed) {
    if (Test-FirewallRules $installed) { Step-Ok 'Firewall rules for UDP 47615 and UDP 5353 are in place' } else { Step-Warn 'Firewall rules are missing or point to another program. Run setup.ps1 again.' }
} else {
    Step-Warn ('Not installed in ' + $MN_InstallDir + '. Run setup.ps1.')
}
$public = @(Get-NetConnectionProfile -ErrorAction SilentlyContinue | Where-Object { $_.NetworkCategory -eq 'Public' })
if ($public.Count -gt 0) { Step-Warn ('The network "' + $public[0].Name + '" is Public. The firewall rules only apply to Private networks.') }

if ($exe) {
    Write-Head 'Models'
    & $exe models list
    if ($answers) {
        Write-Head 'Numbers (marginalia-node.exe stats)'
        & $exe stats
        try {
            $st = (& $exe status | ConvertFrom-Json)
            Write-Head 'Reachable at'
            foreach ($a in @($st.addresses)) { Step-Info $a }
        } catch { }
    }
}
$log = Join-Path $MN_InstallDir 'node.log'
if (Test-Path $log) {
    Write-Head 'Last lines of node.log'
    Get-Content $log -Tail 8
}
