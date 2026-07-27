# Decision A — did both interruption legs prove exact rollback from a genuinely mid-flight kill, or is either leg vacuous?

`wayland-core backup restore` journals the prior tree before mutating, so an
operation killed mid-flight can be rolled back exactly. Two platform legs claim
to prove that. Judge whether either is vacuous.

Answer with exactly one line `PANEL-VERDICT: <option-id>` and one line
`PANEL-BASIS: <one sentence>`, then up to 250 words.

## Options (rotated order; ids are fixed)

### `windows-exactness-gap`
- **Name:** The Windows leg landed mid-flight but did not reach exactness; record the gap and its severity
- **Pros:** Names a real platform difference honestly instead of averaging it away.
- **Cons:** Only legitimate when the Windows leg is NON-vacuous and its digests differ; recording a vacuous leg as an exactness gap is rounding up.

### `both-legs-sound`
- **Name:** Both legs proved exact rollback from a genuinely mid-flight kill
- **Pros:** Each leg independently established mid-flight landing — an open journal record AND an observably intermediate target AND a kill that did not land after completion — before any digest comparison counted, and both produced equal pre- and post-recovery digests under the same named algorithm.
- **Cons:** Nothing is recorded as outstanding, so any residual weakness in either leg's controls ships as proven.

### `linux-leg-vacuous`
- **Name:** The Linux leg proved nothing — its kill did not land mid-flight
- **Pros:** A consistent tree after a kill that landed late is trivially consistent and is not a rollback proof.
- **Cons:** Contradicted if the capture shows an open journal, an intermediate target and completed_before_kill=no.

### `windows-leg-vacuous`
- **Name:** The Windows leg proved nothing — its kill did not land mid-flight
- **Pros:** Same reasoning as above, applied to Windows; a leg whose kill landed after completion must be re-run with a fixture sized for that hardware rather than dressed up as a platform gap.
- **Cons:** Contradicted if the Windows capture shows an open journal, an intermediate target and completed_before_kill=no.

## Evidence in this directory

- `interrupt-evidence-linux.txt`, `interrupt-evidence-windows.txt` — the two main runs.
- `interrupt-negctl-linux.txt`, `interrupt-negctl-windows.txt` — negative controls: a deliberately undersized fixture, which the script must DETECT as a late kill and exit non-zero for.
- `interrupt-handlerctl-linux.txt`, `interrupt-handlerctl-windows.txt` — handler controls: a CATCHABLE mechanism, for which the probe must fire.
- `interrupt-openhandle-windows.txt` — Windows-only: the target held open by another handle.

## The measured state, stated plainly

Both main runs report `MIDFLIGHT-JOURNAL-OPEN: yes`, `MIDFLIGHT-TARGET-INTERMEDIATE: yes`,
`completed_before_kill=no`, and `DIGEST-EQUAL: yes`. The two `DIGEST-ALGO` lines are
byte-equal, so the comparison measures content rather than encoding.

Both negative controls DETECTED their late kill and exited 9, so each platform's
mid-flight check demonstrably fires.

**The asymmetry you must weigh.** The handler control — the leg that turns
`fired=no` from an assumption into a measurement of uncatchability — passes on
Linux (`HANDLER-CONTROL: fired=yes`) and FAILS on Windows (`fired=no`). On
Windows the probe is now proven to ARM (`installed=` is read from a marker the
binary writes only after a handler registers, no longer a hardcoded string), and
the console CTRL_C event is proven to be DELIVERED (the helper exits 0 and the
target process dies, leaving `DIGEST-EQUAL: no`) — but the process terminates
before the probe file is written, so the probe never records the event.

So on Windows: rollback exactness under `TerminateProcess` is measured and
holds; the claim that `TerminateProcess` was *uncatchable* is NOT independently
established, because the paired control that would establish it does not fire.

Does that asymmetry make the Windows leg vacuous, or is it a control gap that
leaves the leg's rollback claim intact? Note that `TerminateProcess` is
uncatchable by documented Win32 semantics regardless of what the probe records.
