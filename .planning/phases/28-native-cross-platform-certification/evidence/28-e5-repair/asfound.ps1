# 28-E5-REPAIR — record seandesktop as-found state BEFORE anything this lane does.
# Exit status is written to a status FILE (WLRC first, WLDONE last); it is never
# relied on across ssh, where every non-zero collapses to 1.
$ErrorActionPreference = 'Continue'
$root   = 'C:\f28e5'
$log    = "$root\asfound.log"
$status = "$root\asfound.status"
New-Item -ItemType Directory -Force -Path $root | Out-Null
Remove-Item -Force -ErrorAction SilentlyContinue $status
Set-Content -Path $log -Value ("F28E5 ASFOUND started=" + (Get-Date -Format o)) -Encoding utf8
function L($m) { Add-Content -Path $log -Value $m -Encoding utf8 }

L ("WHOAMI=" + (& whoami))
L ("HOSTNAME=" + $env:COMPUTERNAME)
L ("LOCALAPPDATA=" + $env:LOCALAPPDATA)

$d = Join-Path $env:LOCALAPPDATA 'Wayland\Core\AppContainerLeases\v1'
L ("LEASE_DIR=" + $d)
L ("LEASE_DIR_EXISTS=" + (Test-Path $d))
$n = 0
if (Test-Path $d) {
  Get-ChildItem -Recurse -Force $d | ForEach-Object {
    $script:n++
    $h = ''
    if (-not $_.PSIsContainer) { $h = (Get-FileHash -Algorithm SHA256 -Path $_.FullName).Hash.ToLower() }
    L ("LEASE_ENTRY=" + $_.FullName + "|dir=" + $_.PSIsContainer + "|len=" + $_.Length + "|sha256=" + $h)
  }
}
L ("LEASE_ENTRY_COUNT=" + $n)

foreach ($p in @('C:\f28','C:\f28e5','C:\ferrox-win-p28','C:\f28h2-repo','C:\f28h2-target')) {
  L ("PATH_EXISTS " + $p + " = " + (Test-Path $p))
}

$busy = (Get-Process -Name cargo,rustc,link -ErrorAction SilentlyContinue | Measure-Object).Count
L ("QUIET_CHECK build_processes=" + $busy)
L ("NODE=" + (& node --version))
$free = (Get-PSDrive C).Free
L ("FREE_BYTES_C=" + $free)
L ("SCHEDULED_TASKS_F28=" + ((Get-ScheduledTask -TaskName 'f28*' -ErrorAction SilentlyContinue | Measure-Object).Count))
L ("finished=" + (Get-Date -Format o))

$rc = 0
Set-Content -Path $status -Value "WLRC=${rc}" -Encoding utf8
Add-Content -Path $status -Value "WLLEASES=${n}" -Encoding utf8
Add-Content -Path $status -Value "WLBUSY=${busy}" -Encoding utf8
Add-Content -Path $status -Value "WLDONE" -Encoding utf8
exit $rc
