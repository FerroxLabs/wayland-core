---
lane: journey-rescue
defect: >-
  scripts/f23-multi-day-journey.sh:67 read a bare $HOME under `set -u` (line 28).
  A systemd transient service does not export HOME, so the scheduled Linux resume
  aborted at that line before doing any work: "line 67: HOME: unbound variable",
  rc=1, on 2026-07-28T14:25:00Z. f23-journey-day3.timer was armed to fail identically.
fix: >-
  Derive the journey root explicitly — --root, then HOME only if actually set, then
  the passwd database (getent on Linux, dscl on Darwin) — and REFUSE if all three
  yield nothing. `set -u` retained. Matches the Windows port's explicit -Root shape.
proof-harness: >-
  systemd-run transient unit with no HOME exported (HOME=[UNSET] measured while USER
  and PATH are set). Counterfactual pair on the identical harness: pre-fix WLRC=1
  "HOME: unbound variable"; post-fix WLRC=0 with day 2 recorded and 6/6 invariants PASS.
chain-decision: >-
  RESUME the existing chain, and run the missed day 2 late as a genuine execution.
  Not restarted (would reset a 3-day clock for zero evidentiary gain) and no row was
  injected. Chain is day1 2026-07-27T14:21:19Z + day2 2026-07-29T10:14:57Z, both real.
timer-verified: >-
  f23-journey-day3.timer fires 2026-07-30 14:31:00 UTC; its service has Environment=
  and User= empty, identical to the rehearsal harness, and executes the repaired
  script via /root/f23-journey-day.sh. Deployed file sha256 == lane commit. The real
  wrapper was rehearsed under systemd-run and exited 0.
fence-exposure: >-
  none. 2 files vs 861d1b1a: this report + scripts/f23-multi-day-journey.sh (+32/-1).
  0 files under .github/workflows/, 0 in crates/wcore-cli/src/{lib,main}.rs, 0 *.rs.
  Windows leg untouched.
status: complete
---

# 23B — Journey Rescue (Phase 23 Criterion 5, Linux leg)

## 1. The defect

`scripts/f23-multi-day-journey.sh` sets `set -uo pipefail` (line 28) and then read a bare
`$HOME` at line 67:

```bash
[ -n "$ROOT" ] || ROOT="$HOME/.f23-journey-$PLATFORM"
```

The deployed timer wrapper `/root/f23-journey-day.sh` does **not** pass `--root`, so that
fallback branch is always taken. A systemd transient service does not export `HOME`, so under
`set -u` the script aborted at line 67 **before it did any work at all**.

Production evidence, `/root/.f23-journey-linux/scheduled-day2.log`:

```
scripts/f23-multi-day-journey.sh: line 67: HOME: unbound variable
```

and `scheduled.log`: `scheduled day 2 exited 1 at 2026-07-28T14:25:00Z`.

**Why it survived review:** the failure is invisible to a shell test. Every interactive and
ssh shell exports `HOME`, so the script passes by hand and dies only under the scheduler.

**The asymmetry:** the Windows port already guarded this exact class with an explicit `-Root`
parameter (`f23-multi-day-journey.ps1:28`) and a `$env:USERPROFILE` fallback that scheduled
tasks do populate. One platform was hardened; the same defect class was left open on the other.

## 2. The fix

`scripts/f23-multi-day-journey.sh`, +32/-1. `set -u` is **retained** — dropping it would trade
one loud failure for a silent one, relocating a multi-day journey's state and letting an empty
run log read as "the journey did not run".

Resolution order: `--root` → `HOME` only if actually set → the passwd database
(`getent` on Linux, `dscl` on Darwin) → **refuse**. The refusal matters: a default here would
silently move the journey root and turn a lost root into a false negative. The script now also
echoes `F23_04_JOURNEY_ROOT=<path>` so the root it chose is on the record every run.

Sweep for other landmines in the same file: only `${PIPESTATUS[0]}` and `${BASH_SOURCE[0]}`
remain, both always-set bash builtins. Known-positive for that grep: 14 `PLATFORM` hits.

## 3. Proof, in the environment that actually broke

A shell test proves nothing here, so both legs ran under `systemd-run`, invoked exactly as the
timer's wrapper does — **no `--root`** (that is the branch under test) and **no `Environment=`**
injected.

**Instrument liveness first** — the harness genuinely lacks `HOME`:

```
HOME=[UNSET]   USER=[root]   PATH_SET=[yes]
```

`USER` and `PATH` being present is what makes this a measurement rather than a broken probe.

### The counterfactual pair (same harness, same args, same unit type)

| Leg | Driver | Status file | Log |
|---|---|---|---|
| A (pre-fix) | pre-fix snapshot of the driver | `WLRC=1` | `line 67: HOME: unbound variable` |
| B (post-fix) | repaired driver | `WLRC=0` | root derived, day 2 recorded |

Leg B output:

```
F23_04_JOURNEY_ROOT=/root/.f23-journey-linux
F23_04_PROVENANCE=ok platform=linux sha=0ed05322462e64cb44e2b80aa15b7357263b8187
F23_04_WAIT_CONDITION_ELAPSED_SECONDS=158017 required=259200
F23_04_WAIT_CONDITION_MET=false
F23_04_DAY=2 platform=linux ts=2026-07-29T10:14:57Z host=Ubuntu-2404-noble-amd64-base pid=4002802
F23_04_INVARIANT=loop-owner|cumulative-budget|authority-envelope|memory-recall|evidence-chain|delivery-once  all status=PASS
F23_04_DAY_INVARIANTS_ALL_PASS=true
F23_04_STEP=OK day=2 platform=linux nonce=4ab44763e42849fd
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 2 filtered out
```

The `1 passed / 0 ignored / 0 filtered`-style count is read back in full (via unproxied ssh, so
the `ignored`/`filtered out` fields survive) — the suite is not vacuous.

`WAIT_CONDITION_MET=false` is load-bearing: the wait correctly refused to complete early.

### The instrument carried the defect class it was hunting — repaired, not noted

My first harness wrote its exit code with `echo "WLRC=${rc}"` inside an inline `systemd-run
/bin/bash -c` string. **systemd performs unit-file variable expansion on an inline command**, so
`${rc}` was substituted from the unit's empty environment and rendered **empty**. Leg A reported
`WLRC=` with `WLDONE` intact — the exact "marker without status = UNREADABLE" state. The
instrument was silently destroying the one value it existed to carry.

Repaired by moving the inner command into a real executable file, removing systemd expansion
from the path entirely. Three-assertion self-test, per the standing rule:

| Assertion | Result |
|---|---|
| (a) known-positive: exit 0 | `WLRC=0` |
| (b) known-negative: exit 7 | `WLRC=7` — a real code, not collapsed to 1 |
| (c) the OLD broken form on that same exit 7 | `WLRC=` (empty) — it would have missed it |

Leg A was then re-run on the repaired instrument; that is the `WLRC=1` reported above.

### The verify gate can fail

Run today, before the span is met, the verify path exits **72**:

```
F23_04_SPAN_SECONDS=158018   F23_04_SPAN_REQUIRED_SECONDS=259200
F23_04_SPAN_MEETS_AUTHORIZED_POLICY=false
```

A verify that passed today would have been self-passing. It also proves the verify wrapper runs
past line 67 under no-`HOME`.

## 4. Continuity decision — RESUME, with the missed day run late

**Chosen:** resume the existing chain and execute the missed day 2 as a real run on 2026-07-29.
**Rejected:** restart (resets a 3-day clock, 259200s, for zero evidentiary gain) and injection
(fabrication).

Reasoning:

1. **The chain carries nothing false.** The failed day-2 run aborted at line 67 *before*
   `run_step`, so it wrote no row and no partial row. `day-one.json`, the durable journal, the
   nonce and the root were never touched. There is nothing to be contaminated by.
2. **A resume needs only day one.** `multi_day_journey_test.rs:724` — `journey_resume` loads
   `day-one.json` and recovers the Goal from the durable journal; it has no dependency on day
   N-1. Verified by reading the resume path, not assumed. So restarting would buy nothing the
   existing chain lacks.
3. **Running day 2 late is not injection.** The row stamps `2026-07-29T10:14:57Z`, the true time
   a real process executed on the real host under the scheduler. Nothing is back-dated. The
   grading lane was right not to inject a row; running the work is the opposite of that.
4. **It restores evidence that would otherwise be lost.** Day 2 is the *resume-while-the-
   condition-is-unmet* step — it proves the wait does not complete early. Had I let day 3 be the
   first resume, it would have completed immediately and the journey would never have
   demonstrated a resume that stays waiting. That is the heart of a wait/resume journey.
5. **The span is unaffected**, and is measured day1→day3 regardless.

**Provenance is explicit and checkable.** A `#`-prefixed block was appended to
`/root/.f23-journey-linux/runlog.txt` (the `F23_04_` greps ignore `#` lines, as they already do
for the existing `# ---- invocation` rows) recording that day 2 was run late by this lane, why,
and that nothing was injected or back-dated. The row's own invocation line independently carries
ts / host / pid / sha.

## 5. Timer verification — checked on the host, not in the tree

```
Thu 2026-07-30 14:31:00 UTC  f23-journey-day3.timer     -> f23-journey-day3.service
Thu 2026-07-30 14:45:00 UTC  f23-journey-verify.timer   -> f23-journey-verify.service
```

- `ExecStart = /root/f23-journey-day.sh 3` → `cd /root/wayland-23B-04` →
  `bash scripts/f23-multi-day-journey.sh`, which is the **repaired** file.
- Deployed `sha256 = e1eb50a9…c953a3be`, **byte-identical to the lane commit**.
- `systemctl show f23-journey-day3.service`: `Environment=` empty, `User=` empty,
  `WorkingDirectory=` empty — the same default environment as the rehearsal harness.
- The **real wrapper** was rehearsed end-to-end under `systemd-run` with no `HOME`:
  `scheduled day 2 exited 0 at 2026-07-29T10:15:49Z`. Safe because day 2 was already recorded,
  so the idempotency guard made it a no-op that still exercised wrapper → script → past line 67
  → root derivation → harness resolution.
- Bare unguarded `$HOME/` reads in the deployed script: **0** (known-positive: 12 `HOME` hits in
  comments and the derivation block, so the grep is alive).

**Arithmetic for day 3:** day one opened `2026-07-27T14:21:20Z`; the condition is met at
`+259200s = 2026-07-30T14:21:20Z`. The timer fires `14:31:00Z`, **580s after** the condition is
met, so `condition_met=true` → `resume_from_wait` → `terminate`. `OnCalendar` timers can only
fire late (default `AccuracySec=1min`), never early, and late only increases the margin.

**Verify was not scheduled by anything.** Day 3 would have completed the goal and left the
criterion with no `F23_04_JOURNEY=PASS` marker, so I armed `f23-journey-verify.timer` for
14:45:00Z. `journey_verify` is non-mutating (re-observes, does not transition). Its wrapper
passes `--root` explicitly — the Windows `-Root` shape at the deployment layer.

## 6. Hazard recorded for whoever touches this next

**Do not run day 3 early to "test" it.** The per-day idempotency guard
(`F23_04_DAY_ALREADY_RECORDED`) would make the armed timer a no-op, and because the condition
is still unmet before 14:21:20Z the goal would never terminate — the journey would silently
never complete. Rehearse with an already-recorded day, as done here.

## 7. Fences

Two files changed vs `861d1b1a`: this report and `scripts/f23-multi-day-journey.sh` (+32/-1).
`0` under `.github/workflows/`, `0` in `crates/wcore-cli/src/{lib,main}.rs`, `0` `*.rs`.
No merge, no PR, no tag, no issue closed, no `wcore-contract generate`. No cargo run on the Mac.
The Windows leg — healthy, day 2 at 2026-07-28T23:58:17Z, day 3 scheduled — was **not touched**.

Host-side changes are confined to the journey's own units and directories: the repaired script
in `/root/wayland-23B-04`, a new `/root/f23-journey-verify.sh` + its timer, and my own
`/root/f23-rescue-proof/` scratch. The temporary pre-fix driver copy was removed. No other
lane's units or run directories were touched; no global `pkill`.

## 8. Honest verdict

The defect is fixed and proven fixed in the environment that broke, with the pre-fix failure
reproduced on the same harness. The armed timer executes the repaired file under a matching
environment.

**What is not yet proven:** that day 3 *did* complete — it fires 2026-07-30T14:31:00Z, after
this lane ends. What is established is that the one cause of the previous failure is gone, that
the deployed path is the repaired path, and that the wrapper now runs to completion under the
exact scheduler environment. The remaining risk is ordinary (host availability), not this defect.
