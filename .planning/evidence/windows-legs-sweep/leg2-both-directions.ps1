# F21-04-03 Windows re-proof, run in BOTH directions.
#
# The finding: two parallel `Spawn` siblings collide on the journal-head CAS, the
# loser's budget authority latches permanently faulted, and both siblings die.
# Measured 6 of 6 on Windows and 3 of 8 on Linux. Repaired at 1eb9b5ca by moving
# the head read inside the append's writer-lock acquisition, and re-proved 6/6 on
# LINUX ONLY. The Windows recurrence is listed unverified in F21-REVERIFY's own
# behavior_unverified_items.
#
# Direction 1 (can it pass): at HEAD the race test must pass every iteration.
# Direction 2 (can it fail): with the repair reverted to its exact pre-fix shape,
#   the same test on the same host must FAIL. Without this, a pass proves only
#   that the test did not happen to fire - not that the fix does anything.
#
# Every count goes to a file. Exit status is never trusted: over ssh+PowerShell
# every non-zero collapses to 1.

$ErrorActionPreference = 'Continue'
$repo  = "D:\wls\repo"
$out   = "D:\wls\out"
$iters = 20
$test  = "budget_authority::tests::concurrent_journal_writer_never_faults_budget_authority"
$env:CARGO_TARGET_DIR = "D:\wls\target"

New-Item -ItemType Directory -Force -Path $out | Out-Null
Set-Location $repo

function Run-Arm([string]$label) {
    $pass = 0; $fail = 0; $ran = 0
    for ($i = 1; $i -le $iters; $i++) {
        $log = Join-Path $out "leg2-$label-iter$i.log"
        cargo test -p wcore-agent --lib $test -- --exact --nocapture *> $log
        $text = Get-Content $log -Raw
        # Read the executed count back. A suite can exit 0 having run ZERO tests,
        # so the pass/fail verdict is only accepted when 1 test actually ran.
        if ($text -match 'test result:\s+(\w+)\.\s+(\d+)\s+passed;\s+(\d+)\s+failed;\s+(\d+)\s+ignored') {
            $p = [int]$Matches[2]; $f = [int]$Matches[3]; $g = [int]$Matches[4]
            if (($p + $f) -eq 1) {
                $ran++
                if ($p -eq 1) { $pass++ } else { $fail++ }
            }
            Add-Content -Path (Join-Path $out "leg2-$label-counts.txt") `
                -Value "iter=$i verdict=$($Matches[1]) passed=$p failed=$f ignored=$g"
        } else {
            Add-Content -Path (Join-Path $out "leg2-$label-counts.txt") `
                -Value "iter=$i verdict=NO-RESULT-LINE"
        }
    }
    return @($ran, $pass, $fail)
}

$report = @()
$report += "LEG=F21-04-03-windows-reproof"
$report += "HOST=$env:COMPUTERNAME"
$report += "UTC=$( (Get-Date).ToUniversalTime().ToString('o') )"
$report += "SHA=$(git rev-parse HEAD)"
$report += "ITERATIONS_REQUESTED=$iters"

Remove-Item -Force (Join-Path $out "leg2-fixed-counts.txt") -ErrorAction SilentlyContinue
Remove-Item -Force (Join-Path $out "leg2-reverted-counts.txt") -ErrorAction SilentlyContinue

# --- Direction 1: HEAD, repair in place.
$r = Run-Arm "fixed"
$report += "FIXED_RAN=$($r[0])"
$report += "FIXED_PASSED=$($r[1])"
$report += "FIXED_FAILED=$($r[2])"

# --- Direction 2: revert the repair and re-run the identical test.
git apply --verbose .planning/evidence/windows-legs-sweep/F21-04-03-known-negative.patch 2>&1 |
    Out-File (Join-Path $out "leg2-patch-apply.log")
$report += "PATCH_APPLIED_DIRTY=$( (git status --porcelain crates/wcore-agent/src/session_journal.rs | Measure-Object -Line).Lines )"
# Prove the mutation is really in the tree rather than trusting git apply's status.
$mutated = Select-String -Path "crates\wcore-agent\src\session_journal.rs" `
    -SimpleMatch "KNOWN-NEGATIVE MUTATION" | Measure-Object | Select-Object -ExpandProperty Count
$report += "MUTATION_MARKER_HITS=$mutated"

if ($mutated -ge 1) {
    $r2 = Run-Arm "reverted"
    $report += "REVERTED_RAN=$($r2[0])"
    $report += "REVERTED_PASSED=$($r2[1])"
    $report += "REVERTED_FAILED=$($r2[2])"
} else {
    $report += "REVERTED_RAN=0"
    $report += "REVERTED_PASSED=0"
    $report += "REVERTED_FAILED=0"
    $report += "REVERTED_NOTE=mutation never landed; arm NOT RUN"
}

# --- Restore. One named path in this scratch clone; no ref is moved.
git checkout -- crates/wcore-agent/src/session_journal.rs
$still = Select-String -Path "crates\wcore-agent\src\session_journal.rs" `
    -SimpleMatch "KNOWN-NEGATIVE MUTATION" | Measure-Object | Select-Object -ExpandProperty Count
$report += "MUTATION_MARKER_AFTER_RESTORE=$still"

$report += "WLDONE"
Set-Content -Path (Join-Path $out "leg2-both-directions.txt") -Value $report
Get-Content (Join-Path $out "leg2-both-directions.txt")
