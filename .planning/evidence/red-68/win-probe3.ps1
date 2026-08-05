$ErrorActionPreference = "Continue"
$bin = "C:\actions-runner-ferrox\_work\wayland-core\wayland-core\target\release\wayland-core.exe"
$root = Join-Path $env:TEMP "red68probe3"
Remove-Item -Recurse -Force $root -ErrorAction SilentlyContinue
New-Item -ItemType Directory -Path $root -Force | Out-Null
$R = "$root\RESULT.txt"

function Tail($path, $n) {
  if (-not (Test-Path $path)) { return "<NOFILE>" }
  $s = (Get-Content $path -ErrorAction SilentlyContinue | Select-Object -Last 40) -join " || "
  if (-not $s) { return "<EMPTY>" }
  if ($s.Length -le $n) { return $s }
  return "...TAIL..." + $s.Substring($s.Length - $n)
}

function Probe($name, $argList, $useWaylandHome, $waitMs) {
  $wd = Join-Path $root $name
  New-Item -ItemType Directory -Path $wd -Force | Out-Null
  $so = Join-Path $root "$name.out"; $se = Join-Path $root "$name.err"
  $si = Join-Path $root "$name.in"; Set-Content -Path $si -Value "" -NoNewline

  $oldHome = $env:HOME; $oldWH = $env:WAYLAND_HOME
  $env:HOME = $wd
  if ($useWaylandHome) { $env:WAYLAND_HOME = $wd } else { Remove-Item Env:\WAYLAND_HOME -ErrorAction SilentlyContinue }

  $p = $null
  try {
    $p = Start-Process -FilePath $bin -ArgumentList $argList -WorkingDirectory $wd `
          -RedirectStandardOutput $so -RedirectStandardError $se -RedirectStandardInput $si `
          -PassThru -NoNewWindow
  } catch { Add-Content $R "SPAWNEXC=$name : $($_.Exception.Message)" }

  if ($null -eq $p) {
    # LOUD: a probe that never launched is NOT a product finding.
    Add-Content $R "PROBE=$name SPAWN=FAILED  (this measurement is UNREADABLE, not evidence)"
    Add-Content $R "----"
    $env:HOME = $oldHome; if ($oldWH) { $env:WAYLAND_HOME = $oldWH } else { Remove-Item Env:\WAYLAND_HOME -ErrorAction SilentlyContinue }
    return
  }
  $pid_ = $p.Id
  $exited = $p.WaitForExit($waitMs)
  $code = "TIMEOUT_KILLED"
  if ($exited) { try { $p.Refresh() } catch {}; try { $code = $p.ExitCode } catch { $code = "UNREADABLE" } }
  else { try { $p.Kill() } catch {}; Start-Sleep -Milliseconds 800 }
  $env:HOME = $oldHome; if ($oldWH) { $env:WAYLAND_HOME = $oldWH } else { Remove-Item Env:\WAYLAND_HOME -ErrorAction SilentlyContinue }

  $outBytes = (Get-Item $so -ErrorAction SilentlyContinue).Length
  $errBytes = (Get-Item $se -ErrorAction SilentlyContinue).Length
  $lines = @(Get-Content $so -ErrorAction SilentlyContinue)
  $ready = @($lines | Where-Object { $_ -like '*"type":"ready"*' }).Count
  $first = if ($lines.Count -gt 0) { $lines[0] } else { "<NO LINE>" }
  Add-Content $R "PROBE=$name SPAWN=OK PID=$pid_ EXIT=$code OUTBYTES=$outBytes ERRBYTES=$errBytes OUTLINES=$($lines.Count) READYFRAMES=$ready"
  Add-Content $R ("FIRSTOUT=" + $first.Substring(0, [Math]::Min(240, $first.Length)))
  Add-Content $R ("STDERRTAIL=" + (Tail $se 1500))
  Add-Content $R "----"
}

# POSITIVE CONTROL FIRST. If this yields no output, every later "no output" line
# is an instrument failure, not a product observation.
Probe "CTRL_version" @("--version") $false 20000
# A: exactly plugin_discovery_e2e's environment -- HOME only, no WAYLAND_HOME.
Probe "A_home_only" @("--json-stream","--provider","anthropic","--api-key","k") $false 45000
# B: same invocation, WAYLAND_HOME set (the crate's canonical hermetic override).
Probe "B_wayland_home" @("--json-stream","--provider","anthropic","--api-key","k") $true 45000
# C: WAYLAND_HOME, long budget -- separates "never emits" from "slow to emit".
Probe "C_wh_long" @("--json-stream","--provider","anthropic","--api-key","k") $true 150000
Add-Content $R "WLDONE"

# --- instrument self-test, three assertions (LANE-BRIEF 6b-ii) ---
$probe = "$root\A_home_only.err"
$full = ((Get-Content $probe -ErrorAction SilentlyContinue) -join " || ")
$v2 = Tail $probe 1500
$v1 = if ($full.Length -gt 2500) { $full.Substring(0, 2500) } else { $full }
$lastLine = (Get-Content $probe -ErrorAction SilentlyContinue | Select-Object -Last 1)
$a1 = ($null -ne $lastLine) -and ($v2 -like "*$lastLine*")
$a2 = ((Tail "$root\definitely-not-here.err" 100) -eq "<NOFILE>")
$a3 = ($full.Length -gt 2500) -and (-not ($v1 -like "*$lastLine*"))
Add-Content $R "SELFTEST A1_tail_contains_last_stderr_line=$a1"
Add-Content $R "SELFTEST A2_missing_file_reports_NOFILE_not_silence=$a2"
Add-Content $R "SELFTEST A3_v1_head_truncation_would_have_MISSED_the_last_line=$a3"
Add-Content $R "SELFTESTDONE"
