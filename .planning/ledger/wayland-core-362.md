---
issue: 362
repo: FerroxLabs/wayland-core
kind: defect
title: "bwrap backend: sandbox process-tree ownership races with ENOENT, and a containment test retries into a pass having never run its probe"
status: open
last_verified_commit: 483a4dcaf
criteria:
  - id: c1
    text: "The ENOENT is traced to a named path and a named window: what is resolved, what opens it, and what removes it in between"
    state: met
    evidence: symbol:crates/wcore-sandbox/src/backends/process_tree.rs::from_observed_root
    owner: core
    note: "MET, and every field is named. RESOLVED BY: bwrap writes {'child-pid': N} to --json-status-fd and read_bwrap_child_pid (bwrap.rs:931) deserialises N. OPENED BY: from_observed_root -> LinuxProcessIdentity::open -> linux_process_start_time, which is std::fs::read_to_string(\"/proc/{pid}/stat\") at process_tree.rs:1498. THE NAMED PATH IS /proc/<child-pid>/stat. REMOVED BY: bubblewrap itself -- N is not our child but bwrap's sandboxed PID-namespace init (--unshare-all implies --unshare-pid), and bwrap waitpid()s and reaps it the instant it exits, which frees /proc/<N>. THE NAMED WINDOW: between that JSON line being written and linux_process_start_time reading the stat file. NOT ARGUED -- an instrument at exactly that point printed `C362-PROBE child_pid=587542 /proc/587542 exists=false` on every failing trial and on no passing one. Full trace: .planning/evidence/core-362/c1-c3-window.md."
  - id: c2
    text: "It is established BY MEASUREMENT whether the race reproduces on a plain Linux host with the sandbox enabled, or only under CI's nested-bwrap-in-docker shape; severity follows from this"
    state: met
    evidence: file:crates/wcore-sandbox/src/backends/process_tree.rs:132:HOW WIDE THE WINDOW HAS TO BE, AND ON WHICH SHAPE
    owner: core
    note: "MET BY MEASUREMENT ON BOTH SHAPES, and the verdict is IT REACHES A PLAIN LINUX HOST. n=10 per cell, pre-fix tree, --retries 0, injected window at the exact race point. plain host / CI image: 1000ms 10-10 / 10-10; 100ms 10-10 / 10-10; 50ms 0-10 / 10-10; 30ms 0-10 / 10-10; 10ms 0-10 / 7-10; 1ms 0-10 / -. So the code path is shape-INDEPENDENT (it fires on a plain host at a 100ms window) and the nested shape is susceptible at a window an order of magnitude narrower, which is why every observed natural occurrence is on the containerized leg. SEVERITY: a Linux user with the sandbox enabled can see a Bash command fail with `sandbox process-tree ownership: No such file or directory` at a rate this harness cannot resolve -- 0/25 natural bounds it under ~11% (one-sided 95%), which is not zero. CORRECTION MADE: the pre-existing doc claimed natural reproduction 'by pinning concurrent bwrap execs onto two CPUs'; run exactly as written that is 0/25 on BOTH shapes, and the source now says so. Confounds stated in the evidence file (bwrap 0.9.0 vs 0.8.0, nesting)."
  - id: c3
    text: "If it reaches a plain host, the ownership acquisition is made race-free and a red arm is quoted verbatim from before the fix"
    state: met
    evidence: test:crates/wcore-sandbox/src/backends/process_tree.rs::an_observed_root_that_is_already_reaped_reports_nothing_to_own
    owner: core
    note: "MET. The fix is `from_observed_root` answering Ok(None) for ENOENT/ESRCH/recycle -- landed in f07482c80, IN the base tree ca15a48bf but never graded until this lane. It is safe because the observed root is a PID-namespace init: a root that is gone has already taken its whole subtree with it. EPERM is still an error, so the fail-open direction is closed. RED ARM, quoted VERBATIM from before the fix, reproduced in this lane on a plain host (mutation compiles: cargo check -p wcore-sandbox --tests RC=0, so the red is behaviour not a build break): `thread 'bwrap_confines_filesystem_writes_outside_allowlist' panicked at crates/wcore-sandbox/src/test_support.rs:235:13: the sandbox backend refused to run the containment probe, so no containment property was tested: ExecFailed(\"sandbox process-tree ownership: No such file or directory (os error 2)\")` and `panicked at crates/wcore-sandbox/tests/backend_integration.rs:210:10: bwrap execute must succeed for a trivial command: ExecFailed(...)` -- both matching CI run 33240249894 including the source positions. GREEN ARM under the SAME injected condition (the probe still reported /proc/<pid> gone, so the condition was present and not merely absent): plain host 2/2 PASS; CI image 0/10 failed at 1000ms and 0/10 at 10ms, against 10/10 and 7/10 pre-fix."
  - id: c4
    text: "bwrap_confines_filesystem_writes_outside_allowlist cannot retry into a pass having never run its probe: a backend that refuses to start the probe fails the run rather than being retried"
    state: met
    evidence: test:crates/wcore-sandbox/tests/probe_retry_ratchet.rs::the_probe_binaries_cannot_be_retried_into_a_pass
    owner: core
    note: "MET. .config/nextest.toml now pins `binary(=backend_integration) + binary(=secret_read_deny)` -- the crate's whole run_contained_probe surface, MECHANISM not instance -- to retries = 0, so a backend that refuses to start the probe fails the RUN instead of being retried. MEASURED BOTH ARMS on hetzner-dsm at --profile ci with a probe that fails its first attempt only: WITHOUT the block `TRY 1 FAIL` / `TRY 2 PASS` / `2 tests run: 2 passed (1 flaky)` / cargo exit 0 -- laundered green; WITH it `FAIL` / `2 tests run: 1 passed, 1 failed` / cargo exit 100. WRONG-REFUSAL CONTROL in the same filterset in both arms: the real bwrap_confines_filesystem_writes_outside_allowlist PASSED both times, so the block makes an unstartable probe fail and does not refuse legitimate traffic. The ratchet test is the second half: an override is a list of names and cannot notice a third caller, so it fails if a run_contained_probe binary is not pinned -- proven red by dropping secret_read_deny from the filter (`NOT pinned to retries = 0 ...: [\"secret_read_deny\"]`), then restored. It also caught ITSELF on the first run, because it names the helper in a string literal in order to search for it; that exclusion is documented at the exclusion. Transcript: .planning/evidence/core-362/c4-retry-laundering.txt."
  - id: c5
    text: "Measured at --retries 0 over N >= 20 on the CI image, with the rate recorded"
    state: met
    evidence: file:.planning/evidence/core-362/c5-retries-0-rate.txt
    owner: core
    note: "MET. N=30 (>= the 20 asked) trials of both bwrap tests at --retries 0 on the CI image (rust:1.95-slim-bookworm + the workflow's package list, run under ci.yml's DOCKER_RUN_SANDBOX posture verbatim), trials and 8 concurrent bwrap noise loops pinned to CPUs 0,1. RATE: 0/30 failed, 0 with the ENOENT ownership race -- 0.0%, upper bound ~9.5% (one-sided 95%). THE ZERO IS INTERPRETABLE ONLY BECAUSE OF ITS CONTROL, and the control is in the same image: on the pre-fix tree with the window injected, that image fails 10/10 at 100ms and 7/10 at 10ms; on the fixed tree it fails 0/10 at both while the probe still reports /proc/<pid> gone. So the harness can see this race in this image and did not see it. CONFOUND, STATED: the image is the CI image but the box is hetzner-dsm (96 cores), not a GitHub 4-core runner, which is more contended -- so this bounds the rate on the image, not on the hosted runner."
---

Criteria are the ticket's own acceptance wording, transcribed so the release gate can count this work. Nothing has been done by the bookkeeping pass that created this file, and nothing here has been graded against the tree.
