---
issue: 1233
repo: FerroxLabs/wayland
kind: defect
title: "Eight helper-attributed env-global hazards, now audited and carried as dated debt"
status: open
last_verified_commit: 17e15bc3f
criteria:
  - id: c1
    text: "Each of the eight pairs in .config/env-global-helper-debt.txt reaches a terminal state: the helper stops writing the process global (the value is stated at the call site, the shape ContainerBackend::with_image already used), or the pair is serialized, or the entry is re-dated with a measured reason. None is left listed with nobody having looked at it."
    state: met
    evidence: "absent:.config/env-global-helper-debt.txt::gh#1233"
    owner: core
    note: "MET at 17e15bc3f. THE LIST IS EMPTY, and it emptied by the FIRST of the three outlets, not the third. The four pairs still open at b5ebde6c3 -- `capability_liveness_narrowing.rs::install` + `::drop` (one RAII pair, two rows), `skill_source_write_refusal.rs::wayland_home`, `sidecar_npm_install_wiring_test.rs::stub_npm_on_path`, `wcore-channel-telegram/src/lib.rs::cfg` -- all now state the value at the CALL SITE. Nothing was re-dated and nothing was serialized. FOUR SEAMS CARRY IT, all in production code and not behind cfg(test), for the reason `wcore_exec_backend::StateDirGuard` is public: three of the four callers are integration tests in `tests/`, which link these crates compiled WITHOUT cfg(test). (1) `wcore_browser::liveness::BrowserProbeTarget` -- the sidecar base URL and program travel as one argument through the new `probe_target`, and `PluginCapabilitySet::narrowed_to_live_against` hands the same value to both the oracle and the code under test, so the guard's two arms are provably the same experiment instead of two runs agreeing about an ambient global. The RAII guard is DELETED outright, which is what removes the second row: a restoring `Drop` is a second write of the same global, not a cleanup. (2) `WorkspacePolicy::with_user_config_root` -- the user skill SOURCE roots, derived inside the builder from `wcore_config::config::SKILL_SOURCE_DIR_NAMES` so an override cannot name a different pair than `user_skill_source_dirs()` builds. (3) `SupervisorConfig::npm_program` + `BrowserBinaryManager::with_npm_program` -- the npm executable, so a stub no longer has to be prepended to PATH, a global `wcore-config` and `wcore-tools` production code read for shell resolution. (4) `TelegramChannel::with_state_dir` -> `LongPollArgs::state_dir` -> `offset_store::state_path(root, name)` -- the offset watermark root. PRODUCTION BEHAVIOUR IS UNCHANGED IN ALL FOUR: the None/absent case of each seam is the existing environment-derived resolution, and only the new constructors are reached from tests. A SECOND DEFECT FELL OUT AND IS NOT CLAIMED AS THIS ROW: four of `skill_source_write_refusal`'s eleven tests never called the `WAYLAND_HOME` helper at all, so they were judged against the OPERATOR'S REAL config dir; stating the root on `session()` fixes that, and the eleven now share one temporary root. THE GATE ITSELF CERTIFIES THE ROWS ARE GONE rather than my reading of the diff: restoring all five deleted lines onto this tree and re-running `python3 scripts/check-test-env-globals.py` gives EXIT=1 with five findings of the form `FAIL: .config/env-global-helper-debt.txt lists <binary> <VAR> at <site>, which no longer matches a hazard the scan reports ... delete it`, one per row, naming `::drop`, `::install`, `::wayland_home`, `::stub_npm_on_path` and `lib.rs::cfg`. With the lines deleted the same command is EXIT=0 and prints `OK: no unserialized test writes a global that its own binary's production code reads`. WHAT IS NOT CLAIMED: this closes the eight PAIRS the debt file listed. It does not claim the class is gone from the repo -- the same run still REPORTS 12 pairs whose only unserialized writer sits in a binary with no sibling test, and 10 writes whose attribution key collides so the walk refuses to convict. Both are printed on every run and neither is exempted by a line in this file."
  - id: c2
    text: "The three temp_state() rows are fixed as ONE helper duplicated across three integration targets, not as three independent fixes, so the duplication does not regrow. This is the same defect as wayland#1250 and the two close together or the overlap is stated."
    state: met
    evidence: "symbol:crates/wcore-exec-backend/src/registry.rs::StateDirGuard"
    owner: core
    note: "MET at 509f4426b. Fixed as ONE mechanism, not three independent edits: every `temp_state()` in wcore-exec-backend now delegates to the single `StateDirGuard` seam in crates/wcore-exec-backend/src/registry.rs, which installs a per-thread override that `state_dir()` consults ahead of the env var. Because the seam lives in production code and the four call sites are one line each, the duplication cannot regrow into four divergent fixes. It covers FOUR integration targets, not the three this row names -- container_wedge, live_equivalence, conformance_matrix, container_orphan_scan -- with fail_closed_matrix already migrated. THE OVERLAP WITH wayland#1250 IS STATED, which is this criterion second arm: wayland-1250 c4 names this ticket by number and its c1/c2 grade the same edit. Landed in 75cc3682b. NOT GRADED: c1, which requires all EIGHT rows to reach a terminal state; six remain listed."
  - id: c3
    text: "The wcore-cli row is treated as the production finding the table says it is -- run_gateway is production code reached from an unserialized test -- so the fix lands in the gateway, or the reason it lands in the test instead is recorded."
    state: met
    evidence: "file:crates/wcore-cli/src/gateway.rs:324:/// Dispatch one `wayland-core gateway <verb>`."
    owner: core
    note: "MET at b5ebde6c3, and it is met by TAKING THE ROW SERIOUSLY AS A PRODUCTION FINDING AND MEASURING IT, which is what this criterion asks for -- but the measurement refutes half the claim, so neither of the two outlets the criterion`s own text offers was taken. Say that plainly rather than pick the nearer one. WHAT WAS MEASURED: `run_gateway` IS production code and it DOES write the process-global WAYLAND_HOME (gateway.rs, behind `scope.home.is_some() && WAYLAND_HOME unset`, load-bearing because Windows Task Scheduler cannot export an environment variable and the credentials store resolves under `wayland_config_dir()` -- twelve deliveries submitted and zero arrived when it did not, measured live at d89b81b6). But NOTHING IN THE wcore-cli TEST BINARY CALLS IT. `run_gateway` has exactly one caller in the workspace. The `reached from an unserialized test` half of the row was manufactured by `check-test-env-globals.py`s caller walk: hop 1 refuses an attribution key declared more than once, hops 2 and 3 did not, and the walk went `run_gateway` <- `run` -- a name wcore-cli declares 31 times -- <- 155 unrelated `run(` call sites, convicting on `fresh_dir_writes_both_files` and `extract_zip_recovers_binary`. Printed directly from the walk on 2026-09-10; the first A/B I ran to establish this was WRONG (it renamed the callee and not the call site, so the chain died on an inconsistent tree) and the printed chain is what replaced it. SO THE FIX LANDS IN TWO PLACES, NEITHER OF THEM A TEST. In the gate: every hop now applies hop 1`s unique-declaration rule and reports rather than follows, with two self-test arms proving both directions -- a chain through a colliding name stays quiet, the same chain with the intermediate declared once still fires -- so the guard cannot be a walk that quietly stopped working. Workspace effect measured before and after: exactly one failing verdict changes, from false to correct; nothing that was failing on real evidence goes quiet. In the gateway: `gateway::run` is renamed `dispatch_gateway_command` (this anchor), zero behaviour change, one call site, `cargo check -p wcore-cli --all-targets` clean on hetzner at b5ebde6c3. That rename buys the one thing the old spelling could never give -- a test calling the gateway entry point is now attributed to the write. The debt row is deleted because the gate itself now calls it STALE. WHAT IS STILL TRUE AND IS NOT CLAIMED CLOSED: production code still writes a process global, and the chain still leaves the crate through `main.rs::run`, which collides 31 ways, so the gate REPORTS that write as unattributable rather than proving nothing reaches it. A reader who thinks that is too weak for a `met` should reopen this row -- the evidence to argue from is all here."
  - id: c4
    text: "What happens when a dated debt entry passes its date is DECIDED and enforced: either the gate fails on an expired entry, or the absence of an expiry is recorded as deliberate. A debt file whose dates carry no consequence is a list, not debt."
    state: met
    evidence: "file:scripts/check-test-env-globals.py:949:if expiry < today:"
    owner: core
    note: "MET at 509f4426b, and it is ENFORCED rather than merely decided. scripts/check-test-env-globals.py:949 compares each row expiry against today and, when it has passed, appends `expired on %s and was not renewed ... An expired entry fails exactly as an unlisted site does`, which is a gate failure and not a warning. The malformed-line and unknown-expiry-format arms sit immediately above it, so a row that cannot be dated is refused rather than read as a class-wide exemption. It is WIRED: .github/workflows/ci.yml:1825-1826 runs `--self-test` and then the gate itself on every CI run, and the self-test carries the arm `debt: an expired line does not exempt` (line 777) so the enforcement is proven in both directions rather than asserted. The decision is also written where a reader of the debt file will see it, in that file own header. WHAT WOULD FALSIFY THIS: deleting the expiry comparison, which the anchored line reds on."
  - id: c5
    text: "These are invisible under nextest by construction (one process per test) and only observable on the shared-process legs. Whatever closes this is graded on a shared-process run, not a nextest one."
    state: met
    evidence: "symbol:crates/wcore-channel-telegram/src/lib.rs::with_state_dir"
    owner: core
    note: "MET at 17e15bc3f. EVERY closure this issue claims was graded on plain `cargo test`, and the instrument was then PROVEN able to fail rather than assumed to be. THE FOUR GRADING RUNS, on hetzner via tools/remote-proof.py slot parallel-2, each with a receipt carrying remote_exit 0 and complete true, all at source 17e15bc3f / tree 6ed97b336: `cargo test -p wcore-agent --test capability_liveness_narrowing --test skill_source_write_refusal` -> `4 passed; 0 failed` and `11 passed; 0 failed`, two processes, one per integration target; `cargo test -p wcore-browser --test sidecar_npm_install_wiring_test` -> `3 passed; 0 failed`; `cargo test -p wcore-channel-telegram --lib` -> `79 passed; 0 failed; ... finished in 0.26s`, ONE process for all 79. Not one of them is a nextest run, and the cargo_args field of each receipt is the record of that. THE INSTRUMENT DEMONSTRATION, which is the half that stops this being a claim about a command line. A scratch commit (9660cb2f5, tree 58af38e1, since reset off the branch) added two tests to the wcore-channel-telegram lib binary that write WAYLAND_HOME to different values with sleeps arranged so arm A reads back after arm B has written. SAME TREE, TWO INSTRUMENTS: `cargo test -p wcore-channel-telegram --lib` -> remote_exit 101, `test offset_store::tests::instrument_arm_a_reads_back_what_it_wrote ... FAILED`, `test result: FAILED. 80 passed; 1 failed`; `cargo nextest run -p wcore-channel-telegram --lib` -> remote_exit 0, `PASS ... instrument_arm_a_reads_back_what_it_wrote`, `Summary [0.745s] 81 tests run: 81 passed, 0 skipped`. So the shared-process leg can see this class and the nextest leg provably cannot, which is exactly what this criterion asserts, measured on one tree rather than reasoned from `--test-threads`. The scratch commit was removed with `git reset --hard 17e15bc3f` and the crate's sources touched afterwards; `git status --porcelain` is empty and the mutation greps to nothing. WHAT IS NOT CLAIMED: the demonstration is CONSTRUCTED. It proves the instrument's discriminating power, not that any of the four historical pairs would have gone red under `cargo test` before the fix -- they would not have, which is precisely why they were carried as debt rather than as failures. A reader who wants the historical red has to build one per pair, and none of them is deterministic."
---

Created 2026-08-31. This issue was filed 2026-08-29/30 by this cycle's own
verification, was in scope for the release gate from that moment, and had no
ledger file -- so scripts/check-release-readiness.py, which reads ledger files
and nothing else, could not count it. CI runs the coverage arm with --offline,
which is the arm that would have said so.

Its body declared no acceptance criteria, so it could not have been closed as
filed either. The criteria above are AUTHORED from measurements the body
already records.

The eight are real hazards only where one test binary is one process. That is
the whole of wayland#1134: the main CI legs run nextest, which gives every test
its own process, so none of these can ever be observed there. Grading a fix on
a nextest run would be a green from the wrong instrument.


## 2026-09-10 -- the wcore-cli row, measured

Done on `w14/gate`. The row read: `run_gateway` is PRODUCTION code that writes
WAYLAND_HOME and is reached from an unserialized test in the 2169-test wcore-cli
lib binary.

Half of that is true. The write is production code and it is real. The reaching
is not: `run_gateway` has one caller in the workspace, `gateway.rs`'s own command
dispatch, itself called only from `main.rs`. The verdict came out of
`check-test-env-globals.py`'s caller walk following a name it cannot
disambiguate -- printed from the walk itself:

    run_gateway <- run [helper] decls=31
    run <- fresh_dir_writes_both_files [UNSERIALIZED-TEST]
    run <- extract_zip_recovers_binary [UNSERIALIZED-TEST]
    ... 155 call sites of `run(` in wcore-cli, none of them the gateway

A FIRST A/B GOT THIS RIGHT FOR THE WRONG REASON and is recorded so nobody
repeats it: renaming `gateway::run` WITHOUT updating its one call site flipped
the gate FAIL -> OK, which looked like proof and was really an inconsistent
tree with no call site left to walk. The printed chain is the evidence; the A/B
is not.

Fixed at the source (every hop now refuses a colliding name, two self-test arms
either way), the entry point renamed so a future test that genuinely calls the
gateway IS attributed, and the row deleted because the gate calls it stale.

Four pairs remain. They are ordinary work, not blocked on anyone.
