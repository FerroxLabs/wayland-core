$ErrorActionPreference = 'Continue'
$log = 'C:\f28\win-matrix.log'
function L($m) { Add-Content -Path $log -Value $m -Encoding utf8 }
New-Item -ItemType Directory -Force -Path C:\f28 | Out-Null
Set-Content -Path $log -Value "F28 WINDOWS MATRIX LEG" -Encoding utf8
L ("started=" + (Get-Date -Format o))

$busy = (Get-Process -Name cargo,rustc,link -ErrorAction SilentlyContinue | Measure-Object).Count
L "QUIET_CHECK build_processes=$busy"
if ($busy -gt 0) { L "NOT_QUIET"; L "F28_TASK_EXIT=9"; exit 9 }

$exe = 'C:\f28\candidate-wayland-core.exe'
$wt  = 'C:\ferrox-win-p28'
$commit = '32e2f57d09fe4b287e513081862217dc9daa5901'
$tree   = '63ec0e6c36ff8e63789aab2f9760870304b671df'
$nonce  = $env:F28_NONCE

$h = (Get-FileHash -Algorithm SHA256 -Path $exe).Hash.ToLower()
L "BINARY=$exe"
L "BINARY_SHA256=$h"
if ($h -ne 'baf9bd692833eb7b9d54f00053739115b6ad5257fbdb0b0e99a8694a2ee996a6') {
  L "BINARY_DIGEST_MISMATCH: this is not the candidate artifact; the family's results would be void"
  L "F28_TASK_EXIT=6"; exit 6
}
L "BINARY_DIGEST_BOUND_TO_CANDIDATE_LEDGER"

Push-Location $wt
& git fetch -q origin lane/28-02 2>&1 | ForEach-Object { L $_ }
& git checkout -q FETCH_HEAD 2>&1 | ForEach-Object { L $_ }
L "HARNESS_SHA=$((& git rev-parse HEAD).Trim())"

$m = "$wt\.planning\phases\28-native-cross-platform-certification\evidence\28-01\matrix.tsv"
L "=== capture activeness ==="
& node "$wt\scripts\f28-native-matrix.mjs" --capture-activeness --bin $exe --out C:\f28\win-activeness.json 2>&1 | ForEach-Object { L $_ }
L "ACTIVENESS_RC=$LASTEXITCODE"

L "=== run matrix (windows) ==="
& node "$wt\scripts\f28-native-matrix.mjs" --run --bin $exe --os windows --commit $commit --tree $tree --nonce $nonce --matrix $m --activeness C:\f28\win-activeness.json --log C:\f28\win-matrix-markers.log --json C:\f28\win-matrix.json 2>&1 | ForEach-Object { L $_ }
$runRc = $LASTEXITCODE
L "RUN_RC=$runRc"

L "=== verify markers ==="
& node "$wt\scripts\f28-native-matrix.mjs" --verify C:\f28\win-matrix-markers.log --matrix $m --os windows --commit $commit --tree $tree --nonce $nonce 2>&1 | ForEach-Object { L $_ }
$verRc = $LASTEXITCODE
L "VERIFY_RC=$verRc"
Pop-Location

$busyEnd = (Get-Process -Name cargo,rustc,link -ErrorAction SilentlyContinue | Measure-Object).Count
L "QUIET_CHECK_END build_processes=$busyEnd"
L ("finished=" + (Get-Date -Format o))
$rc = 0
if ($runRc -ne 0) { $rc = 1 }
if ($verRc -ne 0) { $rc = 2 }
if ($busyEnd -gt 0) { $rc = 9 }
if ($rc -eq 0) { L "EXIT=0" } else { L "EXIT=$rc" }
L "F28_TASK_EXIT=$rc"
exit $rc
