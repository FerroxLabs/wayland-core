# 26-02 Windows quarantine leg -- the PAIRED live proof, not a path check.  (attempt 2)
#
# 26-02 proved import quarantine on Linux with a paired live proof driven through a REAL agent
# turn against the REAL binary:
#   t19 NEGATIVE -- import discovered >0, the payload is reported quarantined, the stream shows
#                   the Skill tool RAN and reported the skill unavailable, sentinel ABSENT
#   t20 POSITIVE -- same payload, same turn, differing ONLY by `migrate promote`, sentinel PRESENT
# A Windows leg that only checks where the bytes sit is strictly weaker than that, so this runs
# the SAME two tests rather than re-deriving a lesser claim.
#
# WHY ATTEMPT 2: attempt 1 passed BOTH test names to one `cargo test` invocation, and cargo
# rejects a second positional filter ("error: unexpected argument 't20_...' found"). It failed
# in 0.08s. That is far too fast for a cargo build plus two live agent turns, and the elapsed
# time is what exposed it -- the same signal that stopped the KR-01 leg mis-filing a finding.
# Running each leg as its OWN invocation is also better evidence: the negative and the positive
# each carry their own recorded exit status instead of sharing one.
#
# PRE-MEASURED PORTABILITY PRECONDITION: the payload's directive is `touch <sentinel>`, and
# `touch` is not a cmd builtin. Measured on this box BEFORE the run: it resolves to
# C:\Program Files\Git\usr\bin\touch.exe and `cmd /C "touch <path>"` returns 0 and creates the
# file. So the positive control is CAPABLE of firing. Re-asserted at run time below, because
# SYSTEM and the interactive user do not share a PATH -- without that, a t20 failure would mean
# "the fixture cannot run here", not "containment leaked".

$ErrorActionPreference = 'Continue'
$src = 'C:\wl-winrequeue\src'
$log = 'C:\wl-winrequeue\out\quarantine2.log'
$cargo = 'C:\Users\seand\.cargo\bin\cargo.exe'
function Log($m) { Add-Content -LiteralPath $log -Value $m -Encoding utf8 }

Set-Content -LiteralPath $log -Value "Q_START $(Get-Date -Format o)" -Encoding utf8
Log "Q_WHOAMI $(whoami)"
Log "Q_CWD $((Get-Location).Path)"
Log "Q_HEAD $(& git -C $src rev-parse HEAD)"

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

Set-Location $src

# ---- NEGATIVE leg, on its own.
Log "Q_NEG_INVOKE $(Get-Date -Format o)"
$sw = [System.Diagnostics.Stopwatch]::StartNew()
& $cargo test -p wcore-cli --test migrate_quarantine t19_live_negative_leg_quarantined_payload_does_not_execute -- --exact --nocapture --test-threads=1 *>> $log
$negRc = $LASTEXITCODE
$sw.Stop()
Log "Q_NEG_RC=$negRc"
Log "Q_NEG_SECONDS=$([math]::Round($sw.Elapsed.TotalSeconds,2))"

# ---- POSITIVE control, on its own.
Log "Q_POS_INVOKE $(Get-Date -Format o)"
$sw2 = [System.Diagnostics.Stopwatch]::StartNew()
& $cargo test -p wcore-cli --test migrate_quarantine t20_live_positive_control_same_payload_executes_once_promoted -- --exact --nocapture --test-threads=1 *>> $log
$posRc = $LASTEXITCODE
$sw2.Stop()
Log "Q_POS_RC=$posRc"
Log "Q_POS_SECONDS=$([math]::Round($sw2.Elapsed.TotalSeconds,2))"

# ---- the whole suite, to sit beside Linux's "29 run, 29 passed"
Log "Q_SUITE_INVOKE $(Get-Date -Format o)"
& $cargo test -p wcore-cli --test migrate_quarantine -- --test-threads=1 *>> $log
$suiteRc = $LASTEXITCODE
Log "Q_SUITE_RC=$suiteRc"

# The pairing is only informative if BOTH legs behaved. A negative that passes while the
# positive control did NOT fire proves nothing -- absence would be equally consistent with the
# payload never loading at all.
if ($negRc -eq 0 -and $posRc -eq 0) {
  Log 'Q_VERDICT=PAIRED_PROOF_HOLDS'
} elseif ($negRc -eq 0 -and $posRc -ne 0) {
  Log 'Q_VERDICT=UNINFORMATIVE reason=positive-control-did-not-fire'
} elseif ($negRc -ne 0) {
  Log 'Q_VERDICT=NEGATIVE_LEG_FAILED'
}
Log "Q_EXIT=$negRc"
Log 'Q_MARKER_DONE'
exit 0
