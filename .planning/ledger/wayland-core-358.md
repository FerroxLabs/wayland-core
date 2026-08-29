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
    note: "EXECUTED 2026-08-29 (lane f13-fin-hetzner-residuals) on a real Windows host, which is the one thing it was short of -- the test was authored, compiled and reviewed, and this entry correctly refused to grade it until someone watched it run. HOST: SeanDesktop, the only Windows machine, D:\\resid358 at lane commit d35ac0a0. RAN, the exact command this entry prescribed: `cargo nextest run -p wcore-cli --test harness_owns_spawned_trees_windows` -> `PASS [ 0.290s] (5/5) wcore-cli::harness_owns_spawned_trees_windows dropping_the_guard_kills_a_detached_grandchild_on_windows` / `Summary [ 0.291s] 5 tests run: 5 passed, 0 skipped`. The pass is not vacuous by construction and the run proves it: execution reached the ownership assertion, so the anti-vacuity precondition ahead of it held -- the KERNEL (WindowsJobObject::contains over IsProcessInJob) confirmed the grandchild was inside the guard\'s job and the test runner was NOT, before anything was killed. The red arm on c3 is the other half: the same binary, the same host, with only the job\'s killing withdrawn, fails at the grandchild assertion. Green was re-measured after that mutation was reverted and the file touched: `5 tests run: 5 passed`."
  - id: c3
    text: "The red arm is quoted VERBATIM from a real Windows run, showing the grandchild surviving before the change"
    state: met
    evidence: "symbol:crates/wcore-types/src/job_object.rs::WindowsJobObject"
    owner: core
    note: "OBSERVED 2026-08-29 (lane f13-fin-hetzner-residuals), same host and commit as c2. VERBATIM: `thread \'dropping_the_guard_kills_a_detached_grandchild_on_windows\' (33376) panicked at crates\\wcore-cli\\tests\\harness_owns_spawned_trees_windows.rs:109:5: / the grandchild 38828 outlived the guard -- on Windows killing the direct child does not reach a descendant, so without a Job Object the guard owns the leaf and leaks the TREE (FerroxLabs/wayland-core#358)` / `Summary [ 20.530s] 5 tests run: 4 passed, 1 failed, 0 skipped` / `EXIT=100`. THE MUTATION IS NOT THE ONE THIS ENTRY PRESCRIBED, AND DELIBERATELY SO. Restoring the pre-#358 owned_tree.rs wholesale removes the job entirely, and the test then dies at `guard.job().expect(\'the guard must hold a job on Windows\')` -- a red arm about the missing INSTRUMENT, not about the guard, which is the exact shape #352 c5 records as unusable on the Linux side. Instead both TerminateJobObject call sites and the Drop CloseHandle were put behind `std::hint::black_box(false)` in wcore-types/src/job_object.rs, so the job is still created and assigned and the kernel still answers the membership question, while NOTHING kills the tree. That is precisely the pre-#358 shape -- guard owns the leaf, leaks the tree -- and the failure lands on the ownership assertion at :109 rather than the precondition, with the direct child confirmed dead (execution passed the :105 assertion to reach :109). Reverted with `git checkout --`, file touched, green re-measured (see c2). NOTE FOR THE READER: the mutation-and-revert discipline here matters more than usual on this ticket -- 8d6add71, the macOS red-arm instrument for #352 c5, was NOT reverted and reached integ/f13, where it neutered the Unix guard on every swept site (see wayland-core-352.md c1)."
  - id: c4
    text: "A negative control passes in both arms, so a change that kills too much fails here"
    state: met
    evidence: "test:crates/wcore-cli/tests/harness_guard_kills_only_its_own_tree.rs::dropping_one_guard_leaves_a_sibling_guards_tree_and_the_runner_alive"
    owner: core
    note: "Cross-platform on purpose, so it is graded on the platform where an over-broad walk is reachable TODAY (Unix has had a descendant walk all along) and not only on the platform being changed. Two guarded trees, each parent with a detached grandchild; one guard is dropped and the other tree plus the runner must be untouched. It asserts nothing about what the guard DOES reach, which is what lets it pass in the pre-fix arm. BOTH ARMS MEASURED ON LINUX (hetzner /root/w-f13/win-owned-tree): post-fix guard -> PASS [0.289s] 1 test run: 1 passed; and with `git show integ/f13-base:.../support/owned_tree.rs` restored over the fixed one (the leaf-only Windows guard; job_object occurrences in that file: 0) -> PASS [0.264s] 1 test run: 1 passed. NOT VACUOUS: mutating descendants(root) to start one level too high (walk from the child's PPid so the guard reaps its siblings, bounded to this test binary's own children) reddens it, verbatim -- thread 'dropping_one_guard_leaves_a_sibling_guards_tree_and_the_runner_alive' (2978308) panicked at crates/wcore-cli/tests/harness_guard_kills_only_its_own_tree.rs:99:5: dropping one guard killed a DIFFERENT guard's direct child (2978311) - the guard is reaping outside its own tree (FerroxLabs/wayland-core#358 c4) / TRY 1 FAIL [0.633s] -- and the retry tripped the grandchild assertion at line 105 instead, so both halves of the control fire. The mutation was reverted and the file touched before the green arm was re-measured. What it still does not grade: the Windows-specific over-kill shape (a job reaching beyond the tree it owns); the test runs there and asserts it, but like c2 it has not been executed on Windows."
  - id: c5
    text: "The CI run that executed the Windows arm is cited by URL"
    state: not-met
    owner: core
    note: "No Windows run exists yet, so there is no URL. The route: push this branch with [ci-windows] in the commit subject (the lane/** wildcard is already wired), or run the c2 command directly on SeanDesktop. Cite the run here once c2 and c3 have output."
  - id: c6
    text: "clippy --target x86_64-pc-windows-msvc -p wcore-cli --all-targets -D warnings is clean"
    state: met
    evidence: "file:crates/wcore-cli/tests/support/owned_tree.rs"
    owner: core
    note: "CLOSED 2026-08-29 (lane f13-fin-hetzner-residuals) ON THE REAL msvc TARGET, not the gnu substitute this entry refused to be re-scoped to. RAN, verbatim: on SeanDesktop (the only Windows machine; rustc 1.95.0, x86_64-pc-windows-msvc INSTALLED), in D:\\resid358 at lane commit d35ac0a0, `cargo clippy --target x86_64-pc-windows-msvc -p wcore-cli --all-targets -- -D warnings` -> `Checking wcore-cli v0.13.11 (D:\\resid358\\crates\\wcore-cli)` / `Finished dev profile [unoptimized + debuginfo] target(s)` / `EXIT=0`, no warnings. THE PREVIOUS NOTE WAS RIGHT ABOUT THE HOST AND WRONG ABOUT THE WORLD: msvc is not buildable on the LINUX box (aws-lc-sys 0.41.0 compiles C for the target with the host cc), which is a fact about hetzner and not about the target -- on a native Windows host the MSVC toolchain is the natural one and the build is unremarkable. INSTRUMENT POSITIVE-CONTROLLED, because a clean exit that compiled nothing is worth nothing and the first run of this command finished in 0.68s off cache with no `Checking` line at all. Appending an `unused_mut` to the cfg(windows) test file and re-running gave, verbatim: `error: variable does not need to be mutable / --> crates\\wcore-cli\\tests\\harness_owns_spawned_trees_windows.rs:119:43 / = help: to override `-D warnings` add `#[allow(unused_mut)]` / error: could not compile `wcore-cli` (test \"harness_owns_spawned_trees_windows\") due to 1 previous error` / `EXIT=101`. That proves BOTH halves the criterion needs: --all-targets really compiles the cfg(windows) TEST binary (the target carrying the unsafe Win32_System_JobObjects FFI), and -D warnings really is fatal. The probe was reverted with `git checkout --` and the clean run above is the post-revert run at a verified-clean `git status`."

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
