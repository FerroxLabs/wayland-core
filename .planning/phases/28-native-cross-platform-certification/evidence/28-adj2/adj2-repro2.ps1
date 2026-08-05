# Lane 28-adj2 — F-28-ADJ-002 live proof, with the classifier repaired.
#
# The v1 classifier reported size_error=False while the raw log contained the
# string verbatim: the console wraps long lines, splitting the phrase across a
# newline, so a literal -match missed it. Under-detection is the dangerous
# direction -- it reports the defect ABSENT. Every match here runs against
# whitespace-NORMALISED text, and the classifier is self-tested against a
# known-positive and a known-negative in this same run before any observation
# is trusted.
param(
  [string]$Tag = 'fix',
  [string]$Exe = 'C:\f28h2-target\release\wayland-core.exe'
)
$ErrorActionPreference = 'Continue'
$log    = "C:\f28h2\adj2repro2-$Tag.log"
$status = "C:\f28h2\adj2repro2-$Tag.status"
$dir    = Join-Path $env:LOCALAPPDATA 'Wayland\Core\AppContainerLeases\v1'
$quar   = Join-Path $dir 'quarantine'
$ws     = "C:\f28h2\ws-adj2b-$Tag"
$zero   = Join-Path $dir 'WCore-adj2-0000c0de-00000000000000f2.toml'

Remove-Item -Force -ErrorAction SilentlyContinue $status
Set-Content -Path $log -Value ("F28ADJ2 REPRO2 tag=$Tag started=" + (Get-Date -Format o)) -Encoding utf8
function L($m) { Add-Content -Path $log -Value $m -Encoding utf8 }

# Collapse EVERY run of whitespace (including the console's hard wraps) to one
# space, so a phrase split across lines still matches.
function Flat($t) { return ($t -replace '\s+', ' ') }
function Has($t, $needle) { return (Flat $t).Contains($needle) }

# ---- classifier self-test: it must find a WRAPPED positive and reject a negative
$probeNeedle = 'invalid AppContainer ACL lease size'
$knownPositive = "prefix blah invalid`r`nAppContainer ACL   lease`n size 0 in C:\x.toml suffix"
$knownNegative = "prefix blah everything is fine, no lease problem here suffix"
$posOk = (Has $knownPositive $probeNeedle)
$negOk = -not (Has $knownNegative $probeNeedle)
# And prove the OLD literal matcher would have MISSED the wrapped positive,
# so this self-test is not itself vacuous.
$oldWouldMiss = -not ($knownPositive -match [regex]::Escape($probeNeedle))
L ("CLASSIFIER_SELFTEST=known_positive=$posOk;known_negative=$negOk;old_matcher_missed_it=$oldWouldMiss")
if (-not ($posOk -and $negOk)) {
  L 'CLASSIFIER_BROKEN'
  Set-Content -Path $status -Value 'WLRC=5' -Encoding utf8
  Add-Content -Path $status -Value 'WLDONE' -Encoding utf8
  exit 5
}

L ("EXE_SHA256=" + (Get-FileHash -Algorithm SHA256 -Path $Exe).Hash.ToLower())
Push-Location 'C:\f28h2-repo'
L ("SRC_SHA=" + (& git rev-parse HEAD).Trim())
L ("SRC_DIRTY=" + ((& git status --porcelain) | Measure-Object).Count)
Pop-Location
L ("QUIET_BUILD_PROCS=" + (Get-Process -Name cargo,rustc,link -ErrorAction SilentlyContinue | Measure-Object).Count)

New-Item -ItemType Directory -Force -Path $ws | Out-Null
Set-Content -Path (Join-Path $ws 'seed.txt') -Value 'adj2' -Encoding utf8

$restore = "C:\f28h2\adj2b-lease-restore-$Tag"
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

  $all = $so + " " + $eo
  $ran       = (Has $eo 'ADJ2RAN')
  $disabled  = (Has $all 'sandbox disabled')
  $sizeErr   = (Has $all $probeNeedle)
  $reclaimed = (Has $all 'RECLAIMED a 0-byte AppContainer ACL lease')
  $failClosed= (Has $so 'fail_closed')
  $appc      = (Has $so 'backend appcontainer')
  L ("F28ADJ2B=state=$state;active=$n;quarantined=$q;zerofile_bytes=$zsz;ran=$ran;" +
     "disabled=$disabled;size_error=$sizeErr;reclaimed=$reclaimed;fail_closed_backend=$failClosed;appcontainer=$appc")
}

Get-ChildItem -Path $dir -File -ErrorAction SilentlyContinue | Remove-Item -Force
if (Test-Path $quar) { Remove-Item -Recurse -Force $quar }
Observe 'clean'

[System.IO.File]::WriteAllBytes($zero, @())
L ("ZERO_INSTALLED=" + (Test-Path $zero) + ";bytes=" + (Get-Item $zero).Length)
Observe 'zero-byte-lease'
Observe 'zero-byte-lease-second-run'

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
