---
issue: 1296
repo: FerroxLabs/wayland
kind: defect
title: "wcore-eval-scenarios smoke: spawn returns ENOENT on a path discovery just proved exists (shared-process leg, main red)"
status: open
last_verified_commit: 509f4426b
criteria:
  - id: c1
    text: "`cargo test -p wcore-eval-scenarios --test smoke` passes in the shared-process integration leg, or the reason it cannot is understood and stated."
    state: not-met
    evidence: "file:crates/wcore-eval-scenarios/tests/smoke.rs"
    owner: core
    note: "MECHANISM IDENTIFIED 2026-09-03, CAUSE NOT PROVEN -- both halves stated so neither is overread. WHAT IS NOW MEASURED: (a) the SECOND obvious reading above is refuted the other way round -- discovery did not merely succeed, it was STEERED. smoke.rs mutates CARGO_TARGET_DIR and WCORE_EVAL_BIN, which are PROCESS globals, and a plain `cargo test` run puts all four tests of this binary in one process; binary_discovery_honors_absolute_cargo_target_dir points CARGO_TARGET_DIR at a TempDir holding an EMPTY file named wayland-core, cand.exists() is true of it, and the TempDir is gone by the time a racing sibling reaches exec -- an existing path that execs ENOENT, which is one of the two candidates the original analysis named. (b) The two discovery tests were already in a serial_test group WITH EACH OTHER; the two spawn tests were not, and they call the same function. (c) It is INTERMITTENT, not deterministic: main at 6e4eca07 is GREEN on this leg (run 33637957153), and the same tree failed on PR #420 (run 33702060702). The earlier reading of `finished in 0.00s` as determinism was wrong -- it only means the spawn fails fast. (d) Same container, same flags, same binary: the smoke tests PASS under nextest (process-per-test) in step 23 and fail in the shared process in step 27, which is what attributes this to the process-global class rather than to the artifact or the image. WHAT IS NOT PROVEN: I did not reproduce it. 0/10 unforced on a 96-core host, 0/30 unforced pinned to 2 CPUs, and two forced-window attempts did not land either (the spawn tests call discovery as their first statement and essentially always win the race). The fix removes the only process global in the binary, so it removes the only mechanism by which the shared-process leg can differ from the nextest leg -- but it is hardening on an unproven cause, not a demonstrated repair, and c1 must not be graded met on a green that could equally be the flake not recurring. MEASURED SIDE EFFECT, recorded because the same dynamic hid an unfixed defect in wayland#1250: the shared-process selector is DERIVED from which binaries touch process globals, so this drops `wcore-eval-scenarios smoke` from that leg (73 -> 72 targets, floor 60). Here the removal is correct -- the selector drops it BECAUSE the binary no longer has a global for the leg to catch -- but it does mean a recurrence would surface in the nextest leg only. ORIGINAL NOTE: OPEN and NOT YET REPRODUCED -- stated plainly so the analysis below is not read as a confirmed root cause. On main at 93ede3424, run 33581626099 job 100096935581 step 27: spawns_and_captures_help and hung_scenario_does_not_leak_pid both panic with Io(Os { code: 2, kind: NotFound }) at smoke.rs:54 and smoke.rs:103, while the two binary_discovery_* tests in the same file pass. TWO OBVIOUS READINGS ARE ALREADY REFUTED BY THE RUN. (1) 'the binary was never built' -- step 19 `Pre-build wcore-cli release binary` SUCCEEDED in the same job. (2) 'discovery failed' -- it cannot have: maybe_binary() (smoke.rs:36) returns None on a discovery error and the test RETURNS EARLY AND PASSES, so both panics are on the spawn line AFTER discovery returned Ok(path) and cand.exists() was true (runner.rs:247/255). An existing path that execs ENOENT is a narrow set: a missing ELF interpreter, or a path that stopped existing between the exists() probe and the spawn. Note discover_binary() derives the workspace root from env!(\"CARGO_MANIFEST_DIR\"), which is baked in at COMPILE time; this leg compiles inside the CI container at /work, so the candidate is /work/target/{release,debug}/wayland-core, and a compile and a spawn seeing different mounts is the shape that would produce exactly this. Reproducing needs a host-native `cargo build -p wcore-cli`; the release build I verified is under --target x86_64-unknown-linux-gnu, so target/release/ is empty there and discovery would SKIP rather than fail, which is why I did not reproduce it in passing."
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
