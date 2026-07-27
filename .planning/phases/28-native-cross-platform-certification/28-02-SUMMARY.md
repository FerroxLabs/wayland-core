# 28-02 SUMMARY — observability control, E5 probe set, and the matrix on all three families

**Plan:** `28-02` (wave 2, `depends_on: ["28-01"]`) — requirement **F28-01**.
**Lane branch:** `lane/28-02`. **Merge-base:** `78b91444`.

**TERMINATION STATE: 2 — COMPLETE WITH STATED EXCEPTIONS.**
The control produced a verdict; the probe set covers all nine dimensions; the matrix ran on
all three families; **every one of the 651 cells has a recorded outcome and none is skipped**.
The exceptions are real and are §6: **24 cells are RED**, all of them macOS sandbox cells, for
one measured reason.

---

## 1. What each family ran, asserted on each host before any step

**Candidate `32e2f57d09fe4b287e513081862217dc9daa5901`, tree `63ec0e6c36ff8e63789aab2f9760870304b671df`.**

**The candidate was RE-RESOLVED, not inherited.** Plan 28-01 bound **0 of 6** per-target
digests because CI run `30269095004` was `status=queued` when it measured. That run has since
completed, and this plan downloaded and hashed all six artifacts: **6 of 6 bound, and the
candidate is no longer provisional.** 28-01's single largest stated exception is closed.

| Family | Host | Target | Binary sha256 | Ledger-bound |
|---|---|---|---|---|
| linux | `hetzner-dsm` | `x86_64-unknown-linux-gnu` | `e8431ba208c5e794813e6e0b10a005229d982a9a99f05642e3eee98e2adc47d3` | **yes** |
| macos | this Mac (macOS 26.3, arm64) | `aarch64-apple-darwin` | `945534d6ebfab321bffc8ba6034201035f577031b496e0a72e08f87746ea5af7` | **yes** |
| windows | `seandesktop` | `x86_64-pc-windows-msvc` | `baf9bd692833eb7b9d54f00053739115b6ad5257fbdb0b0e99a8694a2ee996a6` | **yes** |

Every family ran **the CI release artifact itself**, digest-asserted on the host before the
run. `--check-binary-binding` reports `runs_checked: 3, unbound_targets: []`.

**"No macOS binary is obtainable" is refuted by a live run, not by an argument.**
`wayland-core-aarch64-apple-darwin` was downloaded from CI, executed on the certification
Mac, and printed `wayland-core 0.12.25`. No cargo ran on the Mac beyond
`cargo fmt --all -- --check`.

**Choice of candidate, stated because it is a judgement.** `32e2f57d` is the only commit with
a complete set of six CI artifacts, so it is the only commit at which every family can be
certified against a digest-bound build. The integration tip moved from `78b91444` to
`c6766f02` while this plan ran, and is still moving. **This certification therefore covers
`32e2f57d` and NOT the current tip**, and plan 28-04 must not read it as covering the tip.

---

## 2. The control — measured, and it settles the question

Full document: `28-02-OBSERVABILITY-CONTROL.md`. Machine form: `evidence/28-02/controls.json`.
Raw: `evidence/28-02/win-control.log` (one `EXIT=0`, 6 `F28_CONTROL=` records).

**Six observations, two session types × three lease states, both directional controls, from
one quiet window on the single physical Windows box, on the digest-bound candidate binary.**

| Session type | Logon measured | Lease state | `probe_report` | `product_behaviour` |
|---|---|---|---|---|
| scheduled-task | `session_id=1 interactive=True ssh=False` | as-found (0) | available | executed-sandboxed |
| scheduled-task | same | cleared (0) | available | refused-fail-closed † |
| scheduled-task | same | **wedged (1)** | **unavailable** | **refused-fail-closed** |
| ssh | `session_id=0 interactive=False ssh=True` | as-found (0) | available | executed-sandboxed |
| ssh | same | cleared (0) | available | executed-sandboxed |
| ssh | same | **wedged (1)** | **unavailable** | **refused-fail-closed** |

† Recorded as measured, not smoothed over: probe `available`, no worker ran. That is the
swarm dispatch-admission intermittency, filed as `F-28-02-003`.

Directional controls: **6 of 6 behaved directionally** — 2 positive (clean lease ⇒
observable), 2 negative (wedged lease ⇒ unobservable), 2 negative on the **activeness
detector itself** (the identical probe run outside the product ⇒ activeness ABSENT, proving
the detector discriminates rather than firing unconditionally).

### VERDICT: `wedge-clearable`. **The session type made no difference; the lease state made all the difference.**

Session 0, non-interactive, over SSH — the exact condition the standing rule calls
disqualifying — reported the sandbox **available** and ran a **contained** worker.

**Consequence, in terms: the `observation-blocked` skip class is NOT AUTHORISED, for any cell,
on any family, in this phase.** `controls.json` records
`observation_blocked_authorised: false` with empty `authorised_cells`, and `--check-controls`
rejects a record that authorises it under this verdict. **No cell in `results.json` carries
any skip at all.**

### Does it generalise? Partly, and the honest answer is stated

`seandesktop` is **the same host** the two intel files measured. This control is a much
stronger measurement *of that host* — it is the first with discriminating power, because it
varied the logon and the lease state independently where the original varied only the logon
while the lease directory was wedged. But **it is still one physical box.** Generalisation to
other Windows hosts remains **OPEN**, and `KR-06` should not be closed on this alone. What is
settled is the narrower thing the contract requires: whether the channel is sound **in the
certification environment**. It is.

`.planning/intel/APPCONTAINER-SSH-LEASE-WEDGE.md` **existed** at execution time, as did
`APPCONTAINER-SSH-LORE-READJUDICATION.md`. **This plan's measurement agrees with their
mechanism and their direction.** Neither is cited as evidence for anything here — both are
named only as the two hypotheses the control was built to discriminate, which is the only use
contract §4.2 permits.

### The wedge and the silent-disable defect are NOT one defect on this candidate

This was the plan's central security question and the answer is the opposite of what was
feared. **In both wedged observations the probe reported `unavailable` and the product REFUSED
TO EXECUTE** — `ran=False`, no worker output, nothing ran unsandboxed.

So on this candidate the stale lease is a **denial of service** — a file nobody knows to look
for permanently refuses all sandboxed execution, with a message that reads like a platform
limitation — and **not** an elevation of privilege.

**`KR-05`'s "the product continues to execute UNSANDBOXED" half is DISPROVED with executable
counter-evidence** for the delegated-execution surface. Its "logs a message that reads like a
platform limitation" half is **CONFIRMED**. **KR-05 must not be closed on this alone:** this
measures `SandboxRegistry::required_for_session` (the `swarm` path) and does **not** exercise
`default_for_platform()` (the bash-tool path). Scored **HIGH**, not CRITICAL, because the
measured behaviour is fail-closed. See `F-28-02-002`.

### Re-adjudicating what the false belief discounted

| Item | Now |
|---|---|
| `granted_path_is_readable_then_revoked` "fails identically over SSH" | **CONFIRMED-ARTIFACT of the wedge**, not of the logon — reproduced directly at both session types |
| `live_fs_acl` "all 12 panic at their gate regardless of correctness" over SSH | **CONFIRMED-ARTIFACT** — a contained worker executed over session-0 SSH |
| "never conclude a red from an SSH run" | **CONFIRMED-FALSE, retracted.** Remaining plan-brief copies need a serialized cross-lane edit by the orchestrator |
| hosted `windows-2022` reports `is_available() == false` | **NOT RE-RUNNABLE HERE** — no hosted runner in the certification environment. Recorded as not-re-runnable, not as confirmed |
| `job_close_reaps_detached_descendant_*` timeouts | **NOT IN THIS CLASS** — already root-caused to WMI `CommandLine = NULL`, with the SSH explanation explicitly disproved. Not reopened |

`w-process-cleanup-descendant-tree` — the mandatory cell `KR-01` tracks — **ran and PASSED**
on the quiet Windows leg.

---

## 3. The matrix — 651 of 651, zero skipped

Generated document: `28-02-MATRIX-RESULTS.md` (rendered from `results.json`, so the prose and
the data cannot disagree).

| | |
|---|---|
| cells with an outcome | **651 of 651** |
| pass / red / **skip** | 627 / 24 / **0** |
| **critical cells** | **147 — 123 pass, 24 red, 0 skipped** |
| sandbox greens carrying activeness | **50 of 50** |
| linux / macos / windows | 216-0-0 / 192-**24**-0 / 219-0-0 |

**All 147 unskippable critical cells ran.** The set was not narrowed.

### Every sandbox green's activeness observation

Activeness is a **differential**: the same probe outside the product and inside a worker the
product spawned. Absence of a violation is never expressible as a green.

| Family | Greens | Observation |
|---|---|---|
| linux | 24 | process-id namespace changed (`NSpid:1277328` outside → `NSpid:4` inside); filesystem root reduced from 52 entries to 9; DNS resolves outside and not inside |
| windows | 26 | the child was refused `\BaseNamedObjects` with **`0xC0000022`**, which AppContainer confines by construction |
| macos | **0** | **none obtainable — every macOS sandbox cell is a RED** (§6) |

### Windows serialization, stated per the hazard

The two registered Windows runners are one physical box. **The first Windows matrix attempt
(22:29) started with 0 compiler processes but a concurrent lane resumed before it finished;
its own gate reported `EXIT=9` and its rows were DISCARDED, not recorded.** The leg was
re-run once the box was quiet again and every Windows row here comes from that second run,
which measured **0 compiler processes at both ends**. The control legs likewise measured 0 at
both ends.

---

## 4. Gates — real numbers, and every one proved able to fail

**Local (Mac, source and artifact level; no cargo beyond `fmt`): 18 run, 18 passed, 0 failed.**

**Authoritative, hetzner `hetzner-dsm`, worktree `/root/wayland-p28`, SHA asserted on the host
before any build step:**

| Gate | Result |
|---|---|
| `cargo clippy -p wcore-eval-scenarios --all-targets -- -D warnings` | **0 warnings, exit 0** |
| `cargo nextest run --test e5_native_matrix --test e5_matrix_contract` | **37 run, 37 passed, 0 skipped** |
| `f28-native-matrix.mjs --self-test` | **25 assertions, 0 failed** |
| `f28-check-matrix-results.py --self-test` | **62 assertions, 0 failed** |
| marker verification, all three families | linux **216**, macos **216**, windows **219** — all VERIFIED |

### 32 mutations, each rejected, control re-run green after every one

Not asserted — executed, against the **real** artifacts this plan produced.
Logs: `evidence/28-02/gate-mutations.log` and `marker-mutations.log`.

**Results validator — 13/13 rejected**, each with a distinct code: sandbox green with its
activeness removed (`F28R-100`), with `NotMeasured` activeness (`F28R-100`), with a blank
detail (`F28R-101`); a sandbox cell recorded *before* the control (`F28R-092`), citing a
foreign control (`F28R-091`), citing none (`F28R-091`); **every sandbox cell removed so the
precedence gate would pass vacuously** (`F28R-093`); an unattributed red (`F28R-070`); a new
finding with no Phase 28 re-score (`F28R-073`); a CRITICAL cell given a skip (`F28R-080`); an
observation-blocked skip citing **the FAVOURABLE intel file** (`F28R-004`); a fifth skip class
invented mid-run (`F28R-081`); a run whose binary is not the candidate (`F28R-125`).

**Control validator — 9/9 rejected**: a negative control reporting `observable` (`F28R-035`);
a positive reporting `unobservable` (`F28R-036`); a control **asserting its own conclusion**
with `passed:true` beside a mismatch (`F28R-034`); five observations (`F28R-021`); **six
observations that are not the cross product** (`F28R-017`); probe `available` conflated with
`proceeded-unsandboxed` (`F28R-020`); `control_ref` replaced by the intel document
(`F28R-004`); a run recorded as not quiet (`F28R-014`); the class authorised under a verdict
that forbids it (`F28R-023`).

**Marker verifier — 10/10 rejected, against the real 216-cell Linux proof log**: a cell marker
deleted, duplicated, transposed; commit, tree and nonce tampered; a foreign-platform marker;
the final marker moved first and deleted; and **a sandbox green stripped of its activeness
field**.

### One self-passing shape found and corrected in my own work

The suspend/resume probe first reported a **timeout that belonged to the harness**: an
asynchronous child was driven from an event loop the probe then blocked, so its exit was never
delivered. "The measurement timed out" and "the product hung" are indistinguishable from
outside and only one is a finding. Repaired to a synchronous wrapper, then strengthened: the
child now starts **already stopped** and the stopped state is **observed in the process table**
rather than hoped for after a sleep, because a suspension applied after a sleep loses the race
against a short-lived invocation and reports that race as a product result.

### The plan's own gates: 13 HIGH per the linter, and I did not rely on them

`lint-plan-gates.py` over the phase directory reports **4 plans, 75 gates, 13 HIGH** — the
HIGH shapes are in the PLAN files, authored at planning time, and are mostly `test -f` gates
that pass against the untouched tree. **I ran the 18-gate sweep above instead**, every member
of which asserts something this plan created, plus the 32 mutations. Recorded rather than
quietly worked around.

---

## 5. Findings

| id | Severity | Contradicts | Disposition | Subject |
|---|---|---|---|---|
| `F-28-02-001` | **HIGH** | **1** | OPEN (FIXED/DISPROVED only) | macOS sandbox activeness is not obtainable through any black-box surface of the shipped candidate |
| `F-28-02-002` | **HIGH** | — | OPEN (FIXED/DISPROVED only) | the stale-lease wedge is a persistent denial of service; KR-05's unsandboxed-execution half is disproved for the surface measured |
| `F-28-02-003` | MEDIUM | — | → BACKLOG | swarm dispatch admission intermittently refuses with the sandbox available |
| `F-28-02-004` | MEDIUM | — | → BACKLOG | the belief itself: a rule with no discriminating control discounted security evidence for weeks |
| `F-28-02-005` | MEDIUM | — | → BACKLOG | a task run through `backend run --backend local` on macOS created a file outside its workspace |
| `F-28-02-006` | MEDIUM | — | → BACKLOG | the Linux bwrap backend read-binds **all of `/etc`**, so a sandboxed worker reads `/etc/shadow` |
| `F-28-01-R001` | MEDIUM | — | → BACKLOG | `wayland-core channel` is claimed by a phase 24 artifact and absent from the candidate binary |

**`F-28-02-006`, measured and deliberately NOT inflated.** A sandboxed Linux worker read
`/etc/shadow`. I checked the source before scoring it: `SYSTEM_RO_DIRS` includes `/etc` and
the bind is a deliberate `--ro-bind /etc /etc`, so `enforces_read_deny() == true` is **not**
lying — it means the backend honours `fs_read_deny` masks, not that it denies everything
ungranted. This is a hardening gap, not a control that reports itself active while inactive,
and it is the subject matter of no criterion. **MEDIUM, BACKLOG.** Inventing a stricter rule
than the recorded one is what grew Phase 20 to 74 plans.

**`F-28-02-001` is the phase-level exception.** Under A1 its inherited severity is `-`
(this plan raised it); under A2 its subject matter *is* Criterion 1's subject matter, so its
accept and defer paths are **closed**, and it is scored HIGH because a Criterion-subject
property that cannot be evidenced at all means the criterion cannot be honestly asserted.

**No CRITICAL finding was raised.** The one candidate for CRITICAL — a probe reporting the
sandbox available while the product ran unsandboxed — was **measured and does not occur**.

---

## 6. STATED EXCEPTION — macOS sandbox coverage is NOT achieved

**All 24 macOS sandbox-dimension cells are RED.** The other 192 macOS cells pass. No positive activeness observation is obtainable on macOS through any black-box
surface of the shipped candidate:

```
sandbox backend sandbox_exec cannot own descendants that escape a process group;
select Docker for delegated Swarm execution on this host; qualified Docker fallback
is unavailable on this macOS host: docker backend disabled (feature `live-docker` off)
```

The only delegated-execution surface refuses on macOS, and the Docker fallback is compiled
out. Under the contract's activeness rule a cell that cannot produce positive activeness
evidence is a **RED — never a green, and never a skip**. So they are red.

**Criterion 1 is NOT satisfied on macOS by this run, and that is stated here rather than in a
footnote.**

**What would close it:** a black-box surface on macOS that executes a caller-supplied command
through `default_for_platform()`'s sandbox-exec backend and returns enough of the child's own
view to form a containment differential. `backend run --backend local` executes a
caller-supplied task and reports `platform containment backend 'sandbox_exec' probed
available` — but its receipt retains only the artifact **digest**, not its bytes, so the
child's view is not recoverable from it. A probe task run through it also created a file
outside its workspace (`F-28-02-005`).

**These cells are NOT harness-bound.** The probe runs fine on macOS; it is the product's own
surface that refuses. Declaring them harness-bound would be a silent narrowing of coverage.

---

## 7. What I did NOT do

- **No production defect was repaired.** Every defect found is recorded with a Phase 28
  severity and routed. A certification that fixes what it measures is measuring itself.
- **No production file outside `crates/wcore-eval-scenarios` was touched**, gate-checked
  against the **merge-base `78b91444`**, never against the branch name — the shape that made a
  sibling lane report 28 deletions it never made. `git diff --name-only 78b91444 HEAD --
  crates/` returns exactly `src/e5_cases.rs`, `src/lib.rs`, `tests/e5_native_matrix.rs`.
  The `wcore-cli` shared fence is untouched, so **there is nothing to serialize for this lane**.
- **No existing test was modified, renamed, re-gated, `#[ignore]`d, `#[allow]`ed or deleted**,
  and no timeout was raised. No assertion was weakened.
- **No `Cargo.toml` or `Cargo.lock` change**, no new crate, no new dependency, no install.
- **`wcore-contract generate` was NOT run.** No PR, merge, tag, release or issue closure.
- **No receipt was signed and no soak was run** — those are plans 03 and 04.
- **No fifth skip class**, and **no fifth Phase 28 plan**. The four-plan cap is intact.
- **No sandbox verdict rested on inherited belief.** The control ran first, its verdict was
  recorded first, and `--check-control-precedence` proves all 74 sandbox cells postdate it and
  cite it — over a non-empty set, which is itself gate-checked (`F28R-093`).
- **The macOS leg was NOT declared unreachable.** The binary was downloaded, executed, and 216
  cells were run against it.
- The lease directory on `seandesktop` was **restored to the state it was found in**
  (`LEASE_RESTORED count=0 expected=0`, both legs), so no wedge was left for another lane.

## 8. Deviations

1. **The probe EXECUTOR lives in `f28-native-matrix.mjs`, not in Rust.** The plan names that
   file as the marker verifier and it is; it is also the executor. Forced by the plan's own
   black-box requirement: a probe implemented as a cargo-built harness cannot run on the
   certification Mac at all. `e5_cases.rs` holds the canonical definitions and
   `the_executor_mirrors_the_canonical_probe_table_entry_for_entry` asserts the two agree
   entry for entry, so the executor cannot drift from the definition it implements.
2. **The candidate is `32e2f57d`, not the moving integration tip** — §1.
3. **Two harness repair iterations were used, the bound the plan sets** — suspend/resume
   (§4), and the unwritable-HOME fixture, where the Linux host runs as root and root bypasses
   permission bits. The canary write caught it rather than passing over a writable directory;
   HOME now falls back to a regular file so writes fail `ENOTDIR` for every uid.
4. **A finding was added against the 28-01 matrix construction** (below), recorded rather than
   acted on.

**Recorded for 28-04, not acted on.** 28-01 crossed `sandbox-probes` with every one of the 24
surfaces, making 74 sandbox cells. Most of those surfaces never execute sandboxed work, so
their activeness observation is necessarily the *run-level* containment differential rather
than a per-surface one. That is what this plan recorded, and it is honest, but it means the 74
sandbox cells carry 3 distinct observations rather than 74. **I did not narrow the set** —
narrowing would be softening Criterion 1 — and I am flagging the construction so 28-04 can
weigh it deliberately.
