# Downloads the model files the node serves (about 5.5 GB: chat Gemma 4 E4B, search and speech to text). -ChatModel e2b picks the smaller chat model. Safe to stop and run again, it continues.
#   .\compute-node\windows\download-models.ps1
#   .\compute-node\windows\download-models.ps1 -Only search      (or chat, or speech)
#   .\compute-node\windows\download-models.ps1 -SkipSpeech        leave out the speech model (570 MB)
#   .\compute-node\windows\download-models.ps1 -Dir D:\models
# With a built node it simply runs  marginalia-node.exe models download.  Without one (before the first
# build) it downloads the same files itself. If Hugging Face asks for a login, set $env:HF_TOKEN first.
param(
    [string]$Dir = '',
    [ValidateSet('', 'chat', 'search', 'speech')][string]$Only = '',
    [ValidateSet('e2b', 'e4b')][string]$ChatModel = 'e4b',
    [switch]$SkipSpeech
)
. (Join-Path $PSScriptRoot 'common.ps1')
Update-SessionPath

$exe = Find-NodeExe
if ($exe) {
    $a = @('models', 'download')
    if ($Dir) { $a += @('--dir', $Dir) }
    if ($Only) { $a += @('--only', $Only) }
    $a += @('--chat-model', $ChatModel)
    if ($SkipSpeech.IsPresent) { $a += '--skip-speech' }
    Step-Info ('Using ' + $exe)
    & $exe @a
    exit $LASTEXITCODE
}

Step-Info 'marginalia-node.exe is not built yet, downloading the files directly.'
if (-not $Dir) { $Dir = $MN_DefaultModelsDir }
[Net.ServicePointManager]::SecurityProtocol = [Net.SecurityProtocolType]::Tls12
New-Item -ItemType Directory -Force -Path $Dir | Out-Null

# Same files and repositories as compute-node/node/src/models_dl.rs
$all = @(
    @{ Role = 'chat';   Repo = 'ggml-org/gemma-4-E2B-it-GGUF';   File = 'gemma-4-E2B-it-Q4_0.gguf';   Size = 2841481184 },
    @{ Role = 'search'; Repo = 'ggml-org/embeddinggemma-2-GGUF'; File = 'embeddinggemma-2-Q8_0.gguf'; Size = 309855456 },
    @{ Role = 'speech'; Repo = 'ggerganov/whisper.cpp';          File = 'ggml-large-v3-turbo-q5_0.bin';  Size = 574041195 }
)
if ($ChatModel -eq 'e4b') { $all[0] = @{ Role = 'chat'; Repo = 'ggml-org/gemma-4-E4B-it-GGUF'; File = 'gemma-4-E4B-it-Q4_0.gguf'; Size = 4590807392 } }
$wanted = @($all | Where-Object { ($Only -and $_.Role -eq $Only) -or ((-not $Only) -and -not ($SkipSpeech.IsPresent -and $_.Role -eq 'speech')) })

$token = $env:HF_TOKEN
if ($token) { $token = $token.Trim() }

function Format-Eta([double]$s) {
    if ($s -ge 3600) { return ('{0}h {1:00}m' -f [int][math]::Floor($s / 3600), [int][math]::Floor(($s % 3600) / 60)) }
    if ($s -ge 60) { return ('{0}m {1:00}s' -f [int][math]::Floor($s / 60), [int]($s % 60)) }
    return ('{0}s' -f [int]$s)
}

# One try. Returns normally when the file is complete, throws otherwise. A message starting with FATAL: is not retried.
function Get-OnePass([string]$repo, [string]$file, [long]$expected, [string]$final, [string]$part) {
    $have = 0
    if (Test-Path $part) { $have = (Get-Item $part).Length }
    if ($expected -gt 0 -and $have -eq $expected) { Move-Item -Force $part $final; return }
    if ($expected -gt 0 -and $have -gt $expected) { Remove-Item -Force $part; $have = 0 }

    $url = 'https://huggingface.co/' + $repo + '/resolve/main/' + $file
    $resp = $null
    for ($hop = 0; $hop -lt 6; $hop++) {
        $req = [Net.HttpWebRequest]::Create($url)
        $req.AllowAutoRedirect = $false
        $req.UserAgent = 'marginalia-node-setup'
        $req.Timeout = 60000
        $req.ReadWriteTimeout = 60000
        # The token goes to huggingface.co only, never to the download servers it redirects to.
        if ($token -and ([Uri]$url).Host -eq 'huggingface.co') { $req.Headers.Add('Authorization', 'Bearer ' + $token) }
        $req.Headers.Add('Accept-Encoding', 'identity')
        if ($have -gt 0) { $req.AddRange([long]$have) }
        try {
            $resp = $req.GetResponse()
        } catch [Net.WebException] {
            $resp = $_.Exception.Response
            if (-not $resp) { throw 'Cannot reach huggingface.co. Check your internet connection, proxy, VPN or firewall.' }
        }
        $code = [int]$resp.StatusCode
        if ($code -ge 300 -and $code -lt 400) {
            $loc = $resp.Headers['Location']
            $resp.Close()
            $url = (New-Object Uri((New-Object Uri($url)), $loc)).AbsoluteUri
            $resp = $null
            continue
        }
        break
    }
    if (-not $resp) { throw 'Too many redirects.' }
    $code = [int]$resp.StatusCode
    if ($code -eq 401 -or $code -eq 403) {
        $resp.Close()
        throw ('FATAL: Hugging Face refused access (HTTP ' + $code + '). Create a read token at https://huggingface.co/settings/tokens, accept the licence on the model page, then set $env:HF_TOKEN = "hf_..." and run this again.')
    }
    if ($code -eq 404) { $resp.Close(); throw ('FATAL: Not found (HTTP 404): ' + $repo + '/' + $file) }
    if ($code -eq 416) { $resp.Close(); Remove-Item -Force $part -ErrorAction SilentlyContinue; throw 'The earlier partial file could not be continued, starting it again.' }
    if ($code -ne 200 -and $code -ne 206) { $resp.Close(); throw ('Hugging Face answered HTTP ' + $code + '.') }

    $append = ($code -eq 206)
    $total = $resp.ContentLength
    if ($total -gt 0 -and $append) { $total = $total + $have }
    if (-not $append) { $have = 0 }
    $mode = [IO.FileMode]::Create
    if ($append) { $mode = [IO.FileMode]::Append }
    $out = New-Object IO.FileStream($part, $mode, [IO.FileAccess]::Write)
    $inp = $resp.GetResponseStream()
    $buf = New-Object byte[] 262144
    $done = $have
    $clock = [Diagnostics.Stopwatch]::StartNew()
    $lastShow = 0.0
    $speed = 0.0
    $lastT = 0.0
    $lastB = $done
    try {
        while ($true) {
            $n = $inp.Read($buf, 0, $buf.Length)
            if ($n -le 0) { break }
            $out.Write($buf, 0, $n)
            $done += $n
            $t = $clock.Elapsed.TotalSeconds
            if ($t - $lastT -ge 0.5) {
                $inst = ($done - $lastB) / ($t - $lastT)
                if ($speed -eq 0) { $speed = $inst } else { $speed = $speed * 0.7 + $inst * 0.3 }
                $lastT = $t
                $lastB = $done
            }
            if ($t - $lastShow -ge 0.3) {
                $lastShow = $t
                $pct = 0
                $eta = '--'
                if ($total -gt 0) {
                    $pct = [int]($done * 100 / $total)
                    if ($speed -gt 1) { $eta = Format-Eta (($total - $done) / $speed) }
                }
                $line = ('  {0}  {1:N1} / {2:N1} MB  {3,3}%  {4:N1} MB/s  ETA {5}' -f $file, ($done / 1e6), ($total / 1e6), $pct, ($speed / 1e6), $eta)
                Write-Host ("`r" + $line + '   ') -NoNewline
            }
        }
    } finally {
        $out.Flush()
        $out.Close()
        $inp.Close()
        $resp.Close()
        Write-Host ''
    }
    if ($total -gt 0 -and $done -lt $total) { throw ('The connection closed early at ' + [int]($done / 1e6) + ' of ' + [int]($total / 1e6) + ' MB.') }
    if ($total -gt 0 -and $done -gt $total) { Remove-Item -Force $part; throw 'The file came out larger than announced, starting it again.' }
    Move-Item -Force $part $final
}

$failed = @()
foreach ($m in $wanted) {
    $final = Join-Path $Dir $m.File
    $part = $final + '.part'
    if ((Test-Path $final) -and ((Get-Item $final).Length -gt 0) -and ($m.Size -eq 0 -or (Get-Item $final).Length -eq $m.Size)) {
        Step-Skip ($m.File + ' is already complete')
        continue
    }
    Step-Info ('Downloading ' + $m.File + ' into ' + $Dir)
    $tries = 0
    $okFile = $false
    while (-not $okFile) {
        $before = 0
        if (Test-Path $part) { $before = (Get-Item $part).Length }
        try {
            Get-OnePass $m.Repo $m.File ([long]$m.Size) $final $part
            $okFile = $true
        } catch {
            $msg = $_.Exception.Message
            if ($msg.StartsWith('FATAL:')) { Step-Fail ($m.File + ': ' + $msg.Substring(6).Trim()); break }
            $after = 0
            if (Test-Path $part) { $after = (Get-Item $part).Length }
            if ($after -gt $before) { $tries = 0 }
            $tries++
            if ($tries -gt 8) { Step-Fail ($m.File + ': gave up (' + $msg + ') Run this again to continue.'); break }
            $wait = [math]::Min(60, [math]::Pow(2, [math]::Min($tries, 6)))
            Step-Warn ($m.File + ': ' + $msg + ' Trying again in ' + $wait + ' seconds (' + $tries + ' of 8).')
            Start-Sleep -Seconds $wait
        }
    }
    if ($okFile) { Step-Ok ($m.File + ' is complete') } else { $failed += $m.File }
}
if ($failed.Count -gt 0) {
    Step-Fail ('Not downloaded: ' + ($failed -join ', ') + '. Run this script again to continue where it stopped.')
    exit 1
}
Step-Ok ('Models are in ' + $Dir)
exit 0
