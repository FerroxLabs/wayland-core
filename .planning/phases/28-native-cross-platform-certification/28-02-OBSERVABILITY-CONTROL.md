# 28-02 — The Windows sandbox observability control

**Measured 2026-07-27 on `seandesktop`, run `f28ctl-20260727222551`.**
Machine form: `evidence/28-02/controls.json`. Raw log: `evidence/28-02/win-control.log`.
Enforcement: `f28-check-matrix-results.py --check-controls` and `--check-control-directions`.

**VERDICT: `wedge-clearable`.**
**The observation-blocked skip class is NOT AUTHORISED for any cell in this phase.**

---

## 1. What was being settled, and why a control rather than a citation

The program carried a rule — codified in `REQUIREMENTS.md`, repeated in three plan briefs —
that a session-0, non-interactive SSH logon to this box reports the AppContainer backend
unavailable *regardless of correctness*, and therefore that **no Windows sandbox red from an
SSH run may be believed**. That rule was used to discount reds as environment artifacts.

Two intel files now report the opposite: that the false negative came from a stale lease
wedging the production lease directory, not from the logon session. **This plan assumed
neither.** Both are hypotheses, and the certification contract §4.2 forbids citing *either*
file as skip evidence — including the one that reports in the product's favour, because a
laundering channel does not become sound by being pointed at good news.

So the question was measured, at run time, in this environment, before any sandbox cell was
graded.

## 2. Design — one variable at a time, and two things recorded separately

Six observations: **two session types × three lease states**, everything else held constant.

The control drives **the product's own availability path**, not a re-implementation of it.
On Windows `is_available()` is not a capability query — it is a **real sandboxed spawn** of
`cmd.exe /c exit 0` through the full AppContainer path, including dead-lease recovery. A
PowerShell re-implementation would answer a different question confidently, and this
platform's shell has already produced exactly one such answer on this program through a
high-hex-constant parse that made handle probes "succeed" holding nothing.

Two fields are recorded **separately**, with disjoint vocabularies so neither can be derived
from the other:

| Field | Question |
|---|---|
| `probe_report` | What the product's availability probe REPORTED — `available` / `unavailable`. |
| `product_behaviour` | What the product then DID — `executed-sandboxed` / `refused-fail-closed` / `proceeded-unsandboxed` / `indeterminate`. |

That separation is the security half of this task. If a stale lease drives the probe false
**and** a false probe lets the product proceed unsandboxed, then a stale lease is a path to
running with no sandbox at all — an elevation of privilege, not a tooling quirk. The two are
only distinguishable if they are recorded as two things, so `--check-controls` rejects a
record that omits either, and rejects the impossible combination
(`available` + `proceeded-unsandboxed`) outright.

**Activeness is a DIFFERENTIAL observation, never an absence.** The identical command runs
outside the product and inside the worker the product spawns. Outside, it succeeds and the
token carries the ordinary `S-1-16-12288` High integrity label. Inside an AppContainer, msys's
`whoami` dies on `NtCreateDirectoryObject(\BaseNamedObjects\msys-2.0…)` with **`0xC0000022`**
— AppContainer confines to `AppContainerNamedObjects` by construction — and
`System32\whoami.exe` is refused outright. Those are positive signatures of containment. A
detector reading only the inside could not tell a contained child from an uncontained one.

## 3. The six observations

Binary: `C:\f28\candidate-wayland-core.exe`,
`sha256 baf9bd692833eb7b9d54f00053739115b6ad5257fbdb0b0e99a8694a2ee996a6` — the CI run
`30269095004` release artifact for candidate `32e2f57d`, digest-asserted on the host.
Both legs measured **0 compiler processes on the box at both ends**.

| # | Session type | Logon observed | Lease state | Leases | `probe_report` | `product_behaviour` |
|---|---|---|---|---|---|---|
| 1 | scheduled-task | `session_id=1 interactive=True ssh=False` | as-found | 0 | **available** | **executed-sandboxed** |
| 2 | scheduled-task | same | cleared | 0 | **available** | refused-fail-closed † |
| 3 | scheduled-task | same | **wedged** | 1 | **unavailable** | **refused-fail-closed** |
| 4 | ssh | `session_id=0 interactive=False ssh=True` | as-found | 0 | **available** | **executed-sandboxed** |
| 5 | ssh | same | cleared | 0 | **available** | **executed-sandboxed** |
| 6 | ssh | same | **wedged** | 1 | **unavailable** | **refused-fail-closed** |

† Observation 2 is recorded as measured and is **not** smoothed over. The probe reported the
sandbox **available** (`disabled=False`, `mismatch=False`) and no worker ran. That is the
`dispatch admission refused: invalid retained workspace reservation` behaviour a prior
investigation logged as an unexplained one-off and then saw reproduce. It is a **swarm
dispatch-admission** intermittency, not a sandbox failure, and it is filed as a finding
(§7, `F-28-02-003`). It does not disturb the verdict: the availability probe answered
`available` in that observation exactly as in the other three clean-lease ones.

**The lease directory was restored to the state it was found in** (`LEASE_RESTORED count=0
expected=0`, both legs), so this control left no wedge behind for another lane.

## 4. Both directional controls, and why their failure would be fatal

| Control | Direction | Expected | Actual | Passed |
|---|---|---|---|---|
| `pos-clean-lease-scheduled-task` | positive | observable | observable | yes |
| `pos-clean-lease-ssh` | positive | observable | observable | yes |
| `neg-wedged-lease-scheduled-task` | negative | unobservable | unobservable | yes |
| `neg-wedged-lease-ssh` | negative | unobservable | unobservable | yes |
| `neg-activeness-detector-scheduled-task` | negative | activeness-absent | activeness-absent | yes |
| `neg-activeness-detector-ssh` | negative | activeness-absent | activeness-absent | yes |

- A **negative control reporting `observable`** would mean the probe reports availability
  unconditionally, and every green it has ever produced would be worthless. That is a
  CRITICAL finding and `--check-control-directions` rejects the record with `F28R-035`
  rather than letting it be noted in prose.
- A **positive control reporting `unobservable`** would mean the control measures something
  other than what it claims; rejected with `F28R-036`.
- The **activeness detector's** negative control runs the identical command *outside* the
  product and requires activeness to be **absent**. A detector that fired unconditionally
  would make every activeness observation in this phase worthless; this is the check that
  would catch it.
- `passed` is **derived** from `expected == actual`, never trusted: a record asserting
  `passed: true` beside a mismatch is rejected with `F28R-034`.

The wedge was **produced on demand**, not waited for: a real, archived, unreconcilable lease
was installed for observations 3 and 6 (`WEDGE_INSTALLED=True`) and removed afterwards.

## 5. The verdict, and exactly what it authorises

**The session type made no difference. The lease state made all the difference.**

Session 0, non-interactive, over SSH — the precise condition the old rule says is
disqualifying — reported the sandbox **available** and ran a **contained** worker, on a clean
lease directory. The same session type reported **unavailable** with one stale lease present.
The interactive-context leg behaved identically.

> **`wedge-clearable`.** The observation channel in the certification environment is SOUND.
> The false negative is caused by lease state, and it clears.

Consequences, stated in terms:

1. **The observation-blocked skip class is NOT AUTHORISED** — for any cell, on any family, in
   this phase. `controls.json` records `observation_blocked_authorised: false` with an empty
   `authorised_cells`, and `--check-controls` rejects a record that authorises the class
   under this verdict (`F28R-023`). **No cell in `results.json` carries any skip at all.**
2. **Clearing the lease directory is part of the matrix run's setup**, and it was.
3. **The wedge itself is a finding** — `F-28-02-002`, §7.
4. **The old rule is retracted in writing.** "A session-0 SSH logon reports the AppContainer
   backend unavailable regardless of correctness" is **FALSE in the certification
   environment**, measured six ways. No rule, gate or skip in this plan has "sandbox reds
   from SSH are artifacts" as a passing condition.

### 5.1 What this does NOT establish — the generalisation question, answered honestly

`seandesktop` is **the same host** the two intel files measured. This control is therefore a
much stronger measurement *of that host* — it is the first one with discriminating power,
because it varied the logon and the lease state independently, where the original control
varied only the logon while the lease directory was wedged and so could not select between
the two hypotheses. But **it is still one physical box.**

**Generalisation to other Windows hosts remains OPEN**, and `KR-06` should not be closed on
this evidence alone. What is now settled is the narrower thing the contract actually
requires: *whether the channel is sound in the certification environment*. It is.

## 6. Re-adjudicating what the false belief discounted

The verdict is `wedge-clearable`, so every Windows sandbox red previously written off as an
artifact is re-opened. A critical case discounted by a false belief is a skipped case wearing
an explanation.

| # | Item the rule discounted | Re-adjudication |
|---|---|---|
| A1 | `granted_path_is_readable_then_revoked` "fails identically over SSH" — the rule's own control | **CONFIRMED-ARTIFACT of the wedge, not of the logon.** This control reproduces the mechanism directly: clean lease → available, wedged lease → unavailable, at BOTH session types. The observation was real; the inference from it was not. |
| A2 | `live_fs_acl`, all 12 tests, "panic at their gate regardless of correctness" over SSH | **CONFIRMED-ARTIFACT.** Observations 4–5 show a contained worker executing over session-0 SSH. The gate those tests sit behind is `is_available()`, which this control shows answers `available` over SSH on a clean lease directory. |
| A3 | The standing instruction "never conclude a red from an SSH run" | **CONFIRMED-FALSE and retracted.** This is the item with ongoing blast radius; it converts every future Windows sandbox red into an unfalsifiable excuse. It lives in shared, cross-lane files (`REQUIREMENTS.md:59` already carries a strike-through) — the remaining plan-brief copies need a serialized edit by the orchestrator. |
| B1 | Hosted `windows-2022` reports `is_available() == false` | **NOT RE-RUNNABLE HERE.** The certification environment has no hosted `windows-2022` runner; this control covers `seandesktop` only. The lease-wedge explanation does not fit a hosted runner (it starts with an empty lease directory), so this one most likely stands on its own merits — but that is a read, not a measurement, and it is recorded as not-re-runnable rather than as confirmed. |
| C1 | `job_close_reaps_detached_descendant_*` timing out | **NOT IN THIS CLASS.** Already root-caused to WMI returning `CommandLine = NULL` for Low-IL AppContainer processes, with the SSH explanation explicitly disproved under a SYSTEM-context scheduled task. Not reopened. |

**`w-process-cleanup-descendant-tree`** — the mandatory descendant-reaping cell that `KR-01`
tracks — was **run in this phase and PASSED** on the quiet Windows leg
(`results.json`, run `windows-1`). See §7.

## 7. Findings

### `F-28-02-002` — HIGH — the stale-lease wedge is a denial of service, and it is NOT the silent-disable defect

**This is the plan's headline security result, and it is the opposite of what was feared.**

The question was whether the lease wedge and the silent-disable defect are one defect. If a
stale lease drove the probe false *and* a false probe let the product proceed unsandboxed,
that would be an exploitable path to running with no sandbox at all — CRITICAL.

**Measured, and it does not hold on this candidate.** In both wedged observations (3 and 6)
the probe reported `unavailable` and the product **refused to execute**: no worker ran, no
`F28RAN` reached the captured stdout, nothing executed unsandboxed. The dispatcher fails
closed.

So on this candidate the wedge is a **denial of service** — a stale file nobody knows to look
for permanently refuses all sandboxed execution, with a message that reads like a platform
limitation — and **not** an elevation of privilege.

| | Claim | Verdict on this candidate |
|---|---|---|
| KR-05 half 1 | "a security control reports itself active while being inactive" | **DISPROVED** for the surface measured. Under a wedge the control reports itself **INACTIVE**, correctly and loudly. |
| KR-05 half 2 | "the product continues to execute UNSANDBOXED" | **DISPROVED with executable counter-evidence** for the delegated-execution surface: `ran=False` in both wedged observations. |
| KR-05 half 3 | "and logs a message that reads like a platform limitation" | **CONFIRMED.** Still true, and still the reason the lore formed. |

**Scope of the disproof, stated so it is not over-read.** This measures the **delegated
execution** surface (`swarm` → `SandboxRegistry::required_for_session`). It does **not**
exercise the `default_for_platform()` path used by the bash tool, which reaches an
unsandboxed backend only behind an explicit `WAYLAND_ALLOW_NO_SANDBOX=1` opt-in. **KR-05
should not be closed on this evidence alone**; what is closed is the specific
"stale lease ⇒ unsandboxed execution" chain, on this surface, on this candidate. Scored HIGH
rather than CRITICAL because the measured behaviour is fail-closed; the DoS is real and its
subject matter is sandbox availability, which conditions Criterion 1's evidence.

### `F-28-02-003` — MEDIUM — swarm dispatch admission intermittently refuses with the sandbox available

Observation 2: probe `available`, `disabled=False`, `mismatch=False`, and no worker ran.
This is `dispatch admission refused: invalid retained workspace reservation`, previously seen
twice and recorded as "not a one-off". It is not a sandbox failure and contradicts no
criterion. Owner: whoever owns swarm dispatch admission. → BACKLOG.

### `F-28-02-004` — MEDIUM — the belief itself, filed as a finding

A rule with no discriminating control behind it was allowed to discount an entire class of
security evidence for weeks, across at least four documents. The defect is procedural: the
original control was run correctly and reported honestly, but was **not constructed to be
able to select between the competing hypotheses**, and nothing in the process required it to
be. The structural repair already landed in the certification contract §4.2 — an
`observation-blocked` skip now requires a control measured at run time in this environment and
**constructed to fail when the channel is sound**, and a documentary citation in either
direction is rejected by code. → BACKLOG, with the contract clause as its mitigation.

---

## 8. Provenance and honesty statements

- Every number here comes from `evidence/28-02/win-control.log`, which carries the raw product
  output for all six observations and exactly one `EXIT=0` marker written only when both legs
  produced the complete cross product.
- **Both legs ran through a mechanism that survives session teardown**, and the exit marker was
  polled from the log. The ssh call returning was never accepted as evidence.
- **No inherited belief was used in either direction.** Neither intel file is cited as evidence
  for anything here; both are named only as the hypotheses this control was built to
  discriminate.
- The gates that enforce this document were mutation-tested against **this** `controls.json`:
  nine mutations, each rejected with a distinct code, with the unmutated artifact re-accepted
  after every one. See `28-02-SUMMARY.md` §4.
