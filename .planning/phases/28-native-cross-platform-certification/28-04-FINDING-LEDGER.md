# Phase 28 — Adjudicated Finding Ledger

Machine form: `evidence/28-04/findings.tsv`. **This document is generated from it**, so the
prose and the data cannot disagree. Every row is also enumerated inside the signed receipt
`28-04-CERTIFICATION-RECEIPT.json` (`body.findings`), gate-checked by
`f28-verify-bindings.py --check-enumeration`.

Every finding is re-scored against **Phase 28's own four Success Criteria** (amendment A1).
`inherited_severity` is recorded as **provenance only and is never read by a gate** — the
laundering channel A1 closes needs no launderer, because the dangerous findings on this
candidate arrive pre-labelled below HIGH, scored against a different phase's criteria.

A non-`-` `contradicted_criterion` closes ACCEPTED and DEFERRED **regardless of the severity
recorded on the row** (amendment A2), so a mis-scored severity cannot reopen the accept path.

---

## The acceptance gate: **NOT PASSED**

**1 finding carries no terminal disposition.** The gate the panel adopted is
*zero findings at any severity lack an explicit, evidence-backed terminal disposition*, and
that is not met.

> **`F-28-02-002` — HIGH, contradicts criterion `-`, disposition OPEN.**
> the stale AppContainer lease wedge is a PERSISTENT DENIAL OF SERVICE: a file nobody knows to look for permanently refuses all sandboxed execution, with a message that reads like a platform limitation

This is reported rather than engineered away. At HIGH the only available dispositions are
FIXED and DISPROVED; repairing a production defect is outside plan 28-04's scope by design,
and the finding is real and confirmed, so neither is reachable here.

**The re-score that would have opened the accept path was declined deliberately.** A MEDIUM
reading of `F-28-02-002` is arguable under the contract's §3.1 bands — it contradicts no
criterion, the Windows matrix passed 219/219 in the as-found state, and the wedged state was
reached only by the control. MEDIUM would open ACCEPTED and DEFERRED and the gate would pass.
**Re-scoring a finding downward so its accept path opens is one of the three named forgeries
an adjudication plan is most exposed to**, so the row keeps the severity Phase 28's own plan
02 gave it. A later reader may reopen the score deliberately; they should read the row first.

---

## Counts

**63 findings.**

| Phase 28 severity | n |   | Terminal disposition | n |   | Origin | n |
|---|---|---|---|---|---|---|---|
| CRITICAL | 1 |  | ACCEPTED | 28 |  | matrix | 24 |
| HIGH | 8 |  | DEFERRED | 18 |  | candidate-resolution | 18 |
| MEDIUM | 40 |  | FIXED | 9 |  | certification | 7 |
| LOW | 14 |  | DISPROVED | 7 |  | carried-red | 6 |
|  |  |  | OPEN | 1 |  | contract | 3 |
|  |  |  |  |  |  | control | 3 |
|  |  |  |  |  |  | soak | 2 |

**7 findings cross the A2 line** (their subject matter *is* a criterion's
subject matter). Every one is on FIXED or DISPROVED — the accept and defer paths are closed
to them by construction, and none took one.

| id | Criterion | P28 severity | Disposition |
|---|---|---|---|
| `F-28-02-001` | 1 | **HIGH** | **FIXED** |
| `F-28-04-001` | 1 | **HIGH** | **FIXED** |
| `F-KR-07` | 1 | **HIGH** | **FIXED** |
| `F-WR-01` | 1 | **HIGH** | **FIXED** |
| `F-WR-02` | 1 | **HIGH** | **FIXED** |
| `KR-05` | 1 | **CRITICAL** | **DISPROVED** |
| `KR-01` | 2 | **HIGH** | **DISPROVED** |

### Downgrades

**None.** No finding was re-scored below the severity it arrived with, so no independent
downgrade review was required. Checked mechanically by `f28-ledger.py --check-downgrades`,
whose self-test also proves an *upward* re-score is not caught — the ceremony sits exactly
where the incentive is and nowhere else.

### The 46 accepted and deferred findings

Each carries a rationale, a named owner and a `BL-F28-*` id **written into `BACKLOG.md`
before the ledger cited it**, so the id is real rather than aspirational. Each is also
enumerated inside the signed receipt: that enumeration is the entire consideration the
program receives in exchange for not fixing them. **A finding routed to BACKLOG and absent
from the receipt has been absorbed, not dispositioned.**

---

## The findings

## CRITICAL (1)

### `KR-05` — DISPROVED

AppContainer ACL lease SID/profile mismatch: a security control reports itself active while being inactive; the product continues to execute UNSANDBOXED and logs a message that reads like a platform limitation

| | |
|---|---|
| Origin | carried-red |
| Inherited severity (provenance only) | `environment-quirk/non-gating` |
| Phase 28 re-score | **CRITICAL** |
| Contradicted criterion | **1** |
| Dispositions available | `FIXED,DISPROVED` |
| Disposition | **DISPROVED** |

A2 crossing (panel calibration example 2), accept path closed. MEASURED BY CONTROL and the answer is the opposite of what was feared: in BOTH wedged observations the probe reported unavailable and the product REFUSED TO EXECUTE (worker executed=False, no uncontained High-integrity label). On this candidate the wedge is a DENIAL OF SERVICE, not an elevation of privilege, so the CRITICAL claim as written - silently inactive control, execution continues unsandboxed - does not hold. 28-02 said this must not be closed on the delegated surface alone; the additional evidence that closes it is that on Windows is_available() IS a real sandboxed spawn rather than a capability query, driven at BOTH session types, with 6/6 directional controls caught including two negative ones. The residual - the bash-tool path under a wedged lease - is unmeasured and is carried as F-28-04-002 rather than absorbed into this row.

**Counter-evidence.** evidence/28-02/controls.json (obs-scheduled-task-wedged and obs-ssh-wedged: probe_report=unavailable, product_behaviour=refused-fail-closed) and evidence/28-02/win-control.log; 6/6 directional controls passed

## HIGH (8)

### `F-28-02-001` — FIXED

macOS sandbox activeness is not obtainable through any black-box surface of the shipped candidate: 24 critical sandbox-probes cells on macos are RED because no containment differential can be formed

| | |
|---|---|
| Origin | matrix |
| Inherited severity (provenance only) | `-` |
| Phase 28 re-score | **HIGH** |
| Contradicted criterion | **1** |
| Dispositions available | `FIXED,DISPROVED` |
| Disposition | **FIXED** |

A2 crossing raised by 28-02 itself: Criterion 1's subject matter is the hostile matrix including sandbox probes, and a Criterion-subject property that cannot be evidenced at all means the criterion cannot be honestly asserted for that family. FIXED by a real product surface, not by re-grading: `wayland-core sandbox status|exec` dispatches through wcore_tools::bash::BashTool::execute_with_ctx - the agent's OWN shell function - so a regression that stopped routing the agent's shell through containment breaks this probe too. The delegated admission gate was NOT touched: sandbox_exec still cannot own descendants and swarm still refuses on macOS. Admitting sandbox_exec to the delegated path would have turned 24 cells green by weakening a security control, which was the most tempting and most forbidden route available.

**Executable check.** cargo nextest run -p wcore-cli --test sandbox_activeness (2/2) and --lib -E 'test(sandbox_cmd)' (6/6); re-run matrix evidence/28-03/macos-cells.json 216/216 with activeness observed on all 24 sandbox cells; mutation test - probe rewired through a raw shell yields 1 passed 1 FAILED

### `F-28-02-002` — FIXED

the stale AppContainer lease wedge is a PERSISTENT DENIAL OF SERVICE: a file nobody knows to look for permanently refuses all sandboxed execution, with a message that reads like a platform limitation

| | |
|---|---|
| Origin | control |
| Inherited severity (provenance only) | `-` |
| Phase 28 re-score | **HIGH** |
| Contradicted criterion | none |
| Dispositions available | `FIXED,DISPROVED` |
| Disposition | **FIXED** |
| Re-adjudicated by | `28-ADJUDICATION.md` (lane `28-adj`, independent of the lane that authored the repair) |

**Re-adjudicated FIXED on 2026-07-29 by an independent lane that did not author the repair** (`28-ADJUDICATION.md`). The row below records what was tried against the FIXED claim, not what the repair's own summary asserted.

The repair is genuinely merged, which was checked first because the authoring lane's summary still says it is not: `15821c03` and `3f3f93dc` are both ancestors of the integration branch, and the whole `acl_lease` module is byte-identical to what was tested on hardware (sha256 `bc6bdac1`). A dead-owner lease that cannot reconcile against its own profile is now MOVED into a `quarantine` sub-directory and the recovery pass CONTINUES, instead of returning `Err` and aborting the pass on every later `ExecutionIdentity::start`.

**What survived the attack.** `owner_is_live` is the FIRST statement in the per-lease loop, so it dominates all THREE mutating branches - profile deletion, reclamation and cleanup - not merely the reclaim path; the honour-when-alive leg is therefore stronger than the repair claimed for it. The quarantine allow-list is narrow and fails closed on every adjacent shape: only a real directory of that name is skipped, a junction or a plain file of that name still hard-errors, and no product code ever reads back out of that directory, so it introduces no writable surface and no trust crossing. The authoring lane's claim that dropping the allow-list kills only the re-entrancy test is CORRECT, and the reason - which that lane did not state - is that its mutation harness runs each test in its own process while the test lease root is keyed on process id; in the full suite, where all four share a root, the allow-list is guarded harder still.

**What did not survive, and is filed rather than absorbed.** The predicted fourth self-passing gate exists and is now MEASURED. The test named `reclamation_reports_grants_it_could_not_revoke` does not test that: it never calls `reclamation_report`, and its assertion is satisfied by the file move preserving contents. Mutant `M3`, which deletes the residual-grant disclosure so an operator is told nothing was left behind while un-revokable ACL grants remain, leaves the suite at **133 passed, 0 failed, 23 ignored** - byte-identical to pristine. That is `F-28-ADJ-001`, and it is NOT folded into this row: the disclosure is a secondary property of the repair rather than this finding's subject, and even with zero disclosure the repaired state strictly dominates refusing forever, which never revoked those grants either and whose documented remedy - delete the file by hand - disclosed nothing at all. The finding's own message clause IS pinned, by `a_leaked_test_lease_is_diagnosed_by_name`, which asserts the remedy and all three denied false explanations; that is precisely what defeated the internal pass arguing to keep this row OPEN.

**Severity was not touched and no accept path was opened.** The deliberate refusal to downgrade to MEDIUM, recorded by 28-04 and preserved here, still stands: this row reaches a terminal disposition by repair on hardware, which is the only route the contract left open at HIGH. Cross-audit 3/3 FIXED (codex `gpt-5.6-sol`, gemini `3.1-pro`, kimi K3), with the dissent recorded in the adjudication. `KR-05` is NOT closed by this: the `default_for_platform()` residual remains held by `F-28-04-002` (DEFERRED), which this repair narrows but does not moot, since a lease can still wedge through doors this repair did not open.

**Executable check.** independent re-measurement on `SeanDesktop` at `3f3f93dc` (byte-identical to integration HEAD): pristine `wcore-sandbox --lib` = 133 passed, 0 failed, 23 ignored, and mutant `M3` = the SAME 133/0/23 with `MUT_COMPILED=True` and `APPLIED_SHA256=8dc05b5c`, which is what proves `F-28-ADJ-001` (`evidence/28-adj/m3.log`, `m3.diff`, `adj-m3.ps1`); lane 28-h2 mutants M1 (allow-list removed -> `quarantine_directory_does_not_become_a_second_wedge` FAILED) and M2 (`owner_is_live` bypassed -> `live_owner_unreconcilable_lease_is_honoured_not_reclaimed` FAILED) at `evidence/28-h2/mutate2-m1.log` and `mutate2-m2.log`; before/after hardware repro at `evidence/28-h2/repro-before.log` (wedged: refused, `ran=False`, twice, permanent) versus `repro-head.log` (reclaimed in flight, `Exit code: 0`, `F28H2RAN`); `live_owner_is_never_reclaimed` and `killed_owner_is_recovered_before_next_execution` ok under `WAYLAND_SANDBOX_LIVE_WINDOWS=1` against a real `CreateAppContainerProfile` identity (`evidence/28-h2/final.log`)

### `F-28-04-001` — FIXED

the two macOS members of the hostile platform matrix (hard_process_containment_macos, live_integrity_macos) had never been executed on any machine

| | |
|---|---|
| Origin | certification |
| Inherited severity (provenance only) | `-` |
| Phase 28 re-score | **HIGH** |
| Contradicted criterion | **1** |
| Dispositions available | `FIXED,DISPROVED` |
| Disposition | **FIXED** |

Criterion 1 names macOS and 28-CLEANUP's own definition of the hostile platform matrix names both suites, so a family member that has never run means the criterion cannot be honestly asserted for it - A2 closes accept and defer. One predecessor lane recorded this as 'no macOS build host in this lane's reach', which was FALSE and was retracted on measurement: ci.yml already uses macos-latest. NOW MEASURED: a GitHub-hosted macOS runner executed both cases at exactly this plan's base commit cf48b349. The gate's load-bearing check is the third one - no `skip:` line - because both suites open with two early returns that would otherwise report `1 passed` having asserted nothing; that check PASSED, and I verified independently that zero output lines begin with skip: (the only three occurrences in the workflow log are the gate echoing its own source).

**Executable check.** GitHub Actions run 30364529551 at headSha cf48b349, job 'wcore-sandbox live macOS acceptance' conclusion success: both suites 'running 1 test' then 'test result: ok. 1 passed; 0 failed', and 'GATE: PASSED - both required_live_macos_* cases executed on a real macOS host'; log retained at evidence/28-04/macos-ci-30364529551.log

### `F-KR-07` — FIXED

live_cmd_runs_when_allowlist_has_missing_path fails deterministically (12/12 serial) with SandboxError::Timeout against a 10s manifest on a `cmd /c echo`

| | |
|---|---|
| Origin | matrix |
| Inherited severity (provenance only) | `-` |
| Phase 28 re-score | **HIGH** |
| Contradicted criterion | **1** |
| Dispositions available | `FIXED,DISPROVED` |
| Disposition | **FIXED** |

A native Windows AppContainer acceptance case in the hostile platform matrix, so a deterministic red contradicts Criterion 1 and A2 closes accept and defer. A 12-rung ladder showed the red was NEVER about the missing allowlist entry the finding is named for - the absent path alone clears in ~107ms - it was the OTHER entry: the test granted over std::env::temp_dir(), which is unbounded and shared with every process on the host, and apply_explicit_access propagates across the whole subtree on EVERY execution. THIS IS A TEST FIX, NOT A PRODUCT FIX, stated plainly: the product's O(objects) grant cost is unchanged and is filed as F-KR-09. Rung 11's own prediction was REFUTED (19,487ms against a predicted 25s exhaustion) and was left failing in the tree, because a rung edited until it passes measures nothing.

**Executable check.** matched pair in ONE invocation: the unrepaired shape red at KR07B_REMEASURE verdict=FAIL Timeout elapsed_ms=25003 (100.0% of ceiling) while the repaired test printed 'test result: ok. 5 passed; 0 failed'; repaired case 107ms; cost ladder 200 objects 133ms vs 200,000 objects 19,487ms

### `F-WR-01` — FIXED

the KR-01 acceptance test never evaluated its own property: it aborted in its own setup at live_integrity.rs:273 with the sandboxed command exiting 1 on 'Access is denied.', so no descendant existed and the reap assertion ~30 lines below was never reached

| | |
|---|---|
| Origin | matrix |
| Inherited severity (provenance only) | `-` |
| Phase 28 re-score | **HIGH** |
| Contradicted criterion | **1** |
| Dispositions available | `FIXED,DISPROVED` |
| Disposition | **FIXED** |

A member of the native wcore-sandbox live acceptance surface - the hostile platform matrix as 28-CLEANUP defines it - so a deterministic red there contradicts Criterion 1 and A2 closes accept and defer. More serious than a stale known-red: the fix for the underlying defect landed at 2b662fe8 together with this very test, so what existed was a LANDED FIX WHOSE OWN ACCEPTANCE TEST HAD BEEN RED EVER SINCE, with the red attributed to the defect the fix closed. Rebuilt on primitives an 8-rung denial ladder proved (the nested cmd /d /c shape is what is refused; choice.exe exits in <80ms under this token; the descendant must be detached with start "" /b). Assertion NOT relaxed - witness strengthened.

**Executable check.** 12/12 serial passes in evidence/28-03-windows-requeue and F-WR-REPAIR-SUMMARY.md; hard_process_containment_windows re-run after the helper extraction 6 passed / 0 failed

### `F-WR-02` — FIXED

suites report `test result: ok` having executed ZERO tests: 16 all-ignored integration binaries (flavour A) plus live_integrity.rs which printed an affirmative `5 passed` for zero work (flavour B)

| | |
|---|---|
| Origin | matrix |
| Inherited severity (provenance only) | `-` |
| Phase 28 re-score | **HIGH** |
| Contradicted criterion | **1** |
| Dispositions available | `FIXED,DISPROVED` |
| Disposition | **FIXED** |

Criterion 1 subject matter: a suite that runs zero tests cannot evidence that a family passed the hostile matrix, so a green from one is coverage-from-nothing. Flavour B is strictly worse than A because `5 passed` reads as certification while `0 passed; 12 ignored` at least invites a second look. The detector for this class HAD THIS CLASS'S DISEASE - the first inventory generator matched #[ignore] against doc-comment PROSE describing the defect - and was fixed by anchoring on ^\s*#\[ and never collecting comment lines. All 16 now carry an always-running zero_execution_guard (3 + 13), deliberately NOT #[ignore]d, because three suites in this repo carried a guard that was itself ignored and so was inert against precisely its own scenario. Flavours C and D are closed generically at the invocation site with no-tests=fail rather than by restructuring 19 files.

**Executable check.** falsification measured both directions per suite: env SET without --ignored -> 'FAILED. 0 passed; 1 failed; 11 ignored' rc=101, env UNSET -> 'ok. 1 passed; 0 failed; 11 ignored' rc=0; nextest known-positive 'error: no tests to run' rc=4 vs known-negative '50 passed' rc=0

### `KR-01` — DISPROVED

descendant process tree is not reaped; a process survives its owner (wcore-sandbox::live_integrity::live_future_drop_reaps_descendant_job_tree, Windows, deterministic)

| | |
|---|---|
| Origin | carried-red |
| Inherited severity (provenance only) | `known-red/non-gating` |
| Phase 28 re-score | **HIGH** |
| Contradicted criterion | **2** |
| Dispositions available | `FIXED,DISPROVED` |
| Disposition | **DISPROVED** |

A2 crossing (panel calibration example 1): subject matter is an orphaned process and Criterion 2's subject matter is a soak with no orphan process, so the accept path is closed and only FIXED or DISPROVED were available. TESTED on seandesktop and the reap WORKS: 12/12 serial passes across competing load from 4 to 32 processes. The witness was STRENGTHENED, not the assertion relaxed - from heartbeat-file length (which cannot separate reaped from starved and biases toward a false pass under load) to host-side fixed-ProcessId liveness, and capture_alive_descendant_pids now panics if no descendant is ever observed so a run that proves nothing fails as unmeasurable. The carried row was MISATTRIBUTED: the old red never reached its reap assertion, aborting ~30 lines earlier in its own setup.

**Counter-evidence.** evidence/28-03-windows-requeue/KR-01.md and F-WR-REPAIR-SUMMARY.md - KR01_WITNESS_DESCENDANTS_ALIVE_BEFORE_DROP=[31360] then KR01_WITNESS_SURVIVORS_AFTER_DROP=0 of 1, 12/12 serial; ladder rung 5 refutes the serialisation alternative by failing the ORIGINAL shape serially in 2.7s

### `KR-06` — DISPROVED

whether the Windows sandbox is observable in the CERTIFICATION ENVIRONMENT is unmeasured; the standing rule that it cannot be observed over SSH is refuted on one host but does not generalise

| | |
|---|---|
| Origin | carried-red |
| Inherited severity (provenance only) | `standing-rule-now-refuted` |
| Phase 28 re-score | **HIGH** |
| Contradicted criterion | none |
| Dispositions available | `FIXED,DISPROVED` |
| Disposition | **DISPROVED** |

The finding's subject is that observability was UNMEASURED. It is now measured, by a directional control run in the certification environment before any sandbox cell was graded: 3 positive and 3 negative controls, all caught, at both scheduled-task and SSH session types. The standing rule 'never conclude a red from an SSH run' is CONFIRMED-FALSE and retracted. SCOPE, stated rather than dropped: the wedge-clearable verdict is not generalised off seandesktop, the observation-blocked skip class remains NOT AUTHORISED and was never used, and neither AppContainer intel file is cited as evidence for anything.

**Counter-evidence.** evidence/28-02/controls.json verdict=wedge-clearable with 6/6 directional controls; 28-02-OBSERVABILITY-CONTROL.md

## MEDIUM (40)

### `F-28-01-001` — ACCEPTED

the unproven-control corollary of A2 was considered and deliberately NOT applied, so KR-02 and KR-03 stayed below the A2 line

| | |
|---|---|
| Origin | contract |
| Inherited severity (provenance only) | `n/a-raised-by-phase-28` |
| Phase 28 re-score | **MEDIUM** |
| Contradicted criterion | none |
| Dispositions available | `FIXED,DISPROVED,ACCEPTED,DEFERRED` |
| Disposition | **ACCEPTED** |
| Owner | Phase 29 (release acceptance) |
| Backlog | `BL-F28-C4` |

Carried knowingly and recorded with its reasoning so a later reader can apply the corollary DELIBERATELY rather than rediscover it. The decision record names exactly two A2 crossings and 28-01 was directed to name two; applying the corollary would have made four. Cost is bounded because the structural rule reached the same place by a different road - the properties became measured matrix cells under the positive-evidence rule, where a cell that cannot produce positive evidence is a RED and never a skip.

### `F-28-01-003` — ACCEPTED

the severity-amendment commit d0837aa7 - the decision's load-bearing 'later instrument' - ends its own message with 'Phase 28's criteria are untouched (different phase)'

| | |
|---|---|
| Origin | contract |
| Inherited severity (provenance only) | `n/a-raised-by-phase-28` |
| Phase 28 re-score | **MEDIUM** |
| Contradicted criterion | none |
| Dispositions available | `FIXED,DISPROVED,ACCEPTED,DEFERRED` |
| Disposition | **ACCEPTED** |
| Owner | Sean (maintainer decision, reversible by one line) |
| Backlog | `BL-F28-C4` |

This is the STRONGEST available evidence for the losing c4-literal position and it appears in no panel response, so it is carried into the signed receipt rather than left in a contract appendix. It does not by itself overturn the decision: the sentence is as consistent with 'I am not editing that phase's text' as with 'that phase is exempt', and the dissent's own reversal condition is narrower (a restatement made KNOWING the current policy), which this is not. Under A2 the practical gap is narrow, because the findings c4-literal would protect are exactly the ones A2 already removes from the accept path. A reader reopening the acceptance rule should start here.

### `F-28-01-R001` — ACCEPTED

surface `wayland-core agent` is exposed by the candidate binary and appears NOWHERE in any phase 24-27 artifact (class present-but-unclaimed)

| | |
|---|---|
| Origin | candidate-resolution |
| Inherited severity (provenance only) | `n/a-raised-by-phase-28` |
| Phase 28 re-score | **MEDIUM** |
| Contradicted criterion | none |
| Dispositions available | `FIXED,DISPROVED,ACCEPTED,DEFERRED` |
| Disposition | **ACCEPTED** |
| Owner | Phase 29 (release acceptance) |
| Backlog | `BL-F28-SURFACE-UNCLAIMED` |

Certified anyway and flagged: the surface WAS exercised by the matrix and the soak, so this does not reduce coverage - it records that attribution is incomplete. An uncertified surface that ships is worse than an unattributed one, and the surface may predate phases 24-27, which is itself worth knowing at certification time. Carried knowingly. NOTE for a later reader: these IDs are POSITIONAL and shifted between the 28-02 and 28-03 resolutions, so do not diff them by ID.

### `F-28-01-R002` — ACCEPTED

surface `wayland-core crucible` is exposed by the candidate binary and appears NOWHERE in any phase 24-27 artifact (class present-but-unclaimed)

| | |
|---|---|
| Origin | candidate-resolution |
| Inherited severity (provenance only) | `n/a-raised-by-phase-28` |
| Phase 28 re-score | **MEDIUM** |
| Contradicted criterion | none |
| Dispositions available | `FIXED,DISPROVED,ACCEPTED,DEFERRED` |
| Disposition | **ACCEPTED** |
| Owner | Phase 29 (release acceptance) |
| Backlog | `BL-F28-SURFACE-UNCLAIMED` |

Certified anyway and flagged: the surface WAS exercised by the matrix and the soak, so this does not reduce coverage - it records that attribution is incomplete. An uncertified surface that ships is worse than an unattributed one, and the surface may predate phases 24-27, which is itself worth knowing at certification time. Carried knowingly. NOTE for a later reader: these IDs are POSITIONAL and shifted between the 28-02 and 28-03 resolutions, so do not diff them by ID.

### `F-28-01-R004` — ACCEPTED

surface `wayland-core forge` is exposed by the candidate binary and appears NOWHERE in any phase 24-27 artifact (class present-but-unclaimed)

| | |
|---|---|
| Origin | candidate-resolution |
| Inherited severity (provenance only) | `n/a-raised-by-phase-28` |
| Phase 28 re-score | **MEDIUM** |
| Contradicted criterion | none |
| Dispositions available | `FIXED,DISPROVED,ACCEPTED,DEFERRED` |
| Disposition | **ACCEPTED** |
| Owner | Phase 29 (release acceptance) |
| Backlog | `BL-F28-SURFACE-UNCLAIMED` |

Certified anyway and flagged: the surface WAS exercised by the matrix and the soak, so this does not reduce coverage - it records that attribution is incomplete. An uncertified surface that ships is worse than an unattributed one, and the surface may predate phases 24-27, which is itself worth knowing at certification time. Carried knowingly. NOTE for a later reader: these IDs are POSITIONAL and shifted between the 28-02 and 28-03 resolutions, so do not diff them by ID.

### `F-28-01-R007` — ACCEPTED

surface `wayland-core init` is exposed by the candidate binary and appears NOWHERE in any phase 24-27 artifact (class present-but-unclaimed)

| | |
|---|---|
| Origin | candidate-resolution |
| Inherited severity (provenance only) | `n/a-raised-by-phase-28` |
| Phase 28 re-score | **MEDIUM** |
| Contradicted criterion | none |
| Dispositions available | `FIXED,DISPROVED,ACCEPTED,DEFERRED` |
| Disposition | **ACCEPTED** |
| Owner | Phase 29 (release acceptance) |
| Backlog | `BL-F28-SURFACE-UNCLAIMED` |

Certified anyway and flagged: the surface WAS exercised by the matrix and the soak, so this does not reduce coverage - it records that attribution is incomplete. An uncertified surface that ships is worse than an unattributed one, and the surface may predate phases 24-27, which is itself worth knowing at certification time. Carried knowingly. NOTE for a later reader: these IDs are POSITIONAL and shifted between the 28-02 and 28-03 resolutions, so do not diff them by ID.

### `F-28-01-R008` — ACCEPTED

surface `wayland-core mcp-serve` is exposed by the candidate binary and appears NOWHERE in any phase 24-27 artifact (class present-but-unclaimed)

| | |
|---|---|
| Origin | candidate-resolution |
| Inherited severity (provenance only) | `n/a-raised-by-phase-28` |
| Phase 28 re-score | **MEDIUM** |
| Contradicted criterion | none |
| Dispositions available | `FIXED,DISPROVED,ACCEPTED,DEFERRED` |
| Disposition | **ACCEPTED** |
| Owner | Phase 29 (release acceptance) |
| Backlog | `BL-F28-SURFACE-UNCLAIMED` |

Certified anyway and flagged: the surface WAS exercised by the matrix and the soak, so this does not reduce coverage - it records that attribution is incomplete. An uncertified surface that ships is worse than an unattributed one, and the surface may predate phases 24-27, which is itself worth knowing at certification time. Carried knowingly. NOTE for a later reader: these IDs are POSITIONAL and shifted between the 28-02 and 28-03 resolutions, so do not diff them by ID.

### `F-28-01-R010` — ACCEPTED

surface `wayland-core models` is exposed by the candidate binary and appears NOWHERE in any phase 24-27 artifact (class present-but-unclaimed)

| | |
|---|---|
| Origin | candidate-resolution |
| Inherited severity (provenance only) | `n/a-raised-by-phase-28` |
| Phase 28 re-score | **MEDIUM** |
| Contradicted criterion | none |
| Dispositions available | `FIXED,DISPROVED,ACCEPTED,DEFERRED` |
| Disposition | **ACCEPTED** |
| Owner | Phase 29 (release acceptance) |
| Backlog | `BL-F28-SURFACE-UNCLAIMED` |

Certified anyway and flagged: the surface WAS exercised by the matrix and the soak, so this does not reduce coverage - it records that attribution is incomplete. An uncertified surface that ships is worse than an unattributed one, and the surface may predate phases 24-27, which is itself worth knowing at certification time. Carried knowingly. NOTE for a later reader: these IDs are POSITIONAL and shifted between the 28-02 and 28-03 resolutions, so do not diff them by ID.

### `F-28-01-R012` — ACCEPTED

surface `wayland-core project-context` is exposed by the candidate binary and appears NOWHERE in any phase 24-27 artifact (class present-but-unclaimed)

| | |
|---|---|
| Origin | candidate-resolution |
| Inherited severity (provenance only) | `n/a-raised-by-phase-28` |
| Phase 28 re-score | **MEDIUM** |
| Contradicted criterion | none |
| Dispositions available | `FIXED,DISPROVED,ACCEPTED,DEFERRED` |
| Disposition | **ACCEPTED** |
| Owner | Phase 29 (release acceptance) |
| Backlog | `BL-F28-SURFACE-UNCLAIMED` |

Certified anyway and flagged: the surface WAS exercised by the matrix and the soak, so this does not reduce coverage - it records that attribution is incomplete. An uncertified surface that ships is worse than an unattributed one, and the surface may predate phases 24-27, which is itself worth knowing at certification time. Carried knowingly. NOTE for a later reader: these IDs are POSITIONAL and shifted between the 28-02 and 28-03 resolutions, so do not diff them by ID.

### `F-28-01-R014` — ACCEPTED

surface `wayland-core self-update` is exposed by the candidate binary and appears NOWHERE in any phase 24-27 artifact (class present-but-unclaimed)

| | |
|---|---|
| Origin | candidate-resolution |
| Inherited severity (provenance only) | `n/a-raised-by-phase-28` |
| Phase 28 re-score | **MEDIUM** |
| Contradicted criterion | none |
| Dispositions available | `FIXED,DISPROVED,ACCEPTED,DEFERRED` |
| Disposition | **ACCEPTED** |
| Owner | Phase 29 (release acceptance) |
| Backlog | `BL-F28-SURFACE-UNCLAIMED` |

Certified anyway and flagged: the surface WAS exercised by the matrix and the soak, so this does not reduce coverage - it records that attribution is incomplete. An uncertified surface that ships is worse than an unattributed one, and the surface may predate phases 24-27, which is itself worth knowing at certification time. Carried knowingly. NOTE for a later reader: these IDs are POSITIONAL and shifted between the 28-02 and 28-03 resolutions, so do not diff them by ID.

### `F-28-01-R017` — ACCEPTED

surface `wayland-core swarm` is exposed by the candidate binary and appears NOWHERE in any phase 24-27 artifact (class present-but-unclaimed)

| | |
|---|---|
| Origin | candidate-resolution |
| Inherited severity (provenance only) | `n/a-raised-by-phase-28` |
| Phase 28 re-score | **MEDIUM** |
| Contradicted criterion | none |
| Dispositions available | `FIXED,DISPROVED,ACCEPTED,DEFERRED` |
| Disposition | **ACCEPTED** |
| Owner | Phase 29 (release acceptance) |
| Backlog | `BL-F28-SURFACE-UNCLAIMED` |

Certified anyway and flagged: the surface WAS exercised by the matrix and the soak, so this does not reduce coverage - it records that attribution is incomplete. An uncertified surface that ships is worse than an unattributed one, and the surface may predate phases 24-27, which is itself worth knowing at certification time. Carried knowingly. NOTE for a later reader: these IDs are POSITIONAL and shifted between the 28-02 and 28-03 resolutions, so do not diff them by ID.

### `F-28-02-003` — DEFERRED

swarm dispatch admission intermittently refuses with the sandbox reported available (obs-scheduled-task-cleared: probe_report=available, product_behaviour=refused-fail-closed)

| | |
|---|---|
| Origin | control |
| Inherited severity (provenance only) | `-` |
| Phase 28 re-score | **MEDIUM** |
| Contradicted criterion | none |
| Dispositions available | `FIXED,DISPROVED,ACCEPTED,DEFERRED` |
| Disposition | **DEFERRED** |
| Owner | Phase 30 (hardening) |
| Backlog | `BL-F28-SWARM-ADMIT` |

A real intermittency in the delegated-admission path, measured by the control rather than inferred. It fails CLOSED, so it costs availability and not containment, and it contradicts no criterion. Not repaired here.

### `F-28-02-004` — ACCEPTED

the belief itself: a standing rule with no discriminating control ('never conclude a red from an SSH run') discounted real Windows security evidence for weeks

| | |
|---|---|
| Origin | control |
| Inherited severity (provenance only) | `-` |
| Phase 28 re-score | **MEDIUM** |
| Contradicted criterion | none |
| Dispositions available | `FIXED,DISPROVED,ACCEPTED,DEFERRED` |
| Disposition | **ACCEPTED** |
| Owner | orchestrator (serialized cross-lane edit) |
| Backlog | `BL-F28-BELIEF` |

A process defect, and the one this program should be most interested in: an unmeasured belief was treated as a measured result and suppressed evidence about a security control. Retracted at 28-02 and corrected in LANE-BRIEF section 2 and AGENTS.md section 11. Not marked FIXED because 28-02 itself recorded that remaining plan-brief copies still carry the retracted rule and need a serialized cross-lane edit by the orchestrator - claiming FIXED over copies I have not changed would be the same defect in miniature. Carried knowingly with that residual named.

### `F-28-02-005` — DEFERRED

a probe task run through `backend run --backend local` on macOS created a file OUTSIDE its workspace

| | |
|---|---|
| Origin | matrix |
| Inherited severity (provenance only) | `-` |
| Phase 28 re-score | **MEDIUM** |
| Contradicted criterion | none |
| Dispositions available | `FIXED,DISPROVED,ACCEPTED,DEFERRED` |
| Disposition | **DEFERRED** |
| Owner | Phase 30 (hardening) |
| Backlog | `BL-F28-LOCAL-BACKEND` |

ROOT-CAUSED after the fact and the diagnosis reduces it: that path CONSULTS wcore_sandbox::default_for_platform() and refuses when no real backend exists, but never routes the child through SandboxBackend::execute, and its own receipt says containment was 'selected but NOT applied to this child'. So this measured nothing about sandbox-exec - it measured a path that never contained anything. Real, honestly reported by the product, contradicts no criterion. Same subject as F-MA-002.

### `F-28-02-006` — DEFERRED

the Linux bwrap backend read-binds ALL of /etc, so a sandboxed worker reads /etc/shadow

| | |
|---|---|
| Origin | matrix |
| Inherited severity (provenance only) | `-` |
| Phase 28 re-score | **MEDIUM** |
| Contradicted criterion | none |
| Dispositions available | `FIXED,DISPROVED,ACCEPTED,DEFERRED` |
| Disposition | **DEFERRED** |
| Owner | Phase 30 (hardening) |
| Backlog | `BL-F28-BWRAP-ETC` |

Measured and deliberately NOT inflated. The source was read before scoring: SYSTEM_RO_DIRS includes /etc and the bind is a deliberate --ro-bind /etc /etc, so enforces_read_deny()==true is not lying - it means the backend honours fs_read_deny masks, not that it denies everything ungranted. A hardening gap, not a control that reports itself active while inactive, and the subject matter of no criterion. Independently reproduced a second time during the macOS-activeness work (F28_SHADOW=READ inside the sandbox).

### `F-28-04-002` — DEFERRED

KR-05 RESIDUAL: whether the bash-tool path (wcore_sandbox::default_for_platform()) executes unsandboxed under a WEDGED AppContainer lease is unmeasured; the control exercised SandboxRegistry::required_for_session

| | |
|---|---|
| Origin | certification |
| Inherited severity (provenance only) | `-` |
| Phase 28 re-score | **MEDIUM** |
| Contradicted criterion | none |
| Dispositions available | `FIXED,DISPROVED,ACCEPTED,DEFERRED` |
| Disposition | **DEFERRED** |
| Owner | Phase 30 (hardening) |
| Backlog | `BL-F28-WEDGE-BASHPATH` |

Named explicitly rather than absorbed into KR-05's DISPROVED row, because 28-02 said in terms that KR-05 must not be closed on the delegated surface alone. Scored MEDIUM on ordinary merits and NOT raised to HIGH by the unproven-control corollary, which contract section 3.4 recorded as considered and deliberately not applied - inventing a stricter rule than the recorded decision is the failure that grew Phase 20 to 74 plans. What would close it: drive `wayland-core sandbox exec` (or any bash-tool path) on seandesktop with a lease deliberately wedged, and observe whether the child carries a containment signature or an uncontained High-integrity label.

### `F-28-04-003` — DEFERRED

zero-execution flavour (d) survives for plain `cargo test`: 19 feature-gated and 25 platform-gated test binaries print `running 0 tests` / `ok` and exit 0, the largest blanking 16 tests

| | |
|---|---|
| Origin | certification |
| Inherited severity (provenance only) | `-` |
| Phase 28 re-score | **MEDIUM** |
| Contradicted criterion | none |
| Dispositions available | `FIXED,DISPROVED,ACCEPTED,DEFERRED` |
| Disposition | **DEFERRED** |
| Owner | Phase 30 (hardening) |
| Backlog | `BL-F28-FLAVOUR-D` |

The invocation-site fix (no-tests = "fail") closes this for nextest only, and that limit is stated rather than glossed. Measured against a prior estimate of two instances, which is why the detector still exists. Contradicts no criterion, because every Phase 28 gate that matters runs through nextest or asserts an executed count.

### `F-28-04-004` — ACCEPTED

THE CERTIFICATION SPANS TWO CANDIDATES: the Linux and Windows matrix legs are 28-02's at 32e2f57d, while the macOS matrix re-run and all three soak legs are at e4a3f5fc. No single-candidate full matrix exists for this phase.

| | |
|---|---|
| Origin | certification |
| Inherited severity (provenance only) | `-` |
| Phase 28 re-score | **MEDIUM** |
| Contradicted criterion | none |
| Dispositions available | `FIXED,DISPROVED,ACCEPTED,DEFERRED` |
| Disposition | **ACCEPTED** |
| Owner | Phase 29 (release acceptance) |
| Backlog | `BL-F28-TWO-CANDIDATES` |

The single most important thing a reader of this receipt must know, so it is a ledger row rather than a footnote. It does NOT contradict Criterion 3, because the receipt binds each candidate exactly and per-scope rather than picking one and calling it 'the' candidate - the binding is honest, the coverage is what is split. It does not contradict Criterion 1's text either, which requires each family to pass the matrix and does not require one candidate; it does sit against the PHASE GOAL's words ('the exact candidate proves ...'), and that is stated as an exception in the verdict rather than scored as a criterion contradiction. Eleven merges landed between the two, adding 15 surfaces including the `sandbox` verb that makes the macOS re-run possible at all. Carried knowingly: re-running the Linux and Windows matrix at e4a3f5fc is a real measurement this phase did not take, and re-running any measurement to obtain a better number is forbidden by this plan's scope fence.

### `F-28-04-005` — ACCEPTED

the macOS orphan census is NON-AUTHORITATIVE: it observes a process group, and a hostile descendant can leave one, so its zero is a zero OBSERVATION rather than a containment guarantee

| | |
|---|---|
| Origin | soak |
| Inherited severity (provenance only) | `-` |
| Phase 28 re-score | **MEDIUM** |
| Contradicted criterion | none |
| Dispositions available | `FIXED,DISPROVED,ACCEPTED,DEFERRED` |
| Disposition | **ACCEPTED** |
| Owner | Phase 30 (hardening) |
| Backlog | `BL-F28-MACOS-CENSUS` |

Criterion 2's subject matter is 'no orphan process', so this bounds the strength of that claim on one family and is stated as an exception in the verdict. NOT scored HIGH: the instrument is not absent, it is weaker - it demonstrably detects an orphan, because a deliberately orphaned PRODUCT process was planted and FOUND. Linux (cgroup-v2) and Windows (job object) are authoritative and found theirs too. Scoring an instrument-scope limit as a criterion contradiction is the unproven-control corollary the contract declined at section 3.4. The soak.json record declares authoritative:false and carries the caveat in its own text, so no downstream reader can mistake it.

### `F-28-04-006` — ACCEPTED

the soak workload is READ-ONLY by construction, so state_dir_bytes was 301 bytes at the first sample and 301 at the last on every family

| | |
|---|---|
| Origin | soak |
| Inherited severity (provenance only) | `-` |
| Phase 28 re-score | **MEDIUM** |
| Contradicted criterion | none |
| Dispositions available | `FIXED,DISPROVED,ACCEPTED,DEFERRED` |
| Disposition | **ACCEPTED** |
| Owner | Phase 30 (hardening) |
| Backlog | `BL-F28-SOAK-WORKLOAD` |

A true measurement and a weak one, and 28-03 said so under its own heading rather than in a footnote. A green here means 'a thousand read-only sessions wrote nothing' - worth knowing, and NOT the same as 'the product does not accumulate state under use'. The bands document recorded the related limit BEFORE any number existed: in a soak of 1,000 fresh short-lived processes a per-process leak cannot accumulate, so detection weight sits on the slope bands rather than the drift bands. Carried knowingly; a state-accumulating workload is a deliberate future choice, not an oversight.

### `F-28-04-007` — ACCEPTED

the macOS sandbox activeness observation is RUN-LEVEL: one containment differential is applied to all 24 macOS sandbox-probes cells rather than one observation per cell

| | |
|---|---|
| Origin | matrix |
| Inherited severity (provenance only) | `-` |
| Phase 28 re-score | **MEDIUM** |
| Contradicted criterion | none |
| Dispositions available | `FIXED,DISPROVED,ACCEPTED,DEFERRED` |
| Disposition | **ACCEPTED** |
| Owner | Phase 30 (hardening) |
| Backlog | `BL-F28-RUNLEVEL-ACTIVENESS` |

A matrix-CONSTRUCTION concern raised by 28-02, carried unchanged by 28-03, and still true. It is recorded rather than resolved because resolving it would mean re-running a measurement, which this plan's scope fence forbids, and because narrowing the cell set to match the observation would be a silent reduction of coverage. The differential itself is real and per-run (DNS resolves outside and not inside; /etc readable outside and denied inside), and the same construction was used for Linux and Windows.

### `F-28-04-008` — DEFERRED

desktop_contract_corpus is red (CLASS-CONTRACT-01, structural) and was run by no Phase 28 lane

| | |
|---|---|
| Origin | certification |
| Inherited severity (provenance only) | `-` |
| Phase 28 re-score | **MEDIUM** |
| Contradicted criterion | none |
| Dispositions available | `FIXED,DISPROVED,ACCEPTED,DEFERRED` |
| Disposition | **DEFERRED** |
| Owner | Sean / Desktop release coordination |
| Backlog | `BL-F28-CONTRACT-CORPUS` |

Recorded for completeness because three separate lanes named it and declined it. Structural and outside every Phase 28 lane's brief; closing it would require `wcore-contract generate`, which is a release-coordination action explicitly reserved and NOT performed by this phase. Contradicts no Phase 28 criterion - it is a Desktop wire-contract fixture question.

### `F-28-04-009` — DEFERRED

four of the five tests in wcore-agent/actor_acl_test are VACUOUS: all four assert that the tool RUNS, which is trivially true when the deny pre-filter they exist to test does not exist in production

| | |
|---|---|
| Origin | certification |
| Inherited severity (provenance only) | `-` |
| Phase 28 re-score | **MEDIUM** |
| Contradicted criterion | none |
| Dispositions available | `FIXED,DISPROVED,ACCEPTED,DEFERRED` |
| Disposition | **DEFERRED** |
| Owner | Phase 30 (hardening) |
| Backlog | `BL-F28-VACUOUS-GREENS` |

A finding about the EVIDENCE BASE rather than about the candidate, and it is recorded as its own row instead of being left inside F-28C-R01's disposition, because the number it produces is what a reader would act on. This suite reports 4 passed and contributes nothing: the asserted enforcement string occurs in exactly one file in the workspace - the test itself - and every production construction site sets learned_policy: None. It is the same class as an all-ignored suite, one layer deeper: not zero tests executed, but four executed tests that cannot fail. Left byte-identical, and the suite should be read as a FORWARD SPEC rather than a certification input.

### `F-28-04-010` — DEFERRED

`cargo test --test acp_engine_turn` prints `8 passed` and exits 0 while executing NEITHER of the two cases the binary is named for, because `#[path = "support/mod.rs"] mod support;` compiles 8 further non-ignored tests into the same binary

| | |
|---|---|
| Origin | certification |
| Inherited severity (provenance only) | `-` |
| Phase 28 re-score | **MEDIUM** |
| Contradicted criterion | none |
| Dispositions available | `FIXED,DISPROVED,ACCEPTED,DEFERRED` |
| Disposition | **DEFERRED** |
| Owner | Phase 30 (hardening) |
| Backlog | `BL-F28-COUNT-INFLATION` |

The sharpest zero-execution variant this program has found, because it defeats the program's OWN counter-rule: 'read the N passed count back' sees a healthy 8 passed and is satisfied. It also produced a detector FALSE POSITIVE - the inventory reported this file as all-ignored on a single-file scan - and a detector that over-reports trains the reader to skim the list, so the detector was corrected to resolve `mod` declarations and `#[path]` includes. NOT marked FIXED: an always-running guard worded for this specific hazard was added, but it was generated by the same generator as the other nine and its falsification was measured for three suites rather than for this binary individually, and the count-inflation itself is unchanged. Claiming FIXED over a guard I did not watch fail would be this phase's own defect in miniature.

### `F-28C-01` — DEFERRED

tool_token_bench cannot measure BashTool on any host: it dispatches through a context with no sandbox backend, so every Bash row fails closed and the sanity gate refuses to write the markdown - the bench's Bash column has never been produced

| | |
|---|---|
| Origin | matrix |
| Inherited severity (provenance only) | `-` |
| Phase 28 re-score | **MEDIUM** |
| Contradicted criterion | none |
| Dispositions available | `FIXED,DISPROVED,ACCEPTED,DEFERRED` |
| Disposition | **DEFERRED** |
| Owner | Phase 30 (hardening) |
| Backlog | `BL-F28-BENCH-SANDBOX` |

Bench-class, non-blocking, contradicts no criterion. Real and worth knowing: a measurement surface that has never produced its headline column is the same defect family as a suite that reports ok having run nothing.

### `F-28C-02` — DEFERRED

acp_engine_turn reads the host's real config.toml while documenting itself as hermetic, so its result depends on the developer's machine

| | |
|---|---|
| Origin | matrix |
| Inherited severity (provenance only) | `-` |
| Phase 28 re-score | **MEDIUM** |
| Contradicted criterion | none |
| Dispositions available | `FIXED,DISPROVED,ACCEPTED,DEFERRED` |
| Disposition | **DEFERRED** |
| Owner | Phase 30 (hardening) |
| Backlog | `BL-F28-ACP-HERMETIC` |

The real fault behind RED 3. A non-hermetic test in a suite that calls itself the hermetic test seam is an instrument that reports the operator's environment as a product property - the same class as the retracted SSH belief in F-28-02-004.

### `F-28C-03` — DEFERRED

an ACP/A2A session cannot be established on a headless Linux host with no OS keyring unless the operator sets credentials.backend = "encrypted-file" and supplies a passphrase

| | |
|---|---|
| Origin | matrix |
| Inherited severity (provenance only) | `-` |
| Phase 28 re-score | **MEDIUM** |
| Contradicted criterion | none |
| Dispositions available | `FIXED,DISPROVED,ACCEPTED,DEFERRED` |
| Disposition | **DEFERRED** |
| Owner | Phase 30 (hardening) |
| Backlog | `BL-F28-HEADLESS-KEYRING` |

Fail-closed and actionable with two documented remediations, so not a security defect - but headless Linux is the canonical deployment for an agent CLI and this is a first-run wall. Contradicts no criterion.

### `F-28C-R01` — DISPROVED

wcore-agent/actor_acl_test is RED under --ignored: sub_agent_with_deny_policy_short_circuits expects a deny error and gets 'tool-executed'

| | |
|---|---|
| Origin | matrix |
| Inherited severity (provenance only) | `-` |
| Phase 28 re-score | **MEDIUM** |
| Contradicted criterion | none |
| Dispositions available | `FIXED,DISPROVED,ACCEPTED,DEFERRED` |
| Disposition | **DISPROVED** |

DISPROVED as a product defect by a 4-rung ladder, each rung a separate observation: the asserted enforcement string occurs in exactly one file in the workspace - the test itself, zero product sources; CallActor::SubAgent is constructed only in unit tests; every production site sets learned_policy: None. The pre-filter was deliberately removed in v0.8.1 U11 and the tests retained as a forward spec, and rungs 1-3 confirm that claim independently rather than trusting the prose. The unenforced deny path is UNREACHABLE in production, so it is not a live security gap. Left byte-identical - no #[ignore] removed, no assertion relaxed, no test deleted. The suite's four 'passing' tests were found VACUOUS: all four assert that the tool runs, which is trivially true when no pre-filter exists.

**Counter-evidence.** 28-CLEANUP-SUMMARY.md RED 1 rungs 1-4, reproduced verbatim on hetzner serial --ignored

### `F-28C-R02` — DISPROVED

wcore-agent/tool_token_bench_smoke is RED (0 passed / 1 failed, 63s) with 'sandbox UNAVAILABLE and unsandboxed execution is not permitted'

| | |
|---|---|
| Origin | matrix |
| Inherited severity (provenance only) | `-` |
| Phase 28 re-score | **MEDIUM** |
| Contradicted criterion | none |
| Dispositions available | `FIXED,DISPROVED,ACCEPTED,DEFERRED` |
| Disposition | **DISPROVED** |

DISPROVED as a product defect. The gate was UNDIAGNOSABLE BY CONSTRUCTION - it told the operator to see a scratch workdir that cleanup_workdir deleted on the same code path, and Row never retained the content - so that was fixed additively first, which made the ladder possible. Rungs then showed the host CAN sandbox (bwrap present, rc 0), bwrap IS visible to the test's own environment, and no selection ever happened (WAYLAND_ALLOW_NO_SANDBOX=1 produced a byte-identical failure, and that variable is read only by unsandboxed_fallback()). The bench builds a bare ToolRegistry with no engine, so BashTool takes a FailClosedBackend from ctx.sandbox and is structurally unable to execute there. The product's fail-closed refusal is correct and is the audit M-2 design. Harness gap filed as F-28C-01.

**Counter-evidence.** 28-CLEANUP-SUMMARY.md RED 2 rungs 1-5, reproduced on hetzner

### `F-28C-R03` — DISPROVED

wcore-cli/acp_engine_turn is RED (0 passed / 2 failed): both cases fail at engine init_session

| | |
|---|---|
| Origin | matrix |
| Inherited severity (provenance only) | `-` |
| Phase 28 re-score | **MEDIUM** |
| Contradicted criterion | none |
| Dispositions available | `FIXED,DISPROVED,ACCEPTED,DEFERRED` |
| Disposition | **DISPROVED** |

DISPROVED as a product defect, and THE SUITE IS STILL RED AND IS REPORTED RED. Rung 4 is the load-bearing one: removing the host config under an isolated HOME and XDG_CONFIG_HOME made the plaintext error disappear and MOVED THE PANIC from line 87 to line 98 - a moving panic line attributes a failure rather than guessing at it. The original red was host-config contamination: the test calls Config::resolve(), which reads the operator's real ~/.config/wayland-core/config.toml (line 103 backend = "plaintext") despite the file's doc-comment calling this 'the hermetic test seam'. What remains once isolated is a deliberate, documented, fail-closed refusal with two actionable remediations. It was NOT made to pass: doing so would have meant configuring credentials inside the test, which changes what it asserts. Residuals filed as F-28C-02 and F-28C-03.

**Counter-evidence.** 28-CLEANUP-SUMMARY.md RED 3 rungs 1-5, reproduced on hetzner

### `F-28C-R04` — FIXED

two eval-scenarios suites (cross_session_live, live_personas) print an affirmative `1 passed` in 0.00s having executed nothing, via an env-gated early return (flavour B)

| | |
|---|---|
| Origin | matrix |
| Inherited severity (provenance only) | `-` |
| Phase 28 re-score | **MEDIUM** |
| Contradicted criterion | none |
| Dispositions available | `FIXED,DISPROVED,ACCEPTED,DEFERRED` |
| Disposition | **FIXED** |

Confirmed on hardware rather than inherited - the prior inventory listed them as unchecked candidates. Not in the hostile platform matrix (they are LLM-credential eval suites), so A2 does not fire and the score is on ordinary merits. Fixed by the same always-running zero_execution_guard as the other 13, and the guard is deliberately not #[ignore]d. Their defect is independent of the missing DEEPSEEK_API_KEY, and no credential was supplied.

**Executable check.** the 13-guard falsification table in 28-CLEANUP-SUMMARY.md: env SET without --ignored -> FAILED rc=101, env unset -> ok rc=0, against a 'before' of `ok. 0 passed; N ignored`

### `F-28C-R05` — FIXED

a targeted nextest run matching zero tests exits 0 by default, because .config/nextest.toml set no no-tests policy and vx.toml pins nextest unversioned, so the behaviour depended on whichever CLI happened to be installed

| | |
|---|---|
| Origin | matrix |
| Inherited severity (provenance only) | `-` |
| Phase 28 re-score | **MEDIUM** |
| Contradicted criterion | none |
| Dispositions available | `FIXED,DISPROVED,ACCEPTED,DEFERRED` |
| Disposition | **FIXED** |

The generic close for zero-execution flavours (c) and (d) at the INVOCATION SITE rather than file by file: no-tests = "fail" on [profile.default], inherited by ci/e2e/eval, scoped to the whole invocation so a workspace run is unaffected and only a targeted run matching nothing fails. Stated limit: this is a NEXTEST-ONLY guarantee - plain `cargo test` retains the hazard, which is why the detector still exists and why F-28-04-003 carries the residual.

**Executable check.** falsified known-positive against known-negative on hetzner: `cargo nextest run -p wcore-observability --test otlp_local_test` -> 'error: no tests to run' rc=4; `--lib` -> '50 tests run: 50 passed' rc=0

### `F-KR-08` — ACCEPTED

concurrent live AppContainer executions interfere on one host: the same suite yields 3/2, 2/3 and 1/4 in parallel versus a flat 4/1 across 12 serial runs

| | |
|---|---|
| Origin | matrix |
| Inherited severity (provenance only) | `-` |
| Phase 28 re-score | **MEDIUM** |
| Contradicted criterion | none |
| Dispositions available | `FIXED,DISPROVED,ACCEPTED,DEFERRED` |
| Disposition | **ACCEPTED** |
| Owner | Phase 30 (hardening) |
| Backlog | `BL-F28-WIN-PARALLEL` |

Accepted because the observed failure is the product's fail-closed guard DECLINING TO MEASURE an ambiguous scope ('resolve_anchor_pid found 2 candidate anchors ... the descendant scope would be ambiguous') rather than answering wrongly - correct behaviour, not a defect. Its consequence is an operating rule, now recorded: --test-threads=1 is a CORRECTNESS REQUIREMENT for live sandbox suites, not a preference, and any live-Windows figure this program recorded from a parallel run is untrustworthy. This is the cause of CLASS-WIN-LIVE-01, whose exact 3-passed/2-failed signature reproduced 3/3 in parallel and 0/12 in serial.

### `F-KR-09` — DEFERRED

AppContainer ACL grant+revoke is O(objects under the granted path) and is paid on EVERY execution: 133ms at 200 objects, ~10s at %TEMP%'s 57,636, 19,487ms at 200,000

| | |
|---|---|
| Origin | matrix |
| Inherited severity (provenance only) | `-` |
| Phase 28 re-score | **MEDIUM** |
| Contradicted criterion | none |
| Dispositions available | `FIXED,DISPROVED,ACCEPTED,DEFERRED` |
| Disposition | **DEFERRED** |
| Owner | Phase 30 (hardening) |
| Backlog | `BL-F28-ACL-COST` |

A cost defect, not a containment defect, so it is the subject matter of no criterion. It matters in the field rather than in the lab: the allowlist the repaired test itself documents (~/.cache, ~/.cargo, ~/.npm, ~/.rustup) is exactly the large-tree case, so a real user can pay tens of seconds of setup on every sandboxed command. MEDIUM per standing policy, non-blocking.

### `F-MA-001` — DEFERRED

WorkspacePolicy::contained grants the ENTIRE host temp directory as a writable scratch root (scratch_dirs()), so a contained shell may write anywhere under /tmp

| | |
|---|---|
| Origin | matrix |
| Inherited severity (provenance only) | `-` |
| Phase 28 re-score | **MEDIUM** |
| Contradicted criterion | none |
| Dispositions available | `FIXED,DISPROVED,ACCEPTED,DEFERRED` |
| Disposition | **DEFERRED** |
| Owner | Phase 30 (hardening) |
| Backlog | `BL-F28-TEMP-SCRATCH` |

Deliberate and documented in code, and it contradicts no criterion - recorded, not inflated. Measured rather than argued: it is what produced the first red of the new e2e containment gate, whose escape target sat in a tempfile::tempdir() the policy legitimately grants. That test was fixed at its cause rather than having its assertion weakened, and the episode is recorded in a comment in the test so a later reader does not 'fix' it the other way.

### `F-WR-03` — DISPROVED

execute-from-a-granted-directory is uncovered by any green test

| | |
|---|---|
| Origin | matrix |
| Inherited severity (provenance only) | `-` |
| Phase 28 re-score | **MEDIUM** |
| Contradicted criterion | none |
| Dispositions available | `FIXED,DISPROVED,ACCEPTED,DEFERRED` |
| Disposition | **DISPROVED** |

Retired on measurement. Rung 4 of the denial ladder ran the script from a granted directory with cwd:None and it ran to the 20s timeout with a 1749-byte heartbeat, so the path works and is now additionally covered by the repaired descendant test's green.

**Counter-evidence.** evidence/28-kr01-repair/kr01-denial-ladder.log rung 4 - exit 0, heartbeat 1749 bytes

### `F-WR-04` — FIXED

leaked live-test state on the Windows certification host: 564 AppContainer profiles and 68 work directories under C:\Users\Public

| | |
|---|---|
| Origin | matrix |
| Inherited severity (provenance only) | `-` |
| Phase 28 re-score | **MEDIUM** |
| Contradicted criterion | none |
| Dispositions available | `FIXED,DISPROVED,ACCEPTED,DEFERRED` |
| Disposition | **FIXED** |

The count was a direct census of two weeks of historical failures of the very test repaired under F-WR-01: the work directory is removed only on the test's success path, so a test that could not reach its assertion leaked a directory and a profile on every run. Cleaned (564 removed, 0 failures, 0 leases remaining) and the repaired test tears down via reap_stray_descendants() and no longer creates a %PUBLIC% work dir at all. Cleanup was scoped to wcoresandbox*/WCore* profiles and wcore-* dirs; no other lane's state was touched. Note the leaked profiles were ALSO tested as a hypothesis for the parallel-mode failures and REFUTED - removing all 564 changed neither mode's outcome.

**Executable check.** F-WR-REPAIR-SUMMARY.md - 564 profiles removed with 0 failures, 68 work directories, 0 leases remaining, LEFTOVER_MY_TASKS=0

### `F-WR-06` — FIXED

over ssh+PowerShell every non-zero exit status collapses to 1, and stdout sentinels are insufficient because CLIXML progress records splice into the stream and a status line can vanish while its marker survives

| | |
|---|---|
| Origin | matrix |
| Inherited severity (provenance only) | `-` |
| Phase 28 re-score | **MEDIUM** |
| Contradicted criterion | none |
| Dispositions available | `FIXED,DISPROVED,ACCEPTED,DEFERRED` |
| Disposition | **FIXED** |

A measurement-transport defect that silently destroys a result rather than failing loudly: a Windows leg could not distinguish 'failed' from 'failed the way we predicted', so rc==1 passed for every failure mode it was meant to separate. Fixed by a verified carrier rather than by care: the remote writes WLRC first and WLDONE last to a status file, a SEPARATE ssh call reads it back, and exit status is ignored entirely, with three-state grading (no marker = incomplete, marker without status = UNREADABLE, both = true code). Landed in LANE-BRIEF section 2 and AGENTS.md section 11. Related trap recorded: "$LASTEXITCODE:TAG" renders EMPTY because PowerShell reads $VAR: as namespace notation.

**Executable check.** 7/7 faithful over exit codes 0/1/2/3/7/100/255; evidence/28-kr01-repair/F-WR-06-EXIT-STATUS-PATTERN.md; the mid-write UNREADABLE case was then observed live during a real build poll

### `KR-02` — DEFERRED

Windows snapshot private DACL enforcement is unproven; snapshot.rs windows_private_dacl_* fail at their WRITE_DAC reopen step with error 5, identically at parent

| | |
|---|---|
| Origin | carried-red |
| Inherited severity (provenance only) | `known-red/non-gating` |
| Phase 28 re-score | **MEDIUM** |
| Contradicted criterion | none |
| Dispositions available | `FIXED,DISPROVED,ACCEPTED,DEFERRED` |
| Disposition | **DEFERRED** |
| Owner | Phase 30 (hardening) |
| Backlog | `BL-F28-KR02` |

A1 re-score unchanged from 28-01: the observed red is in the test's own reopen step and reproduces at parent, so it is not evidence of a candidate regression, and it contradicts no criterion as scored. Contract section 3.4's unproven-control corollary would move it across the A2 line and was DELIBERATELY NOT APPLIED - inventing a stricter rule than the recorded decision is what grew Phase 20 to 74 plans. Not repaired here: this plan repairs nothing by design.

### `KR-03` — DEFERRED

worker output exhaustion buffer-retention bound is unproven; worker_runtime_limits::multi_worker_output_exhaustion_fails_without_retaining_buffers takes ~35s against a 20s budget and the timeout was deliberately NOT raised

| | |
|---|---|
| Origin | carried-red |
| Inherited severity (provenance only) | `known-red/non-gating` |
| Phase 28 re-score | **MEDIUM** |
| Contradicted criterion | none |
| Dispositions available | `FIXED,DISPROVED,ACCEPTED,DEFERRED` |
| Disposition | **DEFERRED** |
| Owner | Phase 30 (hardening) |
| Backlog | `BL-F28-KR03` |

A1 re-score unchanged: the red is a budget overrun in the test, not an observed retention of buffers, so no criterion-contradicting behaviour is established. Same section 3.4 treatment as KR-02. The soak's resource slopes (harness_rss_bytes <= 2x, all green on three families) are adjacent evidence but do not measure this bound.

## LOW (14)

### `F-28-01-002` — DEFERRED

the standing severity policy is not in AGENTS.md; 28-01 was directed to quote it verbatim from there and that file contains no such text

| | |
|---|---|
| Origin | contract |
| Inherited severity (provenance only) | `n/a-raised-by-phase-28` |
| Phase 28 re-score | **LOW** |
| Contradicted criterion | none |
| Dispositions available | `FIXED,DISPROVED,ACCEPTED,DEFERRED` |
| Disposition | **DEFERRED** |
| Owner | Phase 29 (release acceptance) |
| Backlog | `BL-F28-POLICY-DOC` |

Documentation defect in the program's own instruction set, not in the candidate. Section 1.3 of the contract quotes ROADMAP.md and commit d0837aa7 instead and says so. Either add the policy to AGENTS.md section 11 or correct the plans that cite it as living there.

### `F-28-01-R003` — ACCEPTED

surface `wayland-core fetch` is exposed by the candidate binary and is discussed by phase artifacts but not in a form the resolver recognises as a claim (class attribution-weak)

| | |
|---|---|
| Origin | candidate-resolution |
| Inherited severity (provenance only) | `n/a-raised-by-phase-28` |
| Phase 28 re-score | **LOW** |
| Contradicted criterion | none |
| Dispositions available | `FIXED,DISPROVED,ACCEPTED,DEFERRED` |
| Disposition | **ACCEPTED** |
| Owner | Phase 29 (release acceptance) |
| Backlog | `BL-F28-SURFACE-WEAK` |

A limit of the INSTRUMENT, deliberately never rendered as a fact about the product: the resolver's recall is incomplete and the class exists so that gap cannot read as 'the phase never claimed it'. The bare claim form is barred from accusing, because accusing a phase of claiming a surface that does not exist costs a disposition to clear. Carried knowingly.

### `F-28-01-R005` — ACCEPTED

surface `wayland-core goal` is exposed by the candidate binary and is discussed by phase artifacts but not in a form the resolver recognises as a claim (class attribution-weak)

| | |
|---|---|
| Origin | candidate-resolution |
| Inherited severity (provenance only) | `n/a-raised-by-phase-28` |
| Phase 28 re-score | **LOW** |
| Contradicted criterion | none |
| Dispositions available | `FIXED,DISPROVED,ACCEPTED,DEFERRED` |
| Disposition | **ACCEPTED** |
| Owner | Phase 29 (release acceptance) |
| Backlog | `BL-F28-SURFACE-WEAK` |

A limit of the INSTRUMENT, deliberately never rendered as a fact about the product: the resolver's recall is incomplete and the class exists so that gap cannot read as 'the phase never claimed it'. The bare claim form is barred from accusing, because accusing a phase of claiming a surface that does not exist costs a disposition to clear. Carried knowingly.

### `F-28-01-R006` — ACCEPTED

surface `wayland-core image` is exposed by the candidate binary and is discussed by phase artifacts but not in a form the resolver recognises as a claim (class attribution-weak)

| | |
|---|---|
| Origin | candidate-resolution |
| Inherited severity (provenance only) | `n/a-raised-by-phase-28` |
| Phase 28 re-score | **LOW** |
| Contradicted criterion | none |
| Dispositions available | `FIXED,DISPROVED,ACCEPTED,DEFERRED` |
| Disposition | **ACCEPTED** |
| Owner | Phase 29 (release acceptance) |
| Backlog | `BL-F28-SURFACE-WEAK` |

A limit of the INSTRUMENT, deliberately never rendered as a fact about the product: the resolver's recall is incomplete and the class exists so that gap cannot read as 'the phase never claimed it'. The bare claim form is barred from accusing, because accusing a phase of claiming a surface that does not exist costs a disposition to clear. Carried knowingly.

### `F-28-01-R009` — ACCEPTED

surface `wayland-core migrate` is exposed by the candidate binary and is discussed by phase artifacts but not in a form the resolver recognises as a claim (class attribution-weak)

| | |
|---|---|
| Origin | candidate-resolution |
| Inherited severity (provenance only) | `n/a-raised-by-phase-28` |
| Phase 28 re-score | **LOW** |
| Contradicted criterion | none |
| Dispositions available | `FIXED,DISPROVED,ACCEPTED,DEFERRED` |
| Disposition | **ACCEPTED** |
| Owner | Phase 29 (release acceptance) |
| Backlog | `BL-F28-SURFACE-WEAK` |

A limit of the INSTRUMENT, deliberately never rendered as a fact about the product: the resolver's recall is incomplete and the class exists so that gap cannot read as 'the phase never claimed it'. The bare claim form is barred from accusing, because accusing a phase of claiming a surface that does not exist costs a disposition to clear. Carried knowingly.

### `F-28-01-R011` — ACCEPTED

surface `wayland-core profile` is exposed by the candidate binary and is discussed by phase artifacts but not in a form the resolver recognises as a claim (class attribution-weak)

| | |
|---|---|
| Origin | candidate-resolution |
| Inherited severity (provenance only) | `n/a-raised-by-phase-28` |
| Phase 28 re-score | **LOW** |
| Contradicted criterion | none |
| Dispositions available | `FIXED,DISPROVED,ACCEPTED,DEFERRED` |
| Disposition | **ACCEPTED** |
| Owner | Phase 29 (release acceptance) |
| Backlog | `BL-F28-SURFACE-WEAK` |

A limit of the INSTRUMENT, deliberately never rendered as a fact about the product: the resolver's recall is incomplete and the class exists so that gap cannot read as 'the phase never claimed it'. The bare claim form is barred from accusing, because accusing a phase of claiming a surface that does not exist costs a disposition to clear. Carried knowingly.

### `F-28-01-R013` — ACCEPTED

surface `wayland-core sandbox` is exposed by the candidate binary and is discussed by phase artifacts but not in a form the resolver recognises as a claim (class attribution-weak)

| | |
|---|---|
| Origin | candidate-resolution |
| Inherited severity (provenance only) | `n/a-raised-by-phase-28` |
| Phase 28 re-score | **LOW** |
| Contradicted criterion | none |
| Dispositions available | `FIXED,DISPROVED,ACCEPTED,DEFERRED` |
| Disposition | **ACCEPTED** |
| Owner | Phase 29 (release acceptance) |
| Backlog | `BL-F28-SURFACE-WEAK` |

A limit of the INSTRUMENT, deliberately never rendered as a fact about the product: the resolver's recall is incomplete and the class exists so that gap cannot read as 'the phase never claimed it'. The bare claim form is barred from accusing, because accusing a phase of claiming a surface that does not exist costs a disposition to clear. Carried knowingly.

### `F-28-01-R015` — ACCEPTED

surface `wayland-core session` is exposed by the candidate binary and is discussed by phase artifacts but not in a form the resolver recognises as a claim (class attribution-weak)

| | |
|---|---|
| Origin | candidate-resolution |
| Inherited severity (provenance only) | `n/a-raised-by-phase-28` |
| Phase 28 re-score | **LOW** |
| Contradicted criterion | none |
| Dispositions available | `FIXED,DISPROVED,ACCEPTED,DEFERRED` |
| Disposition | **ACCEPTED** |
| Owner | Phase 29 (release acceptance) |
| Backlog | `BL-F28-SURFACE-WEAK` |

A limit of the INSTRUMENT, deliberately never rendered as a fact about the product: the resolver's recall is incomplete and the class exists so that gap cannot read as 'the phase never claimed it'. The bare claim form is barred from accusing, because accusing a phase of claiming a surface that does not exist costs a disposition to clear. Carried knowingly.

### `F-28-01-R016` — ACCEPTED

surface `wayland-core setup` is exposed by the candidate binary and is discussed by phase artifacts but not in a form the resolver recognises as a claim (class attribution-weak)

| | |
|---|---|
| Origin | candidate-resolution |
| Inherited severity (provenance only) | `n/a-raised-by-phase-28` |
| Phase 28 re-score | **LOW** |
| Contradicted criterion | none |
| Dispositions available | `FIXED,DISPROVED,ACCEPTED,DEFERRED` |
| Disposition | **ACCEPTED** |
| Owner | Phase 29 (release acceptance) |
| Backlog | `BL-F28-SURFACE-WEAK` |

A limit of the INSTRUMENT, deliberately never rendered as a fact about the product: the resolver's recall is incomplete and the class exists so that gap cannot read as 'the phase never claimed it'. The bare claim form is barred from accusing, because accusing a phase of claiming a surface that does not exist costs a disposition to clear. Carried knowingly.

### `F-28-01-R018` — ACCEPTED

surface `wayland-core workflow` is exposed by the candidate binary and is discussed by phase artifacts but not in a form the resolver recognises as a claim (class attribution-weak)

| | |
|---|---|
| Origin | candidate-resolution |
| Inherited severity (provenance only) | `n/a-raised-by-phase-28` |
| Phase 28 re-score | **LOW** |
| Contradicted criterion | none |
| Dispositions available | `FIXED,DISPROVED,ACCEPTED,DEFERRED` |
| Disposition | **ACCEPTED** |
| Owner | Phase 29 (release acceptance) |
| Backlog | `BL-F28-SURFACE-WEAK` |

A limit of the INSTRUMENT, deliberately never rendered as a fact about the product: the resolver's recall is incomplete and the class exists so that gap cannot read as 'the phase never claimed it'. The bare claim form is barred from accusing, because accusing a phase of claiming a surface that does not exist costs a disposition to clear. Carried knowingly.

### `F-28-04-011` — ACCEPTED

the two macOS members of the hostile platform matrix infer containment from a wall-clock bound and a matched write pair rather than from a containment differential of the kind the E5 matrix requires

| | |
|---|---|
| Origin | matrix |
| Inherited severity (provenance only) | `-` |
| Phase 28 re-score | **LOW** |
| Contradicted criterion | none |
| Dispositions available | `FIXED,DISPROVED,ACCEPTED,DEFERRED` |
| Disposition | **ACCEPTED** |
| Owner | Phase 30 (hardening) |
| Backlog | `BL-F28-MACOS-INSTRUMENTS` |

Recorded after reading both test bodies rather than accepting their green. Both ARE substantive, and the 0.05s / 0.04s durations are not a smell: hard_process_containment_macos runs `/bin/sh -c '/bin/sleep 45 & exit N'` under sandbox-exec twice and asserts wall clock < 20s, so FAST IS THE PASS CONDITION - a non-reaping backend leaves the detached sleep holding the stdout pipe and execute blocks to 45s or the 30s manifest timeout, and it physically cannot produce 0.05s. The bound is ONE-SIDED IN THE SAFE DIRECTION: runner load can only make it slower, so load can only produce a false FAIL and never a false PASS. live_integrity_macos is a matched pair whose INSIDE-write half is the built-in control that separates 'the sandbox contained it' from 'the sandbox failed to launch', which is the vacuous-pass trap this program keeps finding. The residual, and the only reason this row exists: neither forms a containment DIFFERENTIAL, so the macOS evidence for Criterion 1 rests on two different instruments (these two suites plus the 216-cell E5 re-run) rather than one.

### `F-MA-002` — DEFERRED

`backend run --backend local` names a containment backend in its effective policy while never applying it to the child

| | |
|---|---|
| Origin | matrix |
| Inherited severity (provenance only) | `-` |
| Phase 28 re-score | **LOW** |
| Contradicted criterion | none |
| Dispositions available | `FIXED,DISPROVED,ACCEPTED,DEFERRED` |
| Disposition | **DEFERRED** |
| Owner | Phase 30 (hardening) |
| Backlog | `BL-F28-LOCAL-BACKEND` |

Honest in the receipt text ('selected but NOT applied to this child'), which is why it is LOW rather than an instance of the KR-05 pattern. It matters because it is the surface 28-02 reached for FIRST when looking for macOS activeness evidence, and it is what F-28-02-005 actually measured.

### `F-WR-05` — DEFERRED

running the sandbox as SYSTEM trips validate_mutex_security: acquire() always builds a 2-entry DACL while the validator expects 1 when the caller's SID IS the SYSTEM SID, and it fails closed with a message that reads like a platform limitation

| | |
|---|---|
| Origin | matrix |
| Inherited severity (provenance only) | `-` |
| Phase 28 re-score | **LOW** |
| Contradicted criterion | none |
| Dispositions available | `FIXED,DISPROVED,ACCEPTED,DEFERRED` |
| Disposition | **DEFERRED** |
| Owner | Phase 30 (hardening) |
| Backlog | `BL-F28-SYSTEM-DACL` |

The KR-05 pattern in miniature - a fail-closed refusal wearing the costume of a platform limitation - but SYSTEM is not the shipping configuration, so LOW. It contradicts no criterion and it cost one lane a run rather than any user anything. Recorded because the misleading-message half of KR-05 is CONFIRMED and this is a second instance of it.

### `KR-04` — ACCEPTED

bash cannot run under Windows AppContainer at all; msys requires \BaseNamedObjects and AppContainer confines to AppContainerNamedObjects by construction (0xC0000022)

| | |
|---|---|
| Origin | carried-red |
| Inherited severity (provenance only) | `known-red/architectural` |
| Phase 28 re-score | **LOW** |
| Contradicted criterion | none |
| Dispositions available | `FIXED,DISPROVED,ACCEPTED,DEFERRED` |
| Disposition | **ACCEPTED** |
| Owner | Phase 29 (release acceptance) |
| Backlog | `BL-F28-KR04` |

Architectural impossibility, not a defect in the candidate: no budget fixes it. The product contract is fail-closed and the test asserts it. Carried knowingly and enumerated in the signed receipt so it is visible at acceptance and again at release rather than absorbed. This is the canonical architectural-impossibility instance and its impossibility_check IS that fail-closed assertion.

---

## How to check this ledger is worth what it claims

```bash
python3 .planning/scripts/f28-ledger.py --self-test
P=.planning/phases/28-native-cross-platform-certification
python3 .planning/scripts/f28-ledger.py --validate $P/evidence/28-04/findings.tsv --allow-open
python3 .planning/scripts/f28-ledger.py --validate $P/evidence/28-04/findings.tsv   # now PASSES: see below
python3 .planning/scripts/f28-ledger.py --check-a2 $P/evidence/28-04/findings.tsv
python3 .planning/scripts/f28-ledger.py --check-downgrades $P/evidence/28-04/findings.tsv
python3 .planning/scripts/f28-ledger.py --check-backlog-ids $P/evidence/28-04/findings.tsv .planning/BACKLOG.md
python3 .planning/scripts/f28-ledger.py --check-completeness $P/evidence/28-04/findings.tsv \
    $P/evidence/28-02/results.json $P/evidence/28-03/soak.json \
    $P/evidence/28-03/candidate.json $P/evidence/28-01/known-red.tsv
```

**The strict `--validate` used to fail with exactly one `F28L-002` on `F-28-02-002`. As of
2026-07-29 it passes, and this paragraph records which of the two fork branches that is.**

The original wording was: *"A run in which it passes means either the finding was repaired or it
was laundered, and the difference is the whole point."* It was **repaired.** `F-28-02-002` was
re-adjudicated `FIXED` by `lane/28-adj`, a lane independent of the one that authored the repair,
against a merged fix re-measured on real Windows hardware; the row above carries the executable
check and `28-ADJUDICATION.md` carries what was tried against the claim and failed to break it.
No severity was downgraded and no accept path was opened — the two forgeries this ledger names.

**Moving the row does not make the gate vacuous, and here is why that is checkable rather than
asserted.** `F28L-002`'s ability to fire never depended on this production row. `--self-test`
proves it against synthetic fixtures — `_ledger_row(disposition=OPEN)` under both `allow_open`
settings (`f28-ledger.py:836`) — and reads `findings.tsv` not at all. A gate whose only proof of
life is a permanently-broken production row is a gate nobody can ever satisfy, which is the
opposite of the discipline. Three checks were run to prove the tooth is still in:

```bash
# 1. the rule still fires on a row with NO disposition
sed '39s/\tFIXED\t/\t\t/' $P/evidence/28-04/findings.tsv > /tmp/nodisp.tsv
python3 .planning/scripts/f28-ledger.py --validate /tmp/nodisp.tsv          # F28L-002, rc=1
# 2. and on a row still OPEN
sed '39s/\tFIXED\t/\tOPEN\t/' $P/evidence/28-04/findings.tsv > /tmp/open.tsv
python3 .planning/scripts/f28-ledger.py --validate /tmp/open.tsv            # F28L-002, rc=1
# 3. and FIXED without an executable check is still rejected, so this row could not be
#    laundered by simply writing FIXED into it
python3 .planning/scripts/f28-ledger.py --validate /tmp/noevidence.tsv      # F28L-008, rc=1
```

All three were measured red before this paragraph was written; the transcript is in
`evidence/28-adj/gate-falsification.log`.

**And read the dissent first.** `28-01-decision-evidence/decision-dissent.txt` says in terms:
*if a future plan, executor or reviewer keeps the four dispositions but drops or softens A2 —
by scoring findings at their inherited severity, or by letting a criterion-contradicting
finding take the ACCEPT path — then this decision has silently become c4-standing with
paperwork.* The two checks that falsify that here are `--check-a2` and the A2 crossings table
above.
