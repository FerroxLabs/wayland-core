---
issue: 358
repo: FerroxLabs/wayland-core
kind: defect
title: "OwnedTree owns only the LEAF on Windows: the grandchild case #1156 was filed about is still open on all 49 swept sites"
status: open
last_verified_commit: bb850cc5
criteria:
  - id: c1
    text: "OwnedTree kills the process TREE on Windows, not just the direct child"
    state: met
    evidence: "symbol:crates/wcore-cli/tests/support/owned_tree.rs::own_windows_tree"
    owner: core
    note: "OwnedTree::new now assigns its child to a fresh kill-on-close Job Object on Windows (own_windows_tree -> wcore_types::job_object::WindowsJobObject::attach_running), and reap() calls TerminateJobObject on it; every one of the 49 swept sites gains this with no call-site edit because it hangs off `new`. The Job Object is the primitive the Windows sandbox and the MCP stdio transport already use -- no second mechanism was invented. The lying stubs are gone rather than left in place: the cfg(windows) child_pids that returned Vec::new() and the cfg(windows) sigkill that did nothing are deleted, and descendants()/the `known` pid list are now cfg(unix)-only concepts. Failure to create the job PANICS -- no fallback, matching the Linux arm that refuses to degrade to pgrep. The one window it does NOT close, stated at attach_running and in the guard module docs: `new` is handed an ALREADY-RUNNING child and the kernel puts only a process's FUTURE descendants into a job, so anything spawned between CreateProcess returning and the assignment landing stays outside it; WindowsJobObject::attach (CREATE_SUSPENDED) is the race-free constructor for a caller that holds the Command, and no site needs it today so none was added speculatively. Compiles for Windows: clippy --target x86_64-pc-windows-gnu -p wcore-cli --all-targets -- -D warnings exits 0 (see c6 for why that is not the msvc run)."
  - id: c2
    text: "A test grades the grandchild case ON WINDOWS: a direct child with a detached grandchild, guard dropped while unwinding, both gone afterwards"
    state: met
    evidence: "test:crates/wcore-cli/tests/harness_owns_spawned_trees_windows.rs::dropping_the_guard_kills_a_detached_grandchild_on_windows"
    owner: core
    note: "EXECUTED ON WINDOWS 2026-08-29. Host SEANDESKTOP (Windows 11 build 26200), toolchain 1.95.0-x86_64-pc-windows-msvc, cargo-nextest 0.9.138, tree at ab6b602f in D:\\wf13w. `cargo nextest run -p wcore-cli --test harness_owns_spawned_trees_windows` -> `PASS [   0.361s] (5/5) wcore-cli::harness_owns_spawned_trees_windows dropping_the_guard_kills_a_detached_grandchild_on_windows`, `Summary [   0.362s] 5 tests run: 5 passed, 0 skipped` (the other four are the mock-LLM support self-tests the shared `support/mod.rs` brings in). NOT VACUOUS, and that is graded rather than argued: the run reached the kernel-side anti-vacuity assertions before anything was killed - both pids alive, `WindowsJobObject::contains(grandchild)` true over IsProcessInJob, and the test runner asserted NOT in the job - and c3 records the red arm in which this same test fails on the grandchild assertion alone. Re-measured green AFTER the c3 red arm was reverted and both files touched: `PASS [   0.149s] 1 test run: 1 passed, 4 skipped`, so the green is a reading of the restored tree and not of a stale binary."
  - id: c3
    text: "The red arm is quoted VERBATIM from a real Windows run, showing the grandchild surviving before the change"
    state: met
    evidence: "test:crates/wcore-cli/tests/harness_owns_spawned_trees_windows.rs::dropping_the_guard_kills_a_detached_grandchild_on_windows"
    owner: core
    note: "OBSERVED ON WINDOWS 2026-08-29, SEANDESKTOP, same host and tree as c2. HOW THE ARM WAS BUILT, because the recipe this file previously carried does not compile: restoring `git show integ/f13-base:crates/wcore-cli/tests/support/owned_tree.rs` alone fails with `error[E0599]: no method named `job` found for struct `OwnedTree<std::process::Child>`` - the new test's kernel-side anti-vacuity block calls `guard.job()`, which the pre-fix guard does not have. So the red arm is the pre-fix guard (`job_object` occurrences in that file: 0) PLUS the removal of that one anti-vacuity block from the test, which is a property of the new mechanism and cannot exist before it; every other assertion is verbatim. Both files were touched after the swap so cargo could not measure the old binary. RED ARM, VERBATIM: `thread 'dropping_the_guard_kills_a_detached_grandchild_on_windows' (45440) panicked at crates\\wcore-cli\\tests\\harness_owns_spawned_trees_windows.rs:80:9: / deliberate panic with the tree still running` then `thread 'dropping_the_guard_kills_a_detached_grandchild_on_windows' (45440) panicked at crates\\wcore-cli\\tests\\harness_owns_spawned_trees_windows.rs:97:5: / the grandchild 33688 outlived the guard - on Windows killing the direct child does not reach a descendant, so without a Job Object the guard owns the leaf and leaks the TREE (FerroxLabs/wayland-core#358)` / `FAIL [  10.169s] (1/1)` / `Summary [  10.170s] 1 test run: 0 passed, 1 failed, 4 skipped`. WHICH ASSERTION FIRED IS THE POINT: the DIRECT-CHILD assertion did not fire, only the grandchild one - the leaf was always killed and the TREE was always leaked, which is precisely the defect #358 names. The 10.169s red against the 0.149s green is the `await_gone` budget expiring, so the two arms are also distinguishable by wall clock. Reverted with `git checkout --` on both files, both touched, and the green re-measured (see c2)."
  - id: c4
    text: "A negative control passes in both arms, so a change that kills too much fails here"
    state: met
    evidence: "test:crates/wcore-cli/tests/harness_guard_kills_only_its_own_tree.rs::dropping_one_guard_leaves_a_sibling_guards_tree_and_the_runner_alive"
    owner: core
    note: "Cross-platform on purpose, so it is graded on the platform where an over-broad walk is reachable TODAY (Unix has had a descendant walk all along) and not only on the platform being changed. Two guarded trees, each parent with a detached grandchild; one guard is dropped and the other tree plus the runner must be untouched. It asserts nothing about what the guard DOES reach, which is what lets it pass in the pre-fix arm. BOTH ARMS MEASURED ON LINUX (hetzner /root/w-f13/win-owned-tree): post-fix guard -> PASS [0.289s] 1 test run: 1 passed; and with `git show integ/f13-base:.../support/owned_tree.rs` restored over the fixed one (the leaf-only Windows guard; job_object occurrences in that file: 0) -> PASS [0.264s] 1 test run: 1 passed. NOT VACUOUS: mutating descendants(root) to start one level too high (walk from the child's PPid so the guard reaps its siblings, bounded to this test binary's own children) reddens it, verbatim -- thread 'dropping_one_guard_leaves_a_sibling_guards_tree_and_the_runner_alive' (2978308) panicked at crates/wcore-cli/tests/harness_guard_kills_only_its_own_tree.rs:99:5: dropping one guard killed a DIFFERENT guard's direct child (2978311) - the guard is reaping outside its own tree (FerroxLabs/wayland-core#358 c4) / TRY 1 FAIL [0.633s] -- and the retry tripped the grandchild assertion at line 105 instead, so both halves of the control fire. The mutation was reverted and the file touched before the green arm was re-measured. What it still does not grade: the Windows-specific over-kill shape (a job reaching beyond the tree it owns); the test runs there and asserts it, but like c2 it has not been executed on Windows."
  - id: c5
    text: "The CI run that executed the Windows arm is cited by URL"
    state: met
    evidence: "test:crates/wcore-cli/tests/harness_owns_spawned_trees_windows.rs::dropping_the_guard_kills_a_detached_grandchild_on_windows"
    owner: core
    note: "https://github.com/FerroxLabs/wayland-core/actions/runs/33258852685 - job 99117201158, `CI (windows-latest, hosted)`, step `Run tests (nextest CI profile)`, on lane/f13-fin-windows-runs at bd184563. THE ARM EXECUTED AND PASSED, quoted from that job's log: `PASS [   0.406s] ( 7619/15962) wcore-cli::harness_owns_spawned_trees_windows dropping_the_guard_kills_a_detached_grandchild_on_windows`, with the four support::mock_llm self-tests in the same binary also PASS. `7619/15962` is the point: it ran as part of the ordinary workspace nextest, not a hand-picked invocation. THE JOB'S OVERALL CONCLUSION IS `failure` AND THAT IS NOT THIS ARM - stated plainly rather than left for a reader to trip over. Nine tests failed in that leg, all pre-existing at the branch point (this branch changed ZERO Rust code relative to ab6b602f - `git diff ab6b602f..HEAD -- \"*.rs\"` has no non-doc-comment line): two were desktop-contract-corpus staleness, since fixed on integ/f13 by e1f151a52 and green after merging it; one is FerroxLabs/wayland-core#374; three are FerroxLabs/wayland-core#387, the wl#1164 bash-resolution regression this same session found and A/B-proved. A criterion asking for the URL of the run that executed the arm is answered by the arm's own line, and the arm passed. Corroborated independently on hardware: c2 records the same test passing on a Windows 11 build 26200 workstation, and c3 the red arm on the same box."
  - id: c6
    text: "clippy --target x86_64-pc-windows-msvc -p wcore-cli --all-targets -D warnings is clean"
    state: met
    evidence: "symbol:crates/wcore-cli/tests/support/owned_tree.rs::own_windows_tree"
    owner: core
    note: "RUN ON REAL MSVC 2026-08-29, SEANDESKTOP, tree at ab6b602f: `cargo clippy --target x86_64-pc-windows-msvc -p wcore-cli --all-targets -- -D warnings` -> EXIT=0, no error and no warning line in the log. This is the run the previous note said -gnu could not substitute for, on the toolchain that actually ships (1.95.0-x86_64-pc-windows-msvc): the real ABI, the real target_env, the real windows-sys import libraries, against the unsafe FFI added for the Job Object. The aws-lc-sys cross-compile failure recorded here was an artefact of cross-compiling from Linux and does not arise natively. -gnu stays green too and is kept in the lane gate for early signal; it is no longer load-bearing for this criterion."
---

The fourth of `#352`'s four asks, split out because it is the only one that
cannot be executed from the Linux build host: it is not a test change but a new
platform capability plus a dependency, in `unsafe` FFI, iterable only through a
`[ci-windows]` push.

On Windows `OwnedTree::reap()` snapshotted an empty descendant set, killed the
direct child and reaped it — so the grandchild case `#1156` was filed about was
still open there, on every swept site at once.

The contract is that the GUARD owns the tree, not the call sites, and that there
is no silent fallback: the Windows arm must be as loud about what it cannot do as
the Linux arm is. Deleting the Windows arm and having `OwnedTree` refuse to
compile there is an acceptable outcome — an honest "not supported" beats a guard
that looks present and owns nothing.

## State after lane `lane/f13-win-owned-tree`

The mechanism is built and the tests are written; what is missing is a Windows
host to run them on. `c1` and `c4` are closed with evidence measured on Linux
(`c1`: the code plus a Windows-target clippy; `c4`: both arms plus a reddening
mutation). `c2`, `c3`, `c5` and `c6` all reduce to the SAME missing run and
should be settled together:

```
# on a Windows host, from this branch
cargo clippy --target x86_64-pc-windows-msvc -p wcore-cli --all-targets -- -D warnings
cargo nextest run -p wcore-cli --test harness_owns_spawned_trees_windows

# then, for c3, restore the pre-fix guard and repeat the nextest run
git show integ/f13-base:crates/wcore-cli/tests/support/owned_tree.rs \
    > crates/wcore-cli/tests/support/owned_tree.rs
touch crates/wcore-cli/tests/support/owned_tree.rs   # else cargo measures the OLD binary
```
