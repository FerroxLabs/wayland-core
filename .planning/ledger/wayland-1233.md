---
issue: 1233
repo: FerroxLabs/wayland
kind: defect
title: "Eight helper-attributed env-global hazards, now audited and carried as dated debt"
status: open
last_verified_commit: b5ebde6c3
criteria:
  - id: c1
    text: "Each of the eight pairs in .config/env-global-helper-debt.txt reaches a terminal state: the helper stops writing the process global (the value is stated at the call site, the shape ContainerBackend::with_image already used), or the pair is serialized, or the entry is re-dated with a measured reason. None is left listed with nobody having looked at it."
    state: not-met
    owner: core
    note: "PARTIAL at b5ebde6c3, and left not-met deliberately. FOUR of the eight pairs have now reached a terminal state: the three `temp_state()` rows left when `StateDirGuard` replaced them (c2), and the `wcore-cli` row left on 2026-09-10 because it was measured and found not to be a hazard at all -- see c3. FOUR REMAIN LISTED, and each needs a real change inside `crates/**` that this lane was scoped out of: `capability_liveness_narrowing.rs::install` + `::drop` (one RAII pair, two rows), `skill_source_write_refusal.rs::wayland_home`, `sidecar_npm_install_wiring_test.rs::stub_npm_on_path`, and `wcore-channel-telegram/src/lib.rs::cfg`. WHAT IS OWED, exactly: for each, either the helper stops writing the process global -- the value stated at the call site, the shape `ContainerBackend::with_image` already uses, which for the first three means passing the sidecar endpoint, the profile root and the npm executable through launch configuration rather than through the environment -- or the pair is serialized across EVERY test in the same binary that reaches the same global, not just the writer, or the entry is re-dated with a reason that is measured rather than restated. Re-dating alone, with no new measurement, is the rot the debt file`s own header warns about and does not close this row. This is NOT blocked on anybody: the work is well-specified and the gate already reds the moment a row goes stale."
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
    state: not-met
    owner: core
    note: "NOT MET, and the instrument is now established rather than assumed. The one pair closed in this lane -- the `wcore-cli` row, c3 -- was graded on a SHARED-PROCESS run, `cargo test -p wcore-cli --lib` on hetzner at b5ebde6c3 -- 2151 passed, 0 failed, 1 ignored, 61.55s, ONE process for the whole lib binary, which is the instrument on which a WAYLAND_HOME contamination from `run_gateway` COULD have been observed. It was not a nextest run and the distinction is the whole of this criterion. WHAT IS OWED: the four pairs c1 still lists are in wcore-agent, wcore-browser and wcore-channel-telegram, and each must be graded on the shared-process leg for ITS binary -- `cargo test -p wcore-agent --test capability_liveness_narrowing --test skill_source_write_refusal`, `cargo test -p wcore-browser --test sidecar_npm_install_wiring_test`, `cargo test -p wcore-channel-telegram --lib` -- run as `cargo test`, never `cargo nextest run`, because nextest gives every test its own process and none of these can be observed there. A green from nextest on any of them is a green from the wrong instrument and must not be accepted. This row closes with c1 and on the same instrument."
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
