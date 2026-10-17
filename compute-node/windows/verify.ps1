# Quick acceptance check of the compute node on Windows. One line per check, details only for the ones that fail.
#   .\compute-node\windows\verify.ps1
#   .\compute-node\windows\verify.ps1 -SkipProbe     leave out the self test (the checks that talk to the node as a phone would)
#   .\compute-node\windows\verify.ps1 -SkipSpeech    you left out the speech model on purpose
#   .\compute-node\windows\verify.ps1 -ModelsDir D:\models
# Works before anything is installed too: then the first failing line says to run setup.ps1.
# The last lines say how many checks passed and the one command to run for the first failing check.
# The checks are numbered. docs\WINDOWS-PLAN.md lists them in the same order.
param(
    [switch]$SkipProbe,
    [switch]$SkipSpeech,
    [string]$ModelsDir = '',
    [switch]$NoWait
)
. (Join-Path $PSScriptRoot 'common.ps1')
Update-SessionPath

$setupCmd = '& "' + (Join-Path $PSScriptRoot 'setup.ps1') + '"'
$devCmd = '& "' + (Join-Path $PSScriptRoot 'dev.ps1') + '"'
$firewallScript = Join-Path $PSScriptRoot 'firewall.ps1'

# ---- Result lines --------------------------------------------------------------------------------
$script:num = 0
$script:counted = 0
$script:passed = 0
$script:failed = 0
$script:firstFix = ''
$script:firstLabel = ''

function Get-CheckLabel([string]$name) {
    $script:num++
    return ('{0:00} {1}' -f $script:num, $name)
}
function Check-Ok([string]$name, [string]$detail) {
    $script:counted++
    $script:passed++
    Step-Ok ((Get-CheckLabel $name) + ': ' + $detail)
}
function Check-Fail([string]$name, [string]$why, [string]$fix, [string[]]$more = @()) {
    $script:counted++
    $script:failed++
    $label = Get-CheckLabel $name
    if (-not $script:firstFix) {
        $script:firstFix = $fix
        $script:firstLabel = $label
    }
    Step-Fail ($label + ': ' + $why)
    foreach ($m in $more) { if ($m) { Step-Info $m } }
    Step-Info ('Fix: ' + $fix)
}
function Check-Skip([string]$name, [string]$why) {
    Step-Skip ((Get-CheckLabel $name) + ': ' + $why)
}
function Check-Note([string]$name, [string]$text) {
    Step-Warn ((Get-CheckLabel $name) + ': ' + $text)
}

function Get-LastLines([string]$text, [int]$count) {
    if (-not $text) { return @() }
    $lines = @(($text -split "`r?`n") | Where-Object { $_.Trim() -ne '' })
    if ($lines.Count -le $count) { return $lines }
    return @($lines[($lines.Count - $count)..($lines.Count - 1)])
}

function New-SilentWav([string]$path, [int]$seconds) {
    $n = 16000 * 2 * $seconds
    $ms = New-Object IO.MemoryStream
    $w = New-Object IO.BinaryWriter($ms)
    $w.Write([Text.Encoding]::ASCII.GetBytes('RIFF'))
    $w.Write([int](36 + $n))
    $w.Write([Text.Encoding]::ASCII.GetBytes('WAVE'))
    $w.Write([Text.Encoding]::ASCII.GetBytes('fmt '))
    $w.Write([int]16)
    $w.Write([int16]1)
    $w.Write([int16]1)
    $w.Write([int]16000)
    $w.Write([int]32000)
    $w.Write([int16]2)
    $w.Write([int16]16)
    $w.Write([Text.Encoding]::ASCII.GetBytes('data'))
    $w.Write([int]$n)
    $w.Write((New-Object byte[] $n))
    $w.Flush()
    [IO.File]::WriteAllBytes($path, $ms.ToArray())
    $w.Close()
}

Write-Host ''
Write-Host 'Marginalia compute node: verify' -ForegroundColor Cyan
Write-Host ''

$cli = Find-NodeExe
if ($ModelsDir) { $modelsPath = [IO.Path]::GetFullPath($ModelsDir) } else { $modelsPath = $MN_DefaultModelsDir }
$tailLog = 'Get-Content "' + $MN_LogFile + '" -Tail 40'

# ---- 01 Programs installed -----------------------------------------------------------------------
$haveNode = Test-Path $MN_InstNode
$haveTray = Test-Path $MN_InstTray
if ($haveNode -and $haveTray) {
    Check-Ok 'programs installed' ('node ' + (Format-Bytes (Get-Item $MN_InstNode).Length) + ', tray ' + (Format-Bytes (Get-Item $MN_InstTray).Length) + ' in ' + $MN_InstallDir)
} else {
    $gone = @()
    if (-not $haveNode) { $gone += 'marginalia-node.exe' }
    if (-not $haveTray) { $gone += 'marginalia-node-tray.exe' }
    Check-Fail 'programs installed' ('not found in ' + $MN_InstallDir + ': ' + ($gone -join ', ') + '. The compute node is not installed yet.') $setupCmd
}

# ---- 02 Installed copy matches the build ----------------------------------------------------------
if (-not ($haveNode -and $haveTray)) {
    Check-Skip 'installed copy is current' 'nothing installed yet'
} elseif (-not (Test-Path $MN_BuiltNode)) {
    Check-Skip 'installed copy is current' 'no build in target\release to compare with'
} else {
    $builtTime = (Get-Item $MN_BuiltNode).LastWriteTime
    if ((Test-Path $MN_BuiltTray) -and (Get-Item $MN_BuiltTray).LastWriteTime -lt $builtTime) { $builtTime = (Get-Item $MN_BuiltTray).LastWriteTime }
    if ((Get-NewestSourceTime) -gt $builtTime) {
        Check-Fail 'installed copy is current' 'the code changed after the last build, so what runs is old' $devCmd
    } elseif (-not (Test-InstalledCurrent $MN_BuiltNode $MN_InstNode) -or ((Test-Path $MN_BuiltTray) -and -not (Test-InstalledCurrent $MN_BuiltTray $MN_InstTray))) {
        Check-Fail 'installed copy is current' 'target\release holds newer programs than the installed ones' $devCmd
    } else {
        Check-Ok 'installed copy is current' 'the installed programs are the last build'
    }
}

# ---- 03 to 05 Models -----------------------------------------------------------------------------
if ($cli) {
    $dlFix = '& "' + $cli + '" models download'
    if ($ModelsDir) { $dlFix += ' --dir "' + $modelsPath + '"' }
} else {
    $dlFix = '& "' + (Join-Path $PSScriptRoot 'download-models.ps1') + '"'
    if ($ModelsDir) { $dlFix += ' -Dir "' + $modelsPath + '"' }
}
$present = @{}
foreach ($m in $MN_Models) {
    $name = $m.Role + ' model'
    $f = Join-Path $modelsPath $m.File
    $have = Test-Path $f
    $present[$m.Role] = $have
    if ($m.Role -eq 'speech' -and $SkipSpeech.IsPresent) {
        Check-Skip $name 'left out on purpose (-SkipSpeech)'
    } elseif (-not $have) {
        Check-Fail $name ($m.File + ' is missing in ' + $modelsPath) $dlFix
    } else {
        $len = (Get-Item $f).Length
        if ($len -ne $m.Size) {
            Check-Fail $name ($m.File + ' has ' + (Format-Bytes $len) + ', expected ' + (Format-Bytes $m.Size) + '. The download did not finish.') $dlFix
        } else {
            Check-Ok $name ($m.File + ' ' + (Format-Bytes $len))
        }
    }
}

# ---- 06 and 07 Scheduled tasks -------------------------------------------------------------------
foreach ($t in @($MN_NodeTask, $MN_TrayTask)) {
    $state = Get-TaskState $t
    if ($state -eq 'not installed') {
        Check-Fail ('task "' + $t + '"') 'the scheduled task does not exist, so it will not start at logon' $setupCmd
    } elseif ($state -eq 'Disabled') {
        Check-Fail ('task "' + $t + '"') 'the scheduled task is disabled' ('Enable-ScheduledTask -TaskName "' + $t + '"')
    } else {
        Check-Ok ('task "' + $t + '"') ('state ' + $state + ', starts at logon')
    }
}

# ---- 08 Firewall ---------------------------------------------------------------------------------
if (-not $haveNode) {
    Check-Skip 'firewall rules' 'nothing installed yet'
} elseif (Test-FirewallRules $MN_InstNode) {
    Check-Ok 'firewall rules' 'UDP 47615 and UDP 5353 are allowed for the installed node (Private networks)'
    $public = @(Get-NetConnectionProfile -ErrorAction SilentlyContinue | Where-Object { $_.NetworkCategory -eq 'Public' })
    if ($public.Count -gt 0) {
        Step-Warn ('       The network "' + $public[0].Name + '" is Public. The rules only apply to Private networks. If the phone is on that network: Settings, Network and Internet, open it, Network profile type: Private.')
    }
} else {
    Check-Fail 'firewall rules' 'the two rules are missing or point to another program' ('in an administrator window: & "' + $firewallScript + '" -Action Add -Exe "' + $MN_InstNode + '"')
}

# ---- 09 Node running -----------------------------------------------------------------------------
$answers = Test-LocalPort $MN_AdminPort
$nodeProc = @(Get-Process -Name 'marginalia-node' -ErrorAction SilentlyContinue)
$age = 99999.0
if ($nodeProc.Count -gt 0) {
    try { $age = ((Get-Date) - $nodeProc[0].StartTime).TotalSeconds } catch { }
}
$settleUntil = (Get-Date).AddSeconds(-1)
if (-not $NoWait.IsPresent -and $age -lt 150) {
    # A node that just started is still loading the models and measuring its speed (up to about two minutes).
    $settleUntil = (Get-Date).AddSeconds(150 - $age)
}
if (-not $answers -and $nodeProc.Count -gt 0 -and (Get-Date) -lt $settleUntil) {
    Step-Info 'The node started a moment ago. Waiting for it to load the models (up to 2 minutes).'
    $answers = Wait-MnNode ([int]($settleUntil - (Get-Date)).TotalSeconds)
}
if ($answers) {
    Check-Ok 'node running' ('answers on 127.0.0.1:' + $MN_AdminPort)
} else {
    $logLines = @()
    if (Test-Path $MN_LogFile) { $logLines = @(Get-LastLines (Get-Content $MN_LogFile -Tail 12 -ErrorAction SilentlyContinue | Out-String) 5 | ForEach-Object { 'log: ' + $_ }) }
    if ($nodeProc.Count -gt 0) {
        Check-Fail 'node running' ('the node process exists but nothing answers on port ' + $MN_AdminPort) $tailLog $logLines
    } elseif (Get-ScheduledTask -TaskName $MN_NodeTask -ErrorAction SilentlyContinue) {
        Check-Fail 'node running' 'the node is not running' ('Start-ScheduledTask -TaskName "' + $MN_NodeTask + '"') ($logLines + @('If it stops again right away read node.log: ' + $tailLog))
    } else {
        Check-Fail 'node running' 'the node is not running' $setupCmd $logLines
    }
}

# ---- 10 Tray running -----------------------------------------------------------------------------
if (Get-Process -Name 'marginalia-node-tray' -ErrorAction SilentlyContinue) {
    Check-Ok 'tray running' 'the tray icon program is running (look near the clock)'
} elseif (Get-ScheduledTask -TaskName $MN_TrayTask -ErrorAction SilentlyContinue) {
    Check-Fail 'tray running' 'the tray program is not running' ('Start-ScheduledTask -TaskName "' + $MN_TrayTask + '"') @('If it does not stay: run it in a window to see why:  & "' + $MN_InstTray + '"')
} else {
    Check-Fail 'tray running' 'the tray program is not running' $setupCmd
}

# ---- 11 Status -----------------------------------------------------------------------------------
$st = $null
if ($answers -and $cli) {
    $st = Get-MnStatus $cli
    # While the node is still starting, wait until the models are loaded and the speeds are measured.
    $expectChat = [bool]$present['chat']
    $expectEmbed = [bool]$present['search']
    $expectSpeech = ([bool]$present['speech'] -and -not $SkipSpeech.IsPresent)
    while ($st -and (Get-Date) -lt $settleUntil) {
        $settled = $true
        if ($expectChat -and ($null -eq $st.chat -or [double]$st.chat_tok_s -le 0)) { $settled = $false }
        if ($expectEmbed -and ($null -eq $st.embed -or [double]$st.embed_per_s -le 0)) { $settled = $false }
        if ($expectSpeech -and ($null -eq $st.speech -or [double]$st.speech_x -le 0)) { $settled = $false }
        if ($settled) { break }
        Start-Sleep -Seconds 3
        $again = Get-MnStatus $cli
        if ($again) { $st = $again }
    }
}
if (-not $answers) {
    Check-Skip 'status' 'needs the running node'
} elseif (-not $cli) {
    Check-Fail 'status' 'marginalia-node.exe was not found to ask the node' $setupCmd
} elseif (-not $st) {
    Check-Fail 'status' 'the node answers on the port but  status  gave no valid JSON' ('& "' + $cli + '" status')
} elseif ($st.paused) {
    Check-Fail 'status' 'the node is paused and turns every job away' ('& "' + $cli + '" resume')
} else {
    $mins = [int]([double]$st.uptime_s / 60)
    Check-Ok 'status' ('name ' + $st.name + ', version ' + $st.version + ', up ' + $mins + ' min, ' + $st.phones + ' phone(s) known, ' + $st.served + ' job(s) served')
}

# ---- 12 to 14 Loaded models and measured speed ---------------------------------------------------
if (-not $st) {
    Check-Skip 'chat loaded' 'needs status'
    Check-Skip 'search loaded' 'needs status'
    Check-Skip 'speech loaded' 'needs status'
} else {
    if ($null -ne $st.chat) {
        if ([double]$st.chat_tok_s -gt 0) { $speed = ('{0:N0} tokens a second' -f [double]$st.chat_tok_s) } else { $speed = 'speed not measured yet' }
        Check-Ok 'chat loaded' ([string]$st.chat + ', ' + $speed)
    } elseif ($present['chat']) {
        Check-Fail 'chat loaded' 'the chat model file is there but the node did not load it' $tailLog @('Common cause: not enough graphics memory. Try  run --cpu  or a smaller  --ctx.')
    } else {
        Check-Fail 'chat loaded' 'the chat model is not loaded because the file is missing' $dlFix
    }
    if ($null -ne $st.embed) {
        if ([double]$st.embed_per_s -gt 0) { $speed = ('{0:N0} passages a second' -f [double]$st.embed_per_s) } else { $speed = 'speed not measured yet' }
        Check-Ok 'search loaded' ([string]$st.embed + ', ' + $speed)
    } elseif ($present['search']) {
        Check-Fail 'search loaded' 'the search model file is there but the node did not load it' $tailLog
    } else {
        Check-Fail 'search loaded' 'the search model is not loaded because the file is missing' $dlFix
    }
    if ($SkipSpeech.IsPresent) {
        Check-Skip 'speech loaded' 'left out on purpose (-SkipSpeech)'
    } elseif ($null -ne $st.speech) {
        if ([double]$st.speech_x -gt 0) { $speed = ('{0:N0} times faster than real time' -f [double]$st.speech_x) } else { $speed = 'speed not measured yet' }
        Check-Ok 'speech loaded' ([string]$st.speech + ', ' + $speed)
    } elseif ($present['speech']) {
        Check-Fail 'speech loaded' 'the speech model file is there but the node did not load it (the build of whisper.cpp inside the node may be at fault)' $tailLog
    } else {
        Check-Fail 'speech loaded' 'the node cannot transcribe recordings because the speech model file is missing' $dlFix
    }
}

# ---- 15 Addresses --------------------------------------------------------------------------------
if (-not $st) {
    Check-Skip 'addresses' 'needs status'
} else {
    $addrs = @($st.addresses)
    if ($addrs.Count -gt 0 -and $addrs[0]) {
        Check-Ok 'addresses' ($addrs -join '  ')
    } else {
        Check-Fail 'addresses' 'the node found no network address a phone could use' 'ipconfig' @('Is this machine on Wi-Fi or Ethernet? The adapter filter is lan_addresses in compute-node\node\src\discovery.rs (WSL, Hyper-V, VMware and VirtualBox adapters are left out on purpose).')
    }
}

# ---- 16 mDNS -------------------------------------------------------------------------------------
if (-not $st) {
    Check-Skip 'mDNS announcement' 'needs status'
} else {
    $seen = $false
    if (Get-Command Resolve-DnsName -ErrorAction SilentlyContinue) {
        try {
            $rec = @(Resolve-DnsName -Name '_marginalia._udp.local' -Type PTR -DnsOnly -QuickTimeout -ErrorAction Stop | Where-Object { $_.Type -eq 'PTR' })
            if ($rec.Count -gt 0) { $seen = $true }
        } catch { }
    }
    if ($seen) {
        Check-Ok 'mDNS announcement' '_marginalia._udp is visible on this network'
    } else {
        Check-Note 'mDNS announcement' 'could not see it from this machine, which proves little (Windows often cannot browse it). Check from the phone: Settings, Compute nodes. If the phone does not list this node, see the mDNS row in docs\WINDOWS-PLAN.md.'
    }
}

# ---- 17 to 20 Self test: act like a phone --------------------------------------------------------
$probeDir = Join-Path $env:TEMP 'marginalia-probe'
$probeBase = @('--state-dir', $probeDir, 'probe', ('127.0.0.1:' + $MN_NodePort))
$probeShown = '& "' + $cli + '" --state-dir "' + $probeDir + '" probe 127.0.0.1:' + $MN_NodePort + ' --knock'
if ($SkipProbe.IsPresent) {
    Check-Skip 'self test: let in' 'left out (-SkipProbe)'
    Check-Skip 'self test: search' 'left out (-SkipProbe)'
    Check-Skip 'self test: chat' 'left out (-SkipProbe)'
    Check-Skip 'self test: speech' 'left out (-SkipProbe)'
} elseif (-not $st) {
    Check-Skip 'self test: let in' 'needs the running node'
    Check-Skip 'self test: search' 'needs the running node'
    Check-Skip 'self test: chat' 'needs the running node'
    Check-Skip 'self test: speech' 'needs the running node'
} else {
    Step-Info 'Self test: connecting to the node like a phone does (a few seconds, the chat answer takes the longest).'
    $r = Invoke-MnCapture $cli ($probeBase + @('--knock')) 180
    $text = $r.Out + "`n" + $r.Err
    $knocked = $false
    if ($text -match 'let in by ([^\r\n]+)') {
        $knocked = $true
        Check-Ok 'self test: let in' ('the node let the test phone in (' + $Matches[1].Trim() + ')')
    } elseif ($text -match 'waiting for') {
        Check-Fail 'self test: let in' 'the node is locked and waits for someone to allow the test phone' ('& "' + $cli + '" approve') @('Then run verify.ps1 again. Or unlock: marginalia-node.exe unlock')
    } elseif ($r.TimedOut) {
        Check-Fail 'self test: let in' 'no answer within 3 minutes' $probeShown @('Output so far: ' + ((Get-LastLines $text 2) -join ' | '))
    } else {
        Check-Fail 'self test: let in' ('the test phone could not connect: ' + ((Get-LastLines $r.Err 1) -join '')) $probeShown @('Same machine, loopback. If this fails the QUIC listener or the certificate pinning is at fault, not the network.')
    }

    if (-not $knocked) {
        Check-Skip 'self test: search' 'needs the test phone to be let in'
        Check-Skip 'self test: chat' 'needs the test phone to be let in'
    } else {
        if ($null -eq $st.embed) {
            Check-Skip 'self test: search' 'the search model is not loaded (see 13)'
        } elseif ($text -match 'embedded (\d+) texts, dim (\d+), norm ([\d.]+), in ([\d.]+)s') {
            Check-Ok 'self test: search' ($Matches[1] + ' passages embedded in ' + $Matches[4] + ' s, ' + $Matches[2] + ' numbers each')
        } else {
            Check-Fail 'self test: search' ('no embedding came back: ' + ((Get-LastLines $r.Err 1) -join '')) $probeShown
        }
        if ($null -eq $st.chat) {
            Check-Skip 'self test: chat' 'the chat model is not loaded (see 12)'
        } elseif ($text -match 'chat: first token ([\d.]+)s, (\d+) tokens \((\d+) prompt\) in ([\d.]+)s') {
            Check-Ok 'self test: chat' ('first token after ' + $Matches[1] + ' s, ' + $Matches[2] + ' tokens in ' + $Matches[4] + ' s')
        } else {
            Check-Fail 'self test: chat' ('no chat answer came back: ' + ((Get-LastLines $r.Err 1) -join '')) $probeShown
        }
    }

    if (-not $knocked) {
        Check-Skip 'self test: speech' 'needs the test phone to be let in'
    } elseif ($SkipSpeech.IsPresent -or $null -eq $st.speech) {
        Check-Skip 'self test: speech' 'the speech model is not loaded (see 14)'
    } else {
        $wav = Join-Path $probeDir 'silence.wav'
        New-SilentWav $wav 3
        $tr = Invoke-MnCapture $cli ($probeBase + @('--transcribe', $wav)) 120
        $ttext = $tr.Out + "`n" + $tr.Err
        if ($ttext -match 'done: language (\S+), (\d+) segments, node worked ([\d.]+) s') {
            Check-Ok 'self test: speech' ('3 seconds of silence went through, node worked ' + $Matches[3] + ' s, ' + $Matches[2] + ' segment(s)')
        } else {
            $shown = '& "' + $cli + '" --state-dir "' + $probeDir + '" probe 127.0.0.1:' + $MN_NodePort + ' --transcribe "' + $wav + '"'
            Check-Fail 'self test: speech' ('transcription did not finish: ' + ((Get-LastLines $ttext 1) -join '')) $shown
        }
    }
}

# ---- Summary -------------------------------------------------------------------------------------
Write-Host ''
if ($script:failed -eq 0) {
    Write-Host ('{0} of {1} checks passed' -f $script:passed, $script:counted) -ForegroundColor Green
    Write-Host 'Nothing to fix.'
    Write-Host ''
    exit 0
}
Write-Host ('{0} of {1} checks passed' -f $script:passed, $script:counted) -ForegroundColor Red
Write-Host ('Start with check ' + $script:firstLabel + '. Run:')
Write-Host ('    ' + $script:firstFix)
Write-Host ''
exit 1
