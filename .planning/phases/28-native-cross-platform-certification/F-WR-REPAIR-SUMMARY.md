# F-WR repair lane — two measurement failures fixed, one carried HIGH disposed

**Branch:** `lane/kr01-repair`, off `plan/f20-unified-audit-repair` at `195a856f`.
**Host:** `SeanD@seandesktop` (one physical box, two runners, other lanes live throughout).
**Never merged into the integration branch. No PR, no tag, no issue closed.**

---

## F-WR-01 — `KR-01` is **DISPROVED**. The reap works.

### It was never measuring the property

The test aborted in its own setup at `live_integrity.rs:273` with the sandboxed command exiting
1 on `Access is denied.`, so no descendant existed and the reap assertion ~30 lines below was
never evaluated. An 8-rung denial ladder (one property per rung) attributed it:

| Rung | Result |
|---|---|
| read from the granted dir | **exit 0**, script content returned |
| write into the granted dir | **exit 0** |
| `cwd` = the granted dir | **exit 0** |
| execute the script, `cwd:None` | **ran to the 20s timeout**, heartbeat **1749 bytes** |
| **the exact KR-01 shape** (nested `cmd /d /c cmd /d /c`) | **exit 1 `Access is denied.`** |

The grant, the `cwd` and execute-from-a-granted-directory all work. **The nested spawn is what
is refused.** Two construction faults, both already documented in sibling files when the test
was written: `choice.exe` (its only sleep primitive) exits in <80ms under this token, and the
descendant must be detached with `start "" /b`, the shape every *passing* descendant test uses.

### The repair, and what was not weakened

Rebuilt on those proven primitives. **The assertion was not relaxed — the witness was
strengthened**, from heartbeat file length (which cannot separate "reaped" from "alive but
starved past both sampling windows", and under load biases toward a **false pass**) to host-side
fixed-`ProcessId` liveness. `capture_alive_descendant_pids` panics if no descendant is ever
observed, so a run that proves nothing now **fails as unmeasurable**. Nothing was `#[ignore]`d
away, no timeout raised, the sandbox not relaxed.

### Measured as a distribution, per the `CLASS-WIN-LIVE-01` warning

| Mode | Profiles | Runs | Result | Reap test |
|---|---|---|---|---|
| **serial** (`--test-threads=1`), leaked profiles present | 564 | 6 | 4 passed / 1 failed, every run | **PASS 6/6** |
| serial, after profile cleanup | 0 | 4 | 4 passed / 1 failed, every run | **PASS 4/4** |
| serial, targeted `--nocapture` | 0 | 2 | pass | **PASS 2/2** |
| **parallel** (default threads), profiles present | 564 | 3 | 3/2, 3/2, 3/2 | fail |
| parallel, after cleanup | 0 | 3 | 2/3, 1/4, 3/2 | fail |

**Reap test: 12/12 PASS serially**, across competing load from 4 to 32 processes and wall clock
from 32s to 130s. Independent witness, printed:

```
KR01_WITNESS_DESCENDANTS_ALIVE_BEFORE_DROP=[31360]   KR01_WITNESS_SURVIVORS_AFTER_DROP=0 of 1
KR01_WITNESS_DESCENDANTS_ALIVE_BEFORE_DROP=[21664]   KR01_WITNESS_SURVIVORS_AFTER_DROP=0 of 1
```

A real descendant PID, observed alive by host-side CIM before the future is dropped, and gone
after. **`KR-01` may be marked DISPROVED with executable evidence, which unblocks 28-04.**

### The non-determinism has a cause, and it is not random

`CLASS-WIN-LIVE-01`'s exact signature (3 passed / 2 failed, those two tests) reproduced **3/3**
in parallel mode and **0/12** in serial. The parallel failure text is:

```
resolve_anchor_pid found 2 candidate anchors (cmd.exe children of pid 44108);
the descendant scope would be ambiguous
```

That is the fail-closed guard **declining to measure** an ambiguous scope rather than answering
wrongly — correct behaviour, not a defect. Concurrent live AppContainer executions interfere on
this host, so **`--test-threads=1` is a correctness requirement for this suite, not a
preference**. Any live-Windows figure this program recorded from a parallel run is untrustworthy.

**The leaked-profile hypothesis is refuted, with evidence:** removing all 564 profiles changed
neither mode's outcome.

---

## F-WR-02 — the zero-test suite, and the worse flavour next door

`cargo test --test live_fs_acl` exits 0 printing `test result: ok` on 0 of 12. Surveying the
class found **two flavours**:

- **Flavour A — every test `#[ignore]`d: 15 integration-test binaries.** Full inventory in the
  evidence directory. `cargo test --test X` runs zero and exits 0.
- **Flavour B — env-gated early `return`: 1 binary, `live_integrity.rs`, 5 tests.** This is
  **strictly worse**: it printed `5 passed` for zero work. Flavour A at least prints
  `0 passed; 12 ignored`, which a reader might notice; an affirmative `5 passed` reads as
  certification. This is the suite `KR-01` lives in.

**Fixed in `live_integrity.rs`** (Flavour B converted to Flavour A plus a guard): the five cases
are now honestly `#[ignore]`d with an asserting gate, and a non-`#[ignore]`d guard fails when
live intent is declared but no acceptance case can run. **Demonstrated falsifiable:**

```
env SET,   no --ignored -> test result: FAILED. 0 passed; 1 failed; 5 ignored
env UNSET, no --ignored -> test result: ok.     1 passed; 0 failed; 5 ignored
```

The first invocation printed `5 passed` before this change.

**Exposure of the remaining call sites, checked rather than assumed:** no CI workflow, justfile
target or script invokes a fully-ignored suite unsafely. `scripts/f20-native-windows-proof.ps1`
and `f20-native-macos-proof.sh` already use `--run-ignored all --no-tests=fail`, which is the
correct pattern. The real exposure is an operator or agent typing the obvious command — which is
why the fix belongs in the suite, where it is unbounded, rather than at call sites.

**I did NOT convert the other 14 Flavour-A binaries.** They are inventoried; the guard is a
~20-line pattern any of them can adopt. Claiming 16 fixed suites would be false.

---

## F-WR-06 — exit status over ssh+PowerShell

Landed earlier in the lane and already merged into `LANE-BRIEF.md` §2 and `AGENTS.md` §11.
`$LASTEXITCODE` is faithful **inside** PowerShell; the collapse is at the **ssh session
boundary** (2/3/7/100/255 all arrive as 1). Stdout sentinels are insufficient — CLIXML progress
records splice into the stream, and a status line was observed vanishing while its marker
survived. Verified carrier: status file, `WLRC` first and `WLDONE` last, read back by a separate
ssh call, exit status ignored. **7/7 faithful.** Three-state grading: no marker = incomplete,
marker without status = **UNREADABLE**, both = true code. The mid-write case was then observed
live during a real build poll, which is what the ordering exists for.

Related trap recorded: `"$LASTEXITCODE:TAG"` renders **empty** — PowerShell reads `$VAR:` as
namespace notation. Brace it.

---

## F-WR-04 — leaked state, cleaned

**564 AppContainer profiles removed (0 failures), 68 work directories** under `C:\Users\Public`
(`wcore-job-cancel-*`, `wcore-r61-*`, probe dirs), **0 leases remaining**. What leaked them: the
work directory is removed only on the test's success path, so the count was a direct census of
historical failures of the very test repaired here — a test that could not reach its assertion
leaked a directory and a profile on every run for two weeks. The repaired test tears down via
`reap_stray_descendants()` and no longer creates a `%PUBLIC%` work dir at all.

Cleanup was scoped to `wcoresandbox*`/`WCore*` profiles and `wcore-*` dirs only. No other lane's
directories or scheduled tasks were touched.

---

## New finding this lane opened

| ID | Severity | Finding |
|---|---|---|
| **F-KR-07** | **HIGH** | `live_cmd_runs_when_allowlist_has_missing_path` fails **deterministically, 12/12 serial runs**, with `SandboxError::Timeout` against a 10s manifest on a `cmd /c echo` — i.e. the field-regression test for "allowlist contains a non-existent path" does not pass on this host. Unaffected by profile cleanup. Reported, **not chased** — outside this lane's brief. |
| **F-KR-08** | **MEDIUM** | Concurrent live AppContainer executions interfere: the same suite yields 3/2, 2/3 and 1/4 in parallel versus a flat 4/1 in 12 serial runs. `--test-threads=1` is a correctness requirement for live sandbox suites. This is the cause of `CLASS-WIN-LIVE-01`. |

`F-WR-03` (execute-from-a-granted-directory uncovered by any green) is **retired**: rung 4 of
the ladder proves that path works, and the repaired test now covers descendant creation with a
green.

`desktop_contract_corpus` was **not run and not chased** — `CLASS-CONTRACT-01`, structural.
`wcore-contract generate` was **not** run.

---

## State left behind

- All six of this lane's scheduled tasks unregistered; `LEFTOVER_MY_TASKS=0`. Ten other-lane
  tasks present and **untouched**.
- `C:\ferrox-win-23B04` `LastWriteTime` still `2026-07-27 22:42:18` — predates this lane; the
  multi-day journey binding is intact. `C:\ferrox-win` never mutated (cloned read-only).
- This lane's tree is `C:\wl-kr01`. 0 leaked profiles, 0 stray `choice.exe`.
- hetzner not used at all.
- Source changes confined to `crates/wcore-sandbox/tests/`. **Neither shared-fence file
  (`crates/wcore-cli/src/{lib,main}.rs`) was touched** — verified against the captured
  merge-base `195a856f`, never against the branch name.
- `hard_process_containment_windows` re-run after the helper extraction: **6 passed / 0 failed**,
  so the refactor caused no regression.
