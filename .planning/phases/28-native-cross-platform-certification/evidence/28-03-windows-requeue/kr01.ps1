# KR-01 -- wcore-sandbox::live_integrity::live_future_drop_reaps_descendant_job_tree
#
# 28-03 predicted this Windows descendant-process-tree reap defect would reproduce under soak
# conditions and force Criterion 2 NOT MET, then never tested it. This tests it.
#
# CONTEXT THAT CHANGES WHAT A REPRODUCTION MEANS: commit 2b662fe8 ("fix(sandbox): own and reap
# process trees", Jul 14) is ALREADY ANCESTRAL here -- it added BOTH this test AND the intended
# reap fix. So this is not "an untested known-red reproduces". If it reproduces, a fix landed
# and did not clear it.
#
# ---------------------------------------------------------------------------------
# SELF-PASSING GATE #1 -- the test's own env gate
# ---------------------------------------------------------------------------------
# The body's first statement is:
#     if std::env::var("WAYLAND_SANDBOX_LIVE_WINDOWS").is_err() { return; }
# so a plain `cargo test` PASSES WITHOUT RUNNING A LINE OF IT. A deliberate VACUOUS CONTROL is
# run first with the var unset: it must pass, and pass fast. That is what a vacuous green looks
# like, measured rather than imagined, and the real run is only readable if it differs.
#
# ---------------------------------------------------------------------------------
# FALSE-GREEN RISK #2 -- and it runs the OPPOSITE way to the soak's
# ---------------------------------------------------------------------------------
# The assertion is "the heartbeat file stopped growing". The heartbeat writes once a second via
# `choice.exe /t 1`, and the test samples it over two 2-second windows. Under heavy competing
# load a descendant that is ALIVE BUT STARVED could miss both windows and be scored as reaped.
# So for THIS leg load biases toward a false PASS, where for the soak it biased toward a false
# red. Two defences:
#   (a) competing load is sampled throughout and reported, and a quiet window is sought first;
#   (b) an INDEPENDENT witness that does not depend on file length at all -- `choice.exe` is
#       spawned only by the heartbeat loop, so a surviving choice.exe after the test is direct
#       evidence the descendant outlived its owner regardless of what the file did.

$ErrorActionPreference = 'Continue'
$src = 'C:\wl-winrequeue\src'
$log = 'C:\wl-winrequeue\out\kr01.log'
$loadFile = 'C:\wl-winrequeue\out\kr01-load.tsv'
$cargo = 'C:\Users\seand\.cargo\bin\cargo.exe'
function Log($m) { Add-Content -LiteralPath $log -Value $m -Encoding utf8 }
function BusyProcs { @(Get-Process -Name cargo,rustc,link,cl -ErrorAction SilentlyContinue) }
function ChoiceProcs { @(Get-Process -Name choice -ErrorAction SilentlyContinue) }
function StaleDirs { @(Get-ChildItem -Path $env:PUBLIC -Directory -Filter 'wcore-job-cancel-*' -ErrorAction SilentlyContinue) }

Set-Content -LiteralPath $log -Value "KR01_START $(Get-Date -Format o)" -Encoding utf8
Log "KR01_WHOAMI $(whoami)"
Log "KR01_CWD $((Get-Location).Path)"
Log "KR01_HEAD $(& git -C $src rev-parse HEAD)"
Log "KR01_TREE_wcore_sandbox $(& git -C $src rev-parse 'HEAD:crates/wcore-sandbox')"
Log "KR01_PUBLIC $env:PUBLIC"
if (-not (Test-Path -LiteralPath $cargo)) { Log 'KR01_EXIT=70 reason=cargo-missing'; Log 'KR01_MARKER_DONE'; exit 70 }

Set-Location $src

# ---- BUILD FIRST, separately, so build load is never inside the measurement window.
Log "KR01_BUILD_INVOKE $(Get-Date -Format o)"
& $cargo test -p wcore-sandbox --test live_integrity --no-run *>> $log
$buildRc = $LASTEXITCODE
Log "KR01_BUILD_RC=$buildRc"
if ($buildRc -ne 0) { Log 'KR01_EXIT=71 reason=build-failed'; Log 'KR01_MARKER_DONE'; exit 71 }

# ---- best-effort quiet window BEFORE measuring, bounded and recorded either way.
$deadline = (Get-Date).AddMinutes(25)
$clear = 0
while ((Get-Date) -lt $deadline) {
  $b = BusyProcs
  if ($b.Count -eq 0) { $clear += 1 } else { $clear = 0 }
  Log ("KR01_WAIT t={0} busy={1} consecutive_clear={2}" -f (Get-Date -Format o), $b.Count, $clear)
  if ($clear -ge 3) { break }
  Start-Sleep -Seconds 15
}
Log "KR01_QUIET_WINDOW $(if ($clear -ge 3) { 'yes' } else { 'no-proceeding-with-load-recorded' })"

Set-Content -LiteralPath $loadFile -Value "iso`tbusy_procs`tchoice_procs" -Encoding utf8
$sampler = Start-Job -ScriptBlock {
  param($f)
  for ($i = 0; $i -lt 2000; $i++) {
    $b = @(Get-Process -Name cargo,rustc,link,cl -ErrorAction SilentlyContinue).Count
    $c = @(Get-Process -Name choice -ErrorAction SilentlyContinue).Count
    Add-Content -LiteralPath $f -Value ("{0}`t{1}`t{2}" -f (Get-Date -Format o), $b, $c)
    Start-Sleep -Seconds 2
  }
} -ArgumentList $loadFile

Log "KR01_CHOICE_BEFORE $((ChoiceProcs).Count)"
Log "KR01_WORKDIRS_BEFORE $((StaleDirs).Count)"
foreach ($d in StaleDirs) { Log "KR01_STALE_BEFORE $($d.Name)" }

# ---- WITNESS 1: the deliberate VACUOUS control, env var UNSET.
Remove-Item Env:\WAYLAND_SANDBOX_LIVE_WINDOWS -ErrorAction SilentlyContinue
Log "KR01_VACUOUS_INVOKE $(Get-Date -Format o)"
$sw = [System.Diagnostics.Stopwatch]::StartNew()
& $cargo test -p wcore-sandbox --test live_integrity live_future_drop_reaps_descendant_job_tree -- --exact --nocapture *>> $log
$vacRc = $LASTEXITCODE
$sw.Stop()
Log "KR01_VACUOUS_RC=$vacRc"
Log "KR01_VACUOUS_SECONDS=$([math]::Round($sw.Elapsed.TotalSeconds,2))"

# ---- WITNESS 2/3: the REAL run, env var SET.
$env:WAYLAND_SANDBOX_LIVE_WINDOWS = '1'
Log "KR01_LIVE_INVOKE $(Get-Date -Format o)"
$sw2 = [System.Diagnostics.Stopwatch]::StartNew()
& $cargo test -p wcore-sandbox --test live_integrity live_future_drop_reaps_descendant_job_tree -- --exact --nocapture *>> $log
$liveRc = $LASTEXITCODE
$sw2.Stop()
Log "KR01_LIVE_RC=$liveRc"
Log "KR01_LIVE_SECONDS=$([math]::Round($sw2.Elapsed.TotalSeconds,2))"

# ---- WITNESS 4: the independent survivor witness, taken IMMEDIATELY.
$choiceAfter = ChoiceProcs
Log "KR01_CHOICE_AFTER $($choiceAfter.Count)"
foreach ($c in $choiceAfter) { Log "KR01_CHOICE_SURVIVOR pid=$($c.Id) start=$($c.StartTime)" }

$after = StaleDirs
Log "KR01_WORKDIRS_AFTER $($after.Count)"
foreach ($d in $after) {
  Log "KR01_WORKDIR_AFTER $($d.Name)"
  $hb = Join-Path $d.FullName 'heartbeat.txt'
  if (Test-Path -LiteralPath $hb) {
    $len1 = (Get-Item -LiteralPath $hb).Length
    Start-Sleep -Seconds 4
    $len2 = (Get-Item -LiteralPath $hb).Length
    Log "KR01_HEARTBEAT_AFTER name=$($d.Name) len_t0=$len1 len_t4=$len2 still_advancing=$($len2 -gt $len1)"
  }
}

Stop-Job $sampler -ErrorAction SilentlyContinue
Remove-Job $sampler -Force -ErrorAction SilentlyContinue
$rows = @(Get-Content -LiteralPath $loadFile | Select-Object -Skip 1)
$counts = @($rows | ForEach-Object { [int](($_ -split "`t")[1]) })
if ($counts.Count -gt 0) {
  $st = $counts | Measure-Object -Minimum -Maximum -Average
  Log ("KR01_LOAD_SAMPLES {0} MIN {1} MAX {2} MEAN {3}" -f $counts.Count, $st.Minimum, $st.Maximum, [math]::Round($st.Average,2))
}

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
