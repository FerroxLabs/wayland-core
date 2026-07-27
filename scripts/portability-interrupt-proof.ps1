# portability-interrupt-proof.ps1 -- F26-03 Windows interruption and exact-rollback proof.
#
# Usage:
#   powershell -NoProfile -File scripts\portability-interrupt-proof.ps1 -Binary <exe> [-Undersized] [-HandlerControl] [-OpenHandle]
#
# This MIRRORS scripts/portability-interrupt-proof.sh step for step. A Windows
# leg that measured something else could not tell you whether Windows behaves
# differently -- it would only tell you two unrelated things passed. Same
# fixture shape, same target-that-carries-state, same mid-flight checks, same
# verdict grammar, and the same digest read from the product's own
# `backup digest` so both platforms compare identical algorithms by
# construction rather than by a copied string.
#
# Windows-specific legs Linux cannot warn about:
#   -OpenHandle : hold a target file open with another handle during restore.
#                 The sibling-tempfile-plus-rename that makes the write atomic
#                 on Linux behaves differently when Windows will not replace a
#                 file another handle holds. The assertion is NOT that the
#                 restore succeeds -- it is that it either succeeds or fails
#                 CLEANLY with an exact rollback, never leaving a half-state.
#   deep paths  : the fixture carries a deliberately deep/long relative path,
#                 because a restored tree reconstructs full paths under a new
#                 root and that is exactly where Windows path limits bite.

param(
    [Parameter(Mandatory = $true)][string]$Binary,
    [switch]$Undersized,
    [switch]$HandlerControl,
    [switch]$OpenHandle
)

$ErrorActionPreference = 'Continue'

function Fail($msg) {
    Write-Output "PROOF-FAIL: $msg"
    exit 1
}

if (-not (Test-Path -LiteralPath $Binary -PathType Leaf)) {
    Fail "binary does not exist: $Binary"
}
& $Binary backup --help *> $null
if ($LASTEXITCODE -ne 0) { Fail "binary does not support 'backup': $Binary" }

$Work = Join-Path $env:TEMP ("f26-int-" + [Guid]::NewGuid().ToString('N').Substring(0, 12))
New-Item -ItemType Directory -Path $Work -Force | Out-Null
$Src     = Join-Path $Work 'source-home'
$Target  = Join-Path $Work 'target-home'
$Archive = Join-Path $Work 'backup.tar.gz'
$Probe   = Join-Path $Work 'kill-handler-fired'

# --- fixture -----------------------------------------------------------------
if ($Undersized) { $Payloads = 2;   $PaceMs = 1 }
else             { $Payloads = 120; $PaceMs = 25 }
$KillAtMs = 900

# A deliberately deep and long relative path: Windows enforces limits Linux does
# not, and a restored tree is exactly where they bite.
$DeepRel = 'skills/' + (( 1..6 | ForEach-Object { 'deeply-nested-directory-segment-' + $_ } ) -join '/')
$DeepDir = Join-Path $Src ($DeepRel -replace '/', '\')

New-Item -ItemType Directory -Path (Join-Path $Src 'skills')  -Force | Out-Null
New-Item -ItemType Directory -Path (Join-Path $Src 'memory')  -Force | Out-Null
New-Item -ItemType Directory -Path $DeepDir -Force -ErrorAction SilentlyContinue | Out-Null
if (-not (Test-Path -LiteralPath $DeepDir)) {
    Write-Output "DEEP-PATH-CREATE: failed (source side) -- length $($DeepDir.Length)"
} else {
    Set-Content -LiteralPath (Join-Path $DeepDir 'deep-canary.md') -Value 'CANARY-DEEP-PAYLOAD' -NoNewline
}

Set-Content -LiteralPath (Join-Path $Src 'config.toml') -Value "[storage.credentials]`nbackend = `"plaintext`"`n" -NoNewline
for ($i = 0; $i -lt $Payloads; $i++) {
    Set-Content -LiteralPath (Join-Path $Src "skills\skill-$i.md") -Value "CANARY-PAYLOAD-$i" -NoNewline
}
Set-Content -LiteralPath (Join-Path $Src 'memory\notes.md') -Value 'CANARY-MEMORY' -NoNewline

& $Binary backup create --home $Src --out $Archive *> (Join-Path $Work 'create.log')
if ($LASTEXITCODE -ne 0) { Get-Content (Join-Path $Work 'create.log'); Fail 'archive creation failed' }

# --- a target that CARRIES STATE ---------------------------------------------
New-Item -ItemType Directory -Path (Join-Path $Target 'legacy') -Force | Out-Null
Set-Content -LiteralPath (Join-Path $Target 'config.toml') -Value 'PRE-EXISTING-DIVERGED-CONFIG' -NoNewline
Set-Content -LiteralPath (Join-Path $Target 'legacy\keepme.txt') -Value 'PRE-EXISTING-ONLY-HERE' -NoNewline
Set-Content -LiteralPath (Join-Path $Target 'untouched-by-archive.txt') -Value 'PRE-EXISTING-TOP-LEVEL' -NoNewline

# NOTE: the parameter is deliberately NOT called $home. $HOME is a read-only
# automatic variable in PowerShell, so a parameter of that name cannot bind and
# every call fails with "Cannot overwrite variable home because it is read-only
# or constant" -- which surfaces only at run time, on the box.
function Read-DigestField([string]$homePath, [string]$field) {
    $out = & $Binary backup digest --home $homePath 2>$null
    foreach ($line in $out) {
        if ($line -match "^$field`: (.+)$") { return $Matches[1] }
    }
    return ''
}

$DigestPre  = Read-DigestField $Target 'DIGEST'
$DigestAlgo = Read-DigestField $Target 'DIGEST-ALGO'
if ([string]::IsNullOrWhiteSpace($DigestPre))  { Fail 'could not take a pre-operation digest' }
if ([string]::IsNullOrWhiteSpace($DigestAlgo)) { Fail 'the binary did not report a digest algorithm' }

# --- how long does the operation actually take on THIS hardware? -------------
# Measured here rather than inherited from Linux: a fixture tuned on Hetzner can
# finish before the kill lands on this box, which is the precise trap that
# produces a silent vacuous green on the platform that matters most.
$TimingTarget = Join-Path $Work 'timing-target'
New-Item -ItemType Directory -Path $TimingTarget -Force | Out-Null
Set-Content -LiteralPath (Join-Path $TimingTarget 'config.toml') -Value 'x' -NoNewline
$sw = [Diagnostics.Stopwatch]::StartNew()
& $Binary backup restore $Archive --home $TimingTarget --replace --accept-missing-secrets --pace-ms $PaceMs *> (Join-Path $Work 'timing.log')
$timingRc = $LASTEXITCODE
$sw.Stop()
if ($timingRc -ne 0) { Get-Content (Join-Path $Work 'timing.log'); Fail 'the timing run failed' }
$OpExpectedMs = [int]$sw.Elapsed.TotalMilliseconds

# --- optional: hold a target file open during the restore --------------------
$heldStream = $null
if ($OpenHandle) {
    $held = Join-Path $Target 'config.toml'
    try {
        $heldStream = [IO.File]::Open($held, 'Open', 'ReadWrite', 'None')
        Write-Output "OPEN-HANDLE: holding $held exclusively during the restore"
    } catch {
        Write-Output "OPEN-HANDLE: could not open the target file exclusively: $($_.Exception.Message)"
    }
}

# --- the interrupted run ------------------------------------------------------
$env:WAYLAND_BACKUP_KILL_PROBE = $Probe
$restoreArgs = @('backup', 'restore', $Archive, '--home', $Target, '--replace',
                 '--accept-missing-secrets', '--pace-ms', "$PaceMs")
if ($HandlerControl) {
    # The control child needs its OWN console, because a close request is
    # delivered to a console window and is what turns into a catchable
    # CTRL_CLOSE_EVENT inside the process. Measured: launched hidden with its
    # stdio redirected, the child had no console to receive the close request,
    # so the probe could not fire and the control was structurally unable to
    # go green. Redirection is dropped only for this leg; the real run keeps it.
    $proc = Start-Process -FilePath $Binary -PassThru -ArgumentList $restoreArgs
} else {
    $proc = Start-Process -FilePath $Binary -PassThru -WindowStyle Hidden `
        -ArgumentList $restoreArgs `
        -RedirectStandardOutput (Join-Path $Work 'restore.out') `
        -RedirectStandardError  (Join-Path $Work 'restore.err')
}

Start-Sleep -Milliseconds $KillAtMs

$KillLanded = 'no'
$KillName = 'TerminateProcess'
$KillCatchable = 'no'
if ($HandlerControl) { $KillName = 'taskkill-close-request'; $KillCatchable = 'yes' }

if (-not $proc.HasExited) {
    if ($HandlerControl) {
        # A CLOSE REQUEST, not a terminate: this is the catchable mechanism, and
        # the probe must fire for it. Without this pair, `fired=no` in the real
        # run is equally consistent with a probe that was never installed.
        & taskkill /PID $proc.Id *> $null
        $KillLanded = 'yes'
        Start-Sleep -Milliseconds 800
        if (-not $proc.HasExited) { Stop-Process -Id $proc.Id -Force -ErrorAction SilentlyContinue }
    } else {
        # TerminateProcess: cannot be trapped, masked or deferred by the target.
        Stop-Process -Id $proc.Id -Force -ErrorAction SilentlyContinue
        $KillLanded = 'yes'
    }
}
try { $proc.WaitForExit(15000) | Out-Null } catch { }

if ($null -ne $heldStream) { $heldStream.Close(); $heldStream.Dispose() }

# --- did the kill land MID-FLIGHT? -------------------------------------------
$JournalDir = Join-Path $Target '.wayland-backup-journal'
$MidflightJournalOpen = 'no'
if (Test-Path -LiteralPath $JournalDir) {
    $records = @(Get-ChildItem -LiteralPath $JournalDir -Filter '*.json' -ErrorAction SilentlyContinue)
    if ($records.Count -gt 0) { $MidflightJournalOpen = 'yes' }
}

$DigestMid      = Read-DigestField $Target 'DIGEST'
$DigestComplete = Read-DigestField $TimingTarget 'DIGEST'
$RestoredAny = Test-Path -LiteralPath (Join-Path $Target 'skills\skill-0.md')
$MidflightTargetIntermediate = 'no'
if ($RestoredAny -and $DigestMid -ne $DigestComplete -and $DigestMid -ne $DigestPre) {
    $MidflightTargetIntermediate = 'yes'
}

$CompletedBeforeKill = 'no'
if ($MidflightJournalOpen -eq 'no' -or $KillLanded -eq 'no') { $CompletedBeforeKill = 'yes' }

$HandlerFired = 'no'
if (Test-Path -LiteralPath $Probe) { $HandlerFired = 'yes' }

# --- recover and compare ------------------------------------------------------
& $Binary backup recover --home $Target *> (Join-Path $Work 'recover.log')
$recoverRc = $LASTEXITCODE
if ($recoverRc -ne 0) { Get-Content (Join-Path $Work 'recover.log'); Fail "recovery exited $recoverRc" }

$DigestPost = Read-DigestField $Target 'DIGEST'
if ([string]::IsNullOrWhiteSpace($DigestPost)) { Fail 'could not take a post-recovery digest' }
$DigestEqual = 'no'
if ($DigestPre -eq $DigestPost) { $DigestEqual = 'yes' }

# --- verdict block ------------------------------------------------------------
Write-Output "INTERRUPT-PLATFORM: windows"
Write-Output "KILL-MECHANISM: $KillName CATCHABLE: $KillCatchable"
Write-Output "KILL-HANDLER-PROBE: installed=yes fired=$HandlerFired"
Write-Output "FIXTURE-PAYLOADS: $Payloads"
Write-Output "MIDFLIGHT-JOURNAL-OPEN: $MidflightJournalOpen"
Write-Output "MIDFLIGHT-TARGET-INTERMEDIATE: $MidflightTargetIntermediate"
Write-Output "MIDFLIGHT-TIMING: op_expected_ms=$OpExpectedMs kill_at_ms=$KillAtMs completed_before_kill=$CompletedBeforeKill"
Write-Output "DIGEST-ALGO: $DigestAlgo"
Write-Output "DIGEST-PRE: $DigestPre"
Write-Output "DIGEST-POST: $DigestPost"
Write-Output "DIGEST-EQUAL: $DigestEqual"

$deepRestored = 'n/a'
if (-not $Undersized) {
    $deepTargetFile = Join-Path $TimingTarget (($DeepRel + '/deep-canary.md') -replace '/', '\')
    if (Test-Path -LiteralPath $deepTargetFile) { $deepRestored = 'yes' } else { $deepRestored = 'no' }
    Write-Output "DEEP-PATH-RESTORED: $deepRestored len=$($deepTargetFile.Length)"
}

# --- adjudication -------------------------------------------------------------
if ($HandlerControl) {
    if ($HandlerFired -eq 'yes') {
        Write-Output 'HANDLER-CONTROL: fired=yes'
        Write-Output 'PROOF-OK: the probe fires for a catchable mechanism, so fired=no under TerminateProcess is a measurement'
        exit 0
    }
    Write-Output 'HANDLER-CONTROL: fired=no'
    Fail 'the handler probe did NOT fire for a catchable mechanism, so the uncatchability measurement is vacuous on this platform'
}

if ($Undersized) {
    if ($CompletedBeforeKill -eq 'yes' -or $MidflightTargetIntermediate -eq 'no') {
        Write-Output 'NEGATIVE-CONTROL: late-kill-detected'
        Write-Output 'NEGCTL-EXIT: 9'
        Write-Output 'PROOF-FAIL: the operation completed before the kill landed, so this run proves nothing about rollback'
        exit 9
    }
    Write-Output 'NEGATIVE-CONTROL: late-kill-missed'
    Write-Output 'NEGCTL-EXIT: 1'
    Fail 'the undersized fixture was still mid-flight; the negative control did not reproduce a late kill'
}

if ($OpenHandle) {
    # This leg does not need a mid-flight kill and must not require one. Its
    # documented assertion is that a restore contending with another handle
    # either SUCCEEDS or FAILS CLEANLY with an exact rollback -- never a half
    # state. Measured: the contended write fails fast, so the operation is over
    # before any kill could land, and demanding a mid-flight kill scored a
    # correct product outcome as a harness failure.
    if ($DigestEqual -ne 'yes') {
        Fail "open-handle contention left the target neither its old self nor its new one ($DigestPre vs $DigestPost)"
    }
    Write-Output 'OPEN-HANDLE-OUTCOME: resolved cleanly with an exact tree'
    Write-Output 'PROOF-OK: a restore contending with another open handle left the target byte-identical to its pre-operation tree'
    exit 0
}
if ($KillLanded -ne 'yes')                  { Fail 'the process had already exited when the kill was sent' }
if ($MidflightJournalOpen -ne 'yes')        { Fail 'no open journal record: the kill did not land mid-flight' }
if ($MidflightTargetIntermediate -ne 'yes') { Fail 'the target was not observably intermediate: the kill did not land mid-flight' }
if ($CompletedBeforeKill -ne 'no')          { Fail 'the operation completed before the kill landed' }
if ($HandlerFired -ne 'no')                 { Fail 'a catchable-mechanism handler fired, so the kill was NOT uncatchable' }
if ($DigestEqual -ne 'yes')                 { Fail "post-recovery tree differs from the pre-operation tree ($DigestPre vs $DigestPost)" }

$cfg = Get-Content -LiteralPath (Join-Path $Target 'config.toml') -Raw -ErrorAction SilentlyContinue
if ($cfg -notmatch 'PRE-EXISTING-DIVERGED-CONFIG') { Fail 'the diverged config was not restored to its pre-operation content' }
if (-not (Test-Path -LiteralPath (Join-Path $Target 'legacy\keepme.txt')))        { Fail 'a directory the archive does not contain was not restored' }
if (-not (Test-Path -LiteralPath (Join-Path $Target 'untouched-by-archive.txt'))) { Fail 'a top-level file the archive does not contain was not restored' }
if (Test-Path -LiteralPath $JournalDir) { Fail 'journal bookkeeping survived recovery' }

Write-Output 'PROOF-OK: exact rollback from an uncatchable mid-flight kill, over a target that carried state'
exit 0
