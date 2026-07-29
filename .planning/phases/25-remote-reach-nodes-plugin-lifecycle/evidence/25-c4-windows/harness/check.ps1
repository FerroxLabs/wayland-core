# lane 25-c4-windows — workspace check + relevant tests, on Windows.
# Status file: WLRC_* first, WLDONE last. Test counts are read back from the raw
# logs with a NON-proxied reader; the `N passed` / `M ignored` / `K filtered out`
# fields must be asserted, never the exit status (LANE-BRIEF 3.2).
$ErrorActionPreference = "Continue"
$root   = "D:\lane-25c4-win"
$status = "D:\lane-25c4-win\check-status.txt"
Remove-Item $status -ErrorAction SilentlyContinue
Set-Location $root
$env:CARGO_TARGET_DIR = "D:\lane-25c4-win\target"

& cargo check --workspace --all-targets *> "$root\check-workspace.log"
$rc_check = $LASTEXITCODE

& cargo test -p wcore-egress *> "$root\test-egress.log"
$rc_egress = $LASTEXITCODE

& cargo test -p wcore-exec-backend *> "$root\test-execbackend.log"
$rc_exec = $LASTEXITCODE

"WLRC_CHECK=${rc_check}"     | Out-File $status -Encoding ascii
"WLRC_EGRESS=${rc_egress}"   | Out-File $status -Encoding ascii -Append
"WLRC_EXECBACKEND=${rc_exec}"| Out-File $status -Encoding ascii -Append
"WLDONE"                     | Out-File $status -Encoding ascii -Append
