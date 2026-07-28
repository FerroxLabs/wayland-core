# 29-03 Windows downgrade-refusal leg -- mirrors the Linux measurement exactly.  (attempt 2)
#
# Linux (evidence/29-03/live-downgrade.txt) measured the refusal THROUGH THE SHIPPED BINARY
# against the REAL public GitHub API, with NO update-source redirect of any kind, by rebuilding
# the package at version 0.99.0 so the newest real release (v0.12.25) becomes a DOWNGRADE.
# Same construction here: no redirect, no credential, refusal read from the binary's own
# output, and the version re-read afterwards to prove the binary did not swap itself.
#
# WHY ATTEMPT 2: attempt 1 rewrote the first `version = "..."` line in
# crates/wcore-cli/Cargo.toml. That crate declares `version.workspace = true` and carries NO
# version line at all, so the edit matched nothing and the build would have produced a 0.12.25
# binary -- for which the newest real release is the SAME version, not a downgrade, and the
# refusal under test would never have been exercised. The empty `D_BASELINE_VERSION_LINE` in
# attempt 1's log is what exposed it: a field that cannot be empty came back empty.
#
# The version actually lives in the ROOT Cargo.toml under [workspace.package], which is the
# line the Linux run edited too ("baseline package version: version = 0.12.25").
# `self_update.rs:55` reads `env!("CARGO_PKG_VERSION")`, which resolves through that.

$ErrorActionPreference = 'Continue'
$src = 'C:\wl-winrequeue\src'
$log = 'C:\wl-winrequeue\out\downgrade2.log'
$cargo = 'C:\Users\seand\.cargo\bin\cargo.exe'
function Log($m) { Add-Content -LiteralPath $log -Value $m -Encoding utf8 }

Set-Content -LiteralPath $log -Value "D_START $(Get-Date -Format o)" -Encoding utf8
Log "D_WHOAMI $(whoami)"
Log "D_HEAD $(& git -C $src rev-parse HEAD)"

$manifest = Join-Path $src 'Cargo.toml'
if (-not (Test-Path -LiteralPath $manifest)) { Log 'D_EXIT=50 reason=root-manifest-missing'; Log 'D_MARKER_DONE'; exit 50 }
$orig = Get-Content -LiteralPath $manifest -Raw

# Rewrite ONLY the version inside [workspace.package].
$baseline = [regex]::Match($orig, '(?ms)\[workspace\.package\].*?^\s*version\s*=\s*"([^"]+)"').Groups[1].Value
Log "D_BASELINE_VERSION $baseline"
# A field that cannot legitimately be empty is asserted non-empty, because attempt 1 failed
# precisely by rendering an unmeasurable value as blank and carrying on.
if ([string]::IsNullOrWhiteSpace($baseline)) { Log 'D_EXIT=52 reason=baseline-version-not-found'; Log 'D_MARKER_DONE'; exit 52 }

$bumped = [regex]::Replace($orig, '(?ms)(\[workspace\.package\].*?^\s*version\s*=\s*")[^"]+(")', '${1}0.99.0${2}')
Set-Content -LiteralPath $manifest -Value $bumped -NoNewline -Encoding utf8
$check = [regex]::Match((Get-Content -LiteralPath $manifest -Raw), '(?ms)\[workspace\.package\].*?^\s*version\s*=\s*"([^"]+)"').Groups[1].Value
Log "D_BUMPED_VERSION $check"
if ($check -ne '0.99.0') {
  Set-Content -LiteralPath $manifest -Value $orig -NoNewline -Encoding utf8
  Log 'D_EXIT=53 reason=bump-did-not-take'; Log 'D_MARKER_DONE'; exit 53
}

Set-Location $src
Log "D_BUILD_INVOKE $(Get-Date -Format o)"
& $cargo build --release -p wcore-cli --bin wayland-core *>> $log
$buildRc = $LASTEXITCODE
Log "D_BUILD_RC=$buildRc"

$exe = Join-Path $src 'target\release\wayland-core.exe'
if ($buildRc -ne 0 -or -not (Test-Path -LiteralPath $exe)) {
  Set-Content -LiteralPath $manifest -Value $orig -NoNewline -Encoding utf8
  Log 'D_RESTORED_MANIFEST yes'
  Log 'D_EXIT=51 reason=build-failed'; Log 'D_MARKER_DONE'; exit 51
}

Log '--- wayland-core.exe --version ---'
& $exe --version *>> $log
Log "D_VERSION_RC=$LASTEXITCODE"

Log '--- self-update --check-only against the REAL api.github.com, no credential, no redirect ---'
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

Set-Content -LiteralPath $manifest -Value $orig -NoNewline -Encoding utf8
Log 'D_RESTORED_MANIFEST yes'
Log ("D_RESTORED_VERSION " + [regex]::Match((Get-Content -LiteralPath $manifest -Raw), '(?ms)\[workspace\.package\].*?^\s*version\s*=\s*"([^"]+)"').Groups[1].Value)

Log "D_EXIT=0"
Log 'D_MARKER_DONE'
exit 0
