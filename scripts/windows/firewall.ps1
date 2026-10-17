# Adds or removes the Windows Firewall rules for the node. Needs administrator, so setup.ps1 and
# uninstall.ps1 start it with one Windows prompt. You can also run it yourself from an administrator window.
#   .\firewall.ps1 -Action Add -Exe "C:\Users\you\AppData\Local\MarginaliaNode\marginalia-node.exe"
#   .\firewall.ps1 -Action Remove
param(
    [Parameter(Mandatory = $true)][ValidateSet('Add', 'Remove')][string]$Action,
    [string]$Exe = ''
)
. (Join-Path $PSScriptRoot 'common.ps1')

if (-not (Test-IsAdmin)) {
    Step-Fail 'This script needs an administrator window (right click PowerShell, Run as administrator).'
    exit 1
}
foreach ($n in @($MN_FwNode, $MN_FwMdns)) {
    Get-NetFirewallRule -DisplayName $n -ErrorAction SilentlyContinue | Remove-NetFirewallRule -ErrorAction SilentlyContinue
}
if ($Action -eq 'Add') {
    if (-not $Exe -or -not (Test-Path $Exe)) {
        Step-Fail ('The program was not found: ' + $Exe)
        exit 1
    }
    New-NetFirewallRule -DisplayName $MN_FwNode -Direction Inbound -Action Allow -Protocol UDP -LocalPort $MN_NodePort -Program $Exe -Profile Private | Out-Null
    New-NetFirewallRule -DisplayName $MN_FwMdns -Direction Inbound -Action Allow -Protocol UDP -LocalPort 5353 -Program $Exe -Profile Private | Out-Null
    Step-Ok 'Firewall rules added for UDP 47615 and UDP 5353 on Private networks.'
} else {
    Step-Ok 'Firewall rules removed.'
}
