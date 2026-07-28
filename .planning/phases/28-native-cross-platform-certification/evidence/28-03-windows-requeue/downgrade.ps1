# 29-03 Windows downgrade-refusal leg -- mirrors the Linux measurement exactly.
#
# Linux (evidence/29-03/live-downgrade.txt) measured the refusal THROUGH THE SHIPPED BINARY
# against the REAL public GitHub API, with NO update-source redirect of any kind, by
# rebuilding the package at version 0.99.0 so the newest real release (v0.12.25) is a
# DOWNGRADE. Same construction here. In particular:
#   - no redirect: the binary talks to api.github.com, exactly as a user's would
#   - no credential is supplied
#   - the refusal is read from the binary's own output, and the binary is re-versioned
#     afterwards to prove it did not swap itself
#
# Linux reference result: check-only rc=0 with 1 refusal; INSTALL path rc=1; version after
# the refused install still 0.99.0.

$ErrorActionPreference = 'Continue'
$src = 'C:\wl-winrequeue\src'
$log = 'C:\wl-winrequeue\out\downgrade.log'
function Log($m) { Add-Content -LiteralPath $log -Value $m -Encoding utf8 }

Set-Content -LiteralPath $log -Value "D_START $(Get-Date -Format o)" -Encoding utf8
Log "D_WHOAMI $(whoami)"
Log "D_HEAD $(& git -C $src rev-parse HEAD)"

$cargo = 'C:\Users\seand\.cargo\bin\cargo.exe'
$manifest = Join-Path $src 'crates\wcore-cli\Cargo.toml'
if (-not (Test-Path -LiteralPath $manifest)) { Log 'D_EXIT=50 reason=manifest-missing'; Log 'D_MARKER_DONE'; exit 50 }

$orig = Get-Content -LiteralPath $manifest -Raw
Log ("D_BASELINE_VERSION_LINE " + (($orig -split "`n") | Where-Object { $_ -match '^\s*version\s*=' } | Select-Object -First 1))

# ---- throwaway re-version to 0.99.0. Only the FIRST version key (the package version) is
# touched; a global replace would rewrite dependency version requirements too.
$bumped = [regex]::Replace($orig, '(?m)^(\s*version\s*=\s*")[^"]+(")', '${1}0.99.0${2}', 1)
Set-Content -LiteralPath $manifest -Value $bumped -NoNewline -Encoding utf8
Log ("D_BUMPED_VERSION_LINE " + (($bumped -split "`n") | Where-Object { $_ -match '^\s*version\s*=' } | Select-Object -First 1))

Set-Location $src
Log "D_BUILD_INVOKE $(Get-Date -Format o)"
& $cargo build --release -p wcore-cli --bin wayland-core *>> $log
$buildRc = $LASTEXITCODE
Log "D_BUILD_RC=$buildRc"

$exe = Join-Path $src 'target\release\wayland-core.exe'
if ($buildRc -ne 0 -or -not (Test-Path -LiteralPath $exe)) {
  Set-Content -LiteralPath $manifest -Value $orig -NoNewline -Encoding utf8
  Log 'D_RESTORED_MANIFEST yes'
  Log 'D_EXIT=51 reason=build-failed'
  Log 'D_MARKER_DONE'
  exit 51
}

Log '--- wayland-core.exe --version ---'
& $exe --version *>> $log
Log "D_VERSION_RC=$LASTEXITCODE"

Log '--- self-update --check-only against the REAL api.github.com, no credential ---'
& $exe self-update --check-only *>> $log
$checkRc = $LASTEXITCODE
Log "D_CHECK_RC=$checkRc"

Log '--- self-update (INSTALL path, same conditions) ---'
& $exe self-update *>> $log
$installRc = $LASTEXITCODE
Log "D_INSTALL_RC=$installRc"

Log '--- did the binary swap itself? ---'
& $exe --version *>> $log
Log "D_VERSION_AFTER_RC=$LASTEXITCODE"
Log "D_EXE_SHA256_AFTER $((Get-FileHash -LiteralPath $exe -Algorithm SHA256).Hash.ToLower())"

# restore the manifest so the tree is left as fetched
Set-Content -LiteralPath $manifest -Value $orig -NoNewline -Encoding utf8
Log "D_RESTORED_MANIFEST yes"
Log ("D_RESTORED_VERSION_LINE " + ((Get-Content -LiteralPath $manifest) | Where-Object { $_ -match '^\s*version\s*=' } | Select-Object -First 1))

Log "D_EXIT=0"
Log 'D_MARKER_DONE'
exit 0
