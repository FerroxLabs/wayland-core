# Lane 28-h2 — F-28-02-002 repro / proof harness.
#
# Runs the REAL product (wayland-core sandbox status|exec) against three lease
# states and records what an operator actually sees. `sandbox exec` dispatches
# through wcore_tools::bash::BashTool::execute_with_ctx -- the agent's own shell
# function -- so what this observes is what the agent gets.
#
# Grading is on markers written to a status FILE, never on an exit code: every
# non-zero collapses to 1 across the ssh boundary.
param(
  [string]$Tag = 'before',
  [string]$Exe = 'C:\f28h2-target\release\wayland-core.exe'
)

$ErrorActionPreference = 'Continue'
$log    = "C:\f28h2\repro-$Tag.log"
$status = "C:\f28h2\repro-$Tag.status"
$dir    = Join-Path $env:LOCALAPPDATA 'Wayland\Core\AppContainerLeases\v1'
$quar   = Join-Path $dir 'quarantine'
$bak    = 'C:\p22-evidence\stale-leases-backup'
$stale  = 'WCore-storage-00002d20-00000000000000f2.toml'
$ws     = "C:\f28h2\ws-$Tag"

Remove-Item -Force -ErrorAction SilentlyContinue $status
Set-Content -Path $log -Value ("F28H2 REPRO tag=$Tag started=" + (Get-Date -Format o)) -Encoding utf8
function L($m) { Add-Content -Path $log -Value $m -Encoding utf8 }

L ("EXE=" + $Exe)
L ("EXE_SHA256=" + (Get-FileHash -Algorithm SHA256 -Path $Exe).Hash.ToLower())
Push-Location 'C:\f28h2-repo'
L ("SRC_SHA=" + (& git rev-parse HEAD).Trim())
L ("SRC_DIRTY=" + ((& git status --porcelain) | Measure-Object).Count)
Pop-Location
L ("LEASEDIR=" + $dir)

# A result taken while other lanes compile is not a measurement.
L ("QUIET_BUILD_PROCS=" + (Get-Process -Name cargo,rustc,link -ErrorAction SilentlyContinue | Measure-Object).Count)

New-Item -ItemType Directory -Force -Path $ws | Out-Null
Set-Content -Path (Join-Path $ws 'seed.txt') -Value 'f28h2' -Encoding utf8

# Snapshot the as-found lease directory so it is restored exactly.
$restore = "C:\f28h2\lease-restore-$Tag"
if (Test-Path $restore) { Remove-Item -Recurse -Force $restore }
New-Item -ItemType Directory -Force -Path $restore | Out-Null
$asFound = @(Get-ChildItem -Path $dir -File -ErrorAction SilentlyContinue)
foreach ($f in $asFound) { Copy-Item $f.FullName -Destination $restore -Force }
L ("AS_FOUND_LEASES=" + $asFound.Count)

function Observe($state) {
  $n = (Get-ChildItem -Path $dir -File -ErrorAction SilentlyContinue | Measure-Object).Count
  $q = (Get-ChildItem -Path $quar -File -ErrorAction SilentlyContinue | Measure-Object).Count
  L "===== OBSERVATION state=$state active_leases=$n quarantined=$q ====="

  $so = (& $Exe sandbox status 2>&1 | Out-String)
  L "----- RAW `sandbox status` -----"
  L $so
  L "----- END -----"

  # F28H2RAN in the CHILD's own stdout is positive evidence the child executed.
  $eo = (& $Exe sandbox exec --workspace $ws --timeout-ms 90000 "cmd.exe /c echo F28H2RAN" 2>&1 | Out-String)
  L "----- RAW `sandbox exec` -----"
  L $eo
  L "----- END -----"

  $all       = $so + "`n" + $eo
  $available = ($so -match 'available\s*[:=]?\s*true') -or ($so -match '\bavailable\b.*\btrue\b')
  $ran       = ($eo -match 'F28H2RAN')
  $disabled  = ($all -match 'sandbox disabled') -or ($all -match 'AppContainer real-spawn probe failed')
  $mismatch  = ($all -match 'OWN TEST SUITE') -or ($all -match 'SID/profile mismatch') -or ($all -match 'does not match the SID derived')
  $reclaimed = ($all -match 'RECLAIMED a stale AppContainer ACL lease')
  $transient = ($all -match 'If the failure is transient')
  $namesFile = ($all -match [regex]::Escape('AppContainerLeases'))

  L ("F28H2=state=$state;active_leases=$n;quarantined=$q;available=$available;ran=$ran;" +
     "disabled=$disabled;mismatch=$mismatch;reclaimed=$reclaimed;transient_claim=$transient;names_lease_file=$namesFile")
}

# --- state 1: CLEAN (positive control -- sandboxed execution must actually RUN)
Get-ChildItem -Path $dir -File -ErrorAction SilentlyContinue | Remove-Item -Force
if (Test-Path $quar) { Remove-Item -Recurse -Force $quar }
Observe 'clean'

# --- state 2: WEDGED (the finding)
$wedgeOk = $false
if (Test-Path (Join-Path $bak $stale)) {
  Copy-Item (Join-Path $bak $stale) -Destination $dir -Force
  $wedgeOk = Test-Path (Join-Path $dir $stale)
}
L "WEDGE_INSTALLED=$wedgeOk"
Observe 'wedged'

# --- state 3: SECOND RUN with the wedge left in place.
# Permanence is the whole finding: one refusal could be a transient. This also
# catches a fix that only works once (e.g. one that forgets to allow-list its
# own quarantine directory and wedges again on the next pass).
Observe 'wedged-second-run'

# --- restore the lease directory to exactly its as-found state
Get-ChildItem -Path $dir -File -ErrorAction SilentlyContinue | Remove-Item -Force
foreach ($f in (Get-ChildItem -Path $restore -File -ErrorAction SilentlyContinue)) {
  Copy-Item $f.FullName -Destination $dir -Force
}
$after = (Get-ChildItem -Path $dir -File -ErrorAction SilentlyContinue | Measure-Object).Count
L "LEASE_RESTORED count=$after expected=$($asFound.Count)"

$rc = 0
if (-not $wedgeOk) { L 'WEDGE_NOT_INSTALLED'; $rc = 8 }
if ($after -ne $asFound.Count) { L 'LEASE_NOT_RESTORED'; $rc = 7 }
L ("finished=" + (Get-Date -Format o))

Set-Content -Path $status -Value "WLRC=${rc}" -Encoding utf8
Add-Content -Path $status -Value "WLTAG=${Tag}" -Encoding utf8
Add-Content -Path $status -Value "WLDONE" -Encoding utf8
