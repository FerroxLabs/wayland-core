# Phase 22 M-leg negative controls on Windows.
#
# M5 and M1 both PASSED on Windows. A pass alone proves nothing unless the same
# instrument can be made to fail, so each is driven in both directions here.
#
#   M5  operating_system_releases_writer_lease_after_process_exit
#       negative: stub the Windows LockFileEx path to always Acquire.
#       Both arms come from ONE build (env-gated), so a rebuild cannot be
#       mistaken for the cause of the red.
#
#   M1  the_retained_real_binary_corpus_still_reduces_to_the_pinned_state
#       negative: flip one byte of the retained Linux corpus. The digest must
#       then differ (or the chain must refuse to open). Either way it reddens,
#       which proves the digest is computed from the bytes rather than compared
#       to itself.

$ErrorActionPreference = 'Continue'
$repo = "D:\wls\repo"
$out  = "D:\wls\out"
$env:CARGO_TARGET_DIR = "D:\wls\target"
Set-Location $repo
New-Item -ItemType Directory -Force -Path $out | Out-Null

function Result-Line([string]$log) {
    $t = Get-Content $log -Raw
    if ($t -match 'test result:\s+(\w+)\.\s+(\d+)\s+passed;\s+(\d+)\s+failed;\s+(\d+)\s+ignored;\s+(\d+)\s+measured;\s+(\d+)\s+filtered out') {
        return "verdict=$($Matches[1]) passed=$($Matches[2]) failed=$($Matches[3]) ignored=$($Matches[4]) filtered_out=$($Matches[6])"
    }
    return "NO-RESULT-LINE"
}

$r = @()
$r += "LEG=phase22-M-legs-negative-controls"
$r += "HOST=$env:COMPUTERNAME"
$r += "UTC=$( (Get-Date).ToUniversalTime().ToString('o') )"
$r += "SHA=$(git rev-parse HEAD)"

# =====================================================================
# M5 — writer authority lease across a restart
# =====================================================================
git apply .planning\evidence\windows-legs-sweep\leg3-known-negative-journal-lock-stub.patch 2>&1 |
    Out-File "$out\leg3-neg-apply.log"
$hits = (Select-String -Path "crates\wcore-agent\src\session_journal\lease.rs" `
    -SimpleMatch "WL_JOURNAL_LOCK_STUB" | Measure-Object).Count
$r += "M5_STUB_MARKER_HITS=$hits"

if ($hits -ge 1) {
    $env:WL_JOURNAL_LOCK_STUB = "1"
    cargo test -p wcore-agent --test session_journal_test `
        operating_system_releases_writer_lease_after_process_exit -- --exact --nocapture `
        *> "$out\leg3-M5-STUBBED.log"
    $r += "M5_STUBBED=$(Result-Line "$out\leg3-M5-STUBBED.log")"

    Remove-Item Env:\WL_JOURNAL_LOCK_STUB
    cargo test -p wcore-agent --test session_journal_test `
        operating_system_releases_writer_lease_after_process_exit -- --exact --nocapture `
        *> "$out\leg3-M5-STUBBUILD-NOENV.log"
    $r += "M5_SAME_BUILD_NOENV=$(Result-Line "$out\leg3-M5-STUBBUILD-NOENV.log")"
} else {
    $r += "M5_STUBBED=NOT-RUN mutation never landed"
    $r += "M5_SAME_BUILD_NOENV=NOT-RUN"
}
git checkout -- crates/wcore-agent/src/session_journal/lease.rs
$r += "M5_MARKER_AFTER_RESTORE=$((Select-String -Path 'crates\wcore-agent\src\session_journal\lease.rs' -SimpleMatch 'WL_JOURNAL_LOCK_STUB' | Measure-Object).Count)"

# =====================================================================
# M1 — the retained-corpus reduction canary
# =====================================================================
$corpus = ".planning\phases\22-supervision-durable-goals-fleet-loops\22-01-EVIDENCE\linux\session-journal.bin"
$r += "M1_CORPUS_BYTES=$((Get-Item $corpus).Length)"

$bytes = [System.IO.File]::ReadAllBytes($corpus)
# Flip a bit well inside the file, away from frame 0's header.
$idx = [int]($bytes.Length / 2)
$r += "M1_FLIPPED_INDEX=$idx"
$bytes[$idx] = $bytes[$idx] -bxor 0x01
[System.IO.File]::WriteAllBytes($corpus, $bytes)
$dirty = (git status --porcelain $corpus | Measure-Object -Line).Lines
$r += "M1_CORPUS_DIRTY_AFTER_FLIP=$dirty"

cargo test -p wcore-agent --test goal_journal_compat_test -- --nocapture `
    *> "$out\leg3-M1-CORRUPTED.log"
$r += "M1_CORRUPTED=$(Result-Line "$out\leg3-M1-CORRUPTED.log")"

git checkout -- $corpus
$r += "M1_CORPUS_DIRTY_AFTER_RESTORE=$((git status --porcelain $corpus | Measure-Object -Line).Lines)"

cargo test -p wcore-agent --test goal_journal_compat_test -- --nocapture `
    *> "$out\leg3-M1-RESTORED.log"
$r += "M1_RESTORED=$(Result-Line "$out\leg3-M1-RESTORED.log")"

$r += "TREE_DIRTY_AT_END=$((git status --porcelain crates .planning | Measure-Object -Line).Lines)"
$r += "WLDONE"
Set-Content -Path "$out\leg3-negative-controls.txt" -Value $r
Get-Content "$out\leg3-negative-controls.txt"
