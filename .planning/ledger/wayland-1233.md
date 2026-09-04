---
issue: 1233
repo: FerroxLabs/wayland
kind: defect
title: "Eight helper-attributed env-global hazards, now audited and carried as dated debt"
status: open
last_verified_commit: 509f4426b
criteria:
  - id: c1
    text: "Each of the eight pairs in .config/env-global-helper-debt.txt reaches a terminal state: the helper stops writing the process global (the value is stated at the call site, the shape ContainerBackend::with_image already used), or the pair is serialized, or the entry is re-dated with a measured reason. None is left listed with nobody having looked at it."
    state: not-met
    owner: core
    note: "AUTHORED 2026-08-31, not transcribed: the issue body declares no criteria, so this ticket could not have been graded or closed as filed. Derived from a measurement the body already records, so grading it does not re-derive the finding. State is not-met because no lane has claimed it."
  - id: c2
    text: "The three temp_state() rows are fixed as ONE helper duplicated across three integration targets, not as three independent fixes, so the duplication does not regrow. This is the same defect as wayland#1250 and the two close together or the overlap is stated."
    state: met
    evidence: "symbol:crates/wcore-exec-backend/src/registry.rs::StateDirGuard"
    owner: core
    note: "MET at 509f4426b. Fixed as ONE mechanism, not three independent edits: every `temp_state()` in wcore-exec-backend now delegates to the single `StateDirGuard` seam in crates/wcore-exec-backend/src/registry.rs, which installs a per-thread override that `state_dir()` consults ahead of the env var. Because the seam lives in production code and the four call sites are one line each, the duplication cannot regrow into four divergent fixes. It covers FOUR integration targets, not the three this row names -- container_wedge, live_equivalence, conformance_matrix, container_orphan_scan -- with fail_closed_matrix already migrated. THE OVERLAP WITH wayland#1250 IS STATED, which is this criterion second arm: wayland-1250 c4 names this ticket by number and its c1/c2 grade the same edit. Landed in 75cc3682b. NOT GRADED: c1, which requires all EIGHT rows to reach a terminal state; six remain listed."
  - id: c3
    text: "The wcore-cli row is treated as the production finding the table says it is -- run_gateway is production code reached from an unserialized test -- so the fix lands in the gateway, or the reason it lands in the test instead is recorded."
    state: not-met
    owner: core
    note: "AUTHORED 2026-08-31, not transcribed: the issue body declares no criteria, so this ticket could not have been graded or closed as filed. Derived from a measurement the body already records, so grading it does not re-derive the finding. State is not-met because no lane has claimed it."
  - id: c4
    text: "What happens when a dated debt entry passes its date is DECIDED and enforced: either the gate fails on an expired entry, or the absence of an expiry is recorded as deliberate. A debt file whose dates carry no consequence is a list, not debt."
    state: met
    evidence: "file:scripts/check-test-env-globals.py:900:if expiry < today:"
    owner: core
    note: "MET at 509f4426b, and it is ENFORCED rather than merely decided. scripts/check-test-env-globals.py:900 compares each row expiry against today and, when it has passed, appends `expired on %s and was not renewed ... An expired entry fails exactly as an unlisted site does`, which is a gate failure and not a warning. The malformed-line and unknown-expiry-format arms sit immediately above it, so a row that cannot be dated is refused rather than read as a class-wide exemption. It is WIRED: .github/workflows/ci.yml:1644-1645 runs `--self-test` and then the gate itself on every CI run, and the self-test carries the arm `debt: an expired line does not exempt` (line 728) so the enforcement is proven in both directions rather than asserted. The decision is also written where a reader of the debt file will see it, in that file own header. WHAT WOULD FALSIFY THIS: deleting the expiry comparison, which the anchored line reds on."
  - id: c5
    text: "These are invisible under nextest by construction (one process per test) and only observable on the shared-process legs. Whatever closes this is graded on a shared-process run, not a nextest one."
    state: not-met
    owner: core
    note: "AUTHORED 2026-08-31, not transcribed: the issue body declares no criteria, so this ticket could not have been graded or closed as filed. Derived from a measurement the body already records, so grading it does not re-derive the finding. State is not-met because no lane has claimed it."
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
