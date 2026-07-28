# 28-E5-REPAIR — leave seandesktop as it was found, and PROVE it rather than assert it.
$ErrorActionPreference='Continue'
$out = 'C:\Users\seand\f28e5-cleanup.log'
Set-Content -Path $out -Value ("F28E5 CLEANUP started=" + (Get-Date -Format o)) -Encoding utf8
function L($m){ Add-Content -Path $out -Value $m -Encoding utf8 }

foreach ($t in @('f28e5WinMatrix','f28e5WedgeDiff')) {
  Unregister-ScheduledTask -TaskName $t -Confirm:$false -ErrorAction SilentlyContinue
  L ("TASK_REMOVED " + $t + " still_present=" + [bool](Get-ScheduledTask -TaskName $t -ErrorAction SilentlyContinue))
}

if (Test-Path C:\f28e5) { Remove-Item -Recurse -Force C:\f28e5 }
L ("F28E5_DIR_PRESENT=" + (Test-Path C:\f28e5))

$dir  = Join-Path $env:LOCALAPPDATA 'Wayland\Core\AppContainerLeases\v1'
$quar = Join-Path $dir 'quarantine'
L ("LEASE_DIR_EXISTS=" + (Test-Path $dir))
L ("LEASE_FILES=" + ((Get-ChildItem -Path $dir -File -ErrorAction SilentlyContinue | Measure-Object).Count))
L ("QUARANTINE_DIR_EXISTS=" + (Test-Path $quar))

# The archive the wedge artifact was COPIED from must be untouched.
$bak = 'C:\p22-evidence\stale-leases-backup'
L ("ARCHIVE_EXISTS=" + (Test-Path $bak))
Get-ChildItem -Path $bak -File -ErrorAction SilentlyContinue | ForEach-Object {
  L ("ARCHIVE_FILE=" + $_.Name + "|len=" + $_.Length + "|sha256=" + (Get-FileHash -Algorithm SHA256 $_.FullName).Hash.ToLower())
}
L ("OTHER_F28_TASKS=" + ((Get-ScheduledTask -TaskName 'f28*' -ErrorAction SilentlyContinue | Measure-Object).Count))
L ("finished=" + (Get-Date -Format o))
Write-Output 'CLEANUP_DONE'
