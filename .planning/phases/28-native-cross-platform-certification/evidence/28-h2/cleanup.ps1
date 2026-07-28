# Lane 28-h2 — leave the shared Windows box as found.
# The repro drove the REAL product, so it created a real quarantine directory
# under %LOCALAPPDATA%. The reclaimed files are copies of an archived artifact
# that still exists in C:\p22-evidence\stale-leases-backup, so removing them
# destroys no evidence.
$ErrorActionPreference = 'Continue'
$dir  = Join-Path $env:LOCALAPPDATA 'Wayland\Core\AppContainerLeases\v1'
$quar = Join-Path $dir 'quarantine'
Write-Output ("ACTIVE_LEASES_BEFORE=" + (Get-ChildItem -Path $dir -File -ErrorAction SilentlyContinue | Measure-Object).Count)
Get-ChildItem -Path $quar -File -ErrorAction SilentlyContinue | ForEach-Object { Write-Output ("QUARANTINED=" + $_.Name) }
if (Test-Path $quar) { Remove-Item -Recurse -Force $quar }
Write-Output ("QUARANTINE_REMOVED=" + (-not (Test-Path $quar)))
Write-Output ("ACTIVE_LEASES_AFTER=" + (Get-ChildItem -Path $dir -File -ErrorAction SilentlyContinue | Measure-Object).Count)
Write-Output ("BACKUP_INTACT=" + (Get-ChildItem -Path 'C:\p22-evidence\stale-leases-backup' -File -ErrorAction SilentlyContinue | Measure-Object).Count)
Write-Output ("FREE_GB=" + [math]::Round((Get-PSDrive C).Free/1GB,1))
Write-Output "CLEANUPDONE"
