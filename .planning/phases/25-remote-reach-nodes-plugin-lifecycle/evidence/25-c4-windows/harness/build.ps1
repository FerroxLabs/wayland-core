# lane 25-c4-windows — build wayland-core.exe on D:
# Writes WLRC=<code> first and WLDONE last to a status file. The caller reads the
# status file back in a SEPARATE ssh call and IGNORES exit status, because every
# non-zero collapses to 1 over ssh+PowerShell (LANE-BRIEF §3.2).
$ErrorActionPreference = "Continue"
$root   = "D:\lane-25c4-win"
$status = "D:\lane-25c4-win\build-status.txt"
$log    = "D:\lane-25c4-win\build.log"
Remove-Item $status -ErrorAction SilentlyContinue
Set-Location $root
$env:CARGO_TARGET_DIR = "D:\lane-25c4-win\target"
& cargo build -p wcore-cli --bin wayland-core *>&1 | Tee-Object -FilePath $log
$rc = $LASTEXITCODE
"WLRC=${rc}"      | Out-File -FilePath $status -Encoding ascii
"HEAD=$(git rev-parse HEAD)" | Out-File -FilePath $status -Encoding ascii -Append
"EXE_EXISTS=$(Test-Path D:\lane-25c4-win\target\debug\wayland-core.exe)" | Out-File -FilePath $status -Encoding ascii -Append
"WLDONE"          | Out-File -FilePath $status -Encoding ascii -Append
