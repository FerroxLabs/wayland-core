# F23-05 Task 2 — the Windows port of the multi-day journey driver.
#
# Same contract as scripts/f23-multi-day-journey.sh. Invoked ONCE to start the
# journey and ONCE per subsequent calendar day to resume it. Between days the
# process genuinely does not exist.
#
# EXIT-CODE DISCIPLINE — this is the whole reason the port is separate rather
# than a `bash` shim, and it closes a defect this repository actually shipped:
#
#   * `$LASTEXITCODE` is read on the line AFTER the pipeline, never as the
#     trailing value of a `$x = & { … ; $LASTEXITCODE }` block. Such a block
#     returns an ARRAY of every output line PLUS the code, so `if ($x -ne 0)` is
#     an array FILTER whose non-empty result is always truthy. That bug reported
#     an all-PASS 12/12 + 6/6 soak as a failure; the post-mortem lives in
#     scripts/wayland-e2e-windows-soak.ps1:174-190 and :244-255.
#   * The script ALWAYS ends in an explicit `exit`, so an ssh caller's
#     `exit $LASTEXITCODE` carries a real status.
#   * `powershell -File <missing.ps1>` exits 0. The caller therefore asserts a
#     nonce-bound marker as a second, independent check rather than trusting the
#     status alone.

[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)][string]$Binary,
    [Parameter(Mandatory = $true)][string]$Sha,
    [Parameter(Mandatory = $true)][string]$Nonce,
    [string]$Harness = '',
    [string]$Root = '',
    [int]$SpanSeconds = 0,
    [int]$Day = 0,
    [switch]$Verify
)

$ErrorActionPreference = 'Continue'
$Platform = 'windows'
$RepoRoot = Split-Path -Parent (Split-Path -Parent $PSCommandPath)

function Fail([string]$message, [int]$code) {
    Write-Error $message
    exit $code
}

if (-not (Test-Path -LiteralPath $Binary)) {
    Fail "FATAL: $Binary is not a file" 65
}
$Binary = (Resolve-Path -LiteralPath $Binary).Path

# ── Provenance, asserted on EVERY invocation ────────────────────────────────
$buildInfo = & $Binary --build-info 2>&1 | Out-String
$provenanceExit = $LASTEXITCODE
if ($provenanceExit -ne 0) {
    Fail "FATAL: --build-info exited $provenanceExit`n$buildInfo" 67
}
$match = [regex]::Match($buildInfo, '\(source ([0-9a-f]+)\)')
$binSha = if ($match.Success) { $match.Groups[1].Value } else { '' }
if ($binSha -ne $Sha) {
    Fail "FATAL: binary source SHA '$binSha' != commit under test '$Sha'" 68
}
Write-Output "F23_04_PROVENANCE=ok platform=$Platform sha=$Sha"

if ([string]::IsNullOrWhiteSpace($Root)) {
    $Root = Join-Path $env:USERPROFILE ".f23-journey-$Platform"
}
$RunLog = Join-Path $Root 'runlog.txt'
$Decision = Join-Path $RepoRoot '.planning\phases\23B-continuous-agency\23B-04-CLOCK-DECISION.md'

# ── The authorized real-span threshold for THIS platform ────────────────────
$requiredSpan = $null
if (Test-Path -LiteralPath $Decision) {
    $lines = Select-String -LiteralPath $Decision -Pattern "^${Platform}_required_real_span_seconds=(\d+)$" -AllMatches
    if ($lines) {
        $requiredSpan = [int]$lines[-1].Matches[0].Groups[1].Value
    }
}
if ($SpanSeconds -le 0) {
    if ($null -eq $requiredSpan) {
        Fail "FATAL: no -SpanSeconds and no ${Platform}_required_real_span_seconds= in $Decision" 70
    }
    $SpanSeconds = $requiredSpan
}

# ── Resolve the harness ─────────────────────────────────────────────────────
if ([string]::IsNullOrWhiteSpace($Harness)) {
    Push-Location $RepoRoot
    $json = cargo test -p wcore-agent --test multi_day_journey_test --no-run --message-format=json 2>$null
    $cargoExit = $LASTEXITCODE
    Pop-Location
    if ($cargoExit -ne 0) {
        Fail "FATAL: could not build the journey harness (cargo exited $cargoExit)" 69
    }
    $candidates = @()
    foreach ($line in $json) {
        $m = [regex]::Match([string]$line, '"executable":"([^"]*multi_day_journey_test[^"]*)"')
        if ($m.Success) { $candidates += $m.Groups[1].Value.Replace('\\', '\') }
    }
    if ($candidates.Count -gt 0) { $Harness = $candidates[-1] }
}
if ([string]::IsNullOrWhiteSpace($Harness) -or -not (Test-Path -LiteralPath $Harness)) {
    Fail "FATAL: journey harness '$Harness' is not executable" 69
}

New-Item -ItemType Directory -Force -Path $Root | Out-Null
if (-not (Test-Path -LiteralPath $RunLog)) { New-Item -ItemType File -Path $RunLog | Out-Null }

function Invoke-Step([int]$StepDay, [string]$Mode) {
    $env:F23_JOURNEY_ROOT = $Root
    $env:F23_JOURNEY_DAY = "$StepDay"
    $env:F23_JOURNEY_MODE = $Mode
    $env:F23_JOURNEY_NONCE = $Nonce
    $env:F23_JOURNEY_SPAN_SECONDS = "$SpanSeconds"
    $env:F23_JOURNEY_PLATFORM = $Platform
    $env:F23_JOURNEY_HOST = $env:COMPUTERNAME
    # Capture on the NEXT line. Never `$out = & { … ; $LASTEXITCODE }`.
    $out = & $Harness --exact f23_journey_step --nocapture --test-threads=1 2>&1 | Out-String
    $script:StepExit = $LASTEXITCODE
    return $out
}

if ($Verify) {
    if (-not (Test-Path -LiteralPath $RunLog)) {
        Fail 'FATAL: the run log carries no day records; the journey did not run' 71
    }
    $logLines = Get-Content -LiteralPath $RunLog
    $dayRows = @($logLines | Where-Object { $_ -match '^F23_04_DAY=\d+ platform=\S+ ts=\S+' })
    if ($dayRows.Count -lt 1) {
        Fail 'FATAL: the run log carries no day records; the journey did not run' 71
    }
    foreach ($line in $logLines) {
        if ($line -match '^F23_04_(DAY|INVARIANT|LOOP_OWNERS_OBSERVED|GOAL_LIFECYCLE|JOURNAL_CURSOR|WAIT_)') {
            Write-Output $line
        }
    }

    $firstTs = [regex]::Match($dayRows[0], 'ts=(\S+)').Groups[1].Value
    $lastTs = [regex]::Match($dayRows[-1], 'ts=(\S+)').Groups[1].Value
    $firstEpoch = [DateTimeOffset]::Parse($firstTs, [Globalization.CultureInfo]::InvariantCulture,
        [Globalization.DateTimeStyles]::AssumeUniversal -bor [Globalization.DateTimeStyles]::AdjustToUniversal).ToUnixTimeSeconds()
    $lastEpoch = [DateTimeOffset]::Parse($lastTs, [Globalization.CultureInfo]::InvariantCulture,
        [Globalization.DateTimeStyles]::AssumeUniversal -bor [Globalization.DateTimeStyles]::AdjustToUniversal).ToUnixTimeSeconds()
    $span = $lastEpoch - $firstEpoch

    Write-Output "F23_04_SPAN_FIRST_TS=$firstTs"
    Write-Output "F23_04_SPAN_LAST_TS=$lastTs"
    Write-Output "F23_04_SPAN_SECONDS=$span"

    if ($null -eq $requiredSpan) {
        Write-Output 'F23_04_SPAN_MEETS_AUTHORIZED_POLICY=false'
        Fail "FATAL: no authorized ${Platform}_required_real_span_seconds= to measure against" 72
    }
    Write-Output "F23_04_SPAN_REQUIRED_SECONDS=$requiredSpan"
    if ($span -lt $requiredSpan) {
        Write-Output 'F23_04_SPAN_MEETS_AUTHORIZED_POLICY=false'
        Fail "FATAL: recomputed span ${span}s is short of the authorized ${requiredSpan}s; the journey did not run and must be re-run rather than re-described" 72
    }
    Write-Output 'F23_04_SPAN_MEETS_AUTHORIZED_POLICY=true'

    $out = Invoke-Step 0 'verify'
    Write-Output $out
    if ($script:StepExit -ne 0) {
        Fail "FATAL: the live verify step exited $($script:StepExit)" 73
    }

    Write-Output "F23_04_JOURNEY=PASS platform=$Platform nonce=$Nonce"
    exit 0
}

if ($Day -le 0) {
    Fail 'FATAL: -Day <n> or -Verify is required' 64
}

# Idempotent per day: a second invocation on the same day must not double-count.
$already = Select-String -LiteralPath $RunLog -Pattern "^F23_04_DAY=$Day platform=$Platform " -SimpleMatch:$false
if ($already) {
    Write-Output "F23_04_DAY_ALREADY_RECORDED=$Day platform=$Platform"
    exit 0
}

$out = Invoke-Step $Day 'day'
$stepExit = $script:StepExit
$stamp = (Get-Date).ToUniversalTime().ToString('yyyy-MM-ddTHH:mm:ssZ')
Add-Content -LiteralPath $RunLog -Value "# ---- invocation day=$Day platform=$Platform ts=$stamp host=$($env:COMPUTERNAME) pid=$PID sha=$Sha rc=$stepExit"
Add-Content -LiteralPath $RunLog -Value $out

Write-Output $out
if ($stepExit -ne 0) {
    Fail "FATAL: journey day $Day exited $stepExit on $Platform" $stepExit
}
Write-Output "F23_04_JOURNEY_DAY_RECORDED=$Day platform=$Platform nonce=$Nonce"
exit 0
