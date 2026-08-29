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
    state: not-met
    owner: core
    note: "RE-GRADED 2026-08-29 from met to not-met -- needs-platform-run. The criterion is a RUNTIME claim about what the Windows kernel does, and NOTHING HAS EVER RUN ON WINDOWS. Every fact that closed it is a fact about the SOURCE; not one of them says TerminateJobObject reached a descendant on a real machine. Grading it met while its own grading test (c2) is not-met, and while c3 and c5 record that no Windows run exists at all, reached past the evidence. CODE-LEVEL FACTS RE-VERIFIED HERE at e1f151a5 so nobody repeats them: own_windows_tree resolves at crates/wcore-cli/tests/support/owned_tree.rs:183 and panics rather than falling back; OwnedTree::new assigns the child at :293-296 via child.pid().map(own_windows_tree); reap() calls job.terminate() at :399-402 and Drop calls reap() at :471-474; WindowsJobObject::attach_running (CreateJobObjectW plus JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE plus AssignProcessToJobObject) is crates/wcore-types/src/job_object.rs:147 and ::contains (IsProcessInJob) is :209; 67 OwnedTree::new call sites across 36 files inherit it with no call-site edit (measured on this tree, definition file excluded); the lying Windows stubs are gone (the cfg(windows) child_pids that returned Vec::new(), the cfg(windows) sigkill that did nothing); and cargo clippy --target x86_64-pc-windows-gnu -p wcore-cli --test harness_owns_spawned_trees_windows -- -D warnings exits 0 on this tree. The one window it does not close is stated at attach_running: new is handed an ALREADY-RUNNING child and the kernel puts only a process's FUTURE descendants into a job. WHAT A WINDOWS RUN MUST SHOW BEFORE THIS GOES BACK TO met -- three separate facts, none inferable from the others. (1) AssignProcessToJobObject SUCCEEDS on a real runner. A runner that already places its processes in a Job Object is the normal case and nested jobs have been permitted since Windows 8, but an outer job that does not permit nesting still refuses the assignment with ERROR_ACCESS_DENIED -- and own_windows_tree PANICS rather than degrading, so a refusal takes all 67 sites down at once instead of leaking quietly. Evidence: the test does not panic inside own_windows_tree. (2) The grandchild is really INSIDE the job BEFORE anything is killed -- IsProcessInJob true for the grandchild and FALSE for the test runner, the anti-vacuity block at harness_owns_spawned_trees_windows.rs:73-86. (3) TerminateJobObject actually reaps the DESCENDANT and not just the direct child -- both pids gone after the guard is dropped while unwinding, :104-115. That is exactly the c2 command: cargo nextest run -p wcore-cli --test harness_owns_spawned_trees_windows on a Windows host. c1, c2, c3, c5 and c6 all settle on that one run; do not close one of them from it and leave the rest open."
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
    note: "Not observed, and the recipe this criterion used to prescribe DOES NOT COMPILE -- corrected here rather than left for the Windows operator to discover on the box, because they get one shot. WHY THE OLD RECIPE FAILS: it said to restore integ/f13-base's crates/wcore-cli/tests/support/owned_tree.rs, keep both new test files, and expect the grandchild assertion to fail. That base file has no job field, no fn job and zero job_object references (measured: grep -c job_object on it returns 0, and its single hit for the word job is a CI doc comment), and harness_owns_spawned_trees_windows.rs:73 calls guard.job(), which cannot resolve through Deref<Target = Child> either. MEASURED on hetzner, verbatim: error[E0599]: no method named `job` found for struct `support::owned_tree::OwnedTree<std::process::Child>` in the current scope --> crates/wcore-cli/tests/harness_owns_spawned_trees_windows.rs:73:21 // error: could not compile `wcore-cli` (test harness_owns_spawned_trees_windows) due to 1 previous error. Stubbing that call out to make it build deletes lines 73-86, which ARE the anti-vacuity block (IsProcessInJob on the grandchild, and on the runner). A red arm that has to delete its own anti-vacuity proves nothing: without it a red is indistinguishable from a fixture whose grandchild was never in the job. A SECOND OBVIOUS SHAPE ALSO FAILS, silently and in the dangerous direction: merely deleting job.terminate() from reap() does NOT redden the test. WindowsJobObject::Drop itself calls TerminateJobObject then CloseHandle (job_object.rs:377-389) and the job carries JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE (:168), so dropping the guard still kills the tree and that arm comes back GREEN. Do not use it. THE RED ARM THAT ACTUALLY WORKS -- one file, one hunk, anti-vacuity intact. In crates/wcore-cli/tests/support/owned_tree.rs, inside pub fn reap at lines 399-402, replace the terminate with a leak of the same handle. BEFORE: #[cfg(windows)] if let Some(job) = self.job.as_ref() { job.terminate(); }. AFTER: #[cfg(windows)] if let Some(job) = self.job.take() { std::mem::forget(job); }. Then touch crates/wcore-cli/tests/support/owned_tree.rs (an edit or a cp/mv restore with an older mtime makes cargo skip the rebuild and measure the WRONG binary) and run the c2 command. WHY THIS IS THE RIGHT MUTATION: the job is still created and the child is still assigned to it, so guard.job() still exists, IsProcessInJob is still true for the grandchild and false for the runner, and BOTH anti-vacuity assertions still run and still pass. What is removed is the job's reach at kill time -- both of it: terminate() is gone, and mem::forget suppresses the kill-on-close Drop. reap() is then exactly the pre-fix behaviour: TerminateProcess on the direct child and nothing else. EXPECTED RESULT: the direct-child assertion at :104 stays GREEN and :109 goes RED with -- the grandchild <pid> outlived the guard -- on Windows killing the direct child does not reach a descendant, so without a Job Object the guard owns the leaf and leaks the TREE (FerroxLabs/wayland-core#358). That single red IS also the kernel fact c3 asks for: it is the observation that a Windows child does not die with its parent. VERIFIED TO COMPILE FOR WINDOWS FROM THE LINUX HOST, so the operator does not burn a cycle on a build error: with the mutation applied, cargo clippy --target x86_64-pc-windows-gnu -p wcore-cli --test harness_owns_spawned_trees_windows -- -D warnings exits 0, and Checking wcore-cli appears in that run so it is not a cache artifact. POSITIVE CONTROL for that exit 0, run on the same file: appending a deliberate type error to owned_tree.rs makes the identical command fail with error[E0308]: mismatched types. The mutation was reverted, the file touched, and git status is clean. AFTERWARDS on the Windows box: git checkout -- crates/wcore-cli/tests/support/owned_tree.rs and touch the same file before re-running the green arm."
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
    state: not-met
    owner: core
    note: "-gnu is clean; -msvc was NOT run, and this ledger's own note says -gnu does not substitute, so the criterion stays open rather than being quietly re-scoped to the target that happened to work. RAN, exit 0, no warnings: cargo clippy --target x86_64-pc-windows-gnu -p wcore-cli --all-targets -- -D warnings. It does compile the cfg(windows) test binary and the cfg(windows) guard arms -- it caught a real unused_mut in the new Windows test that Linux clippy is blind to. -msvc is not buildable on the Linux host, and that is NOT a missing rustup target: rust-std-x86_64-pc-windows-msvc IS installed on the pinned 1.95.0 toolchain. It fails inside aws-lc-sys 0.41.0's build script, which compiles C for the target with the host cc -- `cargo:warning=GNU compiler is not supported for this target`, then `.../aws-lc/crypto/x509/../asn1/../internal.h:552:3: error: unknown type name 'pthread_rwlock_t'`, then `error occurred in cc-rs: command did not execute successfully`. Closing it needs an MSVC cross sysroot (clang-cl plus the Windows SDK/CRT, e.g. via xwin) that this box does not have (no clang-cl, no ~/.xwin-cache), or the same invocation on SeanDesktop / a [ci-windows] job. NOTE FOR WHOEVER RUNS IT: -gnu is not -msvc -- they differ in ABI, in target_env, and in which windows-sys link libraries resolve, and this change is unsafe FFI against Win32_System_JobObjects, so a -gnu pass is evidence the cfg arms type-check, not that the msvc build is clean."
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
host to run them on. Only `c4` is closed — it is graded on Linux on purpose,
where an over-broad walk is reachable today, and both its arms plus a reddening
mutation were measured there.

`c1` was closed on Linux evidence and has been re-graded `not-met`: it is a
statement about what the Windows kernel does at runtime, every fact behind it is
a fact about the source, and no Windows process has ever run this code. `c1`,
`c2`, `c3`, `c5` and `c6` now all reduce to the SAME missing run and settle
together — closing one of them from that run and leaving the rest open is the
error this ledger just corrected.

```
# on a Windows host, from this branch — c1, c2, c5, c6
cargo clippy --target x86_64-pc-windows-msvc -p wcore-cli --all-targets -- -D warnings
cargo nextest run -p wcore-cli --test harness_owns_spawned_trees_windows

# then, for c3, the RED ARM. Do NOT restore the base owned_tree.rs: that file has
# no `fn job`, so the test stops at E0599 instead of at the grandchild assertion,
# and the only way to make it build is to delete the anti-vacuity block itself.
# Instead, in crates/wcore-cli/tests/support/owned_tree.rs, inside `pub fn reap`:
#
#     -        if let Some(job) = self.job.as_ref() {
#     -            job.terminate();
#     +        if let Some(job) = self.job.take() {
#     +            std::mem::forget(job);
#          }
#
# The job is still assigned, so both anti-vacuity assertions still run and pass;
# `mem::forget` also suppresses the kill-on-close `Drop`, which a bare deletion of
# `terminate()` would not — that shape comes back GREEN and proves nothing.
touch crates/wcore-cli/tests/support/owned_tree.rs   # else cargo measures the OLD binary
cargo nextest run -p wcore-cli --test harness_owns_spawned_trees_windows
git checkout -- crates/wcore-cli/tests/support/owned_tree.rs && touch crates/wcore-cli/tests/support/owned_tree.rs
```
