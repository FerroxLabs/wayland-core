# f28 Windows soak runner -- executed by a SCHEDULED TASK, never as an ssh session child.
#
# WHY A SCHEDULED TASK: Windows OpenSSH terminates session children on disconnect. A prior
# run on this box wrote 1 of 600 heartbeats as an ssh child and ran to completion under a
# task. Success is read from the EXIT MARKER in this log, never from the ssh call returning.
#
# STATUS DISCIPLINE: $LASTEXITCODE is captured on its own line immediately after the call and
# emitted as an explicit token. It is never carried through a pipeline -- a pipeline reports
# the LAST command's status, which has twice reported a non-zero run as 0 on this program.
# It is never captured as `$x = & { ...; $LASTEXITCODE }` either -- that form array-filters
# and has reported a fully passing run as a failure (wayland-e2e-windows-soak.ps1:174-190).
#
# WHY THIS WAITS: the two registered GitHub runners are ONE PHYSICAL BOX. A red produced
# under a concurrent cargo build is a load artifact, not a recordable product red. The run
# therefore waits for a PROVEN quiet window and names it in the log, and refuses to run at
# all rather than silently record a loaded run as a clean one.

$ErrorActionPreference = 'Continue'
$base = 'C:\wl-winrequeue'
$log  = Join-Path $base 'out\soak.log'
$out  = Join-Path $base 'out\windows-soak.json'
$node = 'C:\Program Files\nodejs\node.exe'
$exe  = Join-Path $base 'in\wayland-core.exe'
$LEDGER = '54b12e8e5576ee54e88a93975c360e6c624202059f449d80574b71adf00c631e'

function Log($m) { Add-Content -LiteralPath $log -Value $m -Encoding utf8 }
function BusyProcs { @(Get-Process -Name cargo,rustc,link,cl -ErrorAction SilentlyContinue) }

Set-Content -LiteralPath $log -Value "F28_SOAK_START $(Get-Date -Format o)" -Encoding utf8
Log "F28_SOAK_WHOAMI $(whoami)"
Log "F28_SOAK_CWD $((Get-Location).Path)"
Log "F28_SOAK_PID $PID"

# ---- digest binding, asserted on the HOST before the first session.
# A family running a different build is not certifying the candidate, so this fails closed.
if (-not (Test-Path -LiteralPath $exe)) { Log 'F28_SOAK_EXIT=90 reason=binary-missing'; Log 'F28_SOAK_MARKER_DONE'; exit 90 }
$actual = (Get-FileHash -LiteralPath $exe -Algorithm SHA256).Hash.ToLower()
Log "F28_SOAK_BINARY_SHA256 $actual"
Log "F28_SOAK_LEDGER_SHA256 $LEDGER"
if ($actual -ne $LEDGER) { Log 'F28_SOAK_EXIT=91 reason=digest-mismatch'; Log 'F28_SOAK_MARKER_DONE'; exit 91 }
Log 'F28_SOAK_DIGEST_BOUND yes'

if (-not (Test-Path -LiteralPath $node)) { Log 'F28_SOAK_EXIT=92 reason=node-missing'; Log 'F28_SOAK_MARKER_DONE'; exit 92 }
foreach ($f in @('in\f28-native-soak.mjs','in\candidate.json','in\bands.json')) {
  if (-not (Test-Path -LiteralPath (Join-Path $base $f))) { Log "F28_SOAK_EXIT=94 reason=missing-input:$f"; Log 'F28_SOAK_MARKER_DONE'; exit 94 }
}

# ---- wait for a quiet window: 3 consecutive clear samples, 20s apart, up to 120 minutes.
$deadline = (Get-Date).AddMinutes(120)
$clear = 0
while ((Get-Date) -lt $deadline) {
  $b = BusyProcs
  if ($b.Count -eq 0) { $clear += 1 } else { $clear = 0 }
  Log ("F28_SOAK_WAIT t={0} busy={1} consecutive_clear={2}" -f (Get-Date -Format o), $b.Count, $clear)
  if ($clear -ge 3) { break }
  Start-Sleep -Seconds 20
}
if ($clear -lt 3) {
  Log 'F28_SOAK_QUIET_WINDOW no'
  Log 'F28_SOAK_EXIT=93 reason=no-quiet-window-within-120min'
  Log 'F28_SOAK_MARKER_DONE'
  exit 93
}
Log 'F28_SOAK_QUIET_WINDOW yes'
Log "F28_SOAK_QUIET_WINDOW_START $(Get-Date -Format o)"

$soakArgs = @(
  (Join-Path $base 'in\f28-native-soak.mjs'),
  '--bin',        $exe,
  '--candidate',  (Join-Path $base 'in\candidate.json'),
  '--bands',      (Join-Path $base 'in\bands.json'),
  '--family',     'windows',
  '--host',       'seandesktop',
  '--target',     'x86_64-pc-windows-msvc',
  '--out',        $out,
  '--sessions',   '1000',
  '--concurrency','4'
)
Log "F28_SOAK_ARGV $($soakArgs -join ' ')"
Log "F28_SOAK_INVOKE $(Get-Date -Format o)"

& $node @soakArgs *>> $log
$rc = $LASTEXITCODE

Log "F28_SOAK_FINISH $(Get-Date -Format o)"
# Competing load re-sampled AFTER the run so the window is provably quiet at BOTH ends
# rather than only at the moment the run was admitted.
$after = BusyProcs
Log "F28_SOAK_COMPETING_AFTER $($after.Count)"
foreach ($b in $after) { Log "F28_SOAK_COMPETING_AFTER_PROC pid=$($b.Id) name=$($b.ProcessName)" }
Log "F28_SOAK_EXIT=$rc"
if (Test-Path -LiteralPath $out) {
  Log "F28_SOAK_OUT_BYTES $((Get-Item -LiteralPath $out).Length)"
} else {
  Log 'F28_SOAK_OUT_BYTES 0'
}
Log 'F28_SOAK_MARKER_DONE'
exit $rc
