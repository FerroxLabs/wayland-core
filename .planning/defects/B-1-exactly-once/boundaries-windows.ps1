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
if (Test-Path $ROOT) { Remove-Item -Recurse -Force $ROOT }
New-Item -ItemType Directory -Force -Path $ROOT | Out-Null

function Say($m) { Write-Output ''; Write-Output "=== $m ===" }
function Census($d) { (& $BIN goal effects --effects-dir $d 2>$null | Select-String 'GOAL-EFFECTS').Line }
function OnDisk($d) {
  $i = @(Get-ChildItem (Join-Path $d 'intents') -ErrorAction SilentlyContinue).Name -join ' '
  $c = @(Get-ChildItem (Join-Path $d 'effects') -ErrorAction SilentlyContinue).Name -join ' '
  $o = @(Get-ChildItem (Join-Path $d 'observed') -ErrorAction SilentlyContinue).Count
  Write-Output "    on-disk: intents=[$i] commits=[$c] observed=$o"
}
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

# A worker that DECLARES nothing landed.
@'
@echo off
exit /b 76
'@ | Set-Content -Encoding ASCII "$ROOT\w-noeffect.cmd"

function ExecTask($d, $t, $k, $worker, $extra) {
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
$env:WAYLAND_GOAL_TASK = 't-w2'; $env:WAYLAND_GOAL_IDEMPOTENCY_KEY = 'idem-w2'
$p = Start-Process -FilePath $BIN -PassThru -WindowStyle Hidden `
     -ArgumentList @('goal','exec-task','--effects-dir',$D,'--','cmd','/c',"$ROOT\w-early.cmd")
Start-Sleep -Seconds 3
Write-Output "    taskkill /T /F on pid $($p.Id)"
taskkill /T /F /PID $p.Id 2>&1 | Out-Null
Start-Sleep -Seconds 2
OnDisk $D; Census $D
Write-Output '  -- the honest retry (this is where it used to duplicate):'
$rc = ExecTask $D 't-w2' 'idem-w2' "$ROOT\w-fast.cmd" $null
Write-Output "    RETRY_EXIT=$rc  (75 = parked)"
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
Write-Output "    FIRST_EXIT=$rc  (75 = parked, NOT retried)"
OnDisk $D; Census $D
Write-Output '  -- the honest retry:'
$rc = ExecTask $D 't-w3' 'idem-w3' "$ROOT\w-fast.cmd" $null
Write-Output "    RETRY_EXIT=$rc  (75 = parked)"
OnDisk $D; Census $D
Write-Output ("  W3_GATE_EXIT=" + (Gate $D 1) + " (0 = exactly one real effect)")

# ------------------------------------------------------------------ W3b ------
Say 'W3b  worker DECLARED no effect (exit 76): plainly retryable, not parked'
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
$cmdline = '"' + $BIN + '" goal run --journal "' + $J + '" --goal ' + $G +
           ' --effects-dir "' + $C + '" --worker-command "cmd /c ' + $ROOT + '\worker.cmd"' +
           ' --width 6 --shard-size 2 --lease 5s > "' + $ROOT + '\run1.log" 2>&1'
$run1 = Start-Process -FilePath 'cmd.exe' -PassThru -WindowStyle Hidden -ArgumentList @('/c', $cmdline)
for ($n=0; $n -lt 240; $n++) {
  if (@(Get-ChildItem "$C\observed" -ErrorAction SilentlyContinue).Count -ge 6) { break }
  Start-Sleep -Milliseconds 250
}
Write-Output ("PRE-KILL observed=" + @(Get-ChildItem "$C\observed" -ErrorAction SilentlyContinue).Count +
              " commits=" + @(Get-ChildItem "$C\effects" -ErrorAction SilentlyContinue).Count +
              " intents=" + @(Get-ChildItem "$C\intents" -ErrorAction SilentlyContinue).Count +
              " engines=" + @(Get-Process wayland-core -ErrorAction SilentlyContinue).Count)
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
Write-Output 'per-task execution counts:'
Get-ChildItem "$C\observed" -ErrorAction SilentlyContinue | ForEach-Object { (Get-Content $_.FullName).Trim() } |
  Group-Object | Sort-Object Count -Descending | ForEach-Object { "  {0} {1}" -f $_.Count, $_.Name }
& $BIN goal status --journal $J --goal $G > "$ROOT\status.json" 2>$null
python -c "import json;s=json.load(open(r'D:\b1-live\status.json'));t=s['tasks'];u=[k for k,v in sorted(t.items()) if v.get('attempts') and v['attempts'][-1]['status']['status']=='unknown'];print('LEDGER completed=%d unresolved=%d %s'%(sum(1 for v in t.values() if v.get('completion')),len(u),u))"
Say 'DONE'
