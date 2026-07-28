# 28-E5-REPAIR — the E5 Windows leg, re-run against a candidate that CONTAINS the
# F-28-02-002 stale-AppContainer-lease repair (15821c03).
#
# This is a deliberate reproduction of `evidence/28-02/f28-win-matrix.ps1` — same three
# node invocations, same matrix.tsv, same --os windows filter, same quiet check, same
# binary-digest assertion — so the result is COMPARABLE with the 219 pass / 0 red row in
# 28-02-MATRIX-RESULTS.md. It is not a fresh bespoke run.
#
# Two changes, both forced and both stated:
#   1. the expected binary digest and the commit/tree bind to THIS candidate, not 32e2f57d;
#   2. exit status is written to a status FILE (WLRC first, WLDONE last) and read back by a
#      SEPARATE ssh call. Over ssh+PowerShell every non-zero exit collapses to 1, so the
#      caller cannot otherwise distinguish "matrix red" (rc=1) from "verifier red" (rc=2)
#      from "box not quiet" (rc=9).
param(
  [Parameter(Mandatory=$true)][string]$Commit,
  [Parameter(Mandatory=$true)][string]$Tree,
  [Parameter(Mandatory=$true)][string]$Nonce,
  [Parameter(Mandatory=$true)][string]$ExpectedSha256
)

$ErrorActionPreference = 'Continue'
$root   = 'C:\f28e5'
$log    = "$root\win-matrix.log"
$status = "$root\win-matrix.status"
$exe    = "$root\candidate-wayland-core.exe"
$mjs    = "$root\f28-native-matrix.mjs"
$mtsv   = "$root\matrix.tsv"

New-Item -ItemType Directory -Force -Path $root | Out-Null
Remove-Item -Force -ErrorAction SilentlyContinue $status
Set-Content -Path $log -Value "F28E5 WINDOWS MATRIX LEG (post-repair candidate)" -Encoding utf8
function L($m) { Add-Content -Path $log -Value $m -Encoding utf8 }
L ("started=" + (Get-Date -Format o))
L ("WHOAMI=" + (& whoami))
L ("COMMIT=$Commit")
L ("TREE=$Tree")
L ("NONCE=$Nonce")

# Quiet check. NOTE: this box is ALSO the self-hosted Windows CI runner, so a lane that
# pushes a branch makes its own certification host non-quiet. Measured this lane.
$busy = (Get-Process -Name cargo,rustc,link -ErrorAction SilentlyContinue | Measure-Object).Count
L "QUIET_CHECK build_processes=$busy"
if ($busy -gt 0) {
  L "NOT_QUIET"
  Set-Content -Path $status -Value "WLRC=9" -Encoding utf8
  Add-Content -Path $status -Value "WLBUSY=${busy}" -Encoding utf8
  Add-Content -Path $status -Value "WLDONE" -Encoding utf8
  exit 9
}

foreach ($p in @($exe, $mjs, $mtsv)) {
  if (-not (Test-Path $p)) {
    L "MISSING_INPUT=$p"
    Set-Content -Path $status -Value "WLRC=7" -Encoding utf8
    Add-Content -Path $status -Value "WLMISSING=${p}" -Encoding utf8
    Add-Content -Path $status -Value "WLDONE" -Encoding utf8
    exit 7
  }
}

$h = (Get-FileHash -Algorithm SHA256 -Path $exe).Hash.ToLower()
L "BINARY=$exe"
L "BINARY_SHA256=$h"
if ($h -ne $ExpectedSha256.ToLower()) {
  L "BINARY_DIGEST_MISMATCH: this is not the candidate artifact; the family's results would be void"
  Set-Content -Path $status -Value "WLRC=6" -Encoding utf8
  Add-Content -Path $status -Value "WLSHA=${h}" -Encoding utf8
  Add-Content -Path $status -Value "WLDONE" -Encoding utf8
  exit 6
}
L "BINARY_DIGEST_BOUND_TO_CANDIDATE_LEDGER"
L ("HARNESS_SHA256=" + (Get-FileHash -Algorithm SHA256 -Path $mjs).Hash.ToLower())
L ("MATRIX_SHA256="  + (Get-FileHash -Algorithm SHA256 -Path $mtsv).Hash.ToLower())

# Lease state BEFORE the run. The F-28-02-002 wedge is on-disk state, so this is the
# variable that decides what the run proves.
$ld = Join-Path $env:LOCALAPPDATA 'Wayland\Core\AppContainerLeases\v1'
$pre = 0; $preq = 0
if (Test-Path $ld) {
  $pre  = (Get-ChildItem -Force -File $ld -ErrorAction SilentlyContinue | Measure-Object).Count
  $qd = Join-Path $ld 'quarantine'
  if (Test-Path $qd) { $preq = (Get-ChildItem -Force -File $qd -ErrorAction SilentlyContinue | Measure-Object).Count }
}
L "LEASES_BEFORE active=$pre quarantined=$preq"

L "=== capture activeness ==="
& node $mjs --capture-activeness --bin $exe --out "$root\win-activeness.json" 2>&1 | ForEach-Object { L $_ }
$actRc = $LASTEXITCODE
L "ACTIVENESS_RC=$actRc"

L "=== run matrix (windows) ==="
& node $mjs --run --bin $exe --os windows --commit $Commit --tree $Tree --nonce $Nonce --matrix $mtsv --activeness "$root\win-activeness.json" --log "$root\win-matrix-markers.log" --json "$root\win-matrix.json" 2>&1 | ForEach-Object { L $_ }
$runRc = $LASTEXITCODE
L "RUN_RC=$runRc"

L "=== verify markers ==="
& node $mjs --verify "$root\win-matrix-markers.log" --matrix $mtsv --os windows --commit $Commit --tree $Tree --nonce $Nonce 2>&1 | ForEach-Object { L $_ }
$verRc = $LASTEXITCODE
L "VERIFY_RC=$verRc"

$post = 0; $postq = 0
if (Test-Path $ld) {
  $post = (Get-ChildItem -Force -File $ld -ErrorAction SilentlyContinue | Measure-Object).Count
  $qd = Join-Path $ld 'quarantine'
  if (Test-Path $qd) { $postq = (Get-ChildItem -Force -File $qd -ErrorAction SilentlyContinue | Measure-Object).Count }
}
L "LEASES_AFTER active=$post quarantined=$postq"

$busyEnd = (Get-Process -Name cargo,rustc,link -ErrorAction SilentlyContinue | Measure-Object).Count
L "QUIET_CHECK_END build_processes=$busyEnd"
L ("finished=" + (Get-Date -Format o))

# Cell counts, read back from the JSON rather than inferred from exit status. A suite that
# exits 0 having run ZERO cells is the known self-passing shape this program keeps measuring.
$cells = 0; $pass = 0; $red = 0; $skip = 0; $sbCells = 0; $sbPass = 0; $sbRed = 0
if (Test-Path "$root\win-matrix.json") {
  $j = Get-Content -Raw "$root\win-matrix.json" | ConvertFrom-Json
  $cells = ($j | Measure-Object).Count
  $pass  = ($j | Where-Object { $_.outcome -eq 'pass' } | Measure-Object).Count
  $red   = ($j | Where-Object { $_.outcome -eq 'red'  } | Measure-Object).Count
  $skip  = ($j | Where-Object { $_.outcome -eq 'skip' } | Measure-Object).Count
  $sb    = $j | Where-Object { $_.dimension -eq 'sandbox-probes' }
  $sbCells = ($sb | Measure-Object).Count
  $sbPass  = ($sb | Where-Object { $_.outcome -eq 'pass' } | Measure-Object).Count
  $sbRed   = ($sb | Where-Object { $_.outcome -eq 'red'  } | Measure-Object).Count
}
L "CELLS=$cells PASS=$pass RED=$red SKIP=$skip"
L "SANDBOX_CELLS=$sbCells SANDBOX_PASS=$sbPass SANDBOX_RED=$sbRed"

$rc = 0
if ($actRc -ne 0)   { $rc = 3 }
if ($runRc -ne 0)   { $rc = 1 }
if ($verRc -ne 0)   { $rc = 2 }
if ($cells -eq 0)   { $rc = 4 }   # zero cells executed is a FAILURE, not a pass
if ($busyEnd -gt 0) { $rc = 9 }
L "EXIT=$rc"

# Brace every variable in a sentinel: "$rc:TAG" renders EMPTY, because PowerShell reads
# `$VAR:` as namespace notation.
Set-Content -Path $status -Value "WLRC=${rc}" -Encoding utf8
Add-Content -Path $status -Value "WLCELLS=${cells}"     -Encoding utf8
Add-Content -Path $status -Value "WLPASS=${pass}"       -Encoding utf8
Add-Content -Path $status -Value "WLRED=${red}"         -Encoding utf8
Add-Content -Path $status -Value "WLSKIP=${skip}"       -Encoding utf8
Add-Content -Path $status -Value "WLSBCELLS=${sbCells}" -Encoding utf8
Add-Content -Path $status -Value "WLSBPASS=${sbPass}"   -Encoding utf8
Add-Content -Path $status -Value "WLSBRED=${sbRed}"     -Encoding utf8
Add-Content -Path $status -Value "WLACTRC=${actRc}"     -Encoding utf8
Add-Content -Path $status -Value "WLRUNRC=${runRc}"     -Encoding utf8
Add-Content -Path $status -Value "WLVERRC=${verRc}"     -Encoding utf8
Add-Content -Path $status -Value "WLLEASEPRE=${pre}"    -Encoding utf8
Add-Content -Path $status -Value "WLLEASEPOST=${post}"  -Encoding utf8
Add-Content -Path $status -Value "WLQPRE=${preq}"       -Encoding utf8
Add-Content -Path $status -Value "WLQPOST=${postq}"     -Encoding utf8
Add-Content -Path $status -Value "WLDONE" -Encoding utf8
exit $rc
