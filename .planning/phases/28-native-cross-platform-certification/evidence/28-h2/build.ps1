# Lane 28-h2 — build wayland-core.exe in the isolated worktree.
# Exit status is written to a status FILE, never relied on across ssh:
# WLRC=<code> is written first, WLDONE last. A separate ssh call reads it back.
param([string]$Tag = 'before')

$ErrorActionPreference = 'Continue'
$wt     = 'C:\f28h2-repo'
$log    = "C:\f28h2\build-$Tag.log"
$status = "C:\f28h2\build-$Tag.status"

Remove-Item -Force -ErrorAction SilentlyContinue $status
Set-Content -Path $log -Value ("BUILD tag=$Tag started=" + (Get-Date -Format o)) -Encoding utf8

$env:CARGO_TARGET_DIR = 'C:\f28h2-target'
Push-Location $wt
Add-Content -Path $log -Value ("SHA=" + (& git rev-parse HEAD).Trim()) -Encoding utf8
Add-Content -Path $log -Value ("DIRTY=" + ((& git status --porcelain) | Measure-Object).Count) -Encoding utf8
& cargo build --release -p wcore-cli 2>&1 | ForEach-Object { Add-Content -Path $log -Value $_ -Encoding utf8 }
$rc = $LASTEXITCODE
Pop-Location

$exe = 'C:\f28h2-target\release\wayland-core.exe'
Add-Content -Path $log -Value ("EXE_EXISTS=" + (Test-Path $exe)) -Encoding utf8
if (Test-Path $exe) {
  Add-Content -Path $log -Value ("EXE_SHA256=" + (Get-FileHash -Algorithm SHA256 -Path $exe).Hash.ToLower()) -Encoding utf8
}
Add-Content -Path $log -Value ("finished=" + (Get-Date -Format o)) -Encoding utf8

# Brace the variable: "$rc:TAG" would render EMPTY (PowerShell namespace notation).
Set-Content -Path $status -Value "WLRC=${rc}" -Encoding utf8
Add-Content -Path $status -Value "WLTAG=${Tag}" -Encoding utf8
Add-Content -Path $status -Value "WLDONE" -Encoding utf8
