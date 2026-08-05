# 28-E5-REPAIR — the control that makes the 26/26 sandbox green mean something.
#
# The main run was taken with an EMPTY lease directory, so on its own it shows only that the
# F-28-02-002 repair does not REGRESS the E5 matrix. This script asks the sharper question:
# with the real archived wedge artifact planted, does the E5 matrix DISTINGUISH the pre-repair
# candidate from the post-repair one? If it does not, the 26 sandbox cells are not measuring
# the thing this lane exists to measure.
#
#   OBS A / MATRIX A : wedge planted, PRE-repair binary  (32e2f57d, sha baf9bd69…)
#   OBS B / MATRIX B : wedge planted, POST-repair binary (6db9e56b, sha 4c48d665…)
#
# The pre-repair binary is the BYTE-IDENTICAL 28-02 Windows candidate, recovered from its
# original CI run (30269095004) and digest-asserted here. The wedge artifact is the real file
# archived at C:\p22-evidence\stale-leases-backup, the same one 28-02 and 28-h2 used.
#
# PRE never clears the wedge (permanence is the finding), so A can run before B without
# re-planting. B is expected to reclaim it, which is checked, and the directory is then
# restored to its as-found state and the restoration is asserted.
param(
  [Parameter(Mandatory=$true)][string]$PreExe,
  [Parameter(Mandatory=$true)][string]$PreSha,
  [Parameter(Mandatory=$true)][string]$PreCommit,
  [Parameter(Mandatory=$true)][string]$PreTree,
  [Parameter(Mandatory=$true)][string]$PostExe,
  [Parameter(Mandatory=$true)][string]$PostSha,
  [Parameter(Mandatory=$true)][string]$PostCommit,
  [Parameter(Mandatory=$true)][string]$PostTree,
  [Parameter(Mandatory=$true)][string]$NonceA,
  [Parameter(Mandatory=$true)][string]$NonceB
)

$ErrorActionPreference = 'Continue'
$root   = 'C:\f28e5'
$log    = "$root\wedge-diff.log"
$status = "$root\wedge-diff.status"
$mjs    = "$root\f28-native-matrix.mjs"
$mtsv   = "$root\matrix.tsv"
$dir    = Join-Path $env:LOCALAPPDATA 'Wayland\Core\AppContainerLeases\v1'
$quar   = Join-Path $dir 'quarantine'
$bak    = 'C:\p22-evidence\stale-leases-backup'
$stale  = 'WCore-storage-00002d20-00000000000000f2.toml'

Remove-Item -Force -ErrorAction SilentlyContinue $status
Set-Content -Path $log -Value ("F28E5 WEDGE DIFFERENTIAL started=" + (Get-Date -Format o)) -Encoding utf8
function L($m) { Add-Content -Path $log -Value $m -Encoding utf8 }

L ("WHOAMI=" + (& whoami))
foreach ($p in @(@('PRE',$PreExe,$PreSha), @('POST',$PostExe,$PostSha))) {
  $h = (Get-FileHash -Algorithm SHA256 -Path $p[1]).Hash.ToLower()
  L ($p[0] + "_EXE=" + $p[1] + " SHA256=" + $h)
  if ($h -ne $p[2].ToLower()) {
    L ($p[0] + "_DIGEST_MISMATCH")
    Set-Content -Path $status -Value "WLRC=6" -Encoding utf8
    Add-Content -Path $status -Value "WLDONE" -Encoding utf8
    exit 6
  }
}
L "BOTH_BINARIES_DIGEST_BOUND"

# Snapshot the as-found lease directory so it is restored exactly (28-h2's discipline).
$restore = "$root\lease-restore"
if (Test-Path $restore) { Remove-Item -Recurse -Force $restore }
New-Item -ItemType Directory -Force -Path $restore | Out-Null
$asFound = @(Get-ChildItem -Path $dir -File -ErrorAction SilentlyContinue)
foreach ($f in $asFound) { Copy-Item $f.FullName -Destination $restore -Force }
L ("AS_FOUND_LEASES=" + $asFound.Count)

function LeaseCounts($tag) {
  $n = (Get-ChildItem -Path $dir  -File -ErrorAction SilentlyContinue | Measure-Object).Count
  $q = (Get-ChildItem -Path $quar -File -ErrorAction SilentlyContinue | Measure-Object).Count
  L "LEASES tag=$tag active=$n quarantined=$q"
  return @($n, $q)
}

function CaptureActiveness($tag, $exe) {
  $out = "$root\act-$tag.json"
  Remove-Item -Force -ErrorAction SilentlyContinue $out
  & node $mjs --capture-activeness --bin $exe --out $out 2>&1 | ForEach-Object { L ("act-$tag> " + $_) }
  L ("ACT_RC_$tag=" + $LASTEXITCODE)
  $observed = 'UNREADABLE'
  if (Test-Path $out) {
    $j = Get-Content -Raw $out | ConvertFrom-Json
    $observed = [string]$j.observed
  }
  L "ACT_OBSERVED_$tag=$observed"
  return $observed
}

function RunMatrix($tag, $exe, $commit, $tree, $nonce, $act) {
  $json = "$root\matrix-$tag.json"
  $mlog = "$root\matrix-$tag-markers.log"
  Remove-Item -Force -ErrorAction SilentlyContinue $json
  & node $mjs --run --bin $exe --os windows --commit $commit --tree $tree --nonce $nonce `
      --matrix $mtsv --activeness $act --log $mlog --json $json 2>&1 |
      Where-Object { $_ -notmatch '^F28_CELL' } | ForEach-Object { L ("mat-$tag> " + $_) }
  L ("RUN_RC_$tag=" + $LASTEXITCODE)
  & node $mjs --verify $mlog --matrix $mtsv --os windows --commit $commit --tree $tree --nonce $nonce 2>&1 |
      ForEach-Object { L ("ver-$tag> " + $_) }
  L ("VERIFY_RC_$tag=" + $LASTEXITCODE)
  $cells = 0; $pass = 0; $red = 0; $sbCells = 0; $sbPass = 0; $sbRed = 0
  if (Test-Path $json) {
    $j = Get-Content -Raw $json | ConvertFrom-Json
    $cells = ($j | Measure-Object).Count
    $pass  = ($j | Where-Object { $_.outcome -eq 'pass' } | Measure-Object).Count
    $red   = ($j | Where-Object { $_.outcome -eq 'red'  } | Measure-Object).Count
    $sb    = $j | Where-Object { $_.dimension -eq 'sandbox-probes' }
    $sbCells = ($sb | Measure-Object).Count
    $sbPass  = ($sb | Where-Object { $_.outcome -eq 'pass' } | Measure-Object).Count
    $sbRed   = ($sb | Where-Object { $_.outcome -eq 'red'  } | Measure-Object).Count
  }
  L "MATRIX_$tag CELLS=$cells PASS=$pass RED=$red SANDBOX_CELLS=$sbCells SANDBOX_PASS=$sbPass SANDBOX_RED=$sbRed"
  return @($cells, $pass, $red, $sbCells, $sbPass, $sbRed)
}

# --- plant the wedge -------------------------------------------------------------
Get-ChildItem -Path $dir -File -ErrorAction SilentlyContinue | Remove-Item -Force
if (Test-Path $quar) { Remove-Item -Recurse -Force $quar }
$wedgeOk = $false
if (Test-Path (Join-Path $bak $stale)) {
  Copy-Item (Join-Path $bak $stale) -Destination $dir -Force
  $wedgeOk = Test-Path (Join-Path $dir $stale)
}
L "WEDGE_INSTALLED=$wedgeOk"
L ("WEDGE_SHA256=" + (Get-FileHash -Algorithm SHA256 -Path (Join-Path $dir $stale) -ErrorAction SilentlyContinue).Hash)
LeaseCounts 'after-plant' | Out-Null

# --- A: PRE-repair binary, wedge present ------------------------------------------
$obsA = CaptureActiveness 'A-pre' $PreExe
$a = RunMatrix 'A-pre' $PreExe $PreCommit $PreTree $NonceA "$root\act-A-pre.json"
$ca = LeaseCounts 'after-A'

# The wedge must SURVIVE the pre-repair run — permanence is the finding, and if it were
# cleared here the B leg would be measuring a clean directory instead of a wedged one.
$wedgeStillThere = Test-Path (Join-Path $dir $stale)
L "WEDGE_SURVIVED_PRE=$wedgeStillThere"

# --- B: POST-repair binary, same wedge --------------------------------------------
$obsB = CaptureActiveness 'B-post' $PostExe
$b = RunMatrix 'B-post' $PostExe $PostCommit $PostTree $NonceB "$root\act-B-post.json"
$cb = LeaseCounts 'after-B'
$wedgeReclaimed = -not (Test-Path (Join-Path $dir $stale))
L "WEDGE_RECLAIMED_BY_POST=$wedgeReclaimed"

# --- restore ----------------------------------------------------------------------
Get-ChildItem -Path $dir -File -ErrorAction SilentlyContinue | Remove-Item -Force
if (Test-Path $quar) { Remove-Item -Recurse -Force $quar }
foreach ($f in (Get-ChildItem -Path $restore -File -ErrorAction SilentlyContinue)) {
  Copy-Item $f.FullName -Destination $dir -Force
}
$after = (Get-ChildItem -Path $dir -File -ErrorAction SilentlyContinue | Measure-Object).Count
$afterQ = (Get-ChildItem -Path $quar -File -ErrorAction SilentlyContinue | Measure-Object).Count
L "LEASE_RESTORED count=$after expected=$($asFound.Count) quarantine=$afterQ"

$rc = 0
if (-not $wedgeOk) { L 'WEDGE_NOT_INSTALLED'; $rc = 8 }
if ($after -ne $asFound.Count -or $afterQ -ne 0) { L 'LEASE_NOT_RESTORED'; $rc = 7 }
L ("finished=" + (Get-Date -Format o))
L "EXIT=$rc"

Set-Content -Path $status -Value "WLRC=${rc}" -Encoding utf8
Add-Content -Path $status -Value "WLOBSA=${obsA}"                 -Encoding utf8
Add-Content -Path $status -Value "WLOBSB=${obsB}"                 -Encoding utf8
Add-Content -Path $status -Value "WLACELLS=$($a[0])"              -Encoding utf8
Add-Content -Path $status -Value "WLAPASS=$($a[1])"               -Encoding utf8
Add-Content -Path $status -Value "WLARED=$($a[2])"                -Encoding utf8
Add-Content -Path $status -Value "WLASBPASS=$($a[4])"             -Encoding utf8
Add-Content -Path $status -Value "WLASBRED=$($a[5])"              -Encoding utf8
Add-Content -Path $status -Value "WLBCELLS=$($b[0])"              -Encoding utf8
Add-Content -Path $status -Value "WLBPASS=$($b[1])"               -Encoding utf8
Add-Content -Path $status -Value "WLBRED=$($b[2])"                -Encoding utf8
Add-Content -Path $status -Value "WLBSBPASS=$($b[4])"             -Encoding utf8
Add-Content -Path $status -Value "WLBSBRED=$($b[5])"              -Encoding utf8
Add-Content -Path $status -Value "WLWEDGEOK=${wedgeOk}"           -Encoding utf8
Add-Content -Path $status -Value "WLWEDGESURVIVED=${wedgeStillThere}" -Encoding utf8
Add-Content -Path $status -Value "WLWEDGERECLAIMED=${wedgeReclaimed}" -Encoding utf8
Add-Content -Path $status -Value "WLRESTORED=${after}"            -Encoding utf8
Add-Content -Path $status -Value "WLDONE" -Encoding utf8
exit $rc
