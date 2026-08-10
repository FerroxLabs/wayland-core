# DEFECT B-1 — Windows leg C only: full fleet, taskkill /T /F mid-wave, restart.
# Round 3. The two earlier attempts were VACUOUS: Start-Process joins its
# -ArgumentList unquoted, so the whitespace-bearing --worker-command was split
# and run1 exited before doing anything. The engine is now launched from a .cmd
# file, where the quoting is under this script's control and verifiable.
$ErrorActionPreference = 'Continue'
$BIN  = 'D:\b1\target\release\wayland-core.exe'
$ROOT = 'D:\b1-liveC'
if (Test-Path $ROOT) { Remove-Item -Recurse -Force $ROOT }
New-Item -ItemType Directory -Force -Path $ROOT | Out-Null
$C = "$ROOT\c"; New-Item -ItemType Directory -Force -Path $C | Out-Null
$J = "$ROOT\fleet.journal"; $G = 'g-b1'

function Census($d) { (& $BIN goal effects --effects-dir $d 2>$null | Select-String 'GOAL-EFFECTS').Line }

Write-Output ("BINARY " + (Get-FileHash $BIN -Algorithm SHA256).Hash.ToLower())
& $BIN --version
Write-Output ("other wayland-core processes: " + @(Get-Process wayland-core -ErrorAction SilentlyContinue).Count)

@'
@echo off
if not exist "%WAYLAND_GOAL_EFFECT_SINK%" mkdir "%WAYLAND_GOAL_EFFECT_SINK%"
(echo %WAYLAND_GOAL_TASK%)>"%WAYLAND_GOAL_EFFECT_SINK%\%WAYLAND_GOAL_TASK%.%RANDOM%.%RANDOM%.%TIME:~6,5%"
if "%WAYLAND_GOAL_TASK%"=="t04" ping -n 41 127.0.0.1 >nul
if "%WAYLAND_GOAL_TASK%"=="t05" ping -n 41 127.0.0.1 >nul
if "%WAYLAND_GOAL_TASK%"=="t02" ping -n 6 127.0.0.1 >nul
if "%WAYLAND_GOAL_TASK%"=="t03" ping -n 6 127.0.0.1 >nul
ping -n 2 127.0.0.1 >nul
'@ | Set-Content -Encoding ASCII "$ROOT\worker.cmd"

& $BIN goal open --journal $J --goal $G --objective 'prove exactly-once survives a real kill' --iterations 8 --max-tokens 10000 | Out-Null
foreach ($i in 0..5) { & $BIN goal task --journal $J --goal $G --task "t0$i" | Out-Null }
foreach ($i in 0..5) { & $BIN goal task --journal $J --goal $G --task ("t0" + ($i+6)) --depends-on "t0$i" | Out-Null }
Write-Output ("declared tasks: " + (& $BIN goal status --journal $J --goal $G | Out-String).Length + " bytes of status")

$launch = @"
@echo off
"$BIN" goal run --journal "$J" --goal $G --effects-dir "$C" --worker-command "cmd /c $ROOT\worker.cmd" --width 6 --shard-size 2 --lease 5s > "$ROOT\run1.log" 2>&1
"@
$launch | Set-Content -Encoding ASCII "$ROOT\run1.cmd"
Write-Output '--- run1.cmd ---'; Get-Content "$ROOT\run1.cmd"

$run1 = Start-Process -FilePath "$ROOT\run1.cmd" -PassThru -WindowStyle Hidden
Write-Output ("run1 launcher pid=" + $run1.Id)
for ($n=0; $n -lt 240; $n++) {
  if (@(Get-ChildItem "$C\observed" -ErrorAction SilentlyContinue).Count -ge 6) { break }
  Start-Sleep -Milliseconds 250
}
$engines = @(Get-Process wayland-core -ErrorAction SilentlyContinue)
Write-Output ("PRE-KILL observed=" + @(Get-ChildItem "$C\observed" -ErrorAction SilentlyContinue).Count +
              " commits=" + @(Get-ChildItem "$C\effects" -ErrorAction SilentlyContinue).Count +
              " intents=" + @(Get-ChildItem "$C\intents" -ErrorAction SilentlyContinue).Count +
              " engines=" + $engines.Count +
              " pingers=" + @(Get-Process PING -ErrorAction SilentlyContinue).Count)
if (@(Get-ChildItem "$C\observed" -ErrorAction SilentlyContinue).Count -lt 6) {
  Write-Output 'ABORT: the pre-kill state never became real. This leg would be vacuous.'
  Get-Content "$ROOT\run1.log" -ErrorAction SilentlyContinue
  exit 3
}
Write-Output ("KILL_AT_UTC=" + (Get-Date).ToUniversalTime().ToString('s') + 'Z')
taskkill /T /F /PID $run1.Id 2>&1 | Out-Null
foreach ($e in $engines) { taskkill /T /F /PID $e.Id 2>&1 | Out-Null }
Start-Sleep -Seconds 4
Write-Output ("POST-KILL engines=" + @(Get-Process wayland-core -ErrorAction SilentlyContinue).Count +
              " pingers=" + @(Get-Process PING -ErrorAction SilentlyContinue).Count)
Write-Output ("POST-KILL " + (Census $C))
Write-Output '--- run1.log ---'
Get-Content "$ROOT\run1.log" -ErrorAction SilentlyContinue | Where-Object { $_ -notmatch 'crash sentinel' }

Start-Sleep -Seconds 8
& $BIN goal run --journal $J --goal $G --effects-dir $C --worker-command "cmd /c $ROOT\worker.cmd" `
  --width 6 --shard-size 2 --lease 60s > "$ROOT\run2.log" 2>&1
Write-Output "RUN2_EXIT=$LASTEXITCODE"
Write-Output '--- run2.log ---'
Get-Content "$ROOT\run2.log" | Where-Object { $_ -notmatch 'crash sentinel' }

Write-Output ''; Write-Output '=== C RESULT ==='
Census $C
Write-Output 'per-task execution counts:'
Get-ChildItem "$C\observed" -ErrorAction SilentlyContinue | ForEach-Object { (Get-Content $_.FullName).Trim() } |
  Group-Object | Sort-Object Count -Descending | ForEach-Object { "  {0} {1}" -f $_.Count, $_.Name }
& $BIN goal status --journal $J --goal $G > "$ROOT\status.json" 2>$null
python -c "import json;s=json.load(open(r'D:\b1-liveC\status.json'));t=s['tasks'];u=[k for k,v in sorted(t.items()) if v.get('attempts') and v['attempts'][-1]['status']['status']=='unknown'];print('LEDGER completed=%d unresolved=%d %s'%(sum(1 for v in t.values() if v.get('completion')),len(u),u))"
Write-Output 'DONE'
