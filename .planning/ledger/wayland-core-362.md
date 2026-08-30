---
issue: 362
repo: FerroxLabs/wayland-core
kind: defect
title: "bwrap backend: sandbox process-tree ownership races with ENOENT, and a containment test retries into a pass having never run its probe"
status: open
last_verified_commit: bc63e94ad
criteria:
  - id: c1
    text: "The ENOENT is traced to a named path and a named window: what is resolved, what opens it, and what removes it in between"
    state: met
    evidence: "symbol:crates/wcore-sandbox/src/backends/process_tree.rs::from_observed_root"
    owner: core
    note: "Resolved: the pid on bubblewrap's --json-status-fd line. Opened: /proc/<pid>/stat plus pidfd_open, inside LinuxProcessIdentity::open. Removed in between: bubblewrap itself, which reaps its PID-namespace init the instant it exits -- the observed root is somebody else's child, which is the one call site where the /proc guarantee ProcessTreeGuard::new relies on does not hold. Traced in the integration base by f07482c80, not by this lane; this lane graded it and measured the window."
  - id: c2
    text: "It is established BY MEASUREMENT whether the race reproduces on a plain Linux host with the sandbox enabled, or only under CI's nested-bwrap-in-docker shape; severity follows from this"
    state: met
    evidence: "file:.planning/F13-362-BWRAP-OWNERSHIP-RACE.md:80:produced zero occurrences"
    owner: core
    note: "MEASURED on hetzner-dsm, both shapes, both arms, binaries fingerprinted by sha256 so the arms are known to be different builds. Plain host pre-fix: 0 ENOENT in 240 starved executions plus 60 under 32 spinners saturating one core. CI image with the ci-linux job's own security-opts, pre-fix: 0 in 240 starved executions and 0 in 25 nextest runs at --retries 0. A direct probe of the deciding condition reported /proc/<child> PRESENT 120 of 120. The ticket's hypothesis is REFUTED: nesting is not the discriminator, scheduling pressure is -- the window only opens once the reading thread stalls 50-75 ms, measured by sweeping an injected stall. Severity: a real defect (a completed command returned as ExecFailed) whose natural rate outside a saturated small-core runner is below the resolution of ~600 executions."
  - id: c3
    text: "If it reaches a plain host, the ownership acquisition is made race-free and a red arm is quoted verbatim from before the fix"
    state: met
    evidence: "test:crates/wcore-sandbox/src/backends/process_tree.rs::an_observed_root_that_is_already_reaped_reports_nothing_to_own"
    owner: core
    note: "Fix in the integration base (f07482c80): from_observed_root answers Ok(None) for ENOENT/ESRCH/recycle and still errors on EPERM, because a dead PID-namespace init has already taken its subtree with it. RED ARM M1 replaced observed_root_is_gone's body with `false` -- pre-fix behaviour -- and reddened two unit tests directly; with the window held open at 200 ms it reproduced the CI panic byte for byte at both call sites (test_support.rs:235 and backend_integration.rs:210). The fixed tree passes the same 200 ms window. Verbatim text in .planning/F13-362-BWRAP-OWNERSHIP-RACE.md."
  - id: c4
    text: "bwrap_confines_filesystem_writes_outside_allowlist cannot retry into a pass having never run its probe: a backend that refuses to start the probe fails the run rather than being retried"
    state: met
    evidence: "test:crates/wcore-sandbox/tests/containment_probe_retries.rs::every_binary_that_can_report_a_vacuous_containment_probe_is_unretryable"
    owner: core
    note: "retries = 0 overrides for binary(=backend_integration) + binary(=secret_read_deny) in BOTH [profile.default] and [profile.ci] of .config/nextest.toml -- the two, and only two, binaries in the workspace that call run_contained_probe, the one function that turns a probe that never ran into a panic. VERIFIED ON THE CONDITION THAT DECIDES, not on the edit: a deliberately failing test in backend_integration runs ONE attempt under --profile ci with the override and is retried TRY 1/2/3 with it neutered. The enumeration is guarded rather than trusted: the anti-rot test derives the caller set from every crates/*/tests/*.rs in the workspace (test_support is pub and seven crates depend on wcore-sandbox), and a companion test refuses a caller in library source, which binary(=<stem>) could not name. No allowlist entry to delete: .config/flaky-allowlist.txt carries no 362 and neither test name."
  - id: c5
    text: "Measured at --retries 0 over N >= 20 on the CI image, with the rate recorded"
    state: met
    evidence: "file:.planning/F13-362-BWRAP-OWNERSHIP-RACE.md:121:25 runs, 25 passed, 0 failed, 0 ENOENT"
    owner: core
    note: "CI image rebuilt from the ci.yml ci-linux Dockerfile verbatim and run under that job's own docker flags (--cap-add SYS_ADMIN, seccomp/apparmor/systempaths unconfined, WCORE_REQUIRE_ENFORCING_SANDBOX=1). 25 runs of `cargo nextest run -p wcore-sandbox -E 'binary(=backend_integration) and test(/^bwrap_/)' --retries 0`: 25 passed, 0 failed, rate 0/25. The pre-fix arm scored 0/25 on the same image, which is why the rate alone does not settle severity and c2 carries the answer."
---

The fix for the race itself arrived in the integration base as `f07482c80`; this lane
graded it against HEAD rather than trusting the ledger, and supplied what the base did
not: the width of the window, the natural rate on both shapes, and the unretryability of
a vacuous containment result.

Closing the GitHub issue is Sean's action, not a lane's.
