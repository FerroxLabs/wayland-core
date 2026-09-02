---
issue: 1296
repo: FerroxLabs/wayland
kind: defect
title: "wcore-eval-scenarios smoke: spawn returns ENOENT on a path discovery just proved exists (shared-process leg, main red)"
status: open
last_verified_commit: 93ede3424
criteria:
  - id: c1
    text: "`cargo test -p wcore-eval-scenarios --test smoke` passes in the shared-process integration leg, or the reason it cannot is understood and stated."
    state: not-met
    evidence: "file:crates/wcore-eval-scenarios/tests/smoke.rs"
    owner: core
    note: "OPEN and NOT YET REPRODUCED -- stated plainly so the analysis below is not read as a confirmed root cause. On main at 93ede3424, run 33581626099 job 100096935581 step 27: spawns_and_captures_help and hung_scenario_does_not_leak_pid both panic with Io(Os { code: 2, kind: NotFound }) at smoke.rs:54 and smoke.rs:103, while the two binary_discovery_* tests in the same file pass. TWO OBVIOUS READINGS ARE ALREADY REFUTED BY THE RUN. (1) 'the binary was never built' -- step 19 `Pre-build wcore-cli release binary` SUCCEEDED in the same job. (2) 'discovery failed' -- it cannot have: maybe_binary() (smoke.rs:36) returns None on a discovery error and the test RETURNS EARLY AND PASSES, so both panics are on the spawn line AFTER discovery returned Ok(path) and cand.exists() was true (runner.rs:247/255). An existing path that execs ENOENT is a narrow set: a missing ELF interpreter, or a path that stopped existing between the exists() probe and the spawn. Note discover_binary() derives the workspace root from env!(\"CARGO_MANIFEST_DIR\"), which is baked in at COMPILE time; this leg compiles inside the CI container at /work, so the candidate is /work/target/{release,debug}/wayland-core, and a compile and a spawn seeing different mounts is the shape that would produce exactly this. Reproducing needs a host-native `cargo build -p wcore-cli`; the release build I verified is under --target x86_64-unknown-linux-gnu, so target/release/ is empty there and discovery would SKIP rather than fail, which is why I did not reproduce it in passing."
  - id: c2
    text: "It is known whether this failure is deterministic or a one-off."
    state: not-met
    evidence: "file:crates/wcore-eval-scenarios/tests/smoke.rs"
    owner: core
    note: "OPEN. This is the first `CI (linux-containerized)` failure on main in this window -- the three prior main reds (b26e4058d, bc13e6e32, 20d990061) were `report` and `CI (macos-latest)`, so there is no second observation to compare against. Determinism must be established before any fix is designed, or the fix will be graded against a flake."
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
