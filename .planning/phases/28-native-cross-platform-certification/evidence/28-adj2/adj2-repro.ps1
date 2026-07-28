# Lane 28-adj2 — F-28-ADJ-002 reproduction attempt.
#
# Claim under test (static reading by the adjudicator, NOT yet reproduced): a
# crash between create_new_nofollow and write_and_sync leaves a 0-byte .toml,
# read_validated_lease rejects it, and the `?` in recover_dead_leases_locked
# aborts the whole pass -- permanently, on every later acquisition.
#
# This script tests the EFFECT half on the real product: does a 0-byte .toml in
# the lease directory refuse sandboxed execution, and keep refusing?
#
# The clean state is measured FIRST and must RUN, so a refusal below cannot be
# an artifact of a machine that was already wedged.
param(
  [string]$Tag = 'adj2',
  [string]$Exe = 'C:\f28h2-target\release\wayland-core.exe'
)
$ErrorActionPreference = 'Continue'
$log    = "C:\f28h2\adj2repro-$Tag.log"
$status = "C:\f28h2\adj2repro-$Tag.status"
$dir    = Join-Path $env:LOCALAPPDATA 'Wayland\Core\AppContainerLeases\v1'
$quar   = Join-Path $dir 'quarantine'
$ws     = "C:\f28h2\ws-adj2-$Tag"
$zero   = Join-Path $dir 'WCore-adj2-0000c0de-00000000000000f2.toml'

Remove-Item -Force -ErrorAction SilentlyContinue $status
Set-Content -Path $log -Value ("F28ADJ2 REPRO tag=$Tag started=" + (Get-Date -Format o)) -Encoding utf8
function L($m) { Add-Content -Path $log -Value $m -Encoding utf8 }

L ("EXE_SHA256=" + (Get-FileHash -Algorithm SHA256 -Path $Exe).Hash.ToLower())
Push-Location 'C:\f28h2-repo'
L ("SRC_SHA=" + (& git rev-parse HEAD).Trim())
L ("SRC_DIRTY=" + ((& git status --porcelain) | Measure-Object).Count)
Pop-Location
L ("QUIET_BUILD_PROCS=" + (Get-Process -Name cargo,rustc,link -ErrorAction SilentlyContinue | Measure-Object).Count)

New-Item -ItemType Directory -Force -Path $ws | Out-Null
Set-Content -Path (Join-Path $ws 'seed.txt') -Value 'adj2' -Encoding utf8

$restore = "C:\f28h2\adj2-lease-restore-$Tag"
if (Test-Path $restore) { Remove-Item -Recurse -Force $restore }
New-Item -ItemType Directory -Force -Path $restore | Out-Null
$asFound = @(Get-ChildItem -Path $dir -File -ErrorAction SilentlyContinue)
foreach ($f in $asFound) { Copy-Item $f.FullName -Destination $restore -Force }
L ("AS_FOUND_LEASES=" + $asFound.Count)

function Observe($state) {
  $n = (Get-ChildItem -Path $dir -File -ErrorAction SilentlyContinue | Measure-Object).Count
  $q = (Get-ChildItem -Path $quar -File -ErrorAction SilentlyContinue | Measure-Object).Count
  $zsz = if (Test-Path $zero) { (Get-Item $zero).Length } else { -1 }
  L "===== OBSERVATION state=$state active=$n quarantined=$q zerofile_bytes=$zsz ====="
  $so = (& $Exe sandbox status 2>&1 | Out-String)
  L '----- RAW sandbox status -----'; L $so; L '----- END -----'
  $eo = (& $Exe sandbox exec --workspace $ws --timeout-ms 90000 "cmd.exe /c echo ADJ2RAN" 2>&1 | Out-String)
  L '----- RAW sandbox exec -----'; L $eo; L '----- END -----'

  $all      = $so + "`n" + $eo
  $ran      = ($eo -match 'ADJ2RAN')
  $disabled = ($all -match 'sandbox disabled') -or ($all -match 'AppContainer real-spawn probe failed')
  # The specific diagnosis the static reading predicts.
  $sizeErr  = ($all -match 'invalid AppContainer ACL lease size')
  $failClosedBackend = ($so -match 'fail_closed')
  L ("F28ADJ2=state=$state;active=$n;quarantined=$q;zerofile_bytes=$zsz;ran=$ran;" +
     "disabled=$disabled;size_error=$sizeErr;fail_closed_backend=$failClosedBackend")
}

# --- state 1: CLEAN. Must RUN, or nothing below means anything. ---------------
Get-ChildItem -Path $dir -File -ErrorAction SilentlyContinue | Remove-Item -Force
if (Test-Path $quar) { Remove-Item -Recurse -Force $quar }
Observe 'clean'

# --- state 2: a 0-byte .toml, exactly what the interrupted create leaves ------
# Written with no content through the ordinary filesystem, which is the state
# the crash window produces; this does not simulate the crash itself.
[System.IO.File]::WriteAllBytes($zero, @())
L ("ZERO_INSTALLED=" + (Test-Path $zero) + ";bytes=" + (Get-Item $zero).Length)
Observe 'zero-byte-lease'

# --- state 3: permanence ------------------------------------------------------
Observe 'zero-byte-lease-second-run'

# --- restore ------------------------------------------------------------------
Remove-Item -Force -ErrorAction SilentlyContinue $zero
if (Test-Path $quar) { Remove-Item -Recurse -Force $quar }
Get-ChildItem -Path $dir -File -ErrorAction SilentlyContinue | Remove-Item -Force
foreach ($f in (Get-ChildItem -Path $restore -File -ErrorAction SilentlyContinue)) {
  Copy-Item $f.FullName -Destination $dir -Force
}
$after = (Get-ChildItem -Path $dir -File -ErrorAction SilentlyContinue | Measure-Object).Count
L "LEASE_RESTORED count=$after expected=$($asFound.Count)"
$rc = 0
if ($after -ne $asFound.Count) { L 'LEASE_NOT_RESTORED'; $rc = 7 }
L ("finished=" + (Get-Date -Format o))
Set-Content -Path $status -Value "WLRC=${rc}" -Encoding utf8
Add-Content -Path $status -Value "WLDONE" -Encoding utf8
