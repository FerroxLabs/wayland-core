# LIVE proof against the SHIPPED wayland-core.exe on Windows: real fanout, real
# uncatchable tree kill (taskkill /T /F), real restart.
#
# Same scenario as the Linux leg, and it carries the same state: 12 tasks, a
# dependency layer, sharding (width 6 / shard-size 2 = 3 shards), a kill that
# lands MID-WAVE with some tasks recorded-but-undelivered and others running,
# and one effect placed on disk with no completion so the idempotency key is
# observable doing something.
$ErrorActionPreference = "Continue"
$BIN = "C:\p22gk\target\release\wayland-core.exe"
$R   = "C:\p22-wire-live"
if (Test-Path $R) { Remove-Item -Recurse -Force $R }
New-Item -ItemType Directory -Force -Path "$R\effects" | Out-Null
$J = "$R\fleet.journal"
$G = "g-live"

# ping -n N sleeps roughly N-1 seconds and needs no console, unlike `timeout`.
@'
@echo off
set D=2
if "%WAYLAND_GOAL_TASK%"=="t02" set D=3
if "%WAYLAND_GOAL_TASK%"=="t03" set D=3
if "%WAYLAND_GOAL_TASK%"=="t04" set D=41
if "%WAYLAND_GOAL_TASK%"=="t05" set D=41
ping -n %D% 127.0.0.1 >nul
exit /b 0
'@ | Set-Content -Encoding ASCII "$R\worker.cmd"

function Effects { (Get-ChildItem "$R\effects\effects" -File -ErrorAction SilentlyContinue | Measure-Object).Count }
function Descendants($rootPid) {
  # Walk the real parent/child tree rather than guessing by image name.
  $all = Get-CimInstance Win32_Process | Select-Object ProcessId, ParentProcessId, Name
  $seen = @{}; $queue = @($rootPid); $out = @()
  while ($queue.Count -gt 0) {
    $cur = $queue[0]; $queue = $queue[1..($queue.Count)]
    foreach ($p in $all | Where-Object { $_.ParentProcessId -eq $cur }) {
      if (-not $seen.ContainsKey($p.ProcessId)) {
        $seen[$p.ProcessId] = $true; $out += $p; $queue += $p.ProcessId
      }
    }
  }
  return $out
}

Write-Output "=== BINARY ==="
& $BIN --version
Write-Output "=== 1. OPEN THE GOAL (shipped verb) ==="
& $BIN goal open --journal $J --goal $G --objective "prove the wire survives a kill" --iterations 8 --max-tokens 10000
Write-Output "=== 2. DECLARE 12 TASKS, 6 OF THEM DEPENDENT ==="
foreach ($i in 0..5) { & $BIN goal task --journal $J --goal $G --task ("t0{0}" -f $i) }
foreach ($i in 0..5) { & $BIN goal task --journal $J --goal $G --task ("t0{0}" -f ($i+6)) --depends-on ("t0{0}" -f $i) }

Write-Output "=== 3. RUN 1, to be killed mid-wave ==="
$p = Start-Process -FilePath $BIN -PassThru -WindowStyle Hidden `
  -RedirectStandardOutput "$R\run1.out" -RedirectStandardError "$R\run1.err" `
  -ArgumentList @("goal","run","--journal",$J,"--goal",$G,"--effects-dir","$R\effects",
                  "--worker-command","$R\worker.cmd","--width","6","--shard-size","2","--lease","5s")
Write-Output "RUN1 pid=$($p.Id)"

# Wait for the state we intend to kill in to be REAL, not hoped for.
for ($k=0; $k -lt 200; $k++) { if ((Effects) -ge 4) { break }; Start-Sleep -Milliseconds 200 }
Write-Output "PRE-KILL effects=$(Effects)"
$before = @(Descendants $p.Id)
Write-Output "PRE-KILL descendants=$(@($before).Count) names=$(($before | ForEach-Object {$_.Name}) -join ',')"
Write-Output "KILL_AT_UTC=$((Get-Date).ToUniversalTime().ToString('yyyy-MM-ddTHH:mm:ssZ'))"

# The uncatchable tree kill. /T takes descendants, /F is SIGKILL-equivalent.
taskkill /T /F /PID $p.Id 2>&1 | Out-String | Write-Output
Start-Sleep -Seconds 3
$after = @(Descendants $p.Id)
Write-Output "POST-KILL descendants=$(@($after).Count)"
Write-Output "POST-KILL run1_exited=$((Get-Process -Id $p.Id -ErrorAction SilentlyContinue) -eq $null)"
Write-Output "POST-KILL effects=$(Effects)"
Write-Output "--- run1.out ---"; Get-Content "$R\run1.out" -ErrorAction SilentlyContinue
Write-Output "--- run1.err ---"; Get-Content "$R\run1.err" -ErrorAction SilentlyContinue

Write-Output "=== 4. PLACE t04's EFFECT ON DISK WITH NO COMPLETION ==="
$env:WAYLAND_GOAL_TASK = "t04"; $env:WAYLAND_GOAL_IDEMPOTENCY_KEY = "idem-t04"
& $BIN goal exec-task --effects-dir "$R\effects"
Remove-Item Env:\WAYLAND_GOAL_TASK, Env:\WAYLAND_GOAL_IDEMPOTENCY_KEY -ErrorAction SilentlyContinue
Write-Output "AFTER-PLANT effects=$(Effects)"

Write-Output "=== 5. WAIT PAST THE 5s LEASE, THEN RESTART ==="
Start-Sleep -Seconds 6
Write-Output "RESTART_AT_UTC=$((Get-Date).ToUniversalTime().ToString('yyyy-MM-ddTHH:mm:ssZ'))"
& $BIN goal run --journal $J --goal $G --effects-dir "$R\effects" `
  --worker-command "$R\worker.cmd" --width 6 --shard-size 2 --lease 60s
Write-Output "RUN2_EXIT=$LASTEXITCODE"

Write-Output "=== 6. THE GATE: exactly 12 effects, counted by the product ==="
& $BIN goal effects --effects-dir "$R\effects" --expect 12
Write-Output "EFFECTS_GATE_EXIT=$LASTEXITCODE"

Write-Output "=== 7. FALSIFY THE GATE (it must be able to go red) ==="
Copy-Item "$R\effects\effects\idem-t00" "$R\effects\effects\idem-t00-DUPLICATE"
& $BIN goal effects --effects-dir "$R\effects" --expect 12
Write-Output "FALSIFIED_GATE_EXIT=$LASTEXITCODE   (nonzero is the PASS here)"
Remove-Item "$R\effects\effects\idem-t00-DUPLICATE"
& $BIN goal effects --effects-dir "$R\effects" --expect 12
Write-Output "RESTORED_GATE_EXIT=$LASTEXITCODE"

Write-Output "=== 8. LEDGER STATE (shipped status verb) ==="
& $BIN goal status --journal $J --goal $G | Set-Content -Encoding UTF8 "$R\status.json"
$s = Get-Content "$R\status.json" -Raw | ConvertFrom-Json
$tasks = $s.tasks.PSObject.Properties
$comp = ($tasks | Where-Object { $_.Value.completion -ne $null }).Count
$deliv = ($tasks | Where-Object { $_.Value.completion -ne $null -and $_.Value.completion.delivered }).Count
$att = ($tasks | ForEach-Object { $_.Value.attempts.Count } | Measure-Object -Sum).Sum
$rel = ($tasks | ForEach-Object { $_.Value.dependency_releases } | Measure-Object -Sum).Sum
Write-Output "GOAL iterations=$($s.iterations_started) resume_count=$($s.resume_count) tasks=$($tasks.Count)"
Write-Output "LEDGER completed=$comp delivered=$deliv attempts=$att dependency_releases=$rel"
foreach ($t in $tasks | Sort-Object Name) {
  Write-Output ("  {0} attempts={1} epoch={2} completed={3} delivered={4}" -f `
    $t.Name, $t.Value.attempts.Count, $t.Value.attempts[-1].epoch,
    ($t.Value.completion -ne $null), $t.Value.completion.delivered)
}
