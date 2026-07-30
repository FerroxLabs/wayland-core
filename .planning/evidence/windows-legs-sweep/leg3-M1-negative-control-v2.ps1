# M1 negative control, REPAIRED.
#
# Run 1 of leg3-negative-controls.ps1 reported the M1 arm as `verdict=ok
# passed=2`, which reads like a completed control and is not one. The mutation
# never landed:
#
#   M1_CORPUS_BYTES=84327          <- Get-Item, a PowerShell cmdlet, uses the
#                                     PowerShell location and worked
#   M1_FLIPPED_INDEX=0             <- [System.IO.File]::ReadAllBytes is .NET and
#                                     resolves a RELATIVE path against the
#                                     PROCESS working directory, which
#                                     Set-Location does not change. It threw,
#                                     $bytes was null, and Length/2 was 0.
#   M1_CORPUS_DIRTY_AFTER_FLIP=0   <- the corpus was untouched
#
# So the "corrupted" arm reduced a pristine corpus and passed for free. This is
# the self-passing known-negative the brief names: an absence assertion on a
# dead instrument. It was caught only by the DIRTY guard, which is the third
# assertion - not known-positive, not known-negative, but "the mutation is
# actually present".
#
# Repair: absolute paths for every .NET call, plus a HARD ABORT when the corpus
# is not dirty after the flip. A control that cannot be shown to have been
# applied is not a control.

$ErrorActionPreference = 'Stop'
$repo = "D:\wls\repo"
$out  = "D:\wls\out"
$env:CARGO_TARGET_DIR = "D:\wls\target"
Set-Location $repo
# Make .NET agree with PowerShell about where we are. This one line is the fix.
[System.Environment]::CurrentDirectory = $repo

$corpus = Join-Path $repo ".planning\phases\22-supervision-durable-goals-fleet-loops\22-01-EVIDENCE\linux\session-journal.bin"

function Result-Line([string]$log) {
    $t = Get-Content $log -Raw
    if ($t -match 'test result:\s+(\w+)\.\s+(\d+)\s+passed;\s+(\d+)\s+failed;\s+(\d+)\s+ignored;\s+(\d+)\s+measured;\s+(\d+)\s+filtered out') {
        return "verdict=$($Matches[1]) passed=$($Matches[2]) failed=$($Matches[3]) ignored=$($Matches[4]) filtered_out=$($Matches[6])"
    }
    return "NO-RESULT-LINE"
}

$r = @()
$r += "LEG=phase22-M1-negative-control-v2"
$r += "HOST=$env:COMPUTERNAME"
$r += "UTC=$( (Get-Date).ToUniversalTime().ToString('o') )"
$r += "SHA=$(git rev-parse HEAD)"
$r += "CORPUS_ABS=$corpus"
$r += "CORPUS_EXISTS=$([System.IO.File]::Exists($corpus))"

$before = [System.IO.File]::ReadAllBytes($corpus)
$r += "CORPUS_BYTES_DOTNET=$($before.Length)"
if ($before.Length -lt 1000) { throw "corpus read returned $($before.Length) bytes - the .NET path is still wrong" }

# SELF-TEST 1: the pristine corpus must PASS. If this fails, the whole arm is
# meaningless because the red below could be anything.
cargo test -p wcore-agent --test goal_journal_compat_test -- --nocapture *> "$out\leg3-M1v2-PRISTINE.log"
$r += "M1_PRISTINE=$(Result-Line "$out\leg3-M1v2-PRISTINE.log")"

# Flip one bit in the middle of the file.
$idx = [int][math]::Floor($before.Length / 2)
$mutated = [byte[]]::new($before.Length)
[Array]::Copy($before, $mutated, $before.Length)
$mutated[$idx] = $mutated[$idx] -bxor 0x01
[System.IO.File]::WriteAllBytes($corpus, $mutated)

$r += "FLIPPED_INDEX=$idx"
$r += "BYTE_BEFORE=$($before[$idx])"
$after = [System.IO.File]::ReadAllBytes($corpus)
$r += "BYTE_AFTER=$($after[$idx])"
$dirty = (git status --porcelain -- $corpus | Measure-Object -Line).Lines
$r += "CORPUS_DIRTY_AFTER_FLIP=$dirty"

# HARD ABORT. Run 1 continued past exactly this point and reported a pass.
if ($dirty -lt 1 -or $before[$idx] -eq $after[$idx]) {
    $r += "ABORT=mutation did not land; the negative control is NOT RUN"
    $r += "WLDONE"
    Set-Content -Path "$out\leg3-M1-negative-v2.txt" -Value $r
    Get-Content "$out\leg3-M1-negative-v2.txt"
    exit 1
}

cargo test -p wcore-agent --test goal_journal_compat_test -- --nocapture *> "$out\leg3-M1v2-CORRUPTED.log"
$r += "M1_CORRUPTED=$(Result-Line "$out\leg3-M1v2-CORRUPTED.log")"

git checkout -- $corpus
$restored = [System.IO.File]::ReadAllBytes($corpus)
$r += "CORPUS_DIRTY_AFTER_RESTORE=$((git status --porcelain -- $corpus | Measure-Object -Line).Lines)"
$r += "BYTE_AFTER_RESTORE=$($restored[$idx])"

cargo test -p wcore-agent --test goal_journal_compat_test -- --nocapture *> "$out\leg3-M1v2-RESTORED.log"
$r += "M1_RESTORED=$(Result-Line "$out\leg3-M1v2-RESTORED.log")"

$r += "TREE_DIRTY_AT_END=$((git status --porcelain crates .planning | Measure-Object -Line).Lines)"
$r += "WLDONE"
Set-Content -Path "$out\leg3-M1-negative-v2.txt" -Value $r
Get-Content "$out\leg3-M1-negative-v2.txt"
