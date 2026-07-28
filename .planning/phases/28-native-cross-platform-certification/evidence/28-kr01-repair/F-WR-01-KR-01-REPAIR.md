# F-WR-01 — KR-01 repaired, measured, and DISPROVED

**Host:** `SeanD@seandesktop`, 2026-07-28. **Lane HEAD under test:** `ceae23b4` (phase A),
`88823052` (phase B). All runs via scheduled task with the F-WR-06 status-file carrier.

## What was actually wrong: the test never reached its assertion

The carried HIGH read *"descendant process tree is not reaped; a process survives its owner"*.
That is not what the red showed. The test aborted at
`live_integrity.rs:273` — the `panic!("runaway descendant exited before cancellation")` arm —
with the sandboxed command exiting **1** on **`Access is denied.`**, so no descendant was ever
created and the reap assertion ~30 lines below was never evaluated.

`2b662fe8` (Jul 14) is ancestral and added **both** the reap fix **and** this test, so a landed
fix carried its own acceptance test as a red for two weeks, with the red attributed to the very
defect the fix closed.

### The denial ladder — 8 rungs, one property each

Rather than guess at the cause, the same manifest was run against commands differing in exactly
one property. Diagnostic source: `crates/wcore-sandbox/tests/kr01_probe.rs` (temporary).

| Rung | Shape | Result |
|---|---|---|
| 1 | read the script from the granted dir, `cwd:None` | **exit 0**, full script content returned |
| 2 | write into the granted dir | **exit 0** |
| 3 | `cwd:Some(work)`, cmd builtin | **exit 0**, `cwd-ok` |
| 4 | execute the script directly, `cwd:None` | **ran to the 20s timeout**, heartbeat **1749 bytes** |
| 5 | **the exact live_integrity shape** — nested `cmd /d /c cmd /d /c <script>`, `cwd:Some` | **exit 1 `Access is denied.`** (2.7s) |
| 6 | same nested shape, `cwd:None` | exit 1 `The current directory is invalid.` |
| 7 | `choice.exe /t 1 /d y /n` | exit 1 `The current directory is invalid.` |
| 8 | builtin-hold heartbeat, nested + cwd | exit 1 `Access is denied.` |

**The grant was never the problem.** Rungs 1–3 prove read, write and cwd all work against the
granted directory, and rung 4 proves execute-from-an-fs-granted-directory works and that the
heartbeat file *does* get written when the script actually runs. This also retires the premise
behind `F-WR-03`: that path is not broken, it was simply never exercised by a green test.

**The nested spawn is what is refused.** Rungs 1/2/4 succeed with `cwd:None` while 6/7 fail on
it — the discriminator is not `cwd`, it is whether the sandboxed command must **create a child
process**. Rungs 5 and 8 show that even with a valid, granted `cwd`, the nested `/c` spawn
returns `Access is denied.`

### Two independent construction faults, both already documented in this repo

1. **`choice.exe` cannot hold under this sandbox.** `live_fs_acl.rs` and
   `hard_process_containment_windows.rs` both record, hardware-verified, that every external
   exe — `choice`/`waitfor`/`timeout`/`ping` — exits in <80ms under the Low-IL AppContainer
   restricted token. KR-01's heartbeat loop used `choice.exe` as its only sleep primitive, so
   the loop could never have held even had it started.
2. **The descendant must be detached with `start "" /b`**, which is the shape every *passing*
   descendant test on this platform uses. KR-01 used a nested `/c`, which is refused.

The knowledge required to build this test correctly was already in the repository, in a sibling
file, when the test was written.

## The repair

`live_future_drop_reaps_descendant_job_tree` rebuilt on the proven primitives:

- anchor detaches a real descendant with `start "" /b cmd /d /s /c "for /L ... do @rem"`;
- anchor holds itself alive with an inline `for /L` so the future is genuinely in flight;
- the idler is asked to hold ~60s against an ~8s anchor, so **absent job ownership it would
  outlive the cancellation** and remain observable.

**The assertion was not weakened — the witness was strengthened.** Heartbeat file length cannot
distinguish "reaped" from "alive but starved past both sampling windows", and under competing
load that bias runs toward a **false PASS**. It is replaced by host-side fixed-`ProcessId`
liveness via CIM: the descendant's PID is captured *while it is provably alive*, and the reap is
asserted against those exact PIDs.

**Non-vacuity is structural.** `capture_alive_descendant_pids(1, 20)` panics if it never
observes a live descendant, so a run that creates none now **fails as unmeasurable** rather
than aborting in setup or passing vacuously. Nothing was `#[ignore]`d away, no timeout was
raised, and the sandbox was not relaxed.

Shared observation helpers were extracted to `crates/wcore-sandbox/tests/common/mod.rs`. A
second private copy of these primitives is precisely how the broken construction arose.

## The measurement — distribution, not a single figure

`lane/254-take` independently reported this suite non-deterministic at one commit (0/5 and 3/2
on different runs), so single runs were treated as uninformative.

**Phase A — 6 runs, serial (`--test-threads=1`), at `ceae23b4`:**

| run | seconds | passed | failed | competing busy procs | leaked profiles | failing test |
|---|---|---|---|---|---|---|
| 1 | 35.20 | 4 | 1 | **32** | 564 | `live_cmd_runs_when_allowlist_has_missing_path` |
| 2 | 32.01 | 4 | 1 | 4 | 564 | same |
| 3 | 130.42 | 4 | 1 | 4 | 564 | same |
| 4 | 34.74 | 4 | 1 | 9 | 564 | same |
| 5 | 32.90 | 4 | 1 | 5 | 564 | same |
| 6 | 33.26 | 4 | 1 | 5 | 564 | same |

**`live_future_drop_reaps_descendant_job_tree` — 6/6 PASS**, across competing load from 4 to 32
processes and wall clock from 32s to 130s. `live_runaway_command_is_bounded_by_timeout` also
passed 6/6 here.

The one failure is deterministic (6/6) and is **a different test**:

```
live_cmd_runs_when_allowlist_has_missing_path
  panicked at live_integrity.rs:210
  AppContainer spawn must succeed despite a non-existent allowlist path: Timeout
```

That is a `SandboxError::Timeout` against a 10s manifest on a `cmd /c echo` — filed separately
below, and NOT the reap property.

## VERDICT on the reap property

**The reap works. `KR-01` is DISPROVED with executable evidence.**

The descendant is created, observed alive by ProcessId, and is gone after the execution future
is dropped — 6/6 serially, and the witness is fail-closed so a run that proved nothing would
have failed rather than passed.

This is a genuine disposition, not a re-attribution: the previous lane could correctly say only
that the property was **UNPROVEN**, because the scenario could not run. It now runs.
