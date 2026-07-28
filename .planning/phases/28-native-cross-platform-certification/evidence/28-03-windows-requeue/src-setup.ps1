# Materialise MY OWN source tree at the lane HEAD. Never touches another lane's checkout:
# the local clone READS C:\ferrox-win for its objects (hardlinked, instant) and every fetch
# and checkout afterwards happens inside C:\wl-winrequeue\src.
#
# C:\ferrox-win-23B04 is deliberately NOT the donor and is never touched -- a live
# multi-day journey is bound to those binaries' provenance until 2026-07-30T23:54:26Z.

$ErrorActionPreference = 'Continue'
$src  = 'C:\wl-winrequeue\src'
$log  = 'C:\wl-winrequeue\out\src-setup.log'
$SHA  = $env:WL_SHA
function Log($m) { Add-Content -LiteralPath $log -Value $m -Encoding utf8 }

Set-Content -LiteralPath $log -Value "SRC_SETUP_START $(Get-Date -Format o)" -Encoding utf8
Log "SRC_SETUP_TARGET_SHA $SHA"
if (-not $SHA) { Log 'SRC_SETUP_EXIT=80 reason=no-sha'; Log 'SRC_SETUP_MARKER_DONE'; exit 80 }

if (-not (Test-Path -LiteralPath $src)) {
  Log 'SRC_SETUP_CLONE begin'
  & git clone --no-checkout --shared C:\ferrox-win $src *>> $log
  $rc = $LASTEXITCODE
  Log "SRC_SETUP_CLONE_RC=$rc"
  if ($rc -ne 0) { Log 'SRC_SETUP_EXIT=81 reason=clone-failed'; Log 'SRC_SETUP_MARKER_DONE'; exit 81 }
}

& git -C $src remote set-url origin 'https://github.com/FerroxLabs/wayland-core.git' *>> $log
Log 'SRC_SETUP_FETCH begin'
& git -C $src fetch --no-tags origin lane/windows-requeue *>> $log
$rc = $LASTEXITCODE
Log "SRC_SETUP_FETCH_RC=$rc"
if ($rc -ne 0) { Log 'SRC_SETUP_EXIT=82 reason=fetch-failed'; Log 'SRC_SETUP_MARKER_DONE'; exit 82 }

& git -C $src checkout --force --detach $SHA *>> $log
$rc = $LASTEXITCODE
Log "SRC_SETUP_CHECKOUT_RC=$rc"
if ($rc -ne 0) { Log 'SRC_SETUP_EXIT=83 reason=checkout-failed'; Log 'SRC_SETUP_MARKER_DONE'; exit 83 }

$head = (& git -C $src rev-parse HEAD)
Log "SRC_SETUP_HEAD $head"
if ($head.Trim() -ne $SHA) { Log 'SRC_SETUP_EXIT=84 reason=head-mismatch'; Log 'SRC_SETUP_MARKER_DONE'; exit 84 }

# The two crate trees the KR-01 transfer argument rests on, read from the tree itself.
Log ("SRC_SETUP_TREE_wcore_sandbox " + (& git -C $src rev-parse 'HEAD:crates/wcore-sandbox'))
Log ("SRC_SETUP_TREE_wcore_types "   + (& git -C $src rev-parse 'HEAD:crates/wcore-types'))
Log 'SRC_SETUP_EXIT=0'
Log 'SRC_SETUP_MARKER_DONE'
exit 0
