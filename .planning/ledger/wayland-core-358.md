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
    state: not-met
    evidence: "test:crates/wcore-cli/tests/harness_owns_spawned_trees_windows.rs::dropping_the_guard_kills_a_detached_grandchild_on_windows"
    owner: core
    note: "AUTHORED AND READY TO EXECUTE, NOT YET RUN -- it compiles for Windows (clippy -gnu, exit 0) but no Windows host has executed it, and a test nobody has watched run is not a graded test, so this stays not-met. Shape: support/process_tree_fixture.rs::spawn_detaching_parent starts powershell.exe, which blocks on a line of stdin and then drives System.Diagnostics.Process with UseShellExecute = false (NOT Start-Process, whose shell-execute default can hand the new process a different parent and take it out of the job) to launch ping.exe, printing its pid. The stdin handshake is load-bearing: the grandchild is created only after OwnedTree::new has assigned the parent to its job, so `the grandchild was inside the job` is true by construction and the test grades the guard rather than a race. Anti-vacuity before anything is killed: both pids live, the KERNEL is asked whether the grandchild is in the job (WindowsJobObject::contains, new here, over IsProcessInJob), and the test runner is asserted NOT to be in it. The run that settles this, on a Windows host (SeanDesktop or a [ci-windows] push): cargo nextest run -p wcore-cli --test harness_owns_spawned_trees_windows"
  - id: c3
    text: "The red arm is quoted VERBATIM from a real Windows run, showing the grandchild surviving before the change"
    state: not-met
    owner: core
    note: "Not observed. Nothing on the Linux build host can produce it: the claim is that Windows TerminateProcess reaches exactly one process and that a child does not die with its parent, which only Windows can answer. The red arm is one step away and is a revert of one file, not a rewrite: `git show integ/f13-base:crates/wcore-cli/tests/support/owned_tree.rs > crates/wcore-cli/tests/support/owned_tree.rs` (that version has the Vec::new() Windows child_pids and no job), keep both new test files, `touch` the restored file so cargo does not skip the rebuild and measure the wrong binary, then run the c2 command on Windows. The expected failure is the grandchild assertion in dropping_the_guard_kills_a_detached_grandchild_on_windows."
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
