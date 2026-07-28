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
2. ~~**These ACL tests cannot be observed over SSH.**~~ **REFUTED 2026-07-27 — do not rely on this.** The original claim was that a non-interactive session-0 SSH logon to `SEANDESKTOP` reports `AppContainerBackend::is_available() == false` regardless of correctness, so only the runner service is a valid environment and no red from an SSH run may be believed. **It is false.** `live_fs_acl` runs **12/12 PASS** over session-0 SSH against a *clean* lease directory — including `granted_path_is_readable_then_revoked`, the very test cited as establishing the rule. The original control never had discriminating power: it varied the *logon* while the lease directory was wedged, and both hypotheses predict that result. The real cause was `wcore-sandbox` acceptance tests writing leases into the **production** lease directory (`sha256(b"storage-test-sid")` matched the wedging files byte-for-byte); Windows `is_available()` is a genuine spawn probe, so a foreign lease makes it fail and the product logs "sandbox disabled" and keeps running **unsandboxed**. Fixed structurally. See `.planning/intel/APPCONTAINER-SSH-LEASE-WEDGE.md` and `.planning/intel/APPCONTAINER-SSH-LORE-READJUDICATION.md`. **A red from an SSH run is now evidence and must be investigated, not discounted.**

### Phase 21 — Child Authority and Budget Inheritance

- [x] **F21-01**: Every child receives the intersection of parent and requested provider, model, tool, filesystem, egress, secret, and approval authority.
- [ ] **F21-02**: Nested children cannot exceed parent depth, fan-out, concurrency, token, cost, or time reservations.
- [ ] **F21-03**: Approval, escalation, cancellation, reservation, refund, and result delivery remain attributable to the correct parent/session actor.
- [ ] **F21-04**: Hostile child tests prove no authority or resource amplification across standalone and host protocol paths.

**Adjudication at the close of plan 21-04** (`21-04-PHASE-VERDICT.md` §3, base
SHA `f2d186f6`) left all four OPEN. **Superseded on 2026-07-27 by
`21-REVERIFICATION.md` at `ac94b1d5`**, a third grading against the repaired
product (F21-02-01 fixed at `6b0083b0`/`3e23b83d`, F21-02-03 reconciled and
merged at `9c3e3687`, F21-04-03 fixed at `1eb9b5ca`, F21-04-02 disproved at
`e879206e`). Linux evidence only; no Windows run was produced.

- **F21-01 — COMPLETE (2026-07-27).** The sentence that kept it open — *"marking this complete would claim an intersection the product does not compute"* — is no longer true. `build_tool_registry` intersects unconditionally against a shared narrow-only `ParentToolAuthority` declared at all six production spawner sites, a dispatch-time `PolicyGate` is installed from the same snapshot, and a source-derived enumeration guard fails if a seventh site appears. Proven LIVE on the shipped binary: `f21_02_01_delegated_child_cannot_obtain_a_tool_the_parent_lacks` plus its differential CONTROL, both green at `ac94b1d5`. Filesystem, egress and secret REFUSED live on both surfaces with real child provider turns; provider, model and approval are NO-CHANNEL with red-able canaries.
- **F21-02 — OPEN.** Depth and fan-out refuse in-process with real numbers, and time/token/cost refuse at the ancestor-rollup seam — but NOT ONE of the six has live evidence, and four of them cannot obtain any: `engine.rs:6173` is still `begin_active_turn(turn_id, None)`, the only `sub_budget(Some(..))` call site is inside `#[cfg(test)]`, and `SubAgentConfig` carries no budget field. The property is still MERELY UNREQUESTABLE, not enforced-and-driven. Never complete on in-process evidence alone.
- **F21-03 — FENCED.** All six lifecycle events now attribute CORRECTLY at the real in-process seam with zero misattributions; refund across a real crash and restart moved NOT-OBSERVABLE → CORRECT; result delivery is CORRECT on the shipped wire; approval and cancellation are CORRECT on the live rendered TUI screen. Its sole remaining blocker is F21-04-01 — no per-child observable on the host protocol — specified in `.planning/SEAM-REQUESTS/F21-04-01.md` as a coordinated Core/Desktop release, not a defect in Core product code.
- **F21-04 — OPEN.** Tool authority is now present and live-proven, F21-04-03 is repaired (6 of 6 live two-sibling runs clean at HEAD), and F21-04-02 is disproved with executable counter-evidence. But the requirement demands proof across BOTH paths and Success Criterion 3 is still NOT MET: tool and fan-out have no host-protocol expression, fan-out is undetermined live on both surfaces, and Windows is unmeasured at this SHA.

### Phase 22 — Supervision, Durable Goals, Fleet, and Loops

- [ ] **F22-01**: CLI, TUI, and host clients can start, list, inspect, log, steer, pause, cancel, resume, retry, and acknowledge child results through versioned commands/events.
- [ ] **F22-02**: One durable Goal/Run kernel owns objective, completion contract, authority, budget, evidence, cursor, wait, progress, and terminal state.
- [ ] **F22-03**: Fleet tasks persist dependencies, attempts, claims, heartbeats, ownership, handoffs, completion, and parent wake-up without duplicate execution.
- [ ] **F22-04**: Direct, ForgeFlows, Fleet, Council, and Anvil execute as explicit strategies under exactly one outer loop owner.
- [ ] **F22-05**: Session-local fixed/dynamic, manual, and event-driven loops are bounded by expiry, iteration, no-progress, concurrency, and cumulative authority/resource limits; persistent scheduled routines remain an explicit non-claim until the Phase 24 runtime closes them.
- [ ] **F22-06**: The existing journal is proved compatible with Goal/Task/Wait records or receives one explicit versioned migration; accepted F12 behavior is not silently rewritten.
- [ ] **F22-07**: Reconnect, stale commands, duplicate acknowledgements, missed intervals, crash/restart, and user preemption preserve one canonical producer state across CLI/TUI/host-protocol paths, with canonical serialized producer fixtures ready for the later D2 consumer gate.

**Execution disposition, 2026-07-26 — every F22 requirement is OPEN, with the unmet clause named.**

| Req | Disposition | Unmet clause |
|---|---|---|
| F22-01 | OPEN, untouched | No versioned command or event was added for start/list/inspect/log/steer/pause/cancel/resume/retry/acknowledge. Plan 22-04 was not executed. |
| F22-02 | OPEN, partially built | The Goal *vocabulary* and the single terminal taxonomy exist (`crates/wcore-types/src/goal.rs`, 7 tests green on Linux). The **durable kernel does not**: no `SessionEvent` variants, no reducer arm, no `ReducedSessionState` field, no cursor exposure, no sole-writer. The requirement's verbs — *owns* objective, authority, budget, evidence, cursor, wait, progress — are all unmet. |
| F22-03 | OPEN, untouched | No task ledger, no claim, no heartbeat, no ownership handoff. Plan 22-03 was not executed. |
| F22-04 | OPEN, measured only | The five-strategy census is delivered (`22-02-LOOP-OWNER-CENSUS.md`) and it corrected the plan's own central assumption. No adapter surface, no loop-owner claim, no nesting refusal was built; the five engines still terminate through five vocabularies. |
| F22-05 | OPEN, vocabulary only | `LoopPolicy` exists as a type with a structural bound on its dynamic form. Nothing enforces expiry, iteration, no-progress, concurrency or cumulative limits at runtime. Plan 22-04 Task 4 was not executed. |
| F22-06 | OPEN, evidence gathered | The verdict `COMPATIBLE-AT-V5` is measured cross-binary and single-variable on Linux and authorized 4-of-4 by panel (`22-01-JOURNAL-COMPAT.md`). It is NOT complete because (a) the Windows halves of M1–M5 were never taken — the reduce instrument did not finish building on a contended box — and (b) no stored-corpus regression test pins the reduction, so nothing keeps F12 honest going forward. |
| F22-07 | OPEN, untouched | No reconnect, stale-command, duplicate-ack or preemption behavior; no D2 producer fixtures. Plan 22-04 was not executed. |

### Phase 23 — Governed Continuous Personal Agency

- [ ] **F23-01**: Generated skills follow detect, draft, quarantine, evaluate, review/policy, promote, observe, revoke, and rollback; unpromoted content cannot execute.
  - **Phase 23A disposition (2026-07-26): INCOMPLETE — explicitly not marked complete.**
    Met: `detect`, `draft`, `quarantine` (sixteen routes enumerated with citations, all
    gated — `23A-01-SURFACE-CENSUS.md`), and `unpromoted content cannot execute`, observed
    at the product surface.
    Partially met: `observe` — `/skill list` tags a draft `(hidden)` and `/skill show`
    reports `visibility: hidden from model` without disclosing the body, but there is no
    governance provenance and no append-only history.
    **Unmet clauses, named:** `evaluate`, `review/policy`, `promote`, `revoke`, `rollback`.
    `run_skills_promote` (`crates/wcore-cli/src/main.rs:2408`) still fails closed, so
    "cannot execute before governed promotion" currently holds because no promotion path
    exists at all — a vacuous satisfaction, recorded as such.
    **Open HIGH blocking the observe clause in practice:** F23A-01-H2 — any errored tool
    call kills the session, so the quarantine refusal is real but not survivable
    (`23A-01-LIVE-EVIDENCE.md` §L4).
- [ ] **F23-02**: Users can inspect and control durable completion contracts, waits, session search, lineage, checkpoints, retry/regenerate, fork, rewind, redacted operational session/evidence export, retention, and unknown-effect reconciliation.
- [ ] **F23-03**: Memory and user modeling expose activation truth, recall provenance, correction, forgetting, privacy, retention, provider choice, and bounded proactive nudges.
- [ ] **F23-04**: Prompt-cache and compaction behavior have visible hit/invalidation reasons, token-pressure state, quality gates, and cost-regression thresholds.
- [ ] **F23-05**: A multi-day resume/wait/complete scenario proves cumulative authority, resource, memory, evidence, and delivery state survives restart without a second loop owner.
- [ ] **F23-06**: `wcore-repomap` becomes a persistent incremental hybrid repository index with content-hash add/change/delete/rename/worktree invalidation, Git-respecting scope, BM25/FTS plus symbols and optional semantic/RRF retrieval, exact-search fallback, provenance/staleness, secret/authority isolation, and warm-start/size/latency/retrieval-quality gates.

**Phase 23B disposition — REVISED 2026-07-28.** All four 23B plans have now executed.
**No F23 requirement is marked complete and the phase is not closed.** The whole-phase
grade is `23B-PHASE-DISPOSITION-v2.md` (2026-07-27), superseded on F23-05 and F23-06 by
the two lanes that landed 2026-07-28; the per-requirement dispositions below are taken
from each plan's own SUMMARY.

- **F23-02 — INCOMPLETE, substantial delivery.** `23B-01-SUMMARY.md`,
  `status: complete-with-named-open-verbs`. `wayland-core session` ships every listed
  verb plus `cancel`, driven against the shipped binary on Linux with captured per-verb
  evidence, and re-proved on current code at lane HEAD with a caller-generated nonce
  (`F23_01_DRIVE=PASS platform=linux nonce=c3ebab28a4160e31`, driver exit 0) against a
  binary whose `--build-info` reports the commit under test. The `reconcile`/`cancel`
  pair closes live Windows UAT defect D2 end to end, including
  `F23_01_D2_RESOLVED_PERSISTS_ACROSS_RESTART=true`. Unmet clauses: macOS not driven,
  Windows not driven, TUI verbs not added or driven, `retry` live-proved only on its
  refusal path. Evidence: `23B-01-LIVE-EVIDENCE.md`.
- **F23-03 — INCOMPLETE, executed.** Plan 23B-02 ran (`23B-02-SUMMARY.md`,
  `status: complete-with-named-open-controls`, `requirements_disposition: F23-03:
  incomplete`). Recall provenance exists and is emitted by the fusion that produced the
  ranking; correction, forgetting, privacy scoping and retention bounding run through the
  unmodified access gate, are audited, and are reachable from `/memory` on the shipped
  surface; a forget reaches the CDC changelog; exclusions are reported rather than silent.
  **Not met, on the plan's own acceptance mechanism:** F23-03 demands that forgetting be
  proved by absence from the ACTUAL OUTBOUND PROVIDER REQUEST BODY, and what exists is a
  proof that the row is deleted and gone from retrieval — the exact shape the plan named
  as the engineered green to avoid. Nothing was driven live; no TUI leg on any platform;
  user-model correction precedence not implemented.
- **F23-04 — INCOMPLETE, not started.** 23B-02 Task 2 was never begun
  (`23B-02-SUMMARY.md`, `requirements_disposition: F23-04: not-started`).
  `cache_diagnostics.rs` still emits telemetry only.
- **F23-05 — INCOMPLETE, IN PROGRESS on a real calendar clock.**
  Plan 23B-04 executed (`23B-04-SUMMARY.md`,
  `status: partial-clock-started-two-platforms`, `requirements_disposition: F23-05:
  in-progress`). Task 1 (the clock policy) is COMPLETE — measured first on
  `hetzner-dsm` against the release binary at the SHA under test, nonce-bound, exit 0,
  with a discriminating control pair (identical durable state, 45s real gap →
  `exceeded=true reason=max_wall_time`; no gap → `exceeded=false`) — then cross-audited.
  Task 2 is STARTED and running: **Linux day one `2026-07-27T14:21:19Z` (`hetzner-dsm`),
  Windows day one `2026-07-27T23:54:26Z` (`SEANDESKTOP`), macOS NOT ACHIEVED — nothing
  run, nothing claimed.** Journey SHA pinned `0ed05322`, asserted through each binary's
  own `--build-info` on every invocation. Task 3 deliberately unstarted.
  **The journey cannot be closed before `2026-07-30T23:54:26Z`.** The plan reports
  none of its four termination states rather than inventing a fifth.
- **F23-06 — COMPLETE ON ITS MANDATORY CLAUSES, optional semantic layer deferred; still
  not marked complete here.** Plan 23B-03 executed (`23B-03-SUMMARY.md`,
  `status: complete-semantic-layer-deferred`, `requirements_disposition: F23-06:
  complete-with-semantic-layer-deferred`, termination state 2). `wcore-repomap` now holds
  a persistent, incrementally-maintained SQLite index — content-hash invalidation across
  add, change, delete, rename and branch switch; Git-respecting scope and worktree
  identity; bounded BM25-plus-symbol retrieval fused by reciprocal rank with an exact
  search fallback; provenance and a staleness verdict on every hit; secret isolation
  proved against the store's own bytes — reachable as
  `wayland-core index build|status|search|verify`. **All three mandatory platform legs
  PASS on real hardware**, each with a caller-generated run-time nonce and each against a
  binary whose `--build-info` source SHA was asserted equal to the commit under test
  before any measurement. **Every mandatory clause of F23-06 is closed.** The only
  deferral is the layer F23-06 itself marks OPTIONAL, with the non-claim recorded.
  Evidence: `23B-03-LIVE-EVIDENCE.md`. Marking the requirement box complete is left to
  the phase-closing plan, consistent with the rest of this file.

**Phase HIGH 23B-H1 — CLOSED, both halves.** The write-path fix is recorded in
`23B-H1-DISPOSITION.md`; the residual it explicitly declined to close — journals ALREADY
on disk carrying an explicit `"effect_receipt":null` failing their checksum on read, i.e.
silent permanent user data loss — is now recovered with content intact **on Linux, macOS
and Windows against real binaries, without loosening the integrity check**
(`23B-H1-RECOVERY-SUMMARY.md`, `status: complete`). Registered as a field regression:
`intel/FIELD-REGRESSIONS.md` FIELD-JOURNAL-002.

**Escalation still open (unchanged).** All four 23B plans specify that the macOS leg
builds its own binary on the Mac via `scripts/f23-macos-binary.sh`; the controlling
execution instruction forbids running Cargo on the Mac. Every macOS row in this phase
remains OPEN and that script still does not exist. Note that
`.planning/intel/MACOS-BINARY-IS-OBTAINABLE.md` records a method that does not require
it, and Phase 28-02 refuted "no macOS binary is obtainable" by executing a downloaded CI
`aarch64-apple-darwin` artifact.

### Phase 24 — Gateway, Automation, Channels, and Typed API

**Execution disposition — REVISED 2026-07-28.** The 2026-07-26 disposition recorded
that only 24-01 had executed and that 24-02, 24-03 and 24-04 were not started.
**That is out of date. All four plans have now executed, plus two unplanned
remediation lanes 24-B and 24-C, and all six SUMMARY files declare
`status: partial`.** The 2026-07-26 phase report
(`24-PHASE-REPORT.md`) is superseded for plan status and for the Criterion 1, 3
and 4 grades; it remains the record of the Windows `cron.rs` creation-flags HIGH
and of the panel-methodology findings.

**The phase is NOT closed and no requirement below is marked complete**
(`24-04-SUMMARY.md` §8). One thing must not be misread: **`24-04-SUMMARY.md`
exists to record that 24-04's own four tasks — journey driver, Windows
interface-evidence decision, three platform journeys, acceptance panel — were
NEVER STARTED.** That lane was directed at Criterion 4's wiring gap instead. A
summary is not a completion.

**No macOS and no Windows evidence exists anywhere in this phase** (24-C
§5.3 records this as a budget outcome, not an impossibility, and names the
method at `.planning/intel/MACOS-BINARY-IS-OBTAINABLE.md`).

Evidence: `24-01-SUMMARY.md`, `24-02-SUMMARY.md`, `24-03-SUMMARY.md`,
`24-04-SUMMARY.md`, `24-B-SUMMARY.md`, `24-C-SUMMARY.md`, and the contracts
`24-01-GATEWAY-CONTRACT.md`, `24-02-AUTOMATION-CONTRACT.md`,
`24-03-SURFACE-CONTRACT.md`, `24-B-GATEWAY-SURFACE.md`,
`24-C-ARRIVAL-CONTRACT.md`.

- [ ] **F24-01**: One persistent gateway runtime provides install/start/stop/restart/status/doctor/logs/drain, profile isolation, active-turn visibility, graceful recovery, and native service management. — **INCOMPLETE, and substantially further on than the 2026-07-26 entry recorded.** From 24-01: a persistent runtime as the mid-layer `wcore-gateway` crate with a lifecycle state machine whose every illegal transition is refused by name; a Windows-safe single-instance lock answering all four documented Windows defect classes; active-turn and pending-delivery counts in a machine-readable status projection; ordered drain distinguishing clean from forced; a native service abstraction with one implementation per OS family; and one HIGH found by hardware measurement and fixed (`crates/wcore-cli/src/cron.rs`'s non-Unix spawn branch set no creation flags — 1 of 600 heartbeats before, 600 of 600 after). **Three of the four clauses the 2026-07-26 entry named unmet are now closed by lane 24-B** (`24-B-SUMMARY.md`): (a) **the shipped binary now has the verbs** — `crates/wcore-cli/src/gateway.rs` exists and provides `gateway install|uninstall|start|stop|restart|status|drain|run`; (b) **graceful recovery is proven live on Linux** — install, start, status, hard kill, **platform-driven** recovery, drain, uninstall, against a real `systemctl --user` service; (c) **native service management has reached a real registry** on Linux. **Four HIGH were found by running it and all four fixed.** STILL UNMET: `doctor` and `logs` are a recorded gap asserted by test (the 4/4 panel chose journey-minimal); **profile isolation is still asserted structurally, not exercised** — no per-profile child has been supervised through the gateway; and **no macOS or Windows lifecycle run exists**. Windows residual from 24-01: the `cron.rs` `creation_flags` call site is still unbuilt on Windows.
- [ ] **F24-02**: The automation plane supports one-shot, interval, cron, natural-language authoring, commitments/heartbeat, hooks/webhooks/polling, history, retry, continuation, and bounded delivery. — **INCOMPLETE, executed. Plan 24-02 ran** (`24-02-SUMMARY.md`, `status: partial`, **termination state 2 of 3 — "Complete with a named gap"**; state 1 explicitly not claimable). Delivered: schedule ownership is a **held lease** rather than an assumption (`crates/wcore-cron/src/lease.rs`, an `flock`/`LockFileEx` exclusive lock on a one-byte sentinel, taken at all three call sites — the gateway automation plane, the session-boot runner in `bootstrap.rs`, and `cron daemon`; leasing only the session side would have left the race exactly where it was). **`flock` and not `fcntl` is load-bearing** — fcntl record locks are process-owned, so the single-owner test could never have gone red. Trigger vocabulary grew from one type to seven with a bound on each; retry and history became enforced rather than documented; all of it is reachable from the shipped binary. **UNMET: the plan's own live criterion is closed on Linux only, and not at all on macOS** — the plan requires live kill-and-continue evidence on both. The persistent scheduling Phase 22 Criterion 4 deferred to Phase 24 is addressed here but not proved on the required platform set.
- [ ] **F24-03**: The channel framework proves setup/auth probes, pairing/access, thread/group binding, profile/agent routing, media normalization, edit/delete/reaction, idempotent outbound delivery, reconnect/reload, and health. — **INCOMPLETE, executed. Plan 24-03 ran** (`24-03-SUMMARY.md`, `status: partial`). Delivered as tested, mutation-proved modules: `wcore_channels::{probe,binding,media,health}`, `manager::{reload,probe_all,health,edit_on,delete_on,take_registered}`, and `ChannelError::Unsupported` distinct from `Rejected`; the operator surface `wayland-core channel list|probe|health|reload` is on the shipped binary and was live-exercised. Design decisions worth keeping: `Channel::probe` defaults to a **named `Unsupported`, never `Ok`** (a default of Ok is an adapter attesting to a configuration it never read); `channel health` REFUSES when no gateway is running rather than printing an empty list; `reload` treats an unfingerprintable adapter as CHANGED, because the other direction keeps a rotated credential in service. Idempotent outbound delivery was closed by lane 24-C. **Phase verdict: PARTIALLY MET on Linux, NOT MET on macOS or Windows.** Not met: the end-to-end inbound matrix from the binary against a fixture (admit → dedupe → access → bind → route), and any evidence at all on the other two platforms. Two HIGH found live and fixed (F24-D-H1 the channel subsystem silently disabled on any host with no LLM provider key; F24-D-H2 `channel health` reporting a failed registration as an empty installation).
- [ ] **F24-04**: A typed API/App SDK provides authenticated roles, idempotent commands, ordered/gap-aware events, compatibility negotiation, remote clients, logs, health, and a redacted support bundle. — **INCOMPLETE, but its Success Criterion is MET on Linux.** 24-03 built `wcore_acp::{roles,idempotency,cursor,negotiate}` and `wcore_gateway::support_bundle` (structural elision first, exact-secret scrubbing as backstop, canary-proved live) and then graded itself honestly: *"a criterion that says clients recover event gaps is not met by a module that could let them"* — the cursor was not on the server's request path. **Lane 24e overturned that** (`24-04-SUMMARY.md`): `RolePolicy` and `HttpHandler::authorize_method` put the server in the decision path before dispatch, `GET /sessions/:id/events` is the resume transport with three refusals carrying distinct statuses, `message/send` tees events into a per-session log from a drain task, and stream ids carry a per-process-run uuid so a pre-restart cursor is refused **by name** rather than served another stream's positions. **A typed client that severed its connection having received ZERO bytes recovered, twelve seconds later, an event the server produced entirely after it had gone — over a real socket, against the real server, and against the shipped binary (`wayland-core acp serve --role viewer|operator|admin`), duplicates and losses both zero.** 13 tests, 10 mutations each reddening its named test, plus a live transcript. **LIMITS, named:** recovery and idempotency are proved on the **HTTP/SSE transport only** — REST `/v1` is role-gated but has no resume route and no idempotency handling, and stdio/WebSocket have none of the three — and **everything is Linux only**. The requirement is not marked complete because those limits are real and because closure is 24-04's to claim, and 24-04 never ran.
- [ ] **F24-05**: Setup-to-recovery and kill/reconnect/delivery journeys pass on native macOS, Linux, and Windows without lost or duplicate work. — **INCOMPLETE. NOT ADDRESSED.** **Plan 24-04's own four tasks were never started** — no journey driver, no receipt schema, no receipt on any platform, no Windows interface-evidence decision, no acceptance panel (`24-04-SUMMARY.md` §8: *"Criterion 5 (live journeys on three platforms): NOT ADDRESSED"*). The `24-04-SUMMARY.md` file records that non-execution; it does not represent it. What DOES exist and must not be mistaken for this requirement: lane 24-C built the **independent delivery sink** the phase could not close Criterion 1's arrival half without, and found a HIGH within one run — **a delivery landed at the destination TWICE across a `kill -9` and a platform-driven restart**. Fixed, re-measured and mutation-proved: of ten deliveries attempted, ten distinct messages exist at an independent destination and none arrived twice; the one delivery whose outcome was UNKNOWN across the kill produced **exactly one** message where before it produced two. That is **Linux only**, is **10 of 12** (the permanently-stalling sink blocks the tick loop — instrument artefact F24-C-M1, not losses), and leaves open the nine adapters inheriting `supports_outbound_idempotency() == false`, for which an outcome-unknown delivery is now correctly **abandoned** rather than duplicated — safe and honest, and not the same thing as delivered.

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

**Amendment 2026-07-28 (lane/26-gaps), on the two clauses 26-04's certification left
OPEN.** Both were measured on real hardware; neither is closed, and the remaining unmet
text is named rather than summarised. Full grading in `26-GAPS-SUMMARY.md`.

- **F26-03, first clause — GENUINELY UNIMPLEMENTED, not superseded, and now evidenced.**
  26-04 measured zero footprint and could not say whether the clause was still wanted.
  It is. Measured at `a170ee24`: `session export` really does emit the redacted envelope
  (874 bytes, names the session, a planted transcript canary ABSENT), while the portable
  artefact `backup create` produces carries that same canary in TWO files — the session
  JSON and the session index's `summary` field — and contains no envelope at all. The
  clause names two different artefacts, and only one is missing: a same-user backup that
  redacted its own transcripts could not restore them, so `backup create`'s behaviour is
  correct for ITS artefact. What does not exist is the portable, share-to-another-party
  corpus. Disposition decided 4-0 (codex, gemini, kimi, plus an internal pass arguing the
  other way and failing to survive rebuttal): record it with these facts and scope it to
  a follow-up plan rather than build a new export surface inside a repair lane. Evidence:
  `evidence/26-gaps/envelope-probe.log`, `scripts/portability-session-envelope-probe.sh`.
- **F26-04 / SC3, the interruption clause — STILL OPEN, for a sharper reason than before.**
  `migrate hermes` and `migrate openclaw` have now been killed mid-apply (35 landed
  mid-flight interruptions across the two paths, `SIGKILL`, swept across the measured
  apply window), which closes 26-04's "never interrupted" objection. But the measurement
  also shows the criterion's literal text is not met by the migration path: **`migrate`
  has no rollback.** It does not return to the pre-operation home; it converges on the
  completed state when the product is driven again. That is a reasonable contract for an
  import and it is now proven (35/35 recovered, external sentinel unchanged), but it is
  not "restore exact pre-operation state on rollback", and the Windows leg does not exist
  for this path. One HIGH was found and fixed on the way — see `F26-GAPS-H1`.

### Phase 27 — Multimodal, Browser, Generation, and Voice Contracts

**All five remain OPEN after the 2026-07-26 execution pass.** Nothing here is
marked complete. Per-requirement disposition with the exact unmet clause is
below; full grading in
`phases/27-multimodal-browser-generation-voice/27-PHASE-VERDICT.md`.

- [ ] **F27-01**: Standalone and host paths share one bounded, open-once, magic-byte-validated attachment/document intake pipeline with explicit provider degradation.
  - **INCOMPLETE — partial.** MET: the document path now uses one bounded,
    open-once, magic-byte-validated intake (`wcore_tools::media_intake`) with an
    ingest cap enforced before any payload read; explicit provider degradation
    for the image class is landed and was proved live on `hetzner-dsm` by
    capturing byte-identical outbound requests with `supports_vision` false and
    true, then gating both the Anthropic and Gemini builders.
  - **UNMET CLAUSE — "share ONE ... pipeline":** the composer path and the
    channel enricher were measured already open-once and correct and were
    deliberately not rewritten through the chokepoint, so the mechanism is
    shared for documents and duplicated elsewhere.
  - **UNMET CLAUSE — host-path proof:** proved on the wire, never in the
    terminal (no PTY drive), and never on macOS (no artifact for this SHA).
- [ ] **F27-02**: Browser, CUA, and web-search capabilities publish live activation/readiness truth and preserve sandbox, egress, approval, and cleanup policy.
  - **INCOMPLETE.** **UNMET CLAUSE — "publish live activation/readiness
    truth":** nothing is published. `browser_suite` and `computer_use` are still
    `true` at HEAD on a machine with no browser binary and no display, measured
    invariant across five single-variable observations, with the very next
    operation failing `spawn camoufox: No such file or directory`. The decision
    is taken (`chain-plus-derived-flags`, 4-0) and blocked on a fenced protocol
    seam; see `.planning/SEAM-REQUESTS/27.md`.
  - **UNMET CLAUSE — "preserve sandbox, egress, approval, and cleanup policy":**
    only origin admission was measured (it holds, fails closed, states its
    reason). Downloads-root confinement, the approval gate, and the process
    count across a session plus one reaper interval have no baseline.
- [ ] **F27-03**: Built-in, MCP-only, late-MCP, and combined image/media generation expose consistent ToolSearch, readiness, credentials, accounting, and failure semantics.
  - **INCOMPLETE.** **UNMET CLAUSE — all four generation shapes:** none was
    exercised; the MCP media-tool fixture was never built. MEASURED AND GOOD:
    the honest-unavailable advisory reaches the model verbatim on the wire,
    naming each capability and the exact variables that enable it. MEASURED GAP:
    it reaches no host — zero protocol events. Accounting is SOURCE-ONLY: a
    media call produces no cost record.
- [ ] **F27-04**: Speech and realtime/voice capability contracts define streaming, interruption, cancellation, provider compatibility, resource accounting, and protocol behavior.
  - **INCOMPLETE — nothing exercised.** **EVERY CLAUSE UNMET.** No audio flowed
    on any machine, no interruption occurred, no cancellation was driven, no
    event ordering was observed. `seandesktop` has audio, a toolchain and was
    verified reachable; the path existed and was not taken. This is an execution
    shortfall, not an environmental impossibility.
- [ ] **F27-05**: Deterministic image/PDF/docx/xlsx/pptx/browser/media/voice corpora and focused packaged smokes pass on native macOS, Linux, and Windows.
  - **INCOMPLETE.** **UNMET CLAUSE — packaged smokes:** zero ran on zero
    platforms. Every Linux measurement came from a `cargo build --release`
    binary in a build tree, which is not a packaged artifact and is not counted
    as one. **PARTIAL — corpora:** the intake corpus exists and is genuinely
    deterministic (18 entries, pinned bytes, byte lengths and SHA-256 in
    `MANIFEST.tsv`); the browser, media and voice corpora were never built and
    no suite consumes any corpus.

### Phase 28 — Native Cross-Platform Certification

- [x] **F28-01**: Native macOS, Linux, and Windows E5 matrices cover sandbox probes, Unicode, long paths, UNC/reparse/symlink cases, process cleanup, suspend/resume, offline, disk-full/read-only, and hostile inputs. — **Complete.** All nine named dimensions ran on all three families, 651 cells (216 linux + 216 macos + 219 windows, the macOS family at its 28-03 re-run which SUPERSEDES 28-02's 24 reds), 0 red, 0 skipped, 147 critical cells and 0 skipped critical cases; the requirement asks for COVERAGE and coverage is what the cell list shows. Stated where it is thin rather than where it is convenient: the coverage spans two candidates (`F-28-04-004`).
- [x] **F28-02**: A 1,000-session and concurrent-child soak completes with secret canaries intact, no orphan processes, and bounded quality/performance deltas. — **Complete.** 3,000 of 3,000 sessions at concurrency 4 across linux, macos and windows; 0 canary detections with the control caught in 6/6 channels on every family; 0 orphans with the control orphan FOUND on every family; every drift and slope inside bands committed before the first session existed. Two stated limits: the macOS census is non-authoritative (`F-28-04-005`) and the workload is read-only (`F-28-04-006`).
- [x] **F28-03**: Signed receipts bind the exact candidate, platform, posture, fixture corpus, environment, artifacts, logs, and skipped-case policy. — **Complete.** All eight bindings present and independently RECOMPUTED from the raw evidence by `f28-verify-bindings.py --verify`, and the artifact verifies under the Rust `CertificationVerifier` too. "Exact candidate" is honoured by binding BOTH candidates per scope rather than picking one (`F-28-04-004`); the binding is exact, the coverage is what is split.
- [ ] **F28-04**: No critical case is skipped and every finding at every severity is resolved before certification acceptance. — **Open.** Its first clause is met (0 skipped critical cases, machine-recomputed); its second is not. `F-28-02-002` (stale AppContainer lease = persistent denial of service, HIGH) has no terminal disposition, so the acceptance gate does not pass. Left open deliberately rather than closed by re-scoring it to MEDIUM, which would have opened the accept path.

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
| F21-01, F21-02, F21-03, F21-04 | Phase 21 | **F21-01 complete; F21-03 fenced on F21-04-01; F21-02 and F21-04 open** — re-graded 2026-07-27 at `ac94b1d5` in `21-REVERIFICATION.md`. SC1 upgraded NOT-MET → MET WITH STATED EXCEPTIONS (the tool guard is present and live-proven); SC2 MET WITH STATED EXCEPTIONS (F21-04-02 discharged, F21-04-01 fenced); SC3 still NOT MET. Superseded prior text: **All four Open** — adjudicated at `21-04-PHASE-VERDICT.md` §3, base `f2d186f6`. Criterion 1 NOT MET (tool authority confirmed absent and DECLINED); Criteria 2 and 3 MET WITH STATED EXCEPTIONS. Six HIGH findings open: F21-02-01, F21-02-03, F21-02-02's live closure, F21-04-01, F21-04-02, F21-04-03. No seal claimed. |
| F22-01, F22-02, F22-03, F22-04, F22-05, F22-06, F22-07 | Phase 22 | Pending |
| F23-01, F23-02, F23-03, F23-04, F23-05, F23-06 | Phase 23 (23A + 23B) | **All six Open. Both sub-phases executed; neither is closed.** 23A: 1 of 4 plans executed, Success Criterion 1 **NOT MET** (`23A-04-SUMMARY.md`) — F23-01 explicitly not marked complete, with `revoke` and `rollback` unimplemented and clause 1 satisfied only vacuously; open HIGH F23A-01-H2. 23B: 4 of 4 plans executed. **F23-06's mandatory clauses are COMPLETE on all three platforms** with only the OPTIONAL semantic layer deferred (`23B-03-SUMMARY.md`). **F23-05 is IN PROGRESS on a real calendar clock** — day one taken on Linux and Windows, macOS NOT ACHIEVED, and it **cannot close before `2026-07-30T23:54:26Z`** (`23B-04-SUMMARY.md`). F23-02 and F23-03 incomplete with named open legs; **F23-04 not started**. Phase HIGH 23B-H1 closed on both halves across three platforms. |
| F24-01, F24-02, F24-03, F24-04, F24-05 | Phase 24 | **All five Open. Revised 2026-07-28 — the prior entry ("only plan 24-01 of four executed") is out of date.** All four plans plus two unplanned lanes (24-B, 24-C) have executed; all six SUMMARY files declare `status: partial`; **the phase is NOT closed and no requirement is marked complete** (`24-04-SUMMARY.md` §8). Criterion 1's delivery-arrival half is **closed on Linux within a stated scope** by 24-C, which found and fixed a HIGH (a delivery landed twice at an independent destination across a `kill -9` and a restart); lane 24-B put the gateway verbs on the shipped binary and proved install→kill→platform recovery→drain→uninstall live on Linux, finding and fixing four more HIGH. **Criterion 4 is MET on Linux, HTTP/SSE only** — a real typed client recovered a real event gap over a real socket against the shipped binary. Criterion 3 PARTIALLY MET on Linux, NOT MET on macOS/Windows. **Criterion 5 NOT ADDRESSED — 24-04's own four tasks were never started, and `24-04-SUMMARY.md` exists to record exactly that.** **No macOS and no Windows evidence exists anywhere in this phase.** Evidence: `phases/24-.../24-{01,02,03,04,B,C}-SUMMARY.md`. |
| F25-01, F25-02 | Phase 25 | **Both Open** — plan 25-01 executed and terminated in state 2 (complete with a bounded cloud gap), base `frontier/p25-remote-reach`. The provider-neutral contract exists, is oracle-conformant and is proven live across THREE reference backends on `hetzner-dsm` with a normalized diff of EQUIVALENT. **Unmet clause, F25-01:** "lifecycle health" and "attestation" are covered, but the contract is proven on three of the four surfaces it must abstract, so provider-neutrality across the cloud transport is asserted rather than demonstrated. **Unmet clause, F25-02:** "and on one hibernating cloud reference backend" — the cloud leg is UNEXERCISED, no Fly credential exists on any proof host, and the backend fails closed rather than falling back. Second unmet clause, F25-02: "identical authority and cleanup semantics" is proven local↔container↔ssh only. See `phases/25-.../25-01-SUMMARY.md`. |
| F25-03 | Phase 25 | **Open — NOT STARTED.** Plan 25-02 was not executed. Unmet clauses: all of them — pairing, capability advertisement, revocation, mixed-version behavior, offline recovery, artifact/receipt attribution. Nothing was built and nothing is claimed. |
| F25-04 | Phase 25 | **Open — NOT STARTED.** Plan 25-02 (the twelve-verb plugin lifecycle) was not executed. Unmet clauses: scaffold, test, sign, inspect, approve, update, rollback, recover, publish and verify-compatibility are all absent from `wayland-core plugin`, which today offers install, list, available, remove and three marketplace verbs. Install and remove alone exist. |
| F25-05 | Phase 25 | **Open — partially evidenced, not met.** Plan 25-04 was not executed. What IS proven, from 25-01's live run on `hetzner-dsm`: cancellation fails closed and leaves no orphan across local, container and ssh, verified against the real process table, the real container listing and the real remote process table; and an absent cloud credential fails closed with an explicit unavailable verdict and never falls back. Unmet clauses: secret/egress denial, key rotation, compromised plugin/backend, and "across every reference backend" — the cloud backend is unexercised and there is no hostile matrix. |
| F26-01, F26-02, F26-03, F26-04, F26-05 | Phase 26 | Pending |
| F27-01, F27-02, F27-03, F27-04, F27-05 | Phase 27 | **All five Open** — executed 2026-07-26 at base `2ecdfdf5`; graded at `27-PHASE-VERDICT.md`. Criterion 1 PARTIAL; Criteria 2, 3, 4 and 5 NOT MET. Phase goal NOT achieved and self-reported as such. Landed: one open-once intake chokepoint for the document path, and the `supports_vision` gate on the Anthropic and Gemini builders (both divergences measured live on `hetzner-dsm`, the second by capturing byte-identical outbound requests, the first by `strace`). Open HIGH: browser/CUA/web readiness still linkage-derived and unpublished (decision taken 4-0, blocked on the fenced protocol seam — `.planning/SEAM-REQUESTS/27.md`); `wcore-browser/src/tool.rs:499` remediation text names `[browser]` when the real key is `[browser.policy]`. Criterion 4 has zero evidence — no audio ever flowed. |
| F28-01, F28-02, F28-03, F28-04 | Phase 28 | **F28-01, F28-02, F28-03 Complete; F28-04 Open.** Adjudicated 2026-07-28 at base `cf48b349`; graded verbatim at `28-04-PHASE-VERDICT.md`. Criteria 1, 2 and 3 MET WITH STATED EXCEPTIONS; Criterion 4 NOT MET. 63 findings adjudicated, 62 with a terminal disposition; 7 cross the A2 line and every one is FIXED or DISPROVED; 0 downgrades. The single blocker is `F-28-02-002` at HIGH, OPEN, and the receipt says so rather than claiming otherwise. The receipt is EVIDENCE, not authorization: no seal, no trust root, no release. |
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
