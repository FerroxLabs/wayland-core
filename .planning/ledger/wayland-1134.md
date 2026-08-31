---
issue: 1134
repo: FerroxLabs/wayland
kind: defect
title: "Test-written process globals are invisible to CI: nextest isolates per process, cargo test does not"
status: closed
last_verified_commit: 856df7d0
criteria:
  - id: c1
    text: "A shared-process lib leg runs in CI, floored so it cannot pass while scanning nothing"
    state: met
    evidence: "file:.github/workflows/ci.yml:2168:Executed $total tests, expected at least $MIN. A suite that exits 0 having run nothing"
    owner: core
    note: "RE-ANCHORED 2026-08-30 for wayland#1198: the old anchor ci.yml:1806 resolved on a line count alone and landed on a bare `#` inside the retry-evidence comment block, ~230 lines above the step it claimed. It now cites the LIB leg's floor branch itself -- the `exit 1` taken when the summed executed count falls under MIN -- which is the half of this criterion (`floored so it cannot pass while scanning nothing`) that a step-name anchor would not prove. ANCHOR IMPRECISE, NOT BROKEN: ci.yml:1806 now lands inside the 'Shared-process lib suite (#1134)' comment block rather than on the cargo test --workspace --lib command itself."
  - id: c2
    text: "A shared-process integration leg runs in CI over the targets that touch process globals"
    state: met
    evidence: "file:.github/workflows/ci.yml:2244:done < <(python3 scripts/check-test-env-globals.py --shared-process-targets)"
    note: "RE-ANCHORED 2026-08-30 for wayland#1198: the old anchor ci.yml:1888 resolved and landed inside the SWARM delegated-dispatch filterset -- a different step, and the wrong one of the two legs this criterion distinguishes. It now cites the line that feeds the integration loop, which is where `over the targets that touch process globals` is actually decided: the target list is DERIVED from the process-global scanner rather than hand-listed."
    owner: core
  - id: c3
    text: "A lint catches the class in CI, with a paired-direction self-test run immediately before it"
    state: met
    evidence: "symbol:scripts/check-test-env-globals.py::classify_site"
    owner: core
    handoff: "FerroxLabs/wayland#1233"
    note: "RE-GRADED 2026-08-29, NARROWED 2026-08-30 after a verifier refutation. The wiring half always held (ci.yml runs --self-test then the gate, unconditional, in a required job). The SUBSTANCE did not: the lint fired only on a set_var written lexically inside a test fn, so the identical write moved one call deep was classified `helper` and never graded -- 153 sites, and the headline one, PinnedRetryBudget::pin, was not even seen, because src/test_utils/mod.rs is an ungated `pub mod` with no cfg(test) span around it. MEASURED before: removing #[serial_test::serial] from its caller at engine.rs made `cargo test -p wcore-agent --lib` fail 3-vs-11 while the lint returned exit 0 with byte-identical counts. MEASURED after: the same mutation returns exit 1 naming test_utils/mod.rs pin/drop as `helper write, reached from an unserialized test`. Callers are resolved by attribution key -- the impl TYPE for a method (so an RAII guard\'s ctor and Drop are one key, and `drop`/`new` name collisions are impossible), the fn name for a free fn and only when declared once in the binary; anything else is reported, never failed. The self-test gained three paired cases: the write one call deep with an unserialized caller (FIRE), the same with a serialized caller (quiet), and a guard nothing constructs (quiet). THE VERIFIER REFUTED THE FIRST VERSION OF THIS AND WAS RIGHT: the debt file keyed its exemptions on (binary, var), so a BRAND-NEW helper writing an already-listed var in an already-listed binary landed silently. They demonstrated it by appending one to crates/wcore-cli/src/lib.rs -- WAYLAND_HOME (listed) exited 0, HOME (unlisted) exited 1, same shape, same file, same command. An exemption keyed on the class is a gate that cannot catch the next instance. FIXED: the file now carries a `<file.rs::fn>` SITE column and is keyed on (binary, var, site); a line without a site is refused as unparseable rather than read as class-wide; a pair with even one uncovered site fails and only the uncovered sites are named. THE SAME PROBE, RE-RUN AT 856df7d0: both arms now exit 1 -- `crates/wcore-cli/src/lib.rs:255 fn verif_plant_WAYLAND_HOME [helper write, reached from an unserialized test]` and the identical line for HOME -- and the restored tree exits 0 with git status --porcelain empty. The nine listed sites (eight pairs, one of which has two sites) are dated debt owned by #1233; a ninth pair, a DIRECT write of WAYLAND_EXEC_CONTAINER_IMAGE that had the gate RED at HEAD, was fixed via ContainerBackend::with_image. Six new paired self-test cases cover the direction the old file got wrong: a listed site is quiet, a NEW site of a listed (binary,var) FIREs, both-listed is quiet (the control that the fire is the missing LINE and not the two-guard shape), a line whose site is gone is STALE, a DIRECT write is never excused, an expired line does not exempt, and a line with no site column is refused."
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
