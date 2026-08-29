---
issue: 373
repo: FerroxLabs/wayland-core
title: "cargo test --workspace --lib --no-fail-fast cannot be run 10x consecutively: osv_check::tests::ssrf_refusal_is_visible_at_default_log_levels fails ~5% of runs"
status: open
kind: defect
last_verified_commit: adb822b6
criteria:
  - id: c1
    text: "The mechanism is named in code: what makes the ERROR not reach the scoped subscriber. Not inferred -- two inferences have already failed"
    state: met
    evidence: "symbol:crates/wcore-tools/src/osv_check.rs::pin_callsite_interest"
    owner: core
    note: "NAMED, AND PROVEN RATHER THAN INFERRED. `tracing` caches one `Interest` per callsite, globally, computed exactly once -- the first time that line executes anywhere in the process -- and WHICH subscriber it is computed against depends on the REGISTERING thread, not on the thread that will read the events. tracing-core 0.1.36 `callsite.rs`: `Dispatchers::rebuilder()` returns `Rebuilder::JustOne` whenever at most one dispatcher is registered (the normal state of a test binary with a single scoped subscriber), and `Rebuilder::for_each` then resolves `dispatcher::get_default(f)` -- the CALLING thread's default. A thread with no subscriber that reaches the callsite first makes `rebuild_callsite_interest` collect nothing, `interest.unwrap_or_else(Interest::never)` caches NEVER, and that `tracing::error!` becomes a no-op for every thread in the process, including one holding a scoped subscriber. The event is not filtered, not routed elsewhere, not late: it is never constructed. The poisoner in this binary is `osv_check::tests::check_refuses_unsafe_endpoint` (osv_check.rs:1066), which drives the same SSRF `error!` at osv_check.rs:747 and installs no subscriber. A scoped `set_default` rebuilds only callsites ALREADY registered, so a registration that lands after it is never healed -- which is exactly why both previously refuted hypotheses had to fail: #1 serialised the two SUBSCRIBER-INSTALLING tests, and the poisoner installs none, so it was never in the group; #2 rebuilt the interest cache immediately after `set_default`, and the poisoning registration lands after that rebuild, on a callsite that did not yet exist to be rebuilt. Both refutations are corroboration, not counter-evidence."
  - id: c2
    text: "A red arm is quoted verbatim from a real run, and the rate is measured at n>=100 on the same instrument as the fix arm"
    state: met
    evidence: "test:crates/wcore-tools/src/osv_check.rs::ssrf_refusal_is_visible_at_default_log_levels"
    owner: core
    note: "Red arm verbatim, BASE arm of the interleaved A/B below, hetzner-dsm, load average ~165-215: `thread 'osv_check::tests::ssrf_refusal_is_visible_at_default_log_levels' (4069575) panicked at crates/wcore-tools/src/osv_check.rs:1356:9: / assertion `left == right` failed / left: [] / right: [Level(Error)]`. DIAGNOSTIC RUN THAT DISCRIMINATED THE MECHANISM, also verbatim and also from a natural failure under the same load: with the test instrumented to emit its own ERROR probe first and to print the global level filter, it failed as `DIAG probe_seen=1 max_before_set=LevelFilter::OFF max_after_set=LevelFilter::TRACE max_at_emit=LevelFilter::TRACE levels=[Level(Error)] / left: 1 / right: 2` -- the scoped subscriber WAS installed and working (it captured the probe), and the global max level WAS permissive at the moment of the emit, so the loss is specific to the osv callsite. That is what rules out the subscriber and the level filter and leaves the interest cache."
  - id: c3
    text: "The fix arm scores 0 failures at n>=100 on that same instrument, and the baseline is re-measured at the same n in the same session"
    state: met
    evidence: "test:crates/wcore-tools/src/osv_check.rs::a_callsite_first_registered_by_a_subscriberless_thread_stays_visible"
    owner: core
    note: "INTERLEAVED, SAME SESSION, SAME INSTRUMENT SHAPE. Two snapshot binaries built from the same worktree and identified before use -- BASE `--list` shows 1294 tests and no `subscriberless` arm, FIX shows 1295 and one; distinct md5s -- and executed strictly alternating BASE,FIX,BASE,FIX so both arms saw the same host load. n=110 per arm on hetzner-dsm: BASE 1 failures of the target test, FIX 0. Whole-binary runs, `--quiet`, the same shared-process shape the CI leg uses. UNDERPOWERED ON ITS OWN AND SAID SO: at the historical ~5/100 rate, 110 samples expects about 5 events, so `1 vs 0` is suggestive, not decisive. What carries this row is the DETERMINISTIC control, not the rate: the shipped guard `a_callsite_first_registered_by_a_subscriberless_thread_stays_visible` is 5 of 5 RED with the pin removed and 5 of 5 GREEN with it, run alone on the same binary, in the same session. Both loops carried a liveness check (the binary must be executable and must print a `test result:` line) because a previous measurement in this repo scored 100/100 against a path that had stopped existing."
  - id: c4
    text: "Both osv_check log-visibility tests keep their exact-equality assertion on [Level(Error)]"
    state: met
    evidence: "test:crates/wcore-tools/src/osv_check.rs::fail_open_is_visible_at_default_log_levels"
    owner: core
    note: "Structurally guaranteed, not merely asserted: `git diff` for `crates/wcore-tools/src/osv_check.rs` at this commit is 134 insertions and ZERO deletions, so neither assertion, nor its message, nor any attribute on either test was touched. No `#[serial]`, no `#[ignore]`, no relaxation to `contains`, no nextest-only move. The fix is one added call at the top of each test plus a helper and a guard test."
  - id: c5
    text: "cargo test --workspace --lib --no-fail-fast passes N>=10 consecutive times on hetzner-dsm, run count recorded"
    state: not-met
    owner: core
    note: "BLOCKED ON AN UNMERGED LANE, not on this defect. Handed over intact from core#361 c6. The three other shared-process races that stand in the way of a clean run are fixed on `lane/f13-fin-flake-584` (`2c8efb46` wcore-config, `812e26075` wcore-observability, `18e59e85f` wcore-cli) and NONE of them is in `origin/integ/f13`, so the ten consecutive clean runs are unreachable from this lane's tree no matter what happens to the osv defect. MEASURED HERE RATHER THAN ASSUMED, and the measurement names the blockers: three consecutive `cargo test --workspace --lib --no-fail-fast` runs on this lane's tree (integ/f13 + this lane's two commits), hetzner-dsm, load average ~170-210 -- run 1 rc=0, run 2 rc=101, run 3 rc=101. Best streak 1, which is the same streak core#373 was filed with. Run 2 reddened on `wcore-observability trace::tests::with_result_snippet_truncates_at_utf8_boundary` and `wcore-agent spawner::spawn_task_set_tests::parallel_spawn_caps_active_child_engines_across_shared_calls`; run 3 on `wcore-cli doctor::tests::which_returns_some_for_known_binary`. The first and third are exactly the races `lane/f13-fin-flake-584` closed in `812e26075` and `18e59e85f`, neither of which is in `origin/integ/f13`. `osv_check::tests::ssrf_refusal_is_visible_at_default_log_levels` -- this issue's own defect -- appeared in NONE of the three. (A deliberately-failing nested fixture crate, `failing_fixture` / `always_fails`, prints a FAILED block into every run's log and is not a failure of the suite; the run's exit code is the signal, not the count of FAILED blocks.) This row can only be closed on the integrated tree, by whichever lane runs last."
---

Split out of core#361 c6 so the remainder has an owner and a contract instead of
disappearing into a "partial". core#361's own defect -- a greedy PII pattern
reaching out of an approval token and collapsing the payload under the
truncation cap -- is closed on `lane/f13-fin-flake-584`.

This file was written by `lane/f13-u-flake-chan`, which found the mechanism, and
it deliberately keeps the contract text authored with the issue. If
`lane/f13-fin-flake-584` also carries a copy of this file, the two are
complementary -- that lane owns the contract and the baseline, this lane owns
c1-c4 -- and they must be merged rather than chosen between.

## Why the two earlier hypotheses could not have worked

Both were measured against the 5/100 baseline and both came back
indistinguishable from it, and the mechanism explains why:

* Serialising the two subscriber-installing tests could not help, because the
  thread that poisons the callsite installs no subscriber at all
  (`check_refuses_unsafe_endpoint`). A `#[serial]` group orders only the tests
  that carry the attribute; an unmarked reader -- or here, an unmarked WRITER of
  the interest cache -- is not in it.
* `rebuild_interest_cache()` after `set_default` could not help, because it
  rebuilds the callsites that are already registered, and the poisoning
  registration is the FIRST registration of that callsite. There was nothing
  there to rebuild.

## What was rejected, and why

* Anything that serialises, isolates, ignores or relaxes either visibility
  assertion. Refused by c4: that assertion is the security-visibility invariant
  for a fail-open, and at `RUST_LOG` unset ERROR is the only level the operator
  sees.
* `taskset` core-constraining, carried over from core#361: it produces a
  DIFFERENT failure (a stack overflow in
  `concurrent_near_cap_admits_exactly_one_retained_workspace`) and measures the
  instrument rather than the subject.

## The class, sized rather than asserted

Four sites in the workspace install a scoped subscriber and assert on what it
captured. One is live and is fixed here; the other three are latent, and each is
protected only by an accident that a later edit can remove:

* `crates/wcore-tools/src/osv_check.rs` -- LIVE, fixed. A sibling test in the
  same binary reaches the same callsite with no subscriber.
* `crates/wcore-mcp/tests/mcp_launch_malware_gate.rs:277` -- same shape (the
  poisoner would be `an_unreachable_osv_endpoint_fails_open` at :219, driving
  the same fail-open `error!`), but ALL 15 tests in that binary carry
  `#[serial(osv_gate)]`, so no thread can register the callsite while the
  asserting test holds its subscriber, and the asserting test's own
  `Dispatch::new` heals anything registered before it. Protected by a serial
  group that exists for the OSV backend global, not for this. MEASURED, not
  reasoned: 60 consecutive runs of that test binary on hetzner-dsm under the
  same load, 0 failures.
* `crates/wcore-agent/tests/plugin_namespace_claim_delivery.rs:87` -- both tests
  in that binary reach the callsites through `boot_browser_and_cua`, which
  always installs the subscriber first, so there is no subscriber-less
  registering thread.
* `crates/wayland-ijfw/src/mcp.rs:498` -- `notify_server_unreachable` is
  production code reached from `mcp.rs:203`; no other test in that crate was
  found driving that path without a subscriber.

Not changed in the three latent sites, deliberately: the fix cannot be shared as
a `#[cfg(test)]` helper across crates, and the obvious shared home
(`wcore-observability`) is not a dependency of `wcore-mcp`, while
`wayland-ijfw` may take NO `wcore-*` dependency at all under the REV-2 audit F2
invariant recorded in its own `Cargo.toml`. Making that a public API is a
dependency-graph decision, and taking it late in a release gate to protect three
sites that are currently not failing is the wrong trade. It is written down here
so the next edit to any of those binaries has the reason in front of it.
