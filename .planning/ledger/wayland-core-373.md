---
issue: 373
repo: FerroxLabs/wayland-core
title: "cargo test --workspace --lib --no-fail-fast cannot be run 10x consecutively: osv_check::tests::ssrf_refusal_is_visible_at_default_log_levels fails ~5% of runs"
status: open
kind: defect
last_verified_commit: d9f7e0a0
criteria:
  - id: c1
    text: "The mechanism is named in code: what makes the ERROR not reach the scoped subscriber. Not inferred -- two inferences have already failed"
    state: not-met
    owner: core
    note: "Refuted #1: concurrent scoped subscribers. `fail_open_is_visible_at_default_log_levels` is the only other subscriber-installing test in the binary; putting both in `#[serial_test::serial(tracing_subscriber)]` scored 5/40, the baseline rate. Refuted #2: stale per-callsite interest. Adding `tracing::callsite::rebuild_interest_cache()` after `set_default` scored 4/100, also the baseline rate. Both reverted; neither is in the tree."
  - id: c2
    text: "A red arm is quoted verbatim from a real run, and the rate is measured at n>=100 on the same instrument as the fix arm"
    state: met
    owner: core
    evidence: "test:crates/wcore-tools/src/osv_check.rs::ssrf_refusal_is_visible_at_default_log_levels"
    note: "Baseline measured on hetzner-dsm at integ/f13 (18e59e85), tight loop of ./target/debug/deps/wcore_tools-78a8b0c9297025a9 --quiet: 5 failures in 100 runs, and 12 in 100 after merging current integ/f13 (d9f7e0a0) -- the rate moves with host load, so measure both arms in the same session. Red arm verbatim -- panicked at crates/wcore-tools/src/osv_check.rs:1356:9, 'assertion `left == right` failed / left: [] / right: [Level(Error)]'."
  - id: c3
    text: "The fix arm scores 0 failures at n>=100 on that same instrument, and the baseline is re-measured at the same n in the same session"
    state: not-met
    owner: core
    note: "The same-session re-measurement is not optional here: the two refuted attempts differed from the baseline only by sampling noise, and the n=40 arm made a 12% rate look real when it was 5%."
  - id: c4
    text: "Both osv_check log-visibility tests keep their exact-equality assertion on [Level(Error)]"
    state: not-met
    owner: core
    note: "A guard on the fix, not work of its own. The assertion is the security-visibility invariant for a fail-open SSRF refusal: at RUST_LOG unset the operator only ever sees ERROR. Ignoring the test, relaxing it to 'at most one ERROR', or moving it to nextest-only is refused."
  - id: c5
    text: "cargo test --workspace --lib --no-fail-fast passes N>=10 consecutive times on hetzner-dsm, run count recorded"
    state: not-met
    owner: core
    note: "Handed over intact from core#361 c6. Three of the four blockers that stood in its way are already closed (wcore-config 2c8efb46, wcore-observability 812e26075, wcore-cli 18e59e85f); this issue's own defect is the fourth and last known one. Best observed streak: 1 consecutive pass."
---

Split out of core#361 c6 so the remainder has an owner and a contract instead of
disappearing into a "partial". core#361's own defect — a greedy PII pattern
reaching out of an approval token and collapsing the payload under the
truncation cap — is closed and its fixture is pinned to the adversarial input.

The full evidence, the two refuted hypotheses, and the reasons the obvious
"fixes" are refused are in the issue body on GitHub. The short version: a scoped
`tracing` subscriber installed with `set_default` intermittently captures
nothing at all, at a measured 5 runs in 100, and both the concurrency
explanation and the interest-cache explanation were measured and came back
indistinguishable from the baseline.
