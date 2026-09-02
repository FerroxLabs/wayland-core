---
issue: 362
repo: FerroxLabs/wayland-core
kind: defect
title: "bwrap backend: sandbox process-tree ownership races with ENOENT, and a containment test retries into a pass having never run its probe"
status: closed
last_verified_commit: 93ede3424
criteria:
  - id: c1
    text: "The ENOENT is traced to a named path and a named window: what is resolved, what opens it, and what removes it in between"
    state: met
    evidence: "symbol:crates/wcore-sandbox/src/backends/process_tree.rs::from_observed_root"
    owner: core
    note: "GRADED AGAINST THE TREE 2026-08-31 (the row was seeded not-met by the 0.13.12 bookkeeping pass and never looked at). NAMED, and every hop re-derived by grep rather than taken from the commit message. RESOLVED: `read_bwrap_child_pid` (crates/wcore-sandbox/src/backends/bwrap.rs:931) parses bubblewrap's `--json-status-fd` line and yields `child-pid`. OPENED: `ProcessTreeGuard::from_observed_root(child_pid)` at bwrap.rs:693 -> `LinuxProcessIdentity::open` (process_tree.rs:1245) -> `linux_process_start_time(pid)`, which reads `/proc/<child-pid>/stat`. That read is the ENOENT; it is the FIRST fallible call in `open`, before `pidfd_open`, so `os error 2` from this path can only be that file. REMOVES IT IN BETWEEN: bubblewrap itself, which reaps its own sandboxed PID-namespace init the instant the command exits. The error string is unique to this site -- `grep -rn 'sandbox process-tree ownership' crates/` returns bwrap.rs:693 as the only producer (bwrap.rs:681 and no_sandbox.rs are the differently-worded `process-tree ownership:` on the DIRECT child, which cannot be absent because the caller has not reaped it). WHY THIS ONE CALL SITE AND NOT `new`: `ProcessTreeGuard::new` opens a direct child the caller has not reaped, and a zombie keeps its `stat` file; `from_observed_root` opens SOMEBODY ELSE'S child."
  - id: c2
    text: "It is established BY MEASUREMENT whether the race reproduces on a plain Linux host with the sandbox enabled, or only under CI's nested-bwrap-in-docker shape; severity follows from this"
    state: met
    evidence: "file:.planning/evidence/f13-sandbox/362-WINDOW-AND-RATE.md:46:It reaches a plain Linux host"
    owner: core
    note: "MEASURED IN BOTH SHAPES, ON THE PRODUCT CODE AS IT WAS BEFORE THE FIX (`git checkout f07482c80^ -- backends/process_tree.rs backends/bwrap.rs`; `cargo check -p wcore-sandbox --tests` RC=0 before any arm was believed). ANSWER: IT REACHES A PLAIN HOST. It is not specific to CI's nesting -- the mechanism is the same on both -- but CI's shape is ~5x more exposed. The window was measured by widening the gap between the status line and the /proc read with a temporary env-gated sleep at bwrap.rs:686, 5 executions per point:\n\n  plain host (hetzner-dsm, bwrap 0.9.0, no container)\n    0/25/50/75ms -> 0/5 failures;  100ms -> 2/5;  125/150/175/200ms -> 5/5\n  CI image (wayland-core-ci:rust-1.95-slim-bookworm, bwrap 0.8.0, DOCKER_RUN_SANDBOX grants)\n    0/3/5/10ms  -> 0/5 failures;   15ms -> 2/5;  20ms and above -> 5/5\n\nSo a ~100 ms scheduling stall loses the race on a plain Linux host and a ~15 ms one loses it in CI. SEVERITY FOLLOWS: a 100 ms stall is ordinary for an agent CLI running Bash on a loaded machine, so this is a product defect on real hosts and not a CI artefact.\n\nHONEST NEGATIVE RESULT, recorded rather than dropped: the race did NOT reproduce NATURALLY in any arm I could build. Unfixed tree, 0 occurrences in each of: 25 CI-image nextest runs of the two named tests; 5 CI-image whole-crate runs pinned to 2 CPUs; 240 executions of `bwrap_execute_echo_returns_exit_zero` pinned to ONE CPU on the plain host; 240 the same inside the CI image. All four arms carry a positive control that every execution actually ran the test -- an earlier version of the same harness was VACUOUS (`taskset -c 0` inside `--cpuset-cpus=3` failed and launched nothing, and 240 non-executions read as 240 clean passes). CPU starvation is not the trigger, and there is a reason: starving the CPU slows the sandboxed child by as much as it slows our read, so it widens both sides of the window at once."
  - id: c3
    text: "If it reaches a plain host, the ownership acquisition is made race-free and a red arm is quoted verbatim from before the fix"
    state: met
    evidence: "test:crates/wcore-sandbox/src/backends/process_tree.rs::an_observed_root_that_is_already_reaped_reports_nothing_to_own"
    owner: core
    note: "The fix is `f07482c80` on integ/f13 and predates this lane; what was missing was the grading, which is done here. `from_observed_root` now answers `Ok(None)` for ENOENT/ESRCH/recycle and STILL ERRORS on EPERM -- reading EPERM as absence is the fail-open direction. RED ARM, PLAIN HOST, PRODUCTION PATH, quoted verbatim (unfixed product code, 200 ms window, `target/debug/deps/backend_integration`):\n\n  thread 'bwrap_confines_filesystem_writes_outside_allowlist' panicked at crates/wcore-sandbox/src/test_support.rs:235:13:\n  the sandbox backend refused to run the containment probe, so no containment property was tested: ExecFailed(\"sandbox process-tree ownership: No such file or directory (os error 2)\")\n\n  thread 'bwrap_execute_echo_returns_exit_zero' panicked at crates/wcore-sandbox/tests/backend_integration.rs:210:10:\n  bwrap execute must succeed for a trivial command: ExecFailed(\"sandbox process-tree ownership: No such file or directory (os error 2)\")\n\n  test result: FAILED. 0 passed; 2 failed; 0 ignored; 0 measured; 6 filtered out\n\nThat is the CI report byte for byte, including both file:line anchors the ticket quotes. THE FIX IS THE ONLY VARIABLE: with the SAME injected window and only the product hunks restored, the same binary passes 0/5 failures at 0, 100, 125, 200, 500 and 2000 ms on the plain host, and at 0, 15, 20, 100, 500 and 2000 ms in the CI image. The instrument was removed and the tree verified clean (`git diff HEAD` empty, `grep -c WCORE_TEST_OWNERSHIP_DELAY_MS` = 0) before any green was recorded."
  - id: c4
    text: "bwrap_confines_filesystem_writes_outside_allowlist cannot retry into a pass having never run its probe: a backend that refuses to start the probe fails the run rather than being retried"
    state: met
    evidence: "test:crates/wcore-sandbox/tests/containment_probes_are_not_retryable.rs::every_containment_probe_binary_is_pinned_to_zero_retries"
    owner: core
    note: "`.config/nextest.toml` pins `binary(=backend_integration) + binary(=secret_read_deny)` to `retries = 0` under `[[profile.ci.overrides]]` -- the two binaries that drive `run_contained_probe`. GRADED ON THE BEHAVIOUR, not on the presence of the block. A probe refusal was FORCED (a panic carrying the real message inserted into `run_contained_probe`; `cargo check -p wcore-sandbox --tests` clean first) and the same test run under `--profile ci` with NO `--retries` override in both directions:\n\n  pin PRESENT:  FAIL [0.166s] (1/1) ... bwrap_confines_filesystem_writes_outside_allowlist\n                Summary [0.168s] 1 test run: 0 passed, 1 failed\n  pin REMOVED:  TRY 1 FAIL / TRY 2 FAIL / TRY 3 FAIL ... bwrap_confines_filesystem_writes_outside_allowlist\n                Summary [0.418s] 1 test run: 0 passed, 1 failed\n\nOne attempt with the pin, three without it. That is the criterion as written. THE MEMBERSHIP IS SCANNED, NOT MAINTAINED: with the pin removed the ratchet also reddens -- `test binaries [\"backend_integration\", \"secret_read_deny\"] run a containment probe and are NOT pinned to retries = 0` (containment_probes_are_not_retryable.rs:155) -- so a new probe binary cannot be added without one. The parser is profile-scoped and carries a discriminating control that a `[profile.default]` pin is not read as a CI pin."
  - id: c5
    text: "Measured at --retries 0 over N >= 20 on the CI image, with the rate recorded"
    state: met
    evidence: "file:.planning/evidence/f13-sandbox/362-WINDOW-AND-RATE.md:102:the two named tests, N=25"
    owner: core
    note: "RATE: 0 failures in 25 runs (0.0%) of `bwrap_execute_echo_returns_exit_zero + bwrap_confines_filesystem_writes_outside_allowlist`, `cargo nextest run -p wcore-sandbox --profile ci --retries 0`, in the CI image under the real `DOCKER_RUN_SANDBOX` grants (`--cap-add SYS_ADMIN`, seccomp/apparmor/systempaths unconfined, `WCORE_REQUIRE_ENFORCING_SANDBOX=1`). Whole crate in the same image and profile: `Summary [33.229s] 210 tests run: 210 passed, 11 skipped`. RE-RUN CLEAN AND SAID SO: a first N=25 pass returned 22/3, and all three failures were MY OWN red-arm mutation landing in the tree while the loop was still running -- the tree was verified at `git diff HEAD` empty and `git rev-parse HEAD` = ede0ceaca before the number above was taken."
---

Criteria are the ticket's own acceptance wording. All five graded against the tree
on 2026-08-31 by lane `sandbox` (branch `lane/f13-sandbox`); the rows had been
seeded `not-met` by the 0.13.12 bookkeeping pass with nothing measured.

The product fix for c1-c3 is `f07482c80`, already on `integ/f13`. This lane did
not write it; it established what nobody had: that the race reaches a PLAIN Linux
host (c2), with the window measured in both shapes, and quoted the red arm
verbatim from the pre-fix product code (c3). c4's `retries = 0` pin and its
ratchet are this lane's, as is the two-arm proof that the pin changes the retry
behaviour and not just the config file.

THE INTERIM ALLOWLIST ENTRIES DO NOT EXIST, checked rather than assumed. The
ticket says both tests "carry a SHORT-DATED allowlist entry ... delete them when
c3/c4 land". They are not in `.config/flaky-allowlist.txt`, and
`git log -S bwrap_confines_filesystem_writes_outside_allowlist -- .config/flaky-allowlist.txt`
returns nothing, so no commit ever added them there. There is nothing to delete;
the eight entries that file does carry are other issues' debt and were left
alone.
