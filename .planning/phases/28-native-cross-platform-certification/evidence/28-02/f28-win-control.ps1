# Phase 28 plan 28-02, task 1 — the Windows sandbox observability control.
#
# ASSUMES NEITHER ANSWER. The old belief (a session-0 logon makes the availability
# probe report unavailable regardless of correctness) and the competing report (a stale
# lease wedge causes it, and clearing leases restores observability over SSH) are BOTH
# hypotheses here. This script holds everything constant except one variable at a time:
#   session type  in { ssh, scheduled-task }   x   lease state in { as-found, cleared, wedged }
#
# It records TWO THINGS SEPARATELY for every observation, because conflating them is how
# a security defect gets filed as a tooling quirk:
#   PROBE_REPORT      — what the product's own availability probe reported.
#   PRODUCT_BEHAVIOUR — what the product then DID: refused, ran contained, or ran with
#                       no sandbox at all.
#
# It observes the same thing the product observes: on Windows `is_available()` is not a
# capability query, it is a REAL SANDBOXED SPAWN through the full AppContainer path.
# This script therefore drives that path through the shipped binary rather than
# re-implementing it in PowerShell — a re-implementation answers a different question
# confidently, and this platform's shell has already produced one such answer on this
# program through a high-hex-constant parse.
#
# ACTIVENESS is a DIFFERENTIAL observation, not an absence. The identical command is run
# outside the product and inside the worker. Outside, it succeeds and reports the
# ordinary High integrity level. Inside an AppContainer, msys's whoami dies on
# NtCreateDirectoryObject(\BaseNamedObjects\msys-...) with 0xC0000022 — AppContainer
# confines to AppContainerNamedObjects by construction — and System32\whoami.exe is
# refused outright. Those signatures are POSITIVE evidence of containment; their absence
# beside a child that demonstrably ran is positive evidence of NO containment.

$ErrorActionPreference = 'Continue'

$session = $env:F28_SESSION_TYPE
$runId   = $env:F28_RUN_ID
if (-not $session) { $session = 'scheduled-task' }
if (-not $runId)   { $runId   = 'unknown' }

$log  = "C:\f28\control2-$session.log"
$exe  = if ($env:F28_EXE) { $env:F28_EXE } else { 'C:\ferrox-win-p28\target\release\wayland-core.exe' }
$dir  = "$env:LOCALAPPDATA\Wayland\Core\AppContainerLeases\v1"
$bak  = 'C:\p22-evidence\stale-leases-backup'
$stale = 'WCore-storage-00002d20-00000000000000f2.toml'
$base = "C:\f28\ctl-$session"
$WC   = 'cmd.exe /c echo F28RAN & whoami /groups & C:\Windows\System32\whoami.exe /groups'

function L($m) { Add-Content -Path $log -Value $m -Encoding utf8 }

New-Item -ItemType Directory -Force -Path C:\f28 | Out-Null
Set-Content -Path $log -Value "F28 OBSERVABILITY CONTROL session=$session run=$runId" -Encoding utf8
L ("started=" + (Get-Date -Format o))

# ---- the run must be quiet: two registered runners are ONE physical box ----------
$busy = (Get-Process -Name cargo,rustc,link -ErrorAction SilentlyContinue | Measure-Object).Count
L "QUIET_CHECK build_processes=$busy"
if ($busy -gt 0) {
  L "NOT_QUIET: $busy compiler processes are running; a result observed under concurrent load is not recordable"
  L "F28_TASK_EXIT=9"
  exit 9
}

Push-Location 'C:\ferrox-win-p28'
$sha  = (& git rev-parse HEAD).Trim()
$tree = (& git rev-parse 'HEAD^{tree}').Trim()
Pop-Location
$binHash = (Get-FileHash -Algorithm SHA256 -Path $exe).Hash.ToLower()
L "HOST_SHA=$sha"
L "HOST_TREE=$tree"
L "BINARY=$exe"
L "BINARY_SHA256=$binHash"

$sid  = (Get-Process -Id $PID).SessionId
$ia   = [Environment]::UserInteractive
$isSsh = [bool]$env:SSH_CONNECTION
L "LOGON session_id=$sid interactive=$ia ssh=$isSsh session_type=$session"

if (Test-Path $base) { cmd /c "rmdir /s /q $base" 2>&1 | Out-Null }
New-Item -ItemType Directory -Force -Path "$base\home" | Out-Null
$env:HOME = "$base\home"; $env:USERPROFILE = "$base\home"

# ---- the as-found lease state, captured before anything is touched ---------------
$asFound = @(Get-ChildItem -Path $dir -File -ErrorAction SilentlyContinue)
L "AS_FOUND_LEASES=$($asFound.Count)"
foreach ($f in $asFound) { L "AS_FOUND_LEASE_FILE=$($f.Name)" }
$restore = "C:\f28\lease-restore-$session"
if (Test-Path $restore) { cmd /c "rmdir /s /q $restore" 2>&1 | Out-Null }
New-Item -ItemType Directory -Force -Path $restore | Out-Null
foreach ($f in $asFound) { Copy-Item $f.FullName -Destination $restore -Force }

function New-Repo($path) {
  if (Test-Path $path) { cmd /c "rmdir /s /q $path" 2>&1 | Out-Null }
  New-Item -ItemType Directory -Force -Path $path | Out-Null
  Push-Location $path
  cmd /c "git init -q -b main ." 2>&1 | Out-Null
  cmd /c "git config user.email ci@e.c" 2>&1 | Out-Null
  cmd /c "git config user.name ci" 2>&1 | Out-Null
  Set-Content -Path "$path\README.md" -Value "seed"
  Set-Content -Path "$path\.gitignore" -Value ".swarm-worktrees/"
  cmd /c "git add -A" 2>&1 | Out-Null
  cmd /c "git -c commit.gpgsign=false commit -q -m init" 2>&1 | Out-Null
  Pop-Location
}

# Classify one raw product output into the two SEPARATE fields.
function Classify($out, $leaseCount) {
  # What the availability probe reported. "sandbox disabled" is the product's own
  # words when `probe_appcontainer_available()` maps a failure to false.
  $disabled = ($out -match 'sandbox disabled') -or ($out -match 'AppContainer real-spawn probe failed')
  $mismatch = ($out -match 'ACL lease SID/profile mismatch') -or ($out -match 'OWN TEST SUITE')
  $probeReport = if ($disabled) { 'unavailable' } else { 'available' }

  # What the product DID. `F28RAN` in the worker's captured stdout is positive
  # evidence the child executed; its absence beside a refusal is positive evidence it
  # did not.
  $ran = ($out -match 'F28RAN')
  # Positive containment signatures, both AppContainer-specific.
  $msys    = ($out -match '0xC0000022') -or ($out -match 'BaseNamedObjects')
  $denied  = ($out -match 'Access is denied')
  $highIl  = ($out -match 'S-1-16-12288')   # ordinary High integrity — NOT contained
  $contained = ($msys -or $denied) -and (-not $highIl)

  $behaviour = 'indeterminate'
  if ($ran -and $contained)        { $behaviour = 'executed-sandboxed' }
  elseif ($ran -and $highIl)       { $behaviour = 'proceeded-unsandboxed' }
  elseif (-not $ran)               { $behaviour = 'refused-fail-closed' }

  return [pscustomobject]@{
    probe_report = $probeReport
    behaviour    = $behaviour
    ran          = $ran
    contained    = $contained
    highIl       = $highIl
    disabled     = $disabled
    mismatch     = $mismatch
    leases       = $leaseCount
  }
}

function Observe($leaseState) {
  $repo = "$base\repo-$leaseState"
  New-Repo $repo
  $n = (Get-ChildItem -Path $dir -File -ErrorAction SilentlyContinue | Measure-Object).Count
  $out = & $exe swarm --workers 1 --worker-command $WC --repo $repo --base-branch main --timeout 90s 2>&1 | Out-String
  $c = Classify $out $n
  L "--- RAW session=$session lease=$leaseState leases=$n ---"
  L $out.Substring(0, [Math]::Min(4000, $out.Length))
  L "--- END RAW ---"
  L ("F28_CONTROL=session=$session;lease=$leaseState;leases=$n;probe_report=$($c.probe_report);" +
     "product_behaviour=$($c.behaviour);ran=$($c.ran);contained=$($c.contained);" +
     "high_il=$($c.highIl);disabled=$($c.disabled);mismatch=$($c.mismatch)")
  return $c
}

# ---- lease state 1: AS FOUND ------------------------------------------------------
$oAsFound = Observe 'as-found'

# ---- lease state 2: CLEARED -------------------------------------------------------
Get-ChildItem -Path $dir -File -ErrorAction SilentlyContinue | Remove-Item -Force
$oCleared = Observe 'cleared'

# ---- lease state 3: WEDGED --------------------------------------------------------
# Produced on demand rather than waited for: an archived, real, unreconcilable lease
# whose owner PID is long dead. If the wedge does not take, that is recorded as a
# control failure rather than smoothed over.
$wedgeOk = $false
if (Test-Path (Join-Path $bak $stale)) {
  Copy-Item (Join-Path $bak $stale) -Destination $dir -Force
  $wedgeOk = Test-Path (Join-Path $dir $stale)
}
L "WEDGE_INSTALLED=$wedgeOk"
$oWedged = Observe 'wedged'

# ---- restore the lease directory to exactly the state it was found in -------------
Get-ChildItem -Path $dir -File -ErrorAction SilentlyContinue | Remove-Item -Force
foreach ($f in (Get-ChildItem -Path $restore -File -ErrorAction SilentlyContinue)) {
  Copy-Item $f.FullName -Destination $dir -Force
}
$after = (Get-ChildItem -Path $dir -File -ErrorAction SilentlyContinue | Measure-Object).Count
L "LEASE_RESTORED count=$after expected=$($asFound.Count)"

# ---- directional controls ---------------------------------------------------------
# NEG-B: the activeness DETECTOR itself. The identical command, run OUTSIDE the
# product, must report activeness ABSENT. A detector that fires unconditionally proves
# nothing, and this is the check that would catch it.
$baseOut = (& cmd.exe /c "echo F28RAN & whoami /groups & C:\Windows\System32\whoami.exe /groups" 2>&1 | Out-String)
$bc = Classify $baseOut 0
L "--- RAW baseline-outside-product ---"
L $baseOut.Substring(0, [Math]::Min(3000, $baseOut.Length))
L "--- END RAW ---"
$negBActual = if ($bc.contained) { 'activeness-present' } else { 'activeness-absent' }
L "F28_DIRECTION=id=neg-activeness-detector-$session;direction=negative;expected=activeness-absent;actual=$negBActual"

# NEG-A: the wedged lease. A genuinely unavailable sandbox must report unobservable.
$negAActual = if ($oWedged.probe_report -eq 'unavailable') { 'unobservable' } else { 'observable' }
L "F28_DIRECTION=id=neg-wedged-lease-$session;direction=negative;expected=unobservable;actual=$negAActual"

# POS: a clean lease directory, where the channel should be sound.
$posActual = if ($oCleared.probe_report -eq 'available') { 'observable' } else { 'unobservable' }
L "F28_DIRECTION=id=pos-clean-lease-$session;direction=positive;expected=observable;actual=$posActual"

$busyEnd = (Get-Process -Name cargo,rustc,link -ErrorAction SilentlyContinue | Measure-Object).Count
L "QUIET_CHECK_END build_processes=$busyEnd"
L ("finished=" + (Get-Date -Format o))

$rc = 0
if ($busyEnd -gt 0) { L "NOT_QUIET_AT_END"; $rc = 9 }
if (-not $wedgeOk)  { L "WEDGE_NOT_INSTALLED"; $rc = 8 }
if ($after -ne $asFound.Count) { L "LEASE_NOT_RESTORED"; $rc = 7 }
L "F28_TASK_EXIT=$rc"
exit $rc
