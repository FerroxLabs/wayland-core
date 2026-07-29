# Lane `ci-green` — running NOTES

Branch `lane/ci-green`, based on `plan/f20-unified-audit-repair` @ `1097cfb3`.
Committed early per LANE-BRIEF §6b-i. Appended after every measurement.

Scope: 3 of the 5 CI failures in run 30434804220 job `CI (linux-containerized)`.
OUT OF SCOPE (do not touch): `wcore-protocol::desktop_contract_corpus` (both tests).

---

## t0 — orientation (before any measurement)

- Worktree toplevel verified: `/Users/seandonahoe/dev/waylandcore-frontier-worktrees/lane-ci-green`.
- HEAD `1097cfb300d19b3524d696cce58ad85d5c7a33fe`.
- `.config/nextest.toml` read. Key facts for Defect 1:
  - `[profile.ci]` does NOT set `run-ignored`; nextest default is to skip `#[ignore]`d tests.
  - `[profile.default] no-tests = "fail"` is inherited by `ci` — an invocation matching ZERO
    tests FAILS. This matters: if I gate the linux test behind `#[ignore]`, a targeted
    proof-host invocation `-E 'test(=on_linux_...)' --run-ignored all` still selects it, so
    `no-tests = fail` cannot silently self-pass.
  - `just test-ci` = `cargo nextest run --workspace --profile ci --no-fail-fast`.
    A workspace run executes thousands of tests, so `no-tests` is not triggered by
    one binary contributing zero.
- Defect 1 test source read: `crates/wcore-exec-backend/tests/node_contract.rs:143-185`.
  Confirmed the precondition loop asserts `HOSTNAME` unset with "Failing rather than skipping."
- Defect 2 site read: `crates/wcore-cli/tests/deterministic_openai_loop.rs:339-395`.
  Scenario budget: `max_total_time(5s)`, turn `max_time(3s)`, `.stop_mid_turn()`.
  Assertion is exact-set `[Failure::CostMissing]`.

## Still to establish

- [ ] Does GH Actions container actually export `HOSTNAME`? (claim, not yet verified from CI log)
- [ ] Where does `OverTime` get produced relative to cancellation in the product?
- [ ] `build.rs::resolve_source_sha` fallback + `artifact.rs:289` validator.
- [ ] Whether the containerized job's git actually fails (dubious ownership vs missing git).
