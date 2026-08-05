$ErrorActionPreference = "Continue"
$root   = "D:\lane-25c4-win"
$status = "$root\build-status.txt"
Remove-Item $status -ErrorAction SilentlyContinue
Set-Location $root
$env:CARGO_TARGET_DIR = "$root\target"
git fetch --quiet origin lane/25-c4-windows
git checkout --quiet -B lane/25-c4-windows FETCH_HEAD
& cargo build -p wcore-cli --bin wayland-core *> "$root\build.log"
$rc = $LASTEXITCODE
"WLRC=${rc}"                 | Out-File $status -Encoding ascii
"HEAD=$(git rev-parse HEAD)" | Out-File $status -Encoding ascii -Append
"EXE=$(Test-Path $root\target\debug\wayland-core.exe)" | Out-File $status -Encoding ascii -Append
"WLDONE"                     | Out-File $status -Encoding ascii -Append
