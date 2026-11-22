<#
.SYNOPSIS
Download or update Binance spot candles as CSV files in data\, for volcandle.

.DESCRIPTION
PowerShell version of tools/binance_update.sh. Both scripts write the same files, so
either one can update the files the other made.

Existing files only get the candles after their last row. Only finished candles are
written; the one still forming is left for the next run. volcandle reloads an open
file by itself. Uses Binance's public market-data API (no account or key needed).

Intervals: 1s 1m 3m 5m 15m 30m 1h 2h 4h 6h 8h 12h 1d 3d 1w
Files go to the data folder next to tools\, or to $env:VOLCANDLE_DATA if it is set.

.PARAMETER Since
New files start at this date (UTC), like 2025-01-01. Default: Binance's first trade.
Existing files always continue after their last row.

.PARAMETER Target
Symbols and intervals, in any order. With no interval, 1h and 1d are used.
With nothing at all, every Binance file in data\ is updated.

.EXAMPLE
tools\binance_update.ps1
Update every Binance file in data\.

.EXAMPLE
tools\binance_update.ps1 ETHUSDT 1h 1d
Download or update ETHUSDT_1h.csv and ETHUSDT_1d.csv.

.EXAMPLE
tools\binance_update.ps1 BTCUSDT ETHUSDT 4h
Several symbols at once.

.EXAMPLE
tools\binance_update.ps1 -Since 2025-01-01 SOLUSDT 15m
A new file that starts at 2025-01-01 instead of the first trade.
#>
[CmdletBinding(PositionalBinding = $false)]
param(
    [string]$Since,
    [switch]$Help,
    [Parameter(ValueFromRemainingArguments = $true)]
    [string[]]$Target
)

Set-StrictMode -Version 2.0
$ErrorActionPreference = 'Stop'
# Windows PowerShell's download progress bar makes Invoke-WebRequest many times slower.
$ProgressPreference = 'SilentlyContinue'
# Binance needs TLS 1.2, which Windows PowerShell 5.1 does not always offer by default.
[Net.ServicePointManager]::SecurityProtocol =
    [Net.ServicePointManager]::SecurityProtocol -bor [Net.SecurityProtocolType]::Tls12

$Api = 'https://data-api.binance.vision/api/v3/klines'
$Header = 'Open time,Open,High,Low,Close,Volume,Close time,Quote asset volume,Number of trades,Taker buy base asset volume,Taker buy quote asset volume,Ignore'
$Intervals = '1s 1m 3m 5m 15m 30m 1h 2h 4h 6h 8h 12h 1d 3d 1w' -split ' '
$Utf8 = New-Object System.Text.UTF8Encoding $false   # no byte-order mark, like the bash script

if ($env:VOLCANDLE_DATA) { $DataDir = $env:VOLCANDLE_DATA }
else { $DataDir = Join-Path (Split-Path $PSScriptRoot) 'data' }
# .NET file methods resolve relative paths against the process folder, not PowerShell's
# current location, so make the path absolute once.
$DataDir = $ExecutionContext.SessionState.Path.GetUnresolvedProviderPathFromPSPath($DataDir)

function Stop-WithError([string]$Message) {
    $Host.UI.WriteErrorLine("binance_update: $Message")
    exit 2
}

# Unix milliseconds -> '2024-03-01 13:00' (UTC).
function Format-Utc([long]$Ms) {
    [DateTimeOffset]::FromUnixTimeMilliseconds($Ms).ToString(
        'yyyy-MM-dd HH:mm', [Globalization.CultureInfo]::InvariantCulture)
}

# GET a URL and return the body, or warn and return $null. Network errors, timeouts,
# 408, 429 and 5xx are retried up to 3 times, 1, 2 and 4 s apart (like curl --retry 3).
function Get-Body([string]$Url) {
    for ($attempt = 0; ; $attempt++) {
        try {
            return (Invoke-WebRequest -Uri $Url -UseBasicParsing -TimeoutSec 30).Content
        } catch {
            $code = 0
            $ex = $_.Exception
            if ($ex.PSObject.Properties['Response'] -and $ex.Response) {
                $code = [int]$ex.Response.StatusCode
            }
            if ($attempt -lt 3 -and ($code -eq 0 -or $code -eq 408 -or $code -eq 429 -or $code -ge 500)) {
                Start-Sleep -Seconds ([int][math]::Pow(2, $attempt))
                continue
            }
            if ($code -ne 0) {
                $text = ''
                if ($_.ErrorDetails) { $text = $_.ErrorDetails.Message }
                Write-Warning "Binance answered HTTP ${code}: $text"
            } else {
                Write-Warning $ex.Message
            }
            return $null
        }
    }
}

# Create or extend data\SYMBOL_INTERVAL.csv. Returns $true on success.
function Update-Candles([string]$Symbol, [string]$Interval) {
    $name = "${Symbol}_$Interval.csv"
    $file = Join-Path $DataDir $name
    $start = $SinceMs
    $last = [long]0
    $exists = Test-Path -LiteralPath $file -PathType Leaf
    if ($exists) {
        if ((Get-Content -LiteralPath $file -TotalCount 1) -cne $Header) {
            Write-Warning "${name}: skipped, it was not made by this script"
            return $false
        }
        $lastTime = ((Get-Content -LiteralPath $file -Tail 1) -split ',')[0]
        if ($lastTime -match '^[0-9]+$') {
            $last = [long]$lastTime
            $start = $last + 1
        }
    }

    $new = New-Object System.Collections.Generic.List[string]
    $now = [DateTimeOffset]::UtcNow.ToUnixTimeMilliseconds()
    while ($true) {
        $body = Get-Body "${Api}?symbol=$Symbol&interval=$Interval&startTime=$start&limit=1000"
        if ($null -eq $body) {
            Write-Warning "${name}: download failed, nothing written"
            return $false
        }
        # [[t,"o","h",...],[...]] -> one CSV line per candle
        $rows = @(foreach ($m in [regex]::Matches($body, '\[([^\[\]]+)\]')) {
                $m.Groups[1].Value.Replace('"', '')
            })
        if ($rows.Count -eq 0) { break }
        foreach ($row in $rows) {
            # Field 7 is the close time; a candle that closes later is still forming.
            if ([long]($row.Split(',')[6]) -lt $now) { $new.Add($row) }
        }
        $start = [long]($rows[-1].Split(',')[0]) + 1
        if ($rows.Count -lt 1000) { break }   # a short page is the last one
    }

    if ($new.Count -eq 0) {
        if ($last -gt 0) { Write-Host "${name}: up to date (last candle $(Format-Utc $last) UTC)" }
        else { Write-Host "${name}: no finished candles yet, nothing written" }
        return $true
    }

    # Write a complete new file, then swap it in, so volcandle never reads half a file.
    if ($exists) {
        $old = [IO.File]::ReadAllText($file).Replace("`r", '')
        if (-not $old.EndsWith("`n")) { $old += "`n" }
    } else {
        $old = "$Header`n"
    }
    $tmp = "$file.part"
    $writer = New-Object System.IO.StreamWriter($tmp, $false, $Utf8)
    try {
        $writer.Write($old)
        foreach ($row in $new) { $writer.Write("$row`n") }   # LF line endings, like bash
    } finally {
        $writer.Dispose()
    }
    if ($exists) { [IO.File]::Replace($tmp, $file, [NullString]::Value) }
    else { [IO.File]::Move($tmp, $file) }

    $first = [long]($new[0].Split(',')[0])
    $final = [long]($new[$new.Count - 1].Split(',')[0])
    $total = $old.Split("`n").Count - 2 + $new.Count   # lines minus the header
    $s = 's'
    if ($new.Count -eq 1) { $s = '' }
    Write-Host "${name}: +$($new.Count) candle$s ($(Format-Utc $first) .. $(Format-Utc $final) UTC), $total in file"
    return $true
}

if ($Help) {
    Get-Help $PSCommandPath -Detailed
    exit 0
}

$SinceMs = [long]0
if ($Since) {
    $date = [datetime]::MinValue
    $styles = [Globalization.DateTimeStyles]'AssumeUniversal, AdjustToUniversal'
    if (-not [datetime]::TryParse($Since, [Globalization.CultureInfo]::InvariantCulture, $styles, [ref]$date)) {
        Stop-WithError "can't read the date '$Since' (use YYYY-MM-DD)"
    }
    $SinceMs = ([DateTimeOffset]$date).ToUnixTimeMilliseconds()
}

$todo = New-Object System.Collections.Generic.List[object]
if (-not $Target) {
    # No arguments: every file in data\ that this script made.
    $files = Get-ChildItem -LiteralPath $DataDir -Filter '*_*.csv' -File -ErrorAction SilentlyContinue
    foreach ($f in $files) {
        if ($f.Extension -ne '.csv') { continue }   # -Filter '*.csv' also matches '.csvx'
        if ((Get-Content -LiteralPath $f.FullName -TotalCount 1) -cne $Header) { continue }
        $cut = $f.BaseName.LastIndexOf('_')
        $todo.Add(@($f.BaseName.Substring(0, $cut), $f.BaseName.Substring($cut + 1)))
    }
    if ($todo.Count -eq 0) {
        Write-Host "No Binance files in $DataDir yet. Download one with, for example:"
        Write-Host "  $PSCommandPath BTCUSDT 1h 1d"
        exit 0
    }
} else {
    $symbols = New-Object System.Collections.Generic.List[string]
    $ivs = New-Object System.Collections.Generic.List[string]
    foreach ($t in $Target) {
        if ($t.StartsWith('-')) {
            Stop-WithError "unknown option $t (options: -Since DATE, -Help)"
        } elseif ($t -ceq '1M') {
            Stop-WithError '1M (monthly) is not offered: on Windows its file name would clash with 1m. Download 1d and pick 1M in volcandle.'
        } elseif ($Intervals -contains $t) {   # -contains ignores case, so 1H works too
            $ivs.Add($t.ToLowerInvariant())
        } elseif ($t -match '^[0-9]+[a-z]+$') {
            Stop-WithError "unknown interval '$t'. Binance has: $($Intervals -join ' ')"
        } else {
            $symbols.Add(($t.ToUpperInvariant() -replace '[/_-]', ''))   # BTC/USDT, btc-usdt -> BTCUSDT
        }
    }
    if ($symbols.Count -eq 0) { Stop-WithError 'name a symbol, e.g. BTCUSDT' }
    if ($ivs.Count -eq 0) { $ivs.Add('1h'); $ivs.Add('1d') }
    foreach ($sym in $symbols) {
        foreach ($iv in $ivs) { $todo.Add(@($sym, $iv)) }
    }
}

[void](New-Item -ItemType Directory -Force -Path $DataDir)
$failed = 0
foreach ($job in $todo) {
    if (-not (Update-Candles $job[0] $job[1])) { $failed = 1 }
}
exit $failed
