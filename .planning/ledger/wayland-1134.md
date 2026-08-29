---
issue: 1134
repo: FerroxLabs/wayland
kind: defect
title: "Test-written process globals are invisible to CI: nextest isolates per process, cargo test does not"
status: closed
last_verified_commit: 43848f75
criteria:
  - id: c1
    text: "A shared-process lib leg runs in CI, floored so it cannot pass while scanning nothing"
    state: met
    evidence: "file:.github/workflows/ci.yml:1954:cargo test --workspace --lib --no-fail-fast"
    owner: core
    note: "ANCHOR IMPRECISE, NOT BROKEN: ci.yml:1806 now lands inside the 'Shared-process lib suite (#1134)' comment block rather than on the cargo test --workspace --lib command itself. It still resolves and still points into the right step; re-anchor it if the workflow is edited again."
  - id: c2
    text: "A shared-process integration leg runs in CI over the targets that touch process globals"
    state: met
    evidence: "file:.github/workflows/ci.yml:2012:Shared-process integration suite (cargo test, one process per target)"
    owner: core
  - id: c3
    text: "A lint catches the class in CI, with a paired-direction self-test run immediately before it"
    state: met
    evidence: "file:.github/workflows/ci.yml:1466:python3 scripts/check-test-env-globals.py --self-test"
    owner: core
    note: "THE LINT WAS BLIND TO THE SHAPE THIS ISSUE OPENS WITH, and that is now measured rather than argued. `PinnedRetryBudget::pin` (crates/wcore-agent/src/test_utils/mod.rs:387) writes WAYLAND_MAX_STREAM_RETRIES three times and the scanner recorded ZERO write sites in that file -- not 'classified as an unaudited helper': never seen, because a test-support module under src/ carries no #[cfg(test)] and the span walk skipped all of it. MEASURED red arm: with #[serial_test::serial] removed from its caller at engine.rs (stream_error_exhausts_retries_then_fails_the_turn), the shipped lint emitted byte-identical counts {serial-attr 459, helper 154, UNSERIALIZED-TEST 53, lock-guarded 20} and the same single failing pair -- it did not see the change at all -- while the widened lint reports WAYLAND_MAX_STREAM_RETRIES in wcore-agent naming test_utils/mod.rs:387 fn pin, and that pair disappears again on restore. Two widenings: src/**/test_utils|test_support|testing|test_helpers is scanned as test code, and a write inside a helper is resolved through a per-binary call graph to ask whether any UNSERIALIZED test reaches it (qualified Type::method edges, plus bare names that are unique across the binary; x.method() and implicit Drop::drop are not recoverable from text and stay in the printed residue). --self-test grew 7 fixtures to 14, covering helper, method, two-hop and src/test_utils writes in BOTH directions -- the gap that let this ship was that all seven original fixtures wrote directly inside a test fn. REMAINDER, stated rather than hidden: the widened lint now reports 8 failing pairs where the shipped one reported 1. Seven are newly visible helper-mediated instances of this very class (temp_state in three wcore-exec-backend targets, stub_npm_on_path, wayland_home, install, cfg) and are real work this lane did not do."
  - id: c4
    text: "That lint proves both directions itself, so it cannot rot into a checker that matches nothing"
    state: met
    evidence: "symbol:scripts/check-test-env-globals.py::self_test"
    owner: core
  - id: c5
    text: "The bare API_KEY exfiltration path the sweep found is closed behind an explicit opt-in"
    state: met
    evidence: "test:crates/wcore-config/tests/credential_off_state_test.rs::bare_api_key_is_not_adopted_as_a_provider_credential"
    owner: core
---

Closed in v0.13.10. `cargo nextest` gives every test its own process, so a
test-written process global can never contaminate a sibling under it — the
entire defect class was invisible to CI at `--retries 0`. Under plain
`cargo test`, which is what a developer runs, one test binary is one process
and the contamination is real.

The fix is three instruments, not one: two shared-process CI legs that can
actually exhibit the class, and a text lint that keeps it from coming back.
Both legs are floored so a run that executes nothing goes red rather than
green. The same sweep turned up a live exfiltration path — a bare `API_KEY`
in the environment being honoured as a provider credential — which is now
ignored unless `WAYLAND_ALLOW_BARE_API_KEY=1`.
