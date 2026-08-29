---
issue: 1134
repo: FerroxLabs/wayland
kind: defect
title: "Test-written process globals are invisible to CI: nextest isolates per process, cargo test does not"
status: closed
last_verified_commit: 9de21aa1
criteria:
  - id: c1
    text: "A shared-process lib leg runs in CI, floored so it cannot pass while scanning nothing"
    state: met
    evidence: "file:.github/workflows/ci.yml:1806"
    owner: core
    note: "ANCHOR IMPRECISE, NOT BROKEN: ci.yml:1806 now lands inside the 'Shared-process lib suite (#1134)' comment block rather than on the cargo test --workspace --lib command itself. It still resolves and still points into the right step; re-anchor it if the workflow is edited again."
  - id: c2
    text: "A shared-process integration leg runs in CI over the targets that touch process globals"
    state: met
    evidence: "file:.github/workflows/ci.yml:1888"
    owner: core
  - id: c3
    text: "A lint catches the class in CI, with a paired-direction self-test run immediately before it"
    state: not-met
    evidence: "file:.github/workflows/ci.yml:1399"
    owner: core
    note: "REFUTED 2026-08-29 by the 0.13.12 close-sweep, recorded verbatim: DOES NOT HOLD AS WRITTEN. The wiring half is fine: ci.yml:1398-1399 runs `check-test-env-globals.py --self-test` immediately followed by the gate, unconditional, in a required job; anchor 1399 is exact. But 'catches the class' is measured false for the class's own headline instance. The lint only fails on kind UNSERIALIZED-TEST - a `set_var` lexically inside a fn carrying a test attribute. A write inside a non-test helper is classified 'helper' and is explicitly not audited; the script prints so itself: `NOT audited by this gate: 153 write(s) inside helper functions`. `PinnedRetryBudget::pin` (crates/wcore-agent/src/test_utils/mod.rs:381) - the exact helper #1134 opens with - is one of those. PROOF, not inference: with the serial attribute removed from its caller at engine.rs:29581 (the mutation that makes `cargo test -p wcore-agent --lib` fail 3-vs-11, see c1), the lint returned EXIT=0, `OK: no unserialized test writes a global that its own binary's production code reads.`, and byte-identical counts {'serial-attr': 459, 'helper': 153, 'UNSERIALIZED-TEST': 49, 'lock-guarded': 20} - it did not even see the change. Graded minor rather than major only because c1's lib leg demonstrably DOES catch that exact regression, so the class is bounded by the legs even though the lint criterion overstates the lint."
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
