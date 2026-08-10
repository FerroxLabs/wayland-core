# DEFECT B-1 — Windows leg, round 2.
#
# Windows kill semantics differ and that is the point of running here:
#   * taskkill /T /F walks the child tree, it is not a POSIX process group
#   * there is no signal/exit-code distinction, so a killed worker and a worker
#     that chose to fail are the same integer
#   * there is no filesystem containment around the worker
$ErrorActionPreference = 'Continue'
$BIN  = 'D:\b1\target\release\wayland-core.exe'
$ROOT = 'D:\b1-live'
# Every effect namespace is GOAL-scoped now -- effects/<goal>/<key>,
# intents/<goal>/<key>, observed/<goal>/<task>/<invocation> -- so the counts
# below recurse and every exec-task names the Goal it runs under.
# NOT $GOAL: PowerShell variable names are case-insensitive, so a $goal
# parameter on ExecTask silently IS this variable and shadows it to $null.
$DEFGOAL = 'g-b1-bnd'
if (Test-Path $ROOT) { Remove-Item -Recurse -Force $ROOT }
New-Item -ItemType Directory -Force -Path $ROOT | Out-Null

function Say($m) { Write-Output ''; Write-Output "=== $m ===" }
function Census($d) { (& $BIN goal effects --effects-dir $d 2>$null | Select-String 'GOAL-EFFECTS').Line }
function OnDisk($d) {
  $i = @(Get-ChildItem (Join-Path $d 'intents') -Recurse -File -ErrorAction SilentlyContinue).Name -join ' '
  $c = @(Get-ChildItem (Join-Path $d 'effects') -Recurse -File -ErrorAction SilentlyContinue).Name -join ' '
  $o = @(Get-ChildItem (Join-Path $d 'observed') -Recurse -File -ErrorAction SilentlyContinue).Count
  Write-Output "    on-disk: intents=[$i] commits=[$c] observed=$o"
}
function IntentCount($d) { @(Get-ChildItem (Join-Path $d 'intents') -Recurse -File -ErrorAction SilentlyContinue).Count }
function ObservedCount($d) { @(Get-ChildItem (Join-Path $d 'observed') -Recurse -File -ErrorAction SilentlyContinue).Count }
function Gate($d, $n) {
  & $BIN goal effects --effects-dir $d --expect $n 2>&1 | Out-Null
  return $LASTEXITCODE
}

@'
@echo off
if not exist "%WAYLAND_GOAL_EFFECT_SINK%" mkdir "%WAYLAND_GOAL_EFFECT_SINK%"
(echo %WAYLAND_GOAL_TASK%)>"%WAYLAND_GOAL_EFFECT_SINK%\%WAYLAND_GOAL_TASK%.%RANDOM%.%RANDOM%.%TIME:~6,5%"
ping -n 31 127.0.0.1 >nul
'@ | Set-Content -Encoding ASCII "$ROOT\w-early.cmd"

@'
@echo off
if not exist "%WAYLAND_GOAL_EFFECT_SINK%" mkdir "%WAYLAND_GOAL_EFFECT_SINK%"
(echo %WAYLAND_GOAL_TASK%)>"%WAYLAND_GOAL_EFFECT_SINK%\%WAYLAND_GOAL_TASK%.%RANDOM%.%RANDOM%.%TIME:~6,5%"
'@ | Set-Content -Encoding ASCII "$ROOT\w-fast.cmd"

# What a KILLED worker looks like from outside on Windows: the effect landed,
# then a nonzero exit that says nothing about it.
@'
@echo off
if not exist "%WAYLAND_GOAL_EFFECT_SINK%" mkdir "%WAYLAND_GOAL_EFFECT_SINK%"
(echo %WAYLAND_GOAL_TASK%)>"%WAYLAND_GOAL_EFFECT_SINK%\%WAYLAND_GOAL_TASK%.%RANDOM%.%RANDOM%.%TIME:~6,5%"
exit /b 1
'@ | Set-Content -Encoding ASCII "$ROOT\w-killed.cmd"

# A worker that DECLARES nothing landed. Both halves are required now: the
# out-of-band receipt AND the exit code, because an exit code on its own is a
# value other tools emit by accident.
@'
@echo off
(echo no-effect)>"%WAYLAND_GOAL_NO_EFFECT_RECEIPT%"
exit /b 91
'@ | Set-Content -Encoding ASCII "$ROOT\w-noeffect.cmd"

# The collision case: the SAME exit code with NO receipt. A worker whose
# pipeline emits it by accident must PARK, never have its intent withdrawn.
@'
@echo off
if not exist "%WAYLAND_GOAL_EFFECT_SINK%" mkdir "%WAYLAND_GOAL_EFFECT_SINK%"
(echo %WAYLAND_GOAL_TASK%)>"%WAYLAND_GOAL_EFFECT_SINK%\%WAYLAND_GOAL_TASK%.%RANDOM%.%RANDOM%.%TIME:~6,5%"
exit /b 91
'@ | Set-Content -Encoding ASCII "$ROOT\w-collide.cmd"

# A worker whose record carries an INVOCATION identity, the way a real effect
# log does. Two runs leave two byte-different records.
@'
@echo off
if not exist "%WAYLAND_GOAL_EFFECT_SINK%" mkdir "%WAYLAND_GOAL_EFFECT_SINK%"
(echo task=%WAYLAND_GOAL_TASK% msg_id=%RANDOM%%RANDOM%%TIME:~6,5%)>"%WAYLAND_GOAL_EFFECT_SINK%\r.%RANDOM%.%RANDOM%.%TIME:~6,5%"
'@ | Set-Content -Encoding ASCII "$ROOT\w-ident.cmd"

function ExecTask($d, $t, $k, $worker, $extra, $g) {
  if (-not $g) { $g = $DEFGOAL }
  $env:WAYLAND_GOAL_ID = $g
  $env:WAYLAND_GOAL_TASK = $t
  $env:WAYLAND_GOAL_IDEMPOTENCY_KEY = $k
  $env:API_KEY = ''; $env:FLUX_API_KEY = ''
  $a = @('goal','exec-task','--effects-dir',$d)
  if ($extra) { $a += $extra }
  $a += @('--','cmd','/c',$worker)
  & $BIN @a 2>&1 | Where-Object { $_ -notmatch 'crash sentinel' } | ForEach-Object { Write-Output ("      " + $_) }
  return $LASTEXITCODE
}

Say 'BINARY'
& $BIN --version
Write-Output ((Get-FileHash $BIN -Algorithm SHA256).Hash.ToLower())
Write-Output ("other wayland-core processes running: " + @(Get-Process wayland-core -ErrorAction SilentlyContinue).Count)

# ------------------------------------------------------------------ W2 -------
Say 'W2  taskkill /T /F AFTER the effect, worker still running'
$D = "$ROOT\w2"; New-Item -ItemType Directory -Force -Path $D | Out-Null
$env:WAYLAND_GOAL_ID = $DEFGOAL; $env:WAYLAND_GOAL_TASK = 't-w2'; $env:WAYLAND_GOAL_IDEMPOTENCY_KEY = 'idem-w2'
$p = Start-Process -FilePath $BIN -PassThru -WindowStyle Hidden `
     -ArgumentList @('goal','exec-task','--effects-dir',$D,'--','cmd','/c',"$ROOT\w-early.cmd")
Start-Sleep -Seconds 3
Write-Output "    taskkill /T /F on pid $($p.Id)"
taskkill /T /F /PID $p.Id 2>&1 | Out-Null
Start-Sleep -Seconds 2
OnDisk $D; Census $D
Write-Output '  -- the honest retry (this is where it used to duplicate):'
$rc = ExecTask $D 't-w2' 'idem-w2' "$ROOT\w-fast.cmd" $null
Write-Output "    RETRY_EXIT=$rc  (90 = parked)"
OnDisk $D; Census $D
Write-Output '  -- operator checks the sink, finds the effect, resolves as produced:'
$rc = ExecTask $D 't-w2' 'idem-w2' "$ROOT\w-fast.cmd" @('--resolve','produced')
Write-Output "    EXIT=$rc"
OnDisk $D; Census $D
Write-Output ("  W2_GATE_EXIT=" + (Gate $D 1) + " (0 = exactly one real effect)")

# ------------------------------------------------------------------ W3 -------
Say 'W3  worker died AFTER its effect with an undeclared nonzero exit'
Write-Output '  This is the leg the FIRST version of the fix failed on Windows:'
Write-Output '  it read exit 1 as "no effect landed", withdrew the intent, and the'
Write-Output '  retry duplicated. taskkill /F and exit 1 are the same integer here.'
$D = "$ROOT\w3"; New-Item -ItemType Directory -Force -Path $D | Out-Null
$rc = ExecTask $D 't-w3' 'idem-w3' "$ROOT\w-killed.cmd" $null
Write-Output "    FIRST_EXIT=$rc  (90 = parked, NOT retried)"
OnDisk $D; Census $D
Write-Output '  -- the honest retry:'
$rc = ExecTask $D 't-w3' 'idem-w3' "$ROOT\w-fast.cmd" $null
Write-Output "    RETRY_EXIT=$rc  (90 = parked)"
OnDisk $D; Census $D
Write-Output ("  W3_GATE_EXIT=" + (Gate $D 1) + " (0 = exactly one real effect)")

# ------------------------------------------------------------------ W3b ------
Say 'W3b  worker DECLARED no effect (receipt + exit 91): plainly retryable, not parked'
$D = "$ROOT\w3b"; New-Item -ItemType Directory -Force -Path $D | Out-Null
$rc = ExecTask $D 't-w3b' 'idem-w3b' "$ROOT\w-noeffect.cmd" $null
Write-Output "    FIRST_EXIT=$rc  (1 = a plain failure)"
OnDisk $D
$rc = ExecTask $D 't-w3b' 'idem-w3b' "$ROOT\w-fast.cmd" $null
Write-Output "    RETRY_EXIT=$rc  (0 = it ran, as it must)"
OnDisk $D; Census $D
Write-Output ("  W3b_GATE_EXIT=" + (Gate $D 1) + " (0 = exactly one real effect)")

# ------------------------------------------------------------------ W5 -------
Say 'W5  the clean path'
$D = "$ROOT\w5"; New-Item -ItemType Directory -Force -Path $D | Out-Null
ExecTask $D 't-w5' 'idem-w5' "$ROOT\w-fast.cmd" $null | Out-Null
ExecTask $D 't-w5' 'idem-w5' "$ROOT\w-fast.cmd" $null | Out-Null
OnDisk $D; Census $D
Write-Output ("  W5_GATE_EXIT=" + (Gate $D 1) + " (0 = exactly one real effect)")

# ------------------------------------------------------------------- P -------
Say 'P  INSTRUMENT POSITIVE CONTROL ON WINDOWS'
$D = "$ROOT\p"; New-Item -ItemType Directory -Force -Path $D | Out-Null
ExecTask $D 't-p' 'idem-p' "$ROOT\w-killed.cmd" $null | Out-Null
ExecTask $D 't-p' 'idem-p' "$ROOT\w-fast.cmd" @('--resolve','retry') | Out-Null
Census $D
Write-Output ("  P_GATE_EXIT=" + (Gate $D 1) + " (NONZERO is the PASS: the counter caught a real duplicate)")

# ------------------------------------------------------------------- C -------
Say 'C  FULL FLEET: taskkill /T /F mid-wave, then restart'
$C = "$ROOT\c"; New-Item -ItemType Directory -Force -Path $C | Out-Null
$J = "$ROOT\fleet.journal"; $G = 'g-b1'
@'
@echo off
if not exist "%WAYLAND_GOAL_EFFECT_SINK%" mkdir "%WAYLAND_GOAL_EFFECT_SINK%"
(echo %WAYLAND_GOAL_TASK%)>"%WAYLAND_GOAL_EFFECT_SINK%\%WAYLAND_GOAL_TASK%.%RANDOM%.%RANDOM%.%TIME:~6,5%"
if "%WAYLAND_GOAL_TASK%"=="t04" ping -n 41 127.0.0.1 >nul
if "%WAYLAND_GOAL_TASK%"=="t05" ping -n 41 127.0.0.1 >nul
if "%WAYLAND_GOAL_TASK%"=="t02" ping -n 5 127.0.0.1 >nul
if "%WAYLAND_GOAL_TASK%"=="t03" ping -n 5 127.0.0.1 >nul
ping -n 2 127.0.0.1 >nul
'@ | Set-Content -Encoding ASCII "$ROOT\worker.cmd"

& $BIN goal open --journal $J --goal $G --objective 'prove exactly-once survives a real kill' --iterations 8 --max-tokens 10000 | Out-Null
foreach ($i in 0..5) { & $BIN goal task --journal $J --goal $G --task "t0$i" | Out-Null }
foreach ($i in 0..5) { & $BIN goal task --journal $J --goal $G --task ("t0" + ($i+6)) --depends-on "t0$i" | Out-Null }

# Launched through cmd.exe so the whitespace-bearing --worker-command survives
# as ONE argument; Start-Process -ArgumentList joins an array unquoted, which
# silently split it last time and made this leg vacuous.
# cmd.exe strips the outer quote pair off a /c string that begins and ends
# with one, which silently produced an EMPTY run1.log and a vacuous kill leg:
# the wait loop timed out at zero observed and the taskkill hit a process that
# had never started. A .cmd FILE has nothing to survive two levels of quoting.
$runner = @(
  '@echo off',
  ('"' + $BIN + '" goal run --journal "' + $J + '" --goal ' + $G +
   ' --effects-dir "' + $C + '" --worker-command "cmd /c ' + $ROOT + '\worker.cmd"' +
   ' --width 6 --shard-size 2 --lease 5s > "' + $ROOT + '\run1.log" 2>&1')
)
Set-Content -Encoding ASCII "$ROOT\run1.cmd" $runner
$run1 = Start-Process -FilePath 'cmd.exe' -PassThru -WindowStyle Hidden -ArgumentList @('/c', "$ROOT\run1.cmd")
for ($n=0; $n -lt 240; $n++) {
  if ((ObservedCount $C) -ge 6) { break }
  Start-Sleep -Milliseconds 250
}
Write-Output ("PRE-KILL observed=" + (ObservedCount $C) +
              " commits=" + @(Get-ChildItem "$C\effects" -Recurse -File -ErrorAction SilentlyContinue).Count +
              " intents=" + (IntentCount $C) +
              " engines=" + @(Get-Process wayland-core -ErrorAction SilentlyContinue).Count)
if ((ObservedCount $C) -eq 0) { Write-Output '  C_VACUOUS=TRUE  (nothing ran before the kill; this leg certifies NOTHING)' }
Write-Output ("KILL_AT_UTC=" + (Get-Date).ToUniversalTime().ToString('s') + 'Z')
taskkill /T /F /PID $run1.Id 2>&1 | Out-Null
Start-Sleep -Seconds 4
Write-Output ("POST-KILL engines=" + @(Get-Process wayland-core -ErrorAction SilentlyContinue).Count)
Write-Output ("POST-KILL " + (Census $C))
Write-Output '--- run1.log ---'
Get-Content "$ROOT\run1.log" -ErrorAction SilentlyContinue | Where-Object { $_ -notmatch 'crash sentinel' }
Start-Sleep -Seconds 7
& $BIN goal run --journal $J --goal $G --effects-dir $C --worker-command "cmd /c $ROOT\worker.cmd" `
  --width 6 --shard-size 2 --lease 60s > "$ROOT\run2.log" 2>&1
Write-Output "RUN2_EXIT=$LASTEXITCODE"
Write-Output '--- run2.log ---'
Get-Content "$ROOT\run2.log" | Where-Object { $_ -notmatch 'crash sentinel' }

Say 'C  RESULT'
Census $C
Write-Output 'per-task execution counts (one directory per task, one file per invocation):'
Get-ChildItem "$C\observed" -Recurse -Directory -ErrorAction SilentlyContinue |
  Where-Object { @(Get-ChildItem $_.FullName -File -ErrorAction SilentlyContinue).Count -gt 0 } |
  ForEach-Object { "  {0} {1}" -f @(Get-ChildItem $_.FullName -File).Count, $_.Name }
& $BIN goal status --journal $J --goal $G > "$ROOT\status.json" 2>$null
python -c "import json;s=json.load(open(r'D:\b1-live\status.json'));t=s['tasks'];u=[k for k,v in sorted(t.items()) if v.get('attempts') and v['attempts'][-1]['status']['status']=='unknown'];print('LEDGER completed=%d unresolved=%d %s'%(sum(1 for v in t.values() if v.get('completion')),len(u),u))"

# ------------------------------------------------------------------ X1 -------
Say 'X1  FINDING 3: the SAME exit code with NO receipt must park, not withdraw'
Write-Output '  91 is this product''s "no effect landed" code. A worker that emits it by'
Write-Output '  accident -- which is what any sysexits-speaking tool does -- must NOT have'
Write-Output '  its intent withdrawn, or the retry duplicates. That is what 76 (EX_PROTOCOL)'
Write-Output '  did before this fix, and Windows is where the first version of it was caught.'
$D = "$ROOT\x1"; New-Item -ItemType Directory -Force -Path $D | Out-Null
$rc = ExecTask $D 't-x1' 'idem-x1' "$ROOT\w-collide.cmd" $null $null
Write-Output "    FIRST_EXIT=$rc  (90 = parked)"
OnDisk $D
Write-Output ("  X1_INTENT_HELD=" + (IntentCount $D) + "  (1 = the intent survived a bare exit code)")
$rc = ExecTask $D 't-x1' 'idem-x1' "$ROOT\w-fast.cmd" $null $null
Write-Output "    RETRY_EXIT=$rc  (90 = parked, NOT re-run)"
Census $D
Write-Output ("  X1_GATE_EXIT=" + (Gate $D 1) + " (0 = exactly one real effect)")

Say 'X1b  and the DECLARED case still withdraws: receipt AND code together'
$D = "$ROOT\x1b"; New-Item -ItemType Directory -Force -Path $D | Out-Null
$rc = ExecTask $D 't-x1b' 'idem-x1b' "$ROOT\w-noeffect.cmd" $null $null
Write-Output "    FIRST_EXIT=$rc  (1 = a plain, retryable failure)"
Write-Output ("  X1b_INTENT_HELD=" + (IntentCount $D) + "  (0 = withdrawn, as declared)")
$rc = ExecTask $D 't-x1b' 'idem-x1b' "$ROOT\w-fast.cmd" $null $null
Write-Output "    RETRY_EXIT=$rc  (0 = it ran, as it must)"
Write-Output ("  X1b_GATE_EXIT=" + (Gate $D 1) + " (0 = exactly one real effect)")

# ------------------------------------------------------------------ X2 -------
Say 'X2  FINDING 1+2: two Goals, ONE --effects-dir, the same task names'
Write-Output '  Before the scoping: goal B declined every task as already-committed and'
Write-Output '  reported success having executed nothing, and a stale intent left by a'
Write-Output '  killed goal A permanently parked a brand-new goal B.'
$D = "$ROOT\x2"; New-Item -ItemType Directory -Force -Path $D | Out-Null
$rc = ExecTask $D 'deploy' 'idem-deploy' "$ROOT\w-fast.cmd" $null 'goal-a'
Write-Output "    GOAL_A_EXIT=$rc"
$rc = ExecTask $D 'deploy' 'idem-deploy' "$ROOT\w-fast.cmd" $null 'goal-b'
Write-Output "    GOAL_B_EXIT=$rc  (0 with produced=yes = B did its own work)"
Census $D
Write-Output ("  X2_GATE_EXIT=" + (Gate $D 2) + " (0 = TWO real effects, one per goal, no duplicate)")
Write-Output '  -- and the intent half, the denial of service the fix itself opened:'
$D = "$ROOT\x2b"; New-Item -ItemType Directory -Force -Path $D | Out-Null
$env:WAYLAND_GOAL_ID = 'goal-a'; $env:WAYLAND_GOAL_TASK = 'deploy'; $env:WAYLAND_GOAL_IDEMPOTENCY_KEY = 'idem-deploy'
$pk = Start-Process -FilePath $BIN -PassThru -WindowStyle Hidden `
      -ArgumentList @('goal','exec-task','--effects-dir',$D,'--','cmd','/c',"$ROOT\w-early.cmd")
Start-Sleep -Seconds 3
taskkill /T /F /PID $pk.Id 2>&1 | Out-Null
Start-Sleep -Seconds 2
Write-Output ("    goal A killed mid-window; intents on disk: " + (IntentCount $D))
$rc = ExecTask $D 'deploy' 'idem-deploy' "$ROOT\w-fast.cmd" $null 'goal-b'
Write-Output "    GOAL_B_EXIT=$rc  (0 = B is not parked by A's corpse)"

# ------------------------------------------------------------------ X3 -------
Say 'X3  FINDING 4: the instrument counts INVOCATIONS, not record contents'
Write-Output '  A worker whose record carries an invocation identity used to make two'
Write-Output '  executions of ONE task read as two DISTINCT effects and zero duplicates.'
$D = "$ROOT\x3"; New-Item -ItemType Directory -Force -Path $D | Out-Null
# One real execution, then the on-disk state a death inside the window leaves
# -- reconstructed, not raced, because a worker that finishes in milliseconds
# wins every race against a taskkill and the leg would measure the race.
ExecTask $D 't-p' 'idem-p' "$ROOT\w-ident.cmd" $null $null | Out-Null
$scope = @(Get-ChildItem "$D\effects" -Directory)[0].Name
New-Item -ItemType Directory -Force -Path "$D\intents\$scope" | Out-Null
Set-Content -Encoding ASCII "$D\intents\$scope\idem-p" 'task=t-p pid=1'
Remove-Item "$D\effects\$scope\idem-p" -ErrorAction SilentlyContinue
# The operator resolves as retry when the effect DID land: a second real
# execution, which the census must report as a duplicate.
ExecTask $D 't-p' 'idem-p' "$ROOT\w-ident.cmd" @('--resolve','retry') $null | Out-Null
Write-Output '  the two records, byte-different by construction:'
Get-ChildItem "$D\observed" -Recurse -File | ForEach-Object { "    " + (Get-Content $_.FullName).Trim() }
$distinct = @(Get-ChildItem "$D\observed" -Recurse -File | ForEach-Object { (Get-Content $_.FullName).Trim() } | Sort-Object -Unique).Count
Write-Output "  distinct CONTENTS = $distinct  (2 -- which is why content cannot be the identity)"
Census $D
Write-Output ("  X3_GATE_EXIT=" + (Gate $D 1) + " (NONZERO is the PASS: the gate went red on a real duplicate)")

# ------------------------------------------------------------------ X4 -------
Say 'X4  FINDING 5: goal run --terminate over PARKED tasks'
Write-Output '  The canonical terminal transition used to report'
Write-Output '  PartiallyCompleted { completed: 0, failed: 0 } over four parked tasks.'
$X4 = "$ROOT\x4"; New-Item -ItemType Directory -Force -Path $X4 | Out-Null
$XJ = "$ROOT\x4.journal"; $XG = 'g-b1-x4'
@'
@echo off
if not exist "%WAYLAND_GOAL_EFFECT_SINK%" mkdir "%WAYLAND_GOAL_EFFECT_SINK%"
(echo %WAYLAND_GOAL_TASK%)>"%WAYLAND_GOAL_EFFECT_SINK%\%WAYLAND_GOAL_TASK%.%RANDOM%.%RANDOM%.%TIME:~6,5%"
exit /b 1
'@ | Set-Content -Encoding ASCII "$ROOT\w-park.cmd"
& $BIN goal open --journal $XJ --goal $XG --objective 'park every task, then terminate' --iterations 4 --max-tokens 10000 | Out-Null
foreach ($i in 0..3) { & $BIN goal task --journal $XJ --goal $XG --task "p0$i" | Out-Null }
& $BIN goal run --journal $XJ --goal $XG --effects-dir $X4 --worker-command "cmd /c $ROOT\w-park.cmd" `
  --width 4 --shard-size 2 --lease 60s --terminate 2>&1 |
  Where-Object { $_ -match 'GOAL: (wave|unresolved|canonical|run_complete)' }
Write-Output '  X4_TERMINAL: the line above must NOT be PartiallyCompleted { completed: 0, failed: 0 }'

# ------------------------------------------------------------------ X5 -------
Say 'X5  FINDING 6: ordinary lease overlap, NO kill anywhere'
Write-Output '  20 pairs of concurrent claimants of the same key. Before the overlap wait,'
Write-Output '  a third of them ended parked with the effect having landed exactly once --'
Write-Output '  a task needing a human for no reason at all.'
$X5 = "$ROOT\x5"; New-Item -ItemType Directory -Force -Path $X5 | Out-Null
$parked = 0; $ok = 0; $other = 0; $dups = 0; $missing = 0
foreach ($i in 1..20) {
  $D = "$X5\p$i"; New-Item -ItemType Directory -Force -Path $D | Out-Null
  $env:WAYLAND_GOAL_ID = 'g-conc'; $env:WAYLAND_GOAL_TASK = "t$i"; $env:WAYLAND_GOAL_IDEMPOTENCY_KEY = "idem-t$i"
  $pa = Start-Process -FilePath $BIN -PassThru -WindowStyle Hidden `
        -ArgumentList @('goal','exec-task','--effects-dir',$D,'--','cmd','/c',"$ROOT\w-ident.cmd")
  $pb = Start-Process -FilePath $BIN -PassThru -WindowStyle Hidden `
        -ArgumentList @('goal','exec-task','--effects-dir',$D,'--','cmd','/c',"$ROOT\w-ident.cmd")
  $pa.WaitForExit(); $pb.WaitForExit()
  foreach ($e in @($pa.ExitCode, $pb.ExitCode)) {
    if ($e -eq 0) { $ok++ } elseif ($e -eq 90) { $parked++ } else { $other++ }
  }
  $line = (& $BIN goal effects --effects-dir $D 2>$null | Select-String 'GOAL-EFFECTS').Line
  if ($line -notmatch 'duplicates=0') { $dups++ }
  if ($line -match 'observed_total=0') { $missing++ }
}
Write-Output "  PAIRS=20 exit0=$ok parked90=$parked other=$other"
Write-Output "  X5_PAIRS_WITH_DUPLICATE=$dups  (0 is the PASS)"
Write-Output "  X5_PAIRS_WITH_NO_EFFECT=$missing  (0 is the PASS)"
Write-Output "  X5_PARKED=$parked  (0 is the PASS: overlap must not park a healthy task)"

Say 'DONE'
