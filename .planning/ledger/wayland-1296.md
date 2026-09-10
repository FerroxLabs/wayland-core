---
issue: 1296
repo: FerroxLabs/wayland
kind: defect
title: "wcore-eval-scenarios smoke: spawn returns ENOENT on a path discovery just proved exists (shared-process leg, main red)"
status: open
last_verified_commit: f6ef1f2df
criteria:
  - id: c1
    text: "`cargo test -p wcore-eval-scenarios --test smoke` passes in the shared-process integration leg, or the reason it cannot is understood and stated."
    state: met
    evidence: "test:crates/wcore-eval-scenarios/tests/smoke.rs::binary_discovery_honors_absolute_cargo_target_dir"
    owner: core
    note: "MET at f6ef1f2df, and BOTH clauses are answered because the criterion needs both. THE COMMAND IT NAMES PASSES: `cargo test -p wcore-eval-scenarios --test smoke` on the Linux proof host, 5 passed / 0 failed in 0.26s, at source f6ef1f2df with a clean committed tree -- binary_discovery_rejects_missing_override, binary_discovery_honors_absolute_cargo_target_dir, an_explicit_override_wins_over_the_target_dir, spawns_and_captures_help and hung_scenario_does_not_leak_pid. Those three discovery tests ARE the ENOENT fix and landed in 2347d8f9c, which c2 is already anchored to. THE LEG CLAUSE, stated rather than assumed: smoke is NOT a member of the shared-process integration leg. scripts/check-test-env-globals.py --shared-process-targets emits 73 targets, two of them from this same crate (runner_contracts, supply_chain_tamper_corpus), and smoke is not among them. THE EXCLUSION IS PRINCIPLED, NOT A QUIET DROP, and I checked that specifically because the issue title says the failure was IN that leg: the leg selects integration binaries that write a process global, since that is the hazard class it exists to exercise, and smoke.rs writes NONE -- grep for set_var/remove_var/env::set over the file returns zero hits, against runner_contracts which returns two. A binary that cannot manifest the class is not evidence about the class. WHAT THIS DOES NOT CLAIM: the pass is on the Hetzner Linux proof host rather than the containerised ci-linux image, and it is the target alone rather than the whole leg -- which is the correct scope precisely because smoke is not in that leg."
  - id: c2
    text: "It is known whether this failure is deterministic or a one-off."
    state: met
    evidence: "commit:2347d8f9c"
    owner: core
    note: "MET at 509f4426b. The question this criterion asks -- deterministic or one-off -- is now ANSWERED, and the answer is INTERMITTENT. Two observations on the same tree settle it: main at 6e4eca07 was GREEN on this leg in run 33637957153, and that same tree FAILED on PR #420 in run 33702060702. The earlier reading of `finished in 0.00s` as evidence of determinism is withdrawn in the same note -- it only means the spawn fails fast. The mechanism behind the intermittency was identified in the same pass and is what 2347d8f9c removes: two discovery tests mutated CARGO_TARGET_DIR and WCORE_EVAL_BIN, which are PROCESS globals, and the shared-process leg puts all four tests of the binary in one process, so a sibling could exec a path pointing into a TempDir that had already dropped. c1 is deliberately NOT graded met alongside this: the fix removes the only process global in the binary and therefore also removes the target from the shared-process leg, so a later green there could not distinguish a repair from the flake not recurring."
---

# An ENOENT on a path that had just been proved to exist

The interesting part is not that a spawn failed. It is that the test's own guard makes the
easy explanation impossible: `maybe_binary()` skips the test when discovery fails, so
reaching the panic means discovery returned a path and `exists()` was true for it. The
binary was also demonstrably built -- step 19 succeeded in the same job.

Not a release blocker: `release.yml` does not run `ci.yml`, and the release's own post-tag
smoke runs `--version` on each published archive on its native OS, which is a separate and
stronger check. Filed alongside #1295, which covers the other red on the same run and the
fact that `main` has been red for four commits.
