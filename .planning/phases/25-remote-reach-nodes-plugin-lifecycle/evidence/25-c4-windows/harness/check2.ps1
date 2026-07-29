$ErrorActionPreference = "Continue"
$root = "D:\lane-25c4-win"
$status = "$root\check-status.txt"
Remove-Item $status -ErrorAction SilentlyContinue
Set-Location $root
$env:CARGO_TARGET_DIR = "$root\target"
$env:CARGO_INCREMENTAL = "0"
$lines = @()

& cargo check --workspace --all-targets *> "$root\check-workspace.log"
$lines += "WLRC_CHECK=$LASTEXITCODE"

& cargo test -p wcore-egress *> "$root\test-egress.log"
$lines += "WLRC_EGRESS=$LASTEXITCODE"

& cargo test -p wcore-exec-backend *> "$root\test-execbackend.log"
$lines += "WLRC_EXECBACKEND=$LASTEXITCODE"

& cargo test -p wcore-cli --lib *> "$root\test-cli-lib.log"
$lines += "WLRC_CLILIB=$LASTEXITCODE"

# --- isolation re-runs of the two failures seen in the parallel suite ---------
& cargo test -p wcore-egress tests::transport_failure_records_one_stable_error_class -- --exact --test-threads=1 *> "$root\iso-egress.log"
$lines += "WLRC_ISO_EGRESS=$LASTEXITCODE"

& cargo test -p wcore-exec-backend registry::tests:: -- --test-threads=1 *> "$root\iso-registry.log"
$lines += "WLRC_ISO_REGISTRY=$LASTEXITCODE"

$lines += "WLDONE"
$lines | Out-File $status -Encoding ascii
