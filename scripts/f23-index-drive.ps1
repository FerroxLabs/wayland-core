# F23-06 live driver, Windows port of scripts/f23-index-drive.sh.
#
# Same contract as the POSIX driver and the rest of the f23 driver family:
#   -Binary <path>  the wayland-core binary to drive
#   -Sha <commit>   the commit under test; the binary's own --build-info source
#                   SHA must equal it, so a stale binary REDDENS
#   -Nonce <hex>    caller-generated at run time, echoed in the terminal PASS
#                   marker, so a stale log cannot satisfy the caller's check
#   -Repo <path>    optional; the workspace to index
#
# Emits exactly one terminal marker:
#   F23_03_DRIVE=PASS platform=windows nonce=<the given nonce>
# and ONLY after every measurement and every check passed. A missing
# measurement is a FAILURE, never a skip.
#
# ── The two PowerShell rules this file exists to obey ────────────────────────
#
# 1. NEVER read an exit code from a block that also emits output. In
#    PowerShell, `$x = & { cmd | Tee-Object …; $LASTEXITCODE }` returns an
#    ARRAY of every output line plus the code, so `if ($x -ne 0)` is an
#    always-truthy array FILTER. That bug reported a fully green 12/12 + 6/6
#    Windows soak as a failure — see scripts/wayland-e2e-windows-soak.ps1
#    lines 174-190 and 244-255 for the worked example and the post-mortem.
#    Every external command here is invoked on its own statement and
#    `$LASTEXITCODE` is read on the NEXT line.
# 2. ALWAYS end with an explicit `exit`. The remote shell is PowerShell, so
#    the ssh command string is PowerShell source, and without an explicit exit
#    the caller's `exit $LASTEXITCODE` carries the wrong status.
#
# This driver's Windows leg is where the byte-range-lock and path-representation
# defect classes are expected to surface. A driver that reported success from
# an always-truthy array filter would hide exactly the finding it exists to
# produce.

[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)][string]$Binary,
    [Parameter(Mandatory = $true)][string]$Sha,
    [Parameter(Mandatory = $true)][string]$Nonce,
    [string]$Repo = ''
)

$ErrorActionPreference = 'Continue'
$PSNativeCommandUseErrorActionPreference = $false
$Platform = 'windows'
$script:Failures = 0

function Fail([string]$Message) {
    Write-Host "  FAIL: $Message"
    $script:Failures = $script:Failures + 1
}

function Measure-Value([string]$Name, [int]$Sample, [string]$Value) {
    Write-Host "F23_03_MEASURE=$Name platform=$Platform sample=$Sample value=$Value"
}

if (-not (Test-Path -LiteralPath $Binary)) {
    Write-Host "FATAL: $Binary does not exist"
    exit 65
}
$Binary = (Resolve-Path -LiteralPath $Binary).Path

if ([string]::IsNullOrWhiteSpace($Repo)) {
    $Repo = (Resolve-Path -LiteralPath (Join-Path $PSScriptRoot '..')).Path
}
else {
    if (-not (Test-Path -LiteralPath $Repo)) {
        Write-Host "FATAL: -Repo $Repo is not a directory"
        exit 64
    }
    $Repo = (Resolve-Path -LiteralPath $Repo).Path
}

# ── Provenance: refuse to measure with a binary built from other code ────────
$buildInfo = & $Binary --build-info 2>&1
$rc = $LASTEXITCODE
if ($rc -ne 0) {
    Write-Host "FATAL: --build-info exited $rc : $buildInfo"
    exit 67
}
$binSha = ''
foreach ($line in @($buildInfo)) {
    $m = [regex]::Match([string]$line, '\(source ([0-9a-f]+)\)')
    if ($m.Success) { $binSha = $m.Groups[1].Value }
}
if ($binSha -ne $Sha) {
    Write-Host "FATAL: binary source SHA '$binSha' != commit under test '$Sha'"
    exit 68
}
Write-Host "F23_03_PROVENANCE=ok platform=$Platform sha=$Sha"

$RunDir = Join-Path ([System.IO.Path]::GetTempPath()) ("f23idx-" + $Nonce)
$Stores = Join-Path $RunDir 'stores'
$Transcripts = Join-Path $RunDir 'transcripts'
$Scratch = Join-Path $RunDir 'scratch'
New-Item -ItemType Directory -Force -Path $Stores, $Transcripts, $Scratch | Out-Null

# Run `wayland-core index …`, capturing stdout+stderr. Sets $script:IdxOut
# (array of lines) and $script:IdxRc. The status is read on the line AFTER the
# invocation and NEVER from a block that also emits output.
function Invoke-Index([string]$Label, [string[]]$IndexArgs) {
    $transcript = Join-Path $Transcripts "$Label.txt"
    $script:IdxOut = & $Binary index @IndexArgs 2>&1
    $script:IdxRc = $LASTEXITCODE
    Set-Content -LiteralPath $transcript -Value (@("# invocation: index $($IndexArgs -join ' ')") + @($script:IdxOut) + @("# exit: $script:IdxRc"))
}

# Extract one key=value field from an `F23_INDEX=<kind> …` output line.
function Get-Field([string]$Kind, [string]$Key) {
    foreach ($line in @($script:IdxOut)) {
        $text = [string]$line
        if ($text.StartsWith("F23_INDEX=$Kind ")) {
            foreach ($token in $text.Substring("F23_INDEX=$Kind ".Length).Split(' ')) {
                if ($token.StartsWith("$Key=")) { return $token.Substring($Key.Length + 1) }
            }
        }
    }
    return ''
}

function Get-HitPaths() {
    $paths = @()
    foreach ($line in @($script:IdxOut)) {
        $text = [string]$line
        if ($text.StartsWith('F23_INDEX=hit ')) {
            foreach ($token in $text.Split(' ')) {
                if ($token.StartsWith('path=')) { $paths += $token.Substring(5) }
            }
        }
    }
    return $paths
}

Write-Host "F23_03_CORPUS=repo path=$Repo"

# ── 1. Cold build and warm start, three samples each ─────────────────────────
$ColdStore = ''
foreach ($sample in 1, 2, 3) {
    $ColdStore = Join-Path $Stores "cold-$sample.db"
    foreach ($suffix in '', '-wal', '-shm') {
        Remove-Item -LiteralPath "$ColdStore$suffix" -Force -ErrorAction SilentlyContinue
    }

    $t0 = [System.Diagnostics.Stopwatch]::StartNew()
    Invoke-Index "cold-build-$sample" @('--root', $Repo, '--store', $ColdStore, 'build')
    $t0.Stop()
    if ($script:IdxRc -ne 0) {
        Fail "cold build sample $sample exited $script:IdxRc"
        continue
    }
    Measure-Value 'cold-build' $sample ([string][int]$t0.ElapsedMilliseconds)

    $records = Get-Field 'build' 'records'
    $symbols = Get-Field 'build' 'symbols'
    $storeBytes = Get-Field 'build' 'store_bytes'
    $readCount = Get-Field 'build' 'read'
    if ([string]::IsNullOrEmpty($records) -or [int]$records -lt 100) {
        Fail "cold build sample $sample indexed '$records' records; a tiny count means the walk did not run"
    }
    Measure-Value 'store-size' $sample $storeBytes
    Write-Host "F23_03_CORPUS=indexed sample=$sample records=$records symbols=$symbols read=$readCount"

    $t1 = [System.Diagnostics.Stopwatch]::StartNew()
    Invoke-Index "warm-start-$sample" @('--root', $Repo, '--store', $ColdStore, 'build')
    $t1.Stop()
    if ($script:IdxRc -ne 0) {
        Fail "warm start sample $sample exited $script:IdxRc"
        continue
    }
    Measure-Value 'warm-start' $sample ([string][int]$t1.ElapsedMilliseconds)

    $warmRead = Get-Field 'build' 'read'
    $warmExtract = Get-Field 'build' 'extracted'
    if ($warmRead -ne '0' -or $warmExtract -ne '0') {
        Fail "warm start sample $sample opened $warmRead files and extracted $warmExtract; incrementality is a READ COUNT and this one is not zero"
    }
    Write-Host "F23_03_WARM=sample=$sample read=$warmRead extracted=$warmExtract"
}

# ── 2. Query latency over the fixed query set ────────────────────────────────
$querySet = @(
    'IndexStore', 'normalize_rel', 'ScopeIdentity', 'SymbolKind', 'IndexOptions',
    'RepoMapError', 'extract_rust', 'semantic_status', 'LlmProvider', 'SessionManager',
    'ToolRegistry', 'ProviderCompat', 'MemoryAccessGate', 'WorkflowRunner', 'CheckpointStore',
    'BudgetAuthorityCoordinator', 'SandboxBackend', 'ExecutionGraph', 'AgentSpawner', 'RepoMap'
)
$latencies = @()
foreach ($q in $querySet) {
    Invoke-Index "search-$q" @('--root', $Repo, '--store', $ColdStore, 'search', $q, '--limit', '10')
    if ($script:IdxRc -ne 0) {
        Fail "search '$q' exited $script:IdxRc"
        continue
    }
    $us = Get-Field 'search' 'elapsed_us'
    if ([string]::IsNullOrEmpty($us)) {
        Fail "search '$q' reported no elapsed_us; a missing measurement is a failure"
        continue
    }
    $latencies += [int]$us
}
if ($latencies.Count -lt 10) {
    Fail "only $($latencies.Count) query-latency samples were collected; the fixed query set has $($querySet.Count) entries and a missing measurement is a failure, not a skip"
}
$sorted = @($latencies | Sort-Object)
if ($sorted.Count -gt 0) {
    # Nearest-rank percentiles — no interpolation, exact for a set this small.
    $p50 = $sorted[[int][math]::Ceiling($sorted.Count * 0.50) - 1]
    $p95 = $sorted[[int][math]::Ceiling($sorted.Count * 0.95) - 1]
    Measure-Value 'latency-p50' 1 ([string]$p50)
    Measure-Value 'latency-p95' 1 ([string]$p95)
    Write-Host "F23_03_LATENCY=samples n=$($sorted.Count) unit=microseconds all=$($sorted -join ' ')"
}

# ── 3. Retrieval quality through the shipped binary ──────────────────────────
$corpus = @(
    @{ q = 'IndexStore'; e = @('crates/wcore-repomap/src/store.rs') },
    @{ q = 'normalize_rel'; e = @('crates/wcore-repomap/src/scope.rs') },
    @{ q = 'ScopeIdentity'; e = @('crates/wcore-repomap/src/scope.rs') },
    @{ q = 'semantic_status'; e = @('crates/wcore-repomap/src/search.rs') },
    @{ q = 'extract_rust'; e = @('crates/wcore-repomap/src/extractor/rust.rs') },
    @{ q = 'extract_typescript'; e = @('crates/wcore-repomap/src/extractor/typescript.rs') },
    @{ q = 'strip_comments_rust_style'; e = @('crates/wcore-repomap/src/extractor/mod.rs') },
    @{ q = 'SymbolKind'; e = @('crates/wcore-repomap/src/types.rs') },
    @{ q = 'IndexOptions'; e = @('crates/wcore-repomap/src/types.rs') },
    @{ q = 'RepoMapError'; e = @('crates/wcore-repomap/src/types.rs') },
    @{ q = 'first_meaningful'; e = @('crates/wcore-repomap/src/lib.rs') },
    @{ q = 'reciprocal rank fusion'; e = @('crates/wcore-repomap/src/search.rs') },
    @{ q = 'content hash invalidation'; e = @('crates/wcore-repomap/src/store.rs') },
    @{ q = 'worktree identity'; e = @('crates/wcore-repomap/src/scope.rs') },
    @{ q = 'bm25 full text'; e = @('crates/wcore-repomap/src/search.rs') },
    @{ q = 'walker gitignore hidden'; e = @('crates/wcore-repomap/src/scope.rs', 'crates/wcore-repomap/src/lib.rs') }
)
$precisionSum = 0.0
$recallSum = 0.0
foreach ($case in $corpus) {
    $label = ($case.q -replace '[^A-Za-z0-9]', '_')
    Invoke-Index "quality-$label" @('--root', $Repo, '--store', $ColdStore, 'search', $case.q, '--limit', '10')
    if ($script:IdxRc -ne 0) {
        Fail "quality query '$($case.q)' exited $script:IdxRc"
        continue
    }
    $hits = Get-HitPaths
    $top = ''
    if ($hits.Count -gt 0) { $top = $hits[0] }
    $p = 0
    if ($case.e -contains $top) { $p = 1 }
    $found = 0
    foreach ($expected in $case.e) { if ($hits -contains $expected) { $found = $found + 1 } }
    $r = [double]$found / [double]$case.e.Count
    $precisionSum = $precisionSum + $p
    $recallSum = $recallSum + $r
    Write-Host ("F23_03_QUALITY_CASE={0} precision_at_1={1} recall_at_10={2:F4} top={3}" -f $label, $p, $r, $top)
}
$precision = $precisionSum / $corpus.Count
$recall = $recallSum / $corpus.Count
Measure-Value 'precision' 1 ("{0:F4}" -f $precision)
Measure-Value 'recall' 1 ("{0:F4}" -f $recall)
Write-Host ("F23_03_QUALITY=corpus queries={0} precision_at_1={1:F4} recall_at_10={2:F4}" -f $corpus.Count, $precision, $recall)

# ── 4. Exact-search fallback ─────────────────────────────────────────────────
Invoke-Index 'fallback' @('--root', $Repo, '--store', $ColdStore, 'search', '=> {', '--limit', '3')
if ($script:IdxRc -eq 0 -and (Get-Field 'search' 'fallback') -eq 'true') {
    Write-Host 'F23_03_FALLBACK_REPORTED=true'
}
else {
    Write-Host 'F23_03_FALLBACK_REPORTED=false'
    Fail 'a punctuation-only literal was not reported as answered by the fallback'
}

# ── 5. Incremental mutations in a scratch repository ─────────────────────────
# Materialised with `git archive | tar -x` and then `git init`-ed, NOT with
# `git clone`: the measurement checkout is a detached-HEAD worktree, and a
# clone of a detached HEAD yields an EMPTY working tree, which would make
# every mutation below pass vacuously.
$Clone = Join-Path $Scratch 'clone'
New-Item -ItemType Directory -Force -Path $Clone | Out-Null
$tarPath = Join-Path $Scratch 'tree.tar'
& git -C $Repo archive HEAD -o $tarPath
$rc = $LASTEXITCODE
if ($rc -ne 0) {
    Write-Host "FATAL: git archive exited $rc"
    exit 69
}
& tar -xf $tarPath -C $Clone
$rc = $LASTEXITCODE
if ($rc -ne 0) {
    Write-Host "FATAL: tar -xf exited $rc"
    exit 69
}
Remove-Item -LiteralPath $tarPath -Force -ErrorAction SilentlyContinue
$scratchFiles = @(Get-ChildItem -LiteralPath $Clone -Recurse -File).Count
if ($scratchFiles -lt 100) {
    Write-Host "FATAL: the scratch tree has only $scratchFiles files; every mutation would pass vacuously"
    exit 69
}
Write-Host "F23_03_SCRATCH=files=$scratchFiles"

& git -C $Clone init -q -b main
$rc = $LASTEXITCODE
if ($rc -ne 0) { Write-Host "FATAL: git init exited $rc"; exit 69 }
& git -C $Clone add .
$rc = $LASTEXITCODE
if ($rc -ne 0) { Write-Host "FATAL: git add exited $rc"; exit 69 }
& git -C $Clone -c user.email=f23@example.invalid -c user.name=f23 -c commit.gpgsign=false commit -qm 'f23 scratch base'
$rc = $LASTEXITCODE
if ($rc -ne 0) { Write-Host "FATAL: git commit exited $rc"; exit 69 }

$MutStore = Join-Path $Stores 'mutations.db'
Invoke-Index 'mutation-base' @('--root', $Clone, '--store', $MutStore, 'build')
if ($script:IdxRc -ne 0) {
    Write-Host "FATAL: the scratch tree could not be indexed (exit $script:IdxRc)"
    exit 70
}
Write-Host "F23_03_MUTATION_BASE=records=$(Get-Field 'build' 'records')"

function Invoke-Mutation([string]$Name, [string]$ExpectField, [int]$MaxExtract) {
    Invoke-Index "mutation-$Name" @('--root', $Clone, '--store', $MutStore, 'build')
    $status = 'PASS'
    if ($script:IdxRc -ne 0) {
        $status = 'FAIL'
        Fail "mutation $Name : index build exited $script:IdxRc"
    }
    $got = Get-Field 'build' $ExpectField
    $extracted = Get-Field 'build' 'extracted'
    $unchanged = Get-Field 'build' 'unchanged'
    if ([string]::IsNullOrEmpty($got) -or [int]$got -lt 1) {
        $status = 'FAIL'
        Fail "mutation $Name : expected $ExpectField >= 1, got '$got'"
    }
    $surplus = 0
    if ([string]::IsNullOrEmpty($extracted)) {
        $status = 'FAIL'
        Fail "mutation $Name : no extracted count reported"
    }
    elseif ([int]$extracted -gt $MaxExtract) {
        $status = 'FAIL'
        $surplus = [int]$extracted - $MaxExtract
        Fail "mutation $Name : re-extracted $extracted files, at most $MaxExtract were touched; unchanged files were re-extracted"
    }
    Write-Host "F23_03_MUTATION=$Name platform=$Platform status=$status unchanged_reextracted=$surplus $ExpectField=$got extracted=$extracted unchanged=$unchanged"
}

$addedPath = Join-Path $Clone 'f23_added.rs'
Set-Content -LiteralPath $addedPath -Value "pub fn f23_drive_added_$Nonce() {}"
Invoke-Mutation 'add' 'added' 1

Set-Content -LiteralPath $addedPath -Value "pub fn f23_drive_added_$Nonce() { let _ = 1; }"
Invoke-Mutation 'edit' 'changed' 1

Remove-Item -LiteralPath $addedPath -Force
Invoke-Mutation 'delete' 'deleted' 0

$renameSrc = Join-Path $Clone 'f23_rename_src.rs'
$renameDst = Join-Path $Clone 'f23_rename_dst.rs'
Set-Content -LiteralPath $renameSrc -Value "pub fn f23_drive_renamed_$Nonce() {}"
Invoke-Index 'mutation-rename-seed' @('--root', $Clone, '--store', $MutStore, 'build')
Move-Item -LiteralPath $renameSrc -Destination $renameDst -Force
Invoke-Mutation 'rename' 'renamed' 0
Remove-Item -LiteralPath $renameDst -Force
Invoke-Index 'mutation-rename-cleanup' @('--root', $Clone, '--store', $MutStore, 'build')

& git -C $Clone checkout -q -b "f23-drive-$Nonce"
$rc = $LASTEXITCODE
if ($rc -ne 0) {
    Write-Host "F23_03_MUTATION=branch-switch platform=$Platform status=FAIL unchanged_reextracted=0 note=checkout-failed"
    Fail "could not create the scratch branch (git checkout exited $rc)"
}
else {
    Set-Content -LiteralPath (Join-Path $Clone 'f23_branch.rs') -Value "pub fn f23_branch_only_$Nonce() {}"
    & git -C $Clone add f23_branch.rs
    & git -C $Clone -c user.email=f23@example.invalid -c user.name=f23 -c commit.gpgsign=false commit -qm 'f23 drive branch'
    Invoke-Mutation 'branch-switch' 'added' 1
    Invoke-Index 'branch-status' @('--root', $Clone, '--store', $MutStore, 'status')
    foreach ($line in @($script:IdxOut)) {
        $text = [string]$line
        if ($text.StartsWith('F23_INDEX=scope recorded=')) {
            Write-Host "F23_03_SCOPE_AFTER_SWITCH=$($text.Substring('F23_INDEX=scope recorded='.Length))"
            break
        }
    }
}

# ── 6. Staleness ─────────────────────────────────────────────────────────────
$stalePath = Join-Path $Clone 'f23_stale.rs'
Set-Content -LiteralPath $stalePath -Value "pub fn f23_stale_marker_$Nonce() {}"
Invoke-Index 'stale-build' @('--root', $Clone, '--store', $MutStore, 'build')
Invoke-Index 'stale-before' @('--root', $Clone, '--store', $MutStore, 'search', "f23_stale_marker_$Nonce", '--limit', '3')
$beforeStale = Get-Field 'hit' 'content_stale'
Set-Content -LiteralPath $stalePath -Value "pub fn f23_stale_marker_$Nonce() { /* edited after indexing */ }"
Invoke-Index 'stale-after' @('--root', $Clone, '--store', $MutStore, 'search', "f23_stale_marker_$Nonce", '--limit', '3')
$afterStale = Get-Field 'hit' 'content_stale'
if ($beforeStale -eq 'false' -and $afterStale -eq 'true') {
    Write-Host 'F23_03_STALENESS_REPORTED=true'
}
else {
    Write-Host 'F23_03_STALENESS_REPORTED=false'
    Fail "staleness: before='$beforeStale' after='$afterStale'; the before-assert is the load-bearing half, since a hit that was ALWAYS stale proves nothing"
}

Invoke-Index 'verify-drifted' @('--root', $Clone, '--store', $MutStore, 'verify')
$verifyRc = $script:IdxRc
Write-Host "F23_03_VERIFY=agrees=$(Get-Field 'verify' 'agrees') exit=$verifyRc"
if ($verifyRc -eq 0) {
    Fail 'verify reported agreement over a tree with a file edited after indexing'
}

# ── 7. Secret isolation, asserted against the store's own bytes ──────────────
# The CONTROL marker is planted in an INDEXED file and must be PRESENT: if it
# were absent the store would hold no content at all, and the isolation
# assertion below would be vacuously true.
$secret = "f23secret${Nonce}zz"
$control = "f23control${Nonce}yy"
$ignoredDir = Join-Path $Clone 'f23-ignored'
New-Item -ItemType Directory -Force -Path $ignoredDir | Out-Null
Set-Content -LiteralPath (Join-Path $ignoredDir 'creds.rs') -Value "const TOKEN: &str = ""$secret"";"
Add-Content -LiteralPath (Join-Path $Clone '.gitignore') -Value 'f23-ignored/'
Set-Content -LiteralPath (Join-Path $Clone 'f23_control.rs') -Value "pub const CONTROL: &str = ""$control"";"

$IsoStore = Join-Path $Stores 'isolation.db'
foreach ($suffix in '', '-wal', '-shm') {
    Remove-Item -LiteralPath "$IsoStore$suffix" -Force -ErrorAction SilentlyContinue
}
Invoke-Index 'isolation-build' @('--root', $Clone, '--store', $IsoStore, 'build')
if ($script:IdxRc -ne 0) { Fail "isolation build exited $script:IdxRc" }

# Counted over the RAW BYTES of the store and its sidecars. Latin1 maps every
# byte to exactly one char, so an ASCII marker embedded anywhere in a binary
# file is found; a UTF-8 or Default decode would silently mangle bytes above
# 0x7F and could drop a match.
function Count-Occurrences([string]$Path, [string]$Needle) {
    if (-not (Test-Path -LiteralPath $Path)) { return 0 }
    $bytes = [System.IO.File]::ReadAllBytes($Path)
    $text = [System.Text.Encoding]::GetEncoding('ISO-8859-1').GetString($bytes)
    $count = 0
    $index = $text.IndexOf($Needle, [System.StringComparison]::Ordinal)
    while ($index -ge 0) {
        $count = $count + 1
        $index = $text.IndexOf($Needle, $index + 1, [System.StringComparison]::Ordinal)
    }
    return $count
}

$controlHits = 0
$secretHits = 0
foreach ($suffix in '', '-wal', '-shm') {
    $controlHits = $controlHits + (Count-Occurrences "$IsoStore$suffix" $control)
    $secretHits = $secretHits + (Count-Occurrences "$IsoStore$suffix" $secret)
}
Write-Host "F23_03_STORE_CONTROL_OCCURRENCES=$controlHits"
Write-Host "F23_03_STORE_NONCE_OCCURRENCES=$secretHits"
if ($controlHits -lt 1) {
    Fail 'the CONTROL marker planted in an INDEXED file is absent from the store bytes; the store holds no content, so the isolation assertion is vacuous'
}
if ($secretHits -ne 0) {
    Fail "CRITICAL: a run-time nonce planted in a gitignored file was found $secretHits time(s) in the store's own bytes; the excluded file was READ"
}

Remove-Item -LiteralPath $RunDir -Recurse -Force -ErrorAction SilentlyContinue

if ($script:Failures -ne 0) {
    Write-Host "F23_03_DRIVE=FAIL platform=$Platform nonce=$Nonce failures=$script:Failures"
    exit 1
}
Write-Host "F23_03_DRIVE=PASS platform=$Platform nonce=$Nonce"
exit 0
