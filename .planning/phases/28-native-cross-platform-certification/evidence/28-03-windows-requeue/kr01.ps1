# KR-01 -- wcore-sandbox::live_integrity::live_future_drop_reaps_descendant_job_tree
#
# 28-03 predicted this Windows descendant-process-tree reap defect would reproduce under
# soak conditions and force Criterion 2 NOT MET, then never tested it. This runs it.
#
# THE SELF-PASSING GATE THIS FILE EXISTS TO DEFEAT
# ------------------------------------------------
# The test's first statement is:
#     if std::env::var("WAYLAND_SANDBOX_LIVE_WINDOWS").is_err() { return; }
# so a plain `cargo test` PASSES IT WITHOUT RUNNING A LINE OF IT. A green from that is
# indistinguishable from a green from a working reap. Three independent non-vacuity
# witnesses are therefore recorded, and the run is only readable if they agree:
#
#   1. A DELIBERATE VACUOUS CONTROL: the same test, same binary, env var UNSET. It must
#      pass, and it must pass FAST. That is what a vacuous green looks like, measured
#      rather than imagined.
#   2. WALL CLOCK. The real body sleeps 10s (heartbeat wait) + 2s + 2s before it can
#      assert. A run under ~5s did not execute the body no matter what it reports.
#   3. THE WORK DIRECTORY. The body creates $PUBLIC\wcore-job-cancel-<pid>-<nanos>. It is
#      removed only on the SUCCESS path, so its presence after a failure is positive
#      evidence the body ran, and its absence before the run rules out a stale hit.

$ErrorActionPreference = 'Continue'
$src = 'C:\wl-winrequeue\src'
$log = 'C:\wl-winrequeue\out\kr01.log'
function Log($m) { Add-Content -LiteralPath $log -Value $m -Encoding utf8 }

Set-Content -LiteralPath $log -Value "KR01_START $(Get-Date -Format o)" -Encoding utf8
Log "KR01_WHOAMI $(whoami)"
Log "KR01_CWD $((Get-Location).Path)"
Log "KR01_HEAD $(& git -C $src rev-parse HEAD)"
Log "KR01_TREE_wcore_sandbox $(& git -C $src rev-parse 'HEAD:crates/wcore-sandbox')"
Log "KR01_PUBLIC $env:PUBLIC"

$cargo = 'C:\Users\seand\.cargo\bin\cargo.exe'
if (-not (Test-Path -LiteralPath $cargo)) { Log 'KR01_EXIT=70 reason=cargo-missing'; Log 'KR01_MARKER_DONE'; exit 70 }

function StaleDirs { @(Get-ChildItem -Path $env:PUBLIC -Directory -Filter 'wcore-job-cancel-*' -ErrorAction SilentlyContinue) }

Log "KR01_WORKDIRS_BEFORE $((StaleDirs).Count)"
foreach ($d in StaleDirs) { Log "KR01_STALE_BEFORE $($d.Name)" }

Set-Location $src

# ---------------------------------------------------------------------------------
# WITNESS 1 -- the deliberate VACUOUS control, env var UNSET.
# ---------------------------------------------------------------------------------
Remove-Item Env:\WAYLAND_SANDBOX_LIVE_WINDOWS -ErrorAction SilentlyContinue
Log "KR01_VACUOUS_INVOKE $(Get-Date -Format o)"
$sw = [System.Diagnostics.Stopwatch]::StartNew()
& $cargo test -p wcore-sandbox --test live_integrity live_future_drop_reaps_descendant_job_tree -- --exact --nocapture *>> $log
$vacRc = $LASTEXITCODE
$sw.Stop()
Log "KR01_VACUOUS_RC=$vacRc"
Log "KR01_VACUOUS_SECONDS=$([math]::Round($sw.Elapsed.TotalSeconds,2))"

# ---------------------------------------------------------------------------------
# WITNESS 2/3 -- the REAL run, env var SET.
# ---------------------------------------------------------------------------------
$env:WAYLAND_SANDBOX_LIVE_WINDOWS = '1'
Log "KR01_LIVE_INVOKE $(Get-Date -Format o)"
$sw2 = [System.Diagnostics.Stopwatch]::StartNew()
& $cargo test -p wcore-sandbox --test live_integrity live_future_drop_reaps_descendant_job_tree -- --exact --nocapture *>> $log
$liveRc = $LASTEXITCODE
$sw2.Stop()
Log "KR01_LIVE_RC=$liveRc"
Log "KR01_LIVE_SECONDS=$([math]::Round($sw2.Elapsed.TotalSeconds,2))"

$after = StaleDirs
Log "KR01_WORKDIRS_AFTER $($after.Count)"
foreach ($d in $after) {
  Log "KR01_WORKDIR_AFTER $($d.Name)"
  $hb = Join-Path $d.FullName 'heartbeat.txt'
  if (Test-Path -LiteralPath $hb) {
    $len1 = (Get-Item -LiteralPath $hb).Length
    Start-Sleep -Seconds 3
    $len2 = (Get-Item -LiteralPath $hb).Length
    # A descendant STILL advancing after the harness exited is the defect itself, observed
    # directly rather than inferred from the assertion message.
    Log "KR01_HEARTBEAT_AFTER name=$($d.Name) len_t0=$len1 len_t3=$len2 still_advancing=$($len2 -gt $len1)"
  }
}

# Verdict is derived from the witnesses TOGETHER, never from the live rc alone.
$bodyRan = ($sw2.Elapsed.TotalSeconds -ge 5)
Log "KR01_BODY_RAN=$bodyRan"
if (-not $bodyRan) {
  Log 'KR01_VERDICT=UNREADABLE reason=live-run-too-fast-to-have-executed-the-body'
} elseif ($liveRc -eq 0) {
  Log 'KR01_VERDICT=DID_NOT_REPRODUCE'
} else {
  Log 'KR01_VERDICT=REPRODUCED'
}
Log "KR01_EXIT=$liveRc"
Log 'KR01_MARKER_DONE'
exit 0
