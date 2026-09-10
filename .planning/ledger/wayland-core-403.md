---
issue: 403
repo: FerroxLabs/wayland-core
kind: defect
title: "The workspace --lib suite is not ten-times-clean: an ephemeral-port race and three single-sample wall-clock ratios (core#373 c5 remainder)"
status: open
last_verified_commit: b47c5013d
criteria:
  - id: c1
    text: "The three `bash::tests` ratio assertions no longer turn on a single wall-clock sample, and the `trusted_local` -> `contained` mutation still reds `a_workspace_that_does_not_walk_cancels_promptly_even_on_a_large_tree` with `cargo check` rc=0 recorded before the red is believed."
    state: met
    evidence: "test:crates/wcore-tools/src/bash/tests.rs::a_workspace_that_does_not_walk_cancels_promptly_even_on_a_large_tree"
    owner: core
    note: "NOT RE-SAMPLED -- RESTATED AS EVENTS, which is what the brief asked for and what #350's own remedy could not deliver here. THE REFUTED REMEDY IS NOT REPEATED: `min(LATENCY_SAMPLES)` silently gutted these assertions because `secret_deny_paths_for_backend` MEMOISES on the policy, so samples 2..n are free -- recorded in core#373 c5 and still true. WHAT THE THREE NOW ASSERT, all four sites: `a_cancelled_bash_does_not_wait_for_the_secret_deny_walk` and its streaming twin replace `elapsed * 3 < walk` with `policy.secret_deny_walk_count() == 0` -- the INJECTED counter #1111 acceptance 1 already asks this memo to be graded with -- and `a_workspace_that_does_not_walk_cancels_promptly_even_on_a_large_tree` replaces BOTH its ratios: the no-walk control becomes `walk_entries()` delta == 0, and the cancelled-exec ratio becomes `secret_deny_walk_count() == 1` (the direct call, and nothing else). No `Instant` remains in any of the three. PRECEDENT, not invention: `workspace_policy::tests::contained_construction_does_not_walk_the_workspace` took exactly this repair for exactly this reason (#1182), and its own doc-comment records the wall-clock control declaring ITSELF dead under load. EVERY ZERO HAS A KNOWN-POSITIVE CONTROL IN THE SAME RUN, because a counter nothing increments reads zero exactly like a walk that was escaped: `assert_uncancelled_exec_walks` drives the SAME policy through the SAME `BashTool` call path with the token not cancelled and requires the counter to move; the no-walk control first requires a contained walk of the same tree to enumerate it (50,507 entries, measured). THE REQUIRED MUTATION, on hetzner-dsm via tools/remote-proof.py slot parallel-2: `trusted_local` -> `contained` at bash/tests.rs:3132 (an expression, quoted back after the edit), commit fa2125855. `cargo check --locked -p wcore-tools --tests` rc=0 FIRST, then `cargo test --locked -p wcore-tools --lib -- cancels_promptly` rc=101: `trusted_local enumerated 50507 filesystem entries on the SAME tree a contained walk visits 50507 of -- expected no walk at all (deny list: 2 entries) / left: 50507 / right: 0`. Deterministic, not probabilistic. A SECOND RED ARM ON THE PRODUCT, because a mutation of a test double grades the double: the v0.13.4 shape was reproduced by paying `secret_deny_paths_for_backend` immediately BEFORE the `is_cancelled` early return in `execute_with_ctx` ONLY (commit 2ddbaf1cd, check rc=0 first). `a_cancelled_bash_does_not_wait_for_the_secret_deny_walk` went rc=101 `left: 1 / right: 0`; the streaming twin stayed GREEN, which is that arm's own site-specificity control. NOT CLAIMED: no single-site mutation of the SHIPPED code reaches the walk on a pre-cancelled call, because three independent guards short-circuit it (`bash.rs:919` early return, the unsaved-guard select, the build select). The arm above is a reproduction of the historical defect shape, not a mutation of a live line, and it is recorded as that. PASS-AFTER at b47c5013d: `cargo test --locked -p wcore-tools --lib` over the five #1111 latency tests, `test result: ok. 5 passed; 0 failed`, and `cargo clippy --locked -p wcore-tools --all-targets -- -D warnings` rc=0. PRIOR NOTE PRESERVED: TEXT RESTORED 2026-09-04 by the post-merge ledger sync -- all three criteria in this file had been truncated at the first line-wrap of the issue bullet, leaving fragments nobody could grade in either direction. Restored verbatim from the Acceptance section of FerroxLabs/wayland-core#403."
  - id: c2
    text: "The two carriers in `crates/wcore-tools/tests/` are either fixed in the same pass or named with a reason they are out of scope."
    state: met
    evidence: "test:crates/wcore-tools/tests/bash_manifest_bound_live_backend.rs::esc_during_the_live_backend_manifest_build_does_not_wait_for_the_walk"
    owner: core
    note: "FIXED IN THE SAME PASS, both files, all four sites -- none named out of scope. `bash_manifest_bound_live_backend.rs`: the two cancellation assertions (`esc_during_the_live_backend_manifest_build_does_not_wait_for_the_walk`, `a_non_walking_posture_on_the_same_tree_is_the_negative_control`) now read `secret_deny_walk_count()` with an uncancelled control on the same policy through the same live-backend path; `the_live_backend_timeout_bounds_the_manifest_build_and_names_it` drops `bounded * 3 < walk` AND the `timer_allowance` probe that fed it, and is graded by the message that NAMES the manifest build -- a string with exactly ONE producer, the `Err(_)` arm of `timeout_at(deadline, build)` in `bash.rs::execute_with_ctx`, reachable only with the build still outstanding. `bash_unsaved_guard_bound_live.rs`: `the_caller_budget_bounds_the_unsaved_work_guard` drops `elapsed * 2 < cost` for the `it was still running after 200 ms` clause, whose only producer is the `Err(_)` arm of `timeout(budget, task)` in `bounded_unsaved_shell_refusal`, where `budget` IS `timeout.min(UNSAVED_GUARD_BUDGET_MS)` -- the caller's budget, which is the whole claim. PASS-AFTER at b47c5013d on hetzner-dsm (live bwrap backend, `host backend = bubblewrap (enforces read-deny)`): 3 passed / 0 failed and 4 passed / 0 failed. RED ARMS, `cargo check --locked -p wcore-tools --tests` rc=0 recorded first on each mutated commit. (1) Commit f92580d44, the v0.13.4 shape (deny walk paid before cancel is honoured, buffered path): ALL THREE manifest tests red -- `Esc paid the deny walk (measured at 46.221661ms) on the live bubblewrap backend / left: 1 / right: 0`, the negative control likewise, and the TIMEOUT test red as `the caller was not told the workspace secret-scan ate the budget ... got: Command timed out after 4ms`. (2) Commit 17c305ba5, `budget = timeout.min(UNSAVED_GUARD_BUDGET_MS)` -> `budget = UNSAVED_GUARD_BUDGET_MS`: `the guard was not cut off by the caller's 200ms budget (it costs 2.610542238s when allowed to finish); got: Command timed out after 200ms`. THAT SECOND ARM IS THE ARGUMENT FOR THE CHANGE, not just its control: the ratio it replaced would have SURVIVED it -- the caller deadline still returns at 200ms against a 2.6s guard, so `elapsed * 2 < cost` still held while the guard was no longer bounded by the caller at all. The event assertion is strictly stronger than the ratio it replaced, not merely steadier. Both arms restored by `git reset --hard` and the tree re-verified green."
  - id: c3
    text: "`cargo test --workspace --lib --no-fail-fast` passes N>=10 CONSECUTIVE times on hetzner-dsm with the per-run rc, host load and `never executed` count recorded, and the host load range stated alongside the streak."
    state: not-met
    owner: core
    note: "PENDING"
---

Created 2026-08-31 to close a COVERAGE gap. It records no work as done.

`scripts/check-criteria-ledger.py` scopes every open `area:core` issue on
wayland and EVERY open issue on wayland-core. This issue was in scope from
the moment it was filed and had no ledger file, so
`scripts/check-release-readiness.py` -- which reads ledger files and nothing
else -- could not count it. CI runs the coverage gate with `--offline`, the
arm that would have reported the gap, so nothing said so for two days.

Criteria are transcribed from the issue body without edit. Where the body's
wording is loose it is LEFT loose rather than tightened here: sharpening a
criterion inside the ledger is how a criterion quietly becomes an easier
adjacent property. Whoever takes this restates it on the ISSUE first.

All three texts were TRUNCATED on that transcription -- each cut at the first
line-wrap of the issue bullet, leaving a fragment beginning with a colon. They
are restored verbatim on 2026-09-04, the same repair wayland-core#404 received
a day earlier for the same defect. Nothing else about this entry changed: all
three criteria remain not-met, and the restoration only makes them gradeable
at all.
