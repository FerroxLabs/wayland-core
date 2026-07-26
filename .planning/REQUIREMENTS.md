# Requirements: Wayland Core Frontier Candidate v2

**Defined:** 2026-07-18
**Core Value:** A simple, bounded, crash-complete, transactionally delegated, operator-complete cross-platform agent proven through the packaged product.

## v1 Requirements

### Program Admission Controls

- [x] **CTRL-01**: A schema-complete versioned capability/maturity ledger with pinned Hermes/OpenClaw baselines exists before Phase 21 and is refreshed at every admitted phase; F30 independently reviews it. — **ESTABLISHED 2026-07-26** (`d06a6051`). Baselines pinned from the real peer trees: Hermes 0.17.0 @ `dbe734beff0caf5e8ee2acbe4277db7f6cf84a21`, OpenClaw 2026.6.2 @ `11a0ad10e91a50d5a0e636494eea4d7ad3eaf9fc`. All 10 ledger rows dispositioned with F03/F05 evidence mapped; zero `UNPINNED`/`PENDING` remain. Two carried limitations are tracked and non-blocking: re-run the F05 capability gate against `delegate_isolation` at `9821ef76` (owner Phase 21), and runtime trials (owner Phase 30, by design). Ledger: `.planning/intel/COMPETITIVE-LEDGER.md`.
- [ ] **CTRL-02**: D1 publishes a pinned Core producer contract, linked Desktop plan, and real consumer/reducer conformance suite before Phase 21 broad execution.
- [ ] **CTRL-03**: New packaged/customer evidence enters a live regression register and cannot be silently settled by older source/test acceptance.
- [ ] **CTRL-04**: D2 freezes the durable Core producer protocol and passes canonical serialized fixtures through the real Desktop consumer/reducer before Phase 23 exits.

### Phase 20 — Transactional Delegated Mutation

All eight complete against ONE exact SHA — `01a5b0ae459c9d5088cfd7e41271a5d4ece1b9bb` (tree `4a5247ca804a88c5fc621402d5e55a3dab10e8a5`, branch `plan/f20-unified-audit-repair`), proved on `hetzner-dsm:/root/wayland` with a clean working tree on 2026-07-25. `cargo build --locked --workspace --all-features` exit 0; `cargo nextest run --profile ci --no-fail-fast` exit 0 — `11519 tests run: 11519 passed (1 slow, 3 flaky), 48 skipped`, zero failed, zero timed out. Evidence: `phases/20-transactional-delegated-mutation/20-56-evidence/{build,test}-01a5b0ae-GREEN.log.gz`. The native Windows/macOS path is Phase 20A and is NOT claimed here.

- [x] **F20-01**: Delegated work is classified as read-only/shared or mutating/isolated before execution.
- [x] **F20-02**: An unmerged child cannot mutate the parent workspace, configuration, repository metadata, or protected state.
- [x] **F20-03**: Conflicting child edits stop for explicit resolution; failed gates cannot merge and preserve diagnostic evidence.
- [x] **F20-04**: Workspace creation, journal state, receipts, candidate gates, parent compare-and-swap, cleanup, and rollback are one coherent lifecycle.
- [x] **F20-05**: Snapshot authority and Windows AppContainer identity fail closed on stale, ambiguous, or unowned state.
- [x] **F20-06**: The accepted F20 successor is integrated into the one admitted candidate with exact focused and aggregate evidence.
- [x] **F20-GATE-01**: Failed, stale, incomplete, malformed, reordered, duplicated, post-terminal, or mismatched candidate gates remain non-landing and preserve durable diagnostics.
- [x] **F20-GATE-02**: Only parent-observed execution of the exact live candidate under qualifying hard containment, followed by authoritative receipt append and replay, can create the opaque candidate-acceptance handoff; caller, child, model, and advisory-evaluator claims cannot.

#### Phase 20 Native-UAT Repair (R1–R12)

Additive, non-overlapping requirements from `native-uat-repair-BRIEF.md` (SPEC, ingested 2026-07-23). They refine F20-05 / Success Criterion #3 (native Windows/macOS identities share one authoritative lifecycle) with concrete, hardware-evidenced acceptance. Full text + acceptance in `.planning/intel/inbox-2026-07-23/requirements.md`.

**Binding run.** Every disposition below is bound to sealed SHA `9821ef7603ac1e687b600cda591af1657c883484` (tree `0a1267a990f3b512782916b6ed26501d0db39222`, tag `f20a-candidate-9821ef76`, local `refs/f20a/candidate`) and to **one** dispatched, Sean-authorized run: `nightly-windows-soak` **`30184651330`** on `FerroxLabs/wayland-core`, `workflow_dispatch`, `completed`/`success`, 2026-07-26T02:30:03Z → 02:48:08Z. Both acceptance markers were emitted at that exact commit/tree/nonce (`nonce=96c91107636c4eaca9130969369b2309ee6dd6582cc4e9e1a7a45e0fb8ec92cf`):

- `F20_NATIVE_WINDOWS_ACCEPTANCE=PASS` — job `89747993276`, runner `ferrox-win-msvc` (`SEANDESKTOP`), 6/6 targets.
- `F20_NATIVE_MACOS_ACCEPTANCE=PASS` — job `89747992986`, runner `f20-macos-ephemeral-1d053640` (`Seans-MacBook-Pro`), 8/8 targets.

Both candidate jobs ran the `Assert checkout is the authorized candidate` step with `F20_EXPECTED_SHA: 9821ef76…` **before** any toolchain or proof work, and pinned `actions/checkout` to that SHA. Eleven of fifteen requirements were complete at the seal, with four recorded OPEN (r2, r8, r12, r13). **The 2026-07-26 post-seal sweep closed r2 and r8 on hardware evidence at a `crates/`-identical commit — 13 complete, r12 and r13 still OPEN** with the specific unmet clause named. Full per-target evidence: `phases/20A-native-windows-macos-uat/20A-04-SUMMARY.md` §13 (seal) and §13.10 (sweep).

- [x] **REQ-native-r1**: `AppContainerBackend::is_available()` returns true on real Windows (add `.write(true)` to `storage.rs` `create_new_nofollow`). — **COMPLETE.** Directly exercised, not merely present: `live_fs_acl.rs::require_live_acceptance()` (`:35`) and `hard_process_containment_windows.rs::require_live_windows()` (`:71`) each hard-`assert!(AppContainerBackend::new().is_available())`. Four dispatched targets run through those guards and PASS on `ferrox-win-msvc`: `windows-retained-handle`, `windows-appcontainer-acl`, `windows-job-object`, `windows-hard-process-containment`. `.write(true)` confirmed in the sealed tree at `storage.rs:421`.
- [x] **REQ-native-r2**: Sandboxed process reads granted files / denied ungranted, grants revoked, isolation preserved (drop deny-only `SidsToDisable` in `CreateRestrictedToken`). — **COMPLETE (3 of 3 acceptance clauses proven; the last two closed 2026-07-26 post-seal).** Clauses 1–2 were proven by run `30184651330`: `granted_path_is_readable_then_revoked` PASS 2.056s (grant ACE observed present on the host DACL during the run, exit 0 + MARKER read, ACE absent after) and `one_execution_grant_never_leaks_to_another_identity` PASS 2.043s. Clause 3 — "a genuine DENY ace still blocks" **and** "a file granted only to normal SIDs is still denied" — is now proven on real Windows hardware: run **`30186873948`**, job **`89753061944`**, runner `ferrox-win-msvc` (`SEANDESKTOP`), `WAYLAND_SANDBOX_LIVE_WINDOWS=[1] (len=1)` asserted byte-exact before the suite:<br>`PASS [ 0.210s] ( 3/12) wcore-sandbox::live_fs_acl deny_ace_still_blocks_granted_read`<br>`PASS [ 0.235s] ( 6/12) wcore-sandbox::live_fs_acl normal_sid_only_grant_is_denied`<br>whole suite `12 tests run: 12 passed, 0 skipped`, job conclusion `success`. Independently reproduced one run earlier (`30186743564` / job `89752739969`, both PASS, 12/12 with one nextest-retried flake — see the flake note in `20A-04-SUMMARY.md` §13.10). **Binding to the seal:** the observation is at commit `2cc1a285`, not literally `9821ef76`; `git diff 9821ef76 2cc1a285 -- crates/ Cargo.lock Cargo.toml` is **empty**, so the product tree under test is byte-identical to the sealed tree and only CI/harness files differ. **Wiring repaired** so this can never recur: the `windows-live-acceptance` job no longer carries `if: inputs.f20_candidate != 'true'`; it now runs in both modes, and in candidate mode `needs: f20-windows-candidate` serializes it after the six-target proof on the shared box and pins its checkout to `f20_expected_sha`. The six-target proof is untouched — `scripts/f20-native-windows-proof.ps1` has zero diff. Caveat recorded, not glossed: the candidate-mode branch of that wiring is verified by review plus a real non-candidate dispatch; no candidate dispatch has been fired since.
- [x] **REQ-native-r3**: `wcore-agent` compiles on `x86_64-pc-windows-msvc` (`READ_CONTROL`/`WRITE_DAC` from `Win32::Storage::FileSystem`). — **COMPLETE.** Target `windows-f20-lifecycle` (`-p wcore-agent --test transactional_delegated_mutation_test`) compiled and ran on the msvc runner: **9 tests run, 9 passed, 0 skipped** in 26.251s. E0432 cannot be open on a binary that linked and executed.
- [x] **REQ-native-r4**: No Linux/macOS regression (Linux 11509/0; macOS 8/8 hold). — **COMPLETE (split evidence, disclosed).** macOS half bound to run `30184651330`: all 8 targets PASS, `F20_NATIVE_MACOS_ACCEPTANCE=PASS`. Linux half is **not** part of the dispatched run: it rests on the Hetzner aggregate at the same sealed SHA recorded in `20A-04-SUMMARY.md` §12.6 — `cargo build --locked --workspace --all-features` exit 0, `cargo nextest run --profile ci --no-fail-fast` exit 0, `11520 tests run: 11520 passed (1 slow, 1 flaky), 48 skipped`. Caveat: no compressed log artifact was retained for that Linux run (unlike `20-56-evidence/`), and it was not re-derived at closeout.
- [x] **REQ-native-r5**: `type_and_hold` asserts on granted-read success, not `choice.exe` exit index (stdin-free hold). — **COMPLETE.** The acceptance names one test exactly: `live_fs_acl.rs::one_execution_grant_never_leaks_to_another_identity`. It is target `windows-retained-handle` and PASSED 2.043s on real Windows. `type_and_hold` (`live_fs_acl.rs:140`) is a `cmd` builtin `for /L` hold — no child image, no stdin, no `choice.exe`; the assertion is `exit_code == 0` + MARKER content.
- [x] **REQ-native-r6**: `dispatch_smoke` Windows-portable (no `fs::rename` of open dir). — **COMPLETE.** The acceptance names one test exactly: `dispatch_rejects_different_head_repository_replacement` must no longer panic with Os code 5 PermissionDenied on Windows. It PASSED 0.121s as test 1/10 of target `windows-public-dispatch` — the whole `dispatch_smoke` binary ran **10/10 passed, 0 skipped**.
- [x] **REQ-native-r7**: `windows-job-object`/`windows-hard-process-containment` targets map to REAL Windows Job-Object containment tests (must be authored). — **COMPLETE.** Both targets select `crates/wcore-sandbox/tests/hard_process_containment_windows.rs` (`#![cfg(windows)]`; real `CreateJobObjectW` / `SetInformationJobObject` / `AssignProcessToJobObject` / `QueryInformationJobObject` calls). In the dispatched run `windows-job-object` ran **4/4 passed**: `active_process_cap_is_enforced` (cap), `breakaway_is_denied` (breakaway), `contained_detached_child_exit` (exit-code fidelity), `job_close_reaps_detached_descendant_with_no_residue` (KILL_ON_JOB_CLOSE + no residue) — the four named acceptance mechanisms. `windows-hard-process-containment` ran `qualified_hard_containment_backend_preflight` **1/1 passed**, asserting `owns_descendants_hard()`, `enforces_read_deny()`, `blocks_powershell()` and driving one live contained command to exit 0.
- [x] **REQ-native-r8**: Structural guard preventing a native proof target mapping to a wrong-OS test. — **COMPLETE (rejection demonstrated on both production guards, 2026-07-26).** The admission direction was already exercised by run `30184651330` (all four OS-specific Windows targets and both OS-specific macOS targets admitted without firing). The missing direction is now shown against the **real** guards, extracted verbatim and driven directly — neither proof script was modified or executed. **Windows**, `Assert-TargetOsGate` AST-extracted from `scripts/f20-native-windows-proof.ps1` in the sealed checkout (`CHECKOUT_HEAD=9821ef76…`, source SHA256 `a79d2ed4…`), pwsh 7.6.3 on `SEANDESKTOP`: control `windows-appcontainer-acl → live_fs_acl` **ADMITTED**; `windows-appcontainer-acl → hard_process_containment_macos` **REJECTED** — `anti-drift: target windows-appcontainer-acl (os=windows) selects a test source cfg-gated for macos: …`; `windows-job-object → hard_process_containment` (the Linux-only Bubblewrap file, the exact historical mis-wiring) **REJECTED** — `anti-drift: target windows-job-object declares os=windows but its selected test source is not cfg-gated for windows (a wrong-OS or ungated test cannot prove windows containment): …`. **macOS**, `assert_target_os_gate` extracted verbatim from `scripts/f20-native-macos-proof.sh` (source SHA256 `26758227…`): control `macos-process-tree → hard_process_containment_macos` **ADMITTED**; `macos-retained-directory → live_fs_acl` (the exact 07-22 failure) **REJECTED (exit 1)** — `anti-drift: macos target macos-retained-directory source is not cfg-gated for macos: …`; `macos-process-tree → hard_process_containment_windows` **REJECTED (exit 1)**. Durable regression added: `scripts/f20-native-uat-proof.test.mjs` grew from 34 to 41 cases, seven of them driving the rule to rejection over the canonical `WINDOWS_TARGET_SOURCES`/`MACOS_TARGET_SOURCES` map (wrong-OS both directions, ungated source, foreign gate alongside a correct one, prose-only cfg, unknown os) plus an admission case over the six real OS-specific sources so the guard cannot pass by rejecting everything. Non-vacuity proven by mutation: breaking the positive gate reds 4 cases, deleting the foreign-OS gate reds 1, counting prose as a gate reds 2. **Finding recorded, deliberately NOT fixed here** (see `20A-04-SUMMARY.md` §13.10): the PowerShell guard's *positive* gate matches whole file text, so `hard_process_containment_macos.rs` satisfied `cfg(windows)` from a **doc comment** at line 13 and was caught only by the negative gate — an ungated source whose prose mentions `cfg(windows)` would be admitted as a Windows target. The bash mirror is not prose-satisfiable (it filters to `#[cfg…]` attribute lines); the PowerShell one should adopt the same filter in the next candidate.
- [x] **REQ-native-r9**: Re-validate the macOS proof harness against real macOS (8/8 confirmed real + green). — **COMPLETE.** All eight targets ran on real macOS in the dispatched run and each resolved to a real, named test: `macos-retained-directory` → `live_integrity_macos::required_live_macos_retained_directory_confines_writes`; `macos-process-tree` → `hard_process_containment_macos::required_live_macos_process_tree_contains_descendants`; `macos-docker-reject-path-replacement` → `docker_smoke::docker_rejects_allow_hosts_policy`; `macos-docker-roundtrip-delete` → `docker_smoke::docker_runs_hello_world`; `macos-public-dispatch` → `wcore-swarm dispatch::tests::sandbox_exec_is_refused_before_descendant_escape_can_spawn`; `macos-docker-cancellation` → `docker_smoke::docker_returns_enforced_resource_limits`; `macos-docker-budget` → `workspace_authority::required_live_macos_docker_rejects_over_budget_result`; `macos-f20-lifecycle` → `transactional_delegated_mutation_test` 9/9. **Deviation from the brief's letter:** the brief named "the ephemeral Scaleway Apple-silicon runner". The actual host is `Seans-MacBook-Pro` (runner `f20-macos-ephemeral-1d053640`, Darwin arm64). Apple silicon and real macOS: yes. Scaleway, and genuinely ambient-secret-free: no — see the `f20-no-ambient-secrets` caveat in `20A-04-SUMMARY.md` §12.7/§13.
- [x] **REQ-native-r10**: Regenerate + commit `Cargo.lock`; reset tainted local tree to pristine `be84bd2`. — **COMPLETE.** `Cargo.lock` is committed in the sealed tree (`git ls-tree 9821ef76 Cargo.lock` → blob `60bcfb50d8f0ba6f8191cf733cd96fa60ee2018b`) and is `+9` lines against `be84bd2`; the brief's named additions resolve (`cap-std`, `fd-lock`, `sha2`, `tar` all present). Consistency proven by `cargo build --locked --workspace --all-features` exit 0 on Hetzner at this SHA (§12.6) — `--locked` fails closed on a stale lock. Caveat: the dispatched run itself does **not** pass `--locked`, so the run is not independent evidence for this requirement.
- [x] **REQ-native-r11**: Windows proof leg runs on an AppContainer-capable self-hosted msvc runner (not hosted `windows-2022`). — **COMPLETE.** Directly exercised. Job `89747993276` = "F20 native Windows candidate (self-hosted msvc)", `Runner name: 'ferrox-win-msvc'`, `Machine name: 'SEANDESKTOP'`, `Runner group name: 'Default'`. The hosted `windows-2022` "Windows soak" job in the same run was **skipped** (candidate mode). AppContainer capability is proven by the `is_available()` assertions under r1, not merely by the label.
- [ ] **REQ-native-r12**: Repaired candidate passes the full gate sequence: build → cross-audit → Hetzner aggregate → native proof (Win+mac) → fresh 20-16 (native NOT deferred) → 20-17 → Sean-authorized 20-18. — **OPEN (native leg closed, review/prep legs not).** CLOSED: build (Hetzner `--locked` exit 0), Hetzner aggregate (11520/11520), native proof Windows **and** macOS bound to one Sean-authorized run at one exact SHA. NOT CLOSED: there is no **fresh 20-16** at `9821ef76`. The only 20-16 on disk (`phases/20-transactional-delegated-mutation/20-16-SUMMARY.md`) is bound to SHA `6937ef61aa2ad2074dd7875f9cde2369fc104461` / tree `6db6fc85`, and its own key-decisions record `native_macos` and `native_windows` as "the only deferred checks" — precisely the deferral r12 forbids. `20-17-SUMMARY.md` is likewise bound to that older SHA; no 20-17 re-prep exists at `9821ef76`. Note: none of these missing clauses appear in Phase 20A's three Success Criteria, which are met — r12 is broader than the phase gate and stays open on its own terms.
- [ ] **REQ-native-r13** (audit addendum): every review gate (incl. the fresh 20-16) emits a schema-validated review artifact for EVERY claimed reviewer; no prose-only "reviews" count toward a PASS. (Closes the 20-08/20-16 attestation gap: 4 claimed reviews had no on-disk artifact.) — **OPEN.** No review gate ran against `9821ef76`, so no artifact was or could be emitted for this candidate. The pre-existing gap is also still measurably open: `20-16-SUMMARY.md` claims **two** reviewers (`wayland-f20-16-repair-review` and the adversarial confirmer `wayland-f20-16-adversarial-confirmer`), while its sole artifact `20-08-INDEPENDENT-REVIEW.md` (schema `wayland-core.phase20-independent-review.v1`) carries exactly **one** `reviewer_id`. Recording a PASS here is what r13 exists to forbid.
- [x] **REQ-native-r14** (audit addendum): re-prove on real hardware the wcore-sandbox **Windows COMPILE** fix (windows_impl module-path debt) and the **macOS proof-harness test-mapping** fix — the actual recorded 20-18 (2026-07-22) failures — not only the 07-23 AppContainer findings. — **COMPLETE.** Windows COMPILE half: `wcore-sandbox`'s `windows_impl` tree compiled and executed on real msvc hardware — targets 1/2 (`live_fs_acl`), 3/5 (`hard_process_containment_windows`) all ran real tests, which is only possible if the module paths resolve. macOS test-mapping half: `macos-retained-directory` now resolves to `live_integrity_macos::required_live_macos_retained_directory_confines_writes` (it previously pointed at the Windows-only retained-handle test) and PASSED 0.065s on real macOS in the dispatched run.
- [x] **REQ-native-r15** (audit addendum, hard gate): reset the working tree to pristine `be84bd2` (remove the boundary-breaking `process.rs` full-token-swap diagnostic) BEFORE any candidate build; write the real "drop deny-only SIDs" fix fresh under the plan, not salvaged from the diagnostic edit. — **COMPLETE.** Lineage: `be84bd2` is a verified ancestor of `0e8e6c1d` ("fix(sandbox): repair Windows AppContainer availability, child reads, msvc build", 2026-07-23), the single commit that lands r1 + r2 + r3 as a fresh 3-file change (`storage.rs +9`, `process.rs +19/-28`, `snapshot.rs +5/-2`) — a rewrite, not a salvage of the full-token-swap diagnostic. At the sealed SHA `process.rs` calls `CreateRestrictedToken(..., DISABLE_MAX_PRIVILEGE, 0, null, ...)` — `DisableSidCount=0`, `SidsToDisable=null` — with no token swap. Pristine state recorded on all four locations at the seal (`20A-04-SUMMARY.md` §12.1), and both candidate jobs re-asserted `HEAD == 9821ef76…` inside the run before any build step.

**Closeout tally (2026-07-26, revised at post-seal sweep):** **13 complete** — r1, **r2**, r3, r4, r5, r6, r7, **r8**, r9, r10, r11, r14, r15. **2 open** — r12 (no fresh 20-16 / 20-17 at this SHA), r13 (no review artifact for this candidate; prior 2-claimed/1-recorded gap still measurable). Phase 20A's three Success Criteria are met by run `30184651330`; the two remaining open requirements are acceptance detail the phase gate does not depend on, and neither is a failing test — each is an **unexercised** path, not a red one.

**Post-seal sweep (2026-07-26), what it cost and what it found.** r2 and r8 were closed on evidence produced after the seal, at a commit whose `crates/` tree is byte-identical to `9821ef76`. The sweep also surfaced two defects that only running the thing could reveal, both recorded in `20A-04-SUMMARY.md` §13.10:
1. **`scripts/wayland-e2e-windows-soak.ps1` could never report success.** At three sites (phases F, G, L) it captured `$exit = & { cargo … | Tee-Object …; $LASTEXITCODE }`. Tee-Object passes output through, so the block returns an array of every output line plus the exit code, and `if ($exit -ne 0)` is an array filter that is always truthy. Measured: job `89752739969` ran 12/12 `live_fs_acl` and 6/6 `hard_process_containment_windows` **all PASS** and PHASE L still reported failure. Fixed by reading `$LASTEXITCODE` after the pipeline; proven on `SEANDESKTOP` pwsh 7.6.3 — `BROKEN_FORM: type=Object[] count=3 verdict=REPORTS_FAILURE`, `FIXED_FORM: type=Int32 value=0 verdict=reports_success`, `FIXED_FORM_ON_REAL_FAILURE: value=3 verdict=REPORTS_FAILURE` (still fails closed on a real non-zero exit).
2. **These ACL tests cannot be observed over SSH.** A non-interactive session-0 SSH logon to `SEANDESKTOP` reports `AppContainerBackend::is_available() == false`, so every test in the file panics at its gate regardless of correctness. Established by control: the CI-certified-green `granted_path_is_readable_then_revoked` fails identically over SSH at the sealed SHA. Only the runner service is a valid environment — do not conclude a red from an SSH run.

### Phase 21 — Child Authority and Budget Inheritance

- [ ] **F21-01**: Every child receives the intersection of parent and requested provider, model, tool, filesystem, egress, secret, and approval authority.
- [ ] **F21-02**: Nested children cannot exceed parent depth, fan-out, concurrency, token, cost, or time reservations.
- [ ] **F21-03**: Approval, escalation, cancellation, reservation, refund, and result delivery remain attributable to the correct parent/session actor.
- [ ] **F21-04**: Hostile child tests prove no authority or resource amplification across standalone and host protocol paths.

**Adjudication at the close of plan 21-04** (`21-04-PHASE-VERDICT.md` §3, base
SHA `f2d186f6`). All four are left OPEN — an explicit incomplete disposition,
not an unfinished one. Three of the four are open on LIVE-evidence grounds
rather than on in-process failure; no requirement here is marked complete on
in-process evidence alone.

- **F21-01 — OPEN.** The tool dimension of the intersection is confirmed absent (`build_tool_registry` registers a requested tool without consulting the parent) and F21-02-01 is DECLINED and open at Sean's authorization; provider intersection has no request channel to intersect. Marking this complete would claim an intersection the product does not compute.
- **F21-02 — OPEN.** Depth, time, token and cost refuse at the in-process ancestor-rollup seam, but all four were NOT-EXPRESSIBLE on both live combinations because no shipped surface carries a child-fillable budget field. The property holds in part by absence of a request channel.
- **F21-03 — OPEN.** Five of six lifecycle events attribute correctly at the real seam on both platforms with zero misattributions, and result delivery is proved correct live on the shipped wire. Refund across a crash is UNPROVEN (F21-04-02) and four of six events have no per-child observable on the host protocol at all (F21-04-01).
- **F21-04 — OPEN.** The hostile corpora ran on both surfaces and both platforms and found no amplification on any dimension they could express, but tool authority stays confirmed-absent and DECLINED, and F21-04-03 shows two parallel `Spawn` siblings failing outright on the shipped binary.

### Phase 22 — Supervision, Durable Goals, Fleet, and Loops

- [ ] **F22-01**: CLI, TUI, and host clients can start, list, inspect, log, steer, pause, cancel, resume, retry, and acknowledge child results through versioned commands/events.
- [ ] **F22-02**: One durable Goal/Run kernel owns objective, completion contract, authority, budget, evidence, cursor, wait, progress, and terminal state.
- [ ] **F22-03**: Fleet tasks persist dependencies, attempts, claims, heartbeats, ownership, handoffs, completion, and parent wake-up without duplicate execution.
- [ ] **F22-04**: Direct, ForgeFlows, Fleet, Council, and Anvil execute as explicit strategies under exactly one outer loop owner.
- [ ] **F22-05**: Session-local fixed/dynamic, manual, and event-driven loops are bounded by expiry, iteration, no-progress, concurrency, and cumulative authority/resource limits; persistent scheduled routines remain an explicit non-claim until the Phase 24 runtime closes them.
- [ ] **F22-06**: The existing journal is proved compatible with Goal/Task/Wait records or receives one explicit versioned migration; accepted F12 behavior is not silently rewritten.
- [ ] **F22-07**: Reconnect, stale commands, duplicate acknowledgements, missed intervals, crash/restart, and user preemption preserve one canonical producer state across CLI/TUI/host-protocol paths, with canonical serialized producer fixtures ready for the later D2 consumer gate.

### Phase 23 — Governed Continuous Personal Agency

- [ ] **F23-01**: Generated skills follow detect, draft, quarantine, evaluate, review/policy, promote, observe, revoke, and rollback; unpromoted content cannot execute.
- [ ] **F23-02**: Users can inspect and control durable completion contracts, waits, session search, lineage, checkpoints, retry/regenerate, fork, rewind, redacted operational session/evidence export, retention, and unknown-effect reconciliation.
- [ ] **F23-03**: Memory and user modeling expose activation truth, recall provenance, correction, forgetting, privacy, retention, provider choice, and bounded proactive nudges.
- [ ] **F23-04**: Prompt-cache and compaction behavior have visible hit/invalidation reasons, token-pressure state, quality gates, and cost-regression thresholds.
- [ ] **F23-05**: A multi-day resume/wait/complete scenario proves cumulative authority, resource, memory, evidence, and delivery state survives restart without a second loop owner.
- [ ] **F23-06**: `wcore-repomap` becomes a persistent incremental hybrid repository index with content-hash add/change/delete/rename/worktree invalidation, Git-respecting scope, BM25/FTS plus symbols and optional semantic/RRF retrieval, exact-search fallback, provenance/staleness, secret/authority isolation, and warm-start/size/latency/retrieval-quality gates.

### Phase 24 — Gateway, Automation, Channels, and Typed API

- [ ] **F24-01**: One persistent gateway runtime provides install/start/stop/restart/status/doctor/logs/drain, profile isolation, active-turn visibility, graceful recovery, and native service management.
- [ ] **F24-02**: The automation plane supports one-shot, interval, cron, natural-language authoring, commitments/heartbeat, hooks/webhooks/polling, history, retry, continuation, and bounded delivery.
- [ ] **F24-03**: The channel framework proves setup/auth probes, pairing/access, thread/group binding, profile/agent routing, media normalization, edit/delete/reaction, idempotent outbound delivery, reconnect/reload, and health.
- [ ] **F24-04**: A typed API/App SDK provides authenticated roles, idempotent commands, ordered/gap-aware events, compatibility negotiation, remote clients, logs, health, and a redacted support bundle.
- [ ] **F24-05**: Setup-to-recovery and kill/reconnect/delivery journeys pass on native macOS, Linux, and Windows without lost or duplicate work.

### Phase 25 — Remote Reach, Nodes, and Plugin Lifecycle

- [ ] **F25-01**: A provider-neutral execution-backend contract covers capabilities, policy, secrets, artifacts, limits, cancellation, attestation, receipts, and lifecycle health.
- [ ] **F25-02**: The same deterministic task runs locally, in a container, over SSH, and on one hibernating cloud reference backend with identical authority and cleanup semantics.
- [ ] **F25-03**: A node/device contract supports pairing, capability advertisement, revocation, mixed-version behavior, offline recovery, and artifact/receipt attribution.
- [ ] **F25-04**: Plugin authors can scaffold, test, sign, install, inspect, approve, execute, update, rollback, remove, publish, recover, and verify compatibility through one governed lifecycle.
- [ ] **F25-05**: Cancellation, secret/egress denial, key rotation, compromised plugin/backend, and orphan scans fail closed across every reference backend.

### Phase 26 — Migration, Export, Backup, and Restore

- [ ] **F26-01**: Hermes and OpenClaw discovery produces a typed dry-run plan without changing state or exposing secret values.
- [ ] **F26-02**: Persona, memory, skills, settings, assets, profiles, credentials, and provenance migrate selectively with conflicts and executable content quarantined.
- [ ] **F26-03**: Users can consume the F23 redacted session/evidence envelope to export a portable profile/session corpus and perform authenticated backup, restore, and reciprocal migration without executing imported content.
- [ ] **F26-04**: Secret sources are explicitly remapped; rollback restores the exact pre-operation state after interruption or partial failure.
- [ ] **F26-05**: Fixture installations and hostile import/export/restore corpora prove isolation, portability, and deterministic reporting.

### Phase 27 — Multimodal, Browser, Generation, and Voice Contracts

- [ ] **F27-01**: Standalone and host paths share one bounded, open-once, magic-byte-validated attachment/document intake pipeline with explicit provider degradation.
- [ ] **F27-02**: Browser, CUA, and web-search capabilities publish live activation/readiness truth and preserve sandbox, egress, approval, and cleanup policy.
- [ ] **F27-03**: Built-in, MCP-only, late-MCP, and combined image/media generation expose consistent ToolSearch, readiness, credentials, accounting, and failure semantics.
- [ ] **F27-04**: Speech and realtime/voice capability contracts define streaming, interruption, cancellation, provider compatibility, resource accounting, and protocol behavior.
- [ ] **F27-05**: Deterministic image/PDF/docx/xlsx/pptx/browser/media/voice corpora and focused packaged smokes pass on native macOS, Linux, and Windows.

### Phase 28 — Native Cross-Platform Certification

- [ ] **F28-01**: Native macOS, Linux, and Windows E5 matrices cover sandbox probes, Unicode, long paths, UNC/reparse/symlink cases, process cleanup, suspend/resume, offline, disk-full/read-only, and hostile inputs.
- [ ] **F28-02**: A 1,000-session and concurrent-child soak completes with secret canaries intact, no orphan processes, and bounded quality/performance deltas.
- [ ] **F28-03**: Signed receipts bind the exact candidate, platform, posture, fixture corpus, environment, artifacts, logs, and skipped-case policy.
- [ ] **F28-04**: No critical case is skipped and every finding at every severity is resolved before certification acceptance.

### Phase 29 — Supply Chain and Release Integrity

- [ ] **F29-01**: Toolchain and dependencies are locked with vulnerability/license policy, SBOM, provenance, artifact signing, and reproducibility or documented deterministic variance.
- [ ] **F29-02**: Installers and updates verify signed manifests, source/artifact identity, rollback/freeze protection, revocation, key rotation, and trust roots for plugins/backends.
- [ ] **F29-03**: Tampered binaries, SBOMs, updates, plugins, backend receipts, manifests, or keys fail closed in clean-room and rotation/compromise drills.
- [ ] **F29-04**: Packaging, deployment preparation, rollback rehearsal, and release acceptance remain distinct evidence states and separate authorization gates.

### Phase 30 — Continuous Scorecard and Frontier Review

- [ ] **F30-01**: Independently review the versioned capability ledger established before Phase 21, including source/configured/constructed/reached/effective/operator-complete/packaged state, security owner, evidence IDs, and pinned Hermes/OpenClaw deltas.
- [ ] **F30-02**: The scorecard is refreshed at every admitted phase; F30 independently reviews rather than first discovers peer gaps.
- [ ] **F30-03**: Wayland, Hermes, and OpenClaw run common correctness, recovery, security, cost, and cognitive-tax trials with pinned baselines, repeated trials, and confidence bounds.
- [ ] **F30-04**: Claims allowed, claims prohibited, limitations, and raw redacted evidence are published without unsupported superiority language.
- [ ] **F30-05**: Sean explicitly approves any source push, frontier positioning, main merge, issue closure, release, or deployment.

## v2 Requirements

### Deferred Breadth

- **DEFR-01**: Add additional cloud execution vendors after the F25 reference contract is accepted.
- **DEFR-02**: Implement Desktop presentation and companion-app surfaces under the linked Desktop plan.
- **DEFR-03**: Broaden channel adapters only after the F24 framework and reference delivery matrix are accepted.

## Out of Scope

| Feature | Reason |
|---------|--------|
| Clean-slate Core rewrite | Discards accepted evidence and increases integration entropy |
| Parallel goal/task/loop state machine | F22A-F22D are canonical |
| Local-only or type-only completion claims | Packaged and native evidence is required |
| Automatic source push, main merge, release, deploy, or issue closure | Sean-only authorization gate |
| Unbounded provider/backend/channel breadth | Prove the contract with bounded reference implementations first |

## Traceability

| Requirement | Phase | Status |
|-------------|-------|--------|
| F20-01, F20-02, F20-03, F20-04, F20-05, F20-06, F20-GATE-01, F20-GATE-02 | Phase 20 | Complete @ `01a5b0ae` |
| REQ-native-r1, r3, r4, r5, r6, r7, r9, r10, r11, r14, r15 | Phase 20A | Complete @ `9821ef76` (run `30184651330`) |
| REQ-native-r2, r8 | Phase 20A | Complete @ `2cc1a285` — post-seal sweep 2026-07-26. `crates/` tree byte-identical to `9821ef76`. r2: both deny clauses PASS on `SEANDESKTOP` (run `30186873948`, job `89753061944`). r8: both production guards driven to rejection with the specific error, plus 7 regression cases. |
| REQ-native-r12, r13 | Phase 20A | **Open** — unexercised acceptance clauses, not failing tests; see per-requirement notes above. Phase 20A closed COMPLETE on its three Success Criteria without them. |
| F21-01, F21-02, F21-03, F21-04 | Phase 21 | **All four Open** — adjudicated at `21-04-PHASE-VERDICT.md` §3, base `f2d186f6`. Criterion 1 NOT MET (tool authority confirmed absent and DECLINED); Criteria 2 and 3 MET WITH STATED EXCEPTIONS. Six HIGH findings open: F21-02-01, F21-02-03, F21-02-02's live closure, F21-04-01, F21-04-02, F21-04-03. No seal claimed. |
| F22-01, F22-02, F22-03, F22-04, F22-05, F22-06, F22-07 | Phase 22 | Pending |
| F23-01, F23-02, F23-03, F23-04, F23-05, F23-06 | Phase 23 | Pending |
| F24-01, F24-02, F24-03, F24-04, F24-05 | Phase 24 | Pending |
| F25-01, F25-02 | Phase 25 | **Both Open** — plan 25-01 executed and terminated in state 2 (complete with a bounded cloud gap), base `frontier/p25-remote-reach`. The provider-neutral contract exists, is oracle-conformant and is proven live across THREE reference backends on `hetzner-dsm` with a normalized diff of EQUIVALENT. **Unmet clause, F25-01:** "lifecycle health" and "attestation" are covered, but the contract is proven on three of the four surfaces it must abstract, so provider-neutrality across the cloud transport is asserted rather than demonstrated. **Unmet clause, F25-02:** "and on one hibernating cloud reference backend" — the cloud leg is UNEXERCISED, no Fly credential exists on any proof host, and the backend fails closed rather than falling back. Second unmet clause, F25-02: "identical authority and cleanup semantics" is proven local↔container↔ssh only. See `phases/25-.../25-01-SUMMARY.md`. |
| F25-03 | Phase 25 | **Open — NOT STARTED.** Plan 25-02 was not executed. Unmet clauses: all of them — pairing, capability advertisement, revocation, mixed-version behavior, offline recovery, artifact/receipt attribution. Nothing was built and nothing is claimed. |
| F25-04 | Phase 25 | **Open — NOT STARTED.** Plan 25-02 (the twelve-verb plugin lifecycle) was not executed. Unmet clauses: scaffold, test, sign, inspect, approve, update, rollback, recover, publish and verify-compatibility are all absent from `wayland-core plugin`, which today offers install, list, available, remove and three marketplace verbs. Install and remove alone exist. |
| F25-05 | Phase 25 | **Open — partially evidenced, not met.** Plan 25-04 was not executed. What IS proven, from 25-01's live run on `hetzner-dsm`: cancellation fails closed and leaves no orphan across local, container and ssh, verified against the real process table, the real container listing and the real remote process table; and an absent cloud credential fails closed with an explicit unavailable verdict and never falls back. Unmet clauses: secret/egress denial, key rotation, compromised plugin/backend, and "across every reference backend" — the cloud backend is unexercised and there is no hostile matrix. |
| F26-01, F26-02, F26-03, F26-04, F26-05 | Phase 26 | Pending |
| F27-01, F27-02, F27-03, F27-04, F27-05 | Phase 27 | Pending |
| F28-01, F28-02, F28-03, F28-04 | Phase 28 | Pending |
| F29-01, F29-02, F29-03, F29-04 | Phase 29 | Pending |
| F30-01, F30-02, F30-03, F30-04, F30-05 | Phase 30 | Pending |

**Coverage:**
- v1 phase requirements: 58 total
- Mapped to phases: 58
- Unmapped: 0
- Duplicate mappings: 0
- Program controls: 4 total (1 established, 3 open) — **CTRL-01 established 2026-07-26** (`intel/COMPETITIVE-LEDGER.md`, commit `d06a6051`). CTRL-03 (`intel/FIELD-REGRESSIONS.md`) remains intentionally cross-cutting/continuous (not single-phase-mapped); CTRL-02/D1 gates broad Phase 21 and CTRL-04/D2 gates Phase 23 exit (`intel/DESKTOP-PROTOCOL-CHECKPOINT.md`).
- Grand total distinct requirement IDs: 77 (58 F-phase + 4 CTRL + 15 REQ-native).
- Additive native-UAT repair requirements: 15 (REQ-native-r1 … r12 plus audit addenda r13–r15) — sub-clauses refining F20-05 / Phase-20 Success Criterion #3, owned by Phase 20A; NOT counted in the 58 (they are acceptance detail, not new scope). Status at Phase 20A close: **11 complete, 4 open** (r2, r8, r12, r13); after the 2026-07-26 post-seal sweep: **13 complete, 2 open** (r12, r13). Full text: `.planning/intel/inbox-2026-07-23/requirements.md`; addenda in `AUDIT-2026-07-23.md`.

---
*Requirements defined: 2026-07-18*
*Last updated: 2026-07-26 — post-seal sweep: r2 and r8 closed on hardware evidence at a `crates/`-identical commit (`2cc1a285`); 13 complete, 2 left open (r12, r13) with the specific unmet clause named. Two harness defects found by running it — a soak phase that could never report success, and AppContainer being unavailable over SSH — recorded in `20A-04-SUMMARY.md` §13.10*
