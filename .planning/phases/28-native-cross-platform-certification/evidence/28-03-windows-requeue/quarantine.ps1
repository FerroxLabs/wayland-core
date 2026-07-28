# 26-02 Windows quarantine leg -- the PAIRED live proof, not a path check.
#
# 26-02 proved import quarantine on Linux with a paired live proof through a REAL agent turn:
#   t19 NEGATIVE  -- import discovered >0, the payload is reported quarantined, the stream
#                    shows the Skill tool RAN and reported the skill unavailable, sentinel ABSENT
#   t20 POSITIVE  -- same payload, same turn, differing ONLY by `migrate promote`, sentinel PRESENT
# A Windows leg that only checks where the bytes sit is strictly weaker than that, so this
# runs the SAME two tests on Windows rather than re-deriving a lesser claim.
#
# PRE-MEASURED PORTABILITY PRECONDITION (recorded so a fixture defect can never be reported
# as a containment result): the payload's shell directive is `touch <sentinel>`. `touch` is
# not a cmd builtin. It was measured on this box BEFORE the run and DOES resolve --
# C:\Program Files\Git\usr\bin\touch.exe -- and `cmd /C "touch <path>"` returns 0 and creates
# the file. So the positive control is CAPABLE of firing here. Had it not been, a t20 failure
# would have meant "the fixture cannot run on Windows", not "containment leaked".

$ErrorActionPreference = 'Continue'
$src = 'C:\wl-winrequeue\src'
$log = 'C:\wl-winrequeue\out\quarantine.log'
function Log($m) { Add-Content -LiteralPath $log -Value $m -Encoding utf8 }

Set-Content -LiteralPath $log -Value "Q_START $(Get-Date -Format o)" -Encoding utf8
Log "Q_WHOAMI $(whoami)"
Log "Q_CWD $((Get-Location).Path)"
Log "Q_HEAD $(& git -C $src rev-parse HEAD)"

# ---- the portability precondition, re-asserted AT RUN TIME on the account that will run the
# test. SYSTEM and the interactive user do not share a PATH, and a `touch` that resolves for
# one and not the other would silently turn the positive control into a false red.
$probe = Join-Path $env:TEMP ("qprobe-" + [guid]::NewGuid().ToString('N') + ".txt")
cmd /C "touch $probe"
$touchRc = $LASTEXITCODE
$touchMade = Test-Path -LiteralPath $probe
Remove-Item -LiteralPath $probe -ErrorAction SilentlyContinue
Log "Q_TOUCH_RC=$touchRc"
Log "Q_TOUCH_CREATED=$touchMade"
if ($touchRc -ne 0 -or -not $touchMade) {
  Log 'Q_EXIT=60 reason=touch-unavailable-positive-control-could-not-fire'
  Log 'Q_MARKER_DONE'
  exit 60
}

$cargo = 'C:\Users\seand\.cargo\bin\cargo.exe'
Set-Location $src

# ---- the two paired legs, named explicitly so the pairing is visible in the record.
Log "Q_PAIRED_INVOKE $(Get-Date -Format o)"
$sw = [System.Diagnostics.Stopwatch]::StartNew()
& $cargo test -p wcore-cli --test migrate_quarantine t19_live_negative_leg_quarantined_payload_does_not_execute t20_live_positive_control_same_payload_executes_once_promoted -- --nocapture --test-threads=1 *>> $log
$pairRc = $LASTEXITCODE
$sw.Stop()
Log "Q_PAIRED_RC=$pairRc"
Log "Q_PAIRED_SECONDS=$([math]::Round($sw.Elapsed.TotalSeconds,2))"

# ---- the whole suite, to sit beside Linux's "29 run, 29 passed"
Log "Q_SUITE_INVOKE $(Get-Date -Format o)"
& $cargo test -p wcore-cli --test migrate_quarantine -- --test-threads=1 *>> $log
$suiteRc = $LASTEXITCODE
Log "Q_SUITE_RC=$suiteRc"

Log "Q_EXIT=$pairRc"
Log 'Q_MARKER_DONE'
exit 0
