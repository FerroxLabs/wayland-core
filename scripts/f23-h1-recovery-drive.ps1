# 23B-H1 read-side recovery — Windows port of scripts/f23-h1-recovery-drive.sh.
#
# Same contract, same two modes, same nonce-bound terminal marker. See the bash
# script for why both halves have to run: without `-Expect unreadable` against a
# binary lacking the recovery, the readable half proves nothing.
#
# Exit-code discipline, carried from scripts/wayland-e2e-windows-soak.ps1:
# $LASTEXITCODE is read on the line AFTER the invocation and never as the
# trailing value of a `& { ... }` block — that array-filter bug reported a fully
# passing run as a failure once already. The script always ends with an explicit
# `exit`.
#
# Targets Windows PowerShell 5.1: no ternary, no null-coalescing.
param(
    [Parameter(Mandatory = $true)][string]$Binary,
    [Parameter(Mandatory = $true)][string]$Sha,
    [Parameter(Mandatory = $true)][string]$Nonce,
    [Parameter(Mandatory = $true)][ValidateSet('readable', 'unreadable')][string]$Expect
)

$ErrorActionPreference = 'Continue'

if (-not (Test-Path -LiteralPath $Binary)) {
    Write-Host "binary is missing: $Binary"
    exit 2
}
$here = Split-Path -Parent $MyInvocation.MyCommand.Path
$gen = Join-Path $here 'f23-h1-legacy-journal.py'
if (-not (Test-Path -LiteralPath $gen)) {
    Write-Host "fixture generator is missing: $gen"
    exit 2
}

# Provenance FIRST. A binary that does not report the commit under test cannot
# prove anything about it.
$buildInfo = & $Binary --build-info 2>&1 | Out-String
$rc = $LASTEXITCODE
if ($rc -ne 0) {
    Write-Host "--build-info failed ($rc): $buildInfo"
    exit 3
}
if ($buildInfo -notmatch [regex]::Escape($Sha)) {
    Write-Host "binary reports '$($buildInfo.Trim())' which does not carry -Sha $Sha"
    exit 3
}
Write-Host "provenance: $($buildInfo.Trim())"

$work = Join-Path ([System.IO.Path]::GetTempPath()) ("f23h1-" + $Nonce)
New-Item -ItemType Directory -Force -Path $work | Out-Null
$session = "s-$Nonce"
$journal = Join-Path $work "$session.journal"

$failed = 0
function Fail([string]$why) {
    Write-Host "FAIL: $why"
    $script:failed = 1
}

try {
    & python3 $gen --out $journal --session-id $session --nonce $Nonce
    $rc = $LASTEXITCODE
    if ($rc -ne 0) {
        Write-Host "fixture generator failed ($rc)"
        exit 4
    }
    $bytes = [System.IO.File]::ReadAllBytes($journal)
    $text = [System.Text.Encoding]::UTF8.GetString($bytes)
    if ($text -notmatch '"effect_receipt":null') {
        Write-Host 'fixture does not carry the explicit null it exists to carry'
        exit 4
    }

    function Invoke-Verb([string]$verb) {
        $out = & $Binary session --dir $work $verb $session 2>&1 | Out-String
        $code = $LASTEXITCODE
        Write-Host "--- session $verb $session (exit $code)"
        Write-Host $out.TrimEnd()
        # A native command's STDERR comes back as an ErrorRecord, and PowerShell
        # WORD-WRAPS that at the console width before Out-String sees it. The
        # product's message is one line; the transcript's copy of it is not.
        # Matching on the wrapped text is matching on the console width, so
        # collapse every whitespace run to one space and match on that. Same
        # assertion, made independent of the terminal — measured: a first
        # Windows run reddened on `outstanding\nreconcile item(s)` while
        # reconcile and cancel behaved exactly as they did on Linux and macOS.
        $flat = ($out -replace '\s+', ' ')
        return @($code, $flat)
    }

    if ($Expect -eq 'unreadable') {
        foreach ($verb in @('reconcile', 'cancel')) {
            $r = Invoke-Verb $verb
            if ($r[0] -eq 0) { Fail "$verb exited 0 on a pre-fix journal" }
            if ($r[1] -notmatch 'journal checksum mismatch') {
                Fail "expected the 23B-01 error from $verb"
            }
        }
    }
    else {
        $r = Invoke-Verb 'reconcile'
        if ($r[0] -ne 0) { Fail "reconcile exited $($r[0]) on a journal the recovery must read" }
        if ($r[1] -match 'journal checksum mismatch') { Fail 'reconcile still reports the 23B-01 error' }
        if ($r[1] -notmatch [regex]::Escape("F23_SESSION=reconcile id=$session outstanding=")) {
            Fail "reconcile did not surface the recovered session id $session"
        }
        if ($r[1] -notmatch 'ref=x1 tool=Write turn=t1') {
            Fail 'reconcile did not surface the journal''s tool execution, tool and turn'
        }

        # `cancel` reaches the reducer, which refuses while a tool execution is
        # outstanding — exit 5 in session_cmd's documented map. That refusal is
        # itself proof the journal was READ.
        $r = Invoke-Verb 'cancel'
        if ($r[0] -ne 5) { Fail "cancel exited $($r[0]), expected the documented 5 (outstanding reconcile)" }
        if ($r[1] -match 'could not be read') { Fail 'cancel still cannot read the journal' }
        if ($r[1] -notmatch 'outstanding reconcile item') {
            Fail 'cancel did not report the outstanding item it read from the journal'
        }

        $r = Invoke-Verb 'reconcile'
        if ($r[0] -ne 0) { Fail 'the journal stopped being readable on a second pass' }
    }
}
finally {
    Remove-Item -Recurse -Force -LiteralPath $work -ErrorAction SilentlyContinue
}

if ($failed -ne 0) {
    Write-Host "F23_H1_DRIVE=FAIL platform=windows mode=$Expect nonce=$Nonce"
    exit 1
}
Write-Host "F23_H1_DRIVE=PASS platform=windows mode=$Expect nonce=$Nonce"
exit 0
