---
issue: 358
repo: FerroxLabs/wayland-core
kind: defect
title: "OwnedTree owns only the LEAF on Windows: the grandchild case #1156 was filed about is still open on all 49 swept sites"
status: open
last_verified_commit: f92d5007
criteria:
  - id: c1
    text: "OwnedTree kills the process TREE on Windows, not just the direct child"
    state: met
    evidence: "symbol:crates/wcore-cli/tests/support/owned_tree.rs::own_windows_tree"
    owner: core
    note: "OwnedTree::new now assigns its child to a fresh kill-on-close Job Object on Windows (own_windows_tree -> wcore_types::job_object::WindowsJobObject::attach_running), and reap() calls TerminateJobObject on it; every one of the 49 swept sites gains this with no call-site edit because it hangs off `new`. The Job Object is the primitive the Windows sandbox and the MCP stdio transport already use -- no second mechanism was invented. The lying stubs are gone rather than left in place: the cfg(windows) child_pids that returned Vec::new() and the cfg(windows) sigkill that did nothing are deleted, and descendants()/the `known` pid list are now cfg(unix)-only concepts. Failure to create the job PANICS -- no fallback, matching the Linux arm that refuses to degrade to pgrep. The one window it does NOT close, stated at attach_running and in the guard module docs: `new` is handed an ALREADY-RUNNING child and the kernel puts only a process's FUTURE descendants into a job, so anything spawned between CreateProcess returning and the assignment landing stays outside it; WindowsJobObject::attach (CREATE_SUSPENDED) is the race-free constructor for a caller that holds the Command, and no site needs it today so none was added speculatively. Compiles for Windows: clippy --target x86_64-pc-windows-gnu -p wcore-cli --all-targets -- -D warnings exits 0 (see c6 for why that is not the msvc run). GROUNDED IN A RUNTIME OBSERVATION, not only in the source: this entry was re-graded to not-met on lane f13-fix-ledger-truth on the correct argument that c1 is a claim about what the WINDOWS KERNEL does and that nothing had ever run there. That argument is now stale, and only because the run happened -- 2026-08-29 on SeanDesktop at lane commit d35ac0a0: the c2 test passed with IsProcessInJob confirming the detached GRANDCHILD was inside the guard's job and the test runner was not, and the c3 red arm (job still created and assigned, its reach at kill time withdrawn) left that same grandchild alive. Tree-kill on Windows is therefore observed, not inferred. If c2 or c3 is ever reopened this criterion goes back to not-met with them."
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
    note: "OBSERVED 2026-08-29 (lane f13-fin-hetzner-residuals), same host and commit as c2. VERBATIM: `thread \'dropping_the_guard_kills_a_detached_grandchild_on_windows\' (33376) panicked at crates\\wcore-cli\\tests\\harness_owns_spawned_trees_windows.rs:109:5: / the grandchild 38828 outlived the guard -- on Windows killing the direct child does not reach a descendant, so without a Job Object the guard owns the leaf and leaks the TREE (FerroxLabs/wayland-core#358)` / `Summary [ 20.530s] 5 tests run: 4 passed, 1 failed, 0 skipped` / `EXIT=100`. THE MUTATION IS NOT THE ONE THIS ENTRY PRESCRIBED, AND DELIBERATELY SO. Restoring the pre-#358 owned_tree.rs wholesale removes the job entirely, and the prescribed recipe then does not even BUILD -- corrected here from `the test dies at guard.job().expect(...)`, which named the wrong failure mode. MEASURED: that base file has zero job_object references and no `fn job` (git show integ/f13-base:crates/wcore-cli/tests/support/owned_tree.rs | grep -c job_object -> 0, grep -n \'fn job\' -> no match) while harness_owns_spawned_trees_windows.rs:73 calls guard.job(), which cannot resolve through Deref<Target = Child>; verbatim: error[E0599]: no method named `job` found for struct `support::owned_tree::OwnedTree<std::process::Child>` in the current scope. Stubbing that call out to make it build deletes the anti-vacuity block (IsProcessInJob on the grandchild, and on the runner), so it would be a red arm about the missing INSTRUMENT, not about the guard -- the exact shape #352 c5 records as unusable on the Linux side. ONE OTHER SHAPE IS A SILENT FALSE GREEN AND MUST NOT BE USED: deleting only job.terminate() from reap() does NOT redden the test, because WindowsJobObject::Drop itself calls TerminateJobObject (job_object.rs:377-389) and the job carries JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE (job_object.rs:168), so dropping the guard still kills the tree and the arm comes back green. The arm actually used survives that trap by withdrawing BOTH reaches. Instead both TerminateJobObject call sites and the Drop CloseHandle were put behind `std::hint::black_box(false)` in wcore-types/src/job_object.rs, so the job is still created and assigned and the kernel still answers the membership question, while NOTHING kills the tree. That is precisely the pre-#358 shape -- guard owns the leaf, leaks the tree -- and the failure lands on the ownership assertion at :109 rather than the precondition, with the direct child confirmed dead (execution passed the :105 assertion to reach :109). Reverted with `git checkout --`, file touched, green re-measured (see c2). NOTE FOR THE READER: the mutation-and-revert discipline here matters more than usual on this ticket -- 8d6add71, the macOS red-arm instrument for #352 c5, was NOT reverted and reached integ/f13, where it neutered the Unix guard on every swept site (see wayland-core-352.md c1)."
  - id: c4
    text: "A negative control passes in both arms, so a change that kills too much fails here"
    state: met
    evidence: "test:crates/wcore-cli/tests/harness_guard_kills_only_its_own_tree.rs::dropping_one_guard_leaves_a_sibling_guards_tree_and_the_runner_alive"
    owner: core
    note: "MET ON REAL WINDOWS. The half that was open was (a) `passes in BOTH arms`, which the previous grading correctly said is a tautology on Linux -- every line #358 changed is cfg(windows), so both arms compile to the same Linux binary. Executed on SEANDESKTOP (the only Windows box; D:\\wf13w at f0060a2e8, x86_64-pc-windows-msvc, `cargo nextest run --profile ci -p wcore-cli`, and this binary carries `retries = 0` in .config/nextest.toml so no attempt can be laundered): POST-FIX ARM -- negative control `dropping_one_guard_leaves_a_sibling_guards_tree_and_the_runner_alive` 20 runs, 20 PASS 0 FAIL, and the under-kill test `harness_owns_spawned_trees_windows` 5/5 PASS in the same arm, so the mechanism it must not over-reach with is demonstrably live. PRE-FIX ARM -- negative control 20 runs, 20 PASS 0 FAIL. Both arms pass; the control is a control. HOW THE PRE-FIX ARM WAS BUILT, AND WHY IT SURVIVES BOTH RECORDED FALSE-GREEN TRAPS. Trap 1, `delete only job.terminate()`: refused, because the job handle's own Drop plus JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE still kills the tree, so that mutation leaves a guard that still owns the tree and the arm would be fake. Trap 2, wholesale-restore of the base file: refused, because base `owned_tree.rs` has no `fn job` and `harness_owns_spawned_trees_windows.rs` calls `guard.job()`, so the arm is E0599 and never runs at all. What was done instead is ONE line in `OwnedTree::new` -- `let job = child.pid().map(own_windows_tree)` becomes `let job: Option<WindowsJobObject> = None` -- so no job is ever CREATED (nothing to close, nothing to terminate: the guard degrades to killing the direct child, which is exactly the pre-#358 behaviour) while the `job()` accessor survives and the crate still compiles. THE MUTATION IS PROVEN TO HAVE LANDED ON EXECUTABLE CODE, not on a comment and not on dead code: in that same arm the under-kill test goes RED, verbatim -- `FAIL + LEAK [ 0.371s] (5/5) wcore-cli::harness_owns_spawned_trees_windows dropping_the_guard_kills_a_detached_grandchild_on_windows` / `thread ... panicked at crates\\wcore-cli\\tests\\harness_owns_spawned_trees_windows.rs:73:27: the guard must hold a job on Windows` / `error: test run failed`, UNDERKILL_EXIT=100 -- and nextest's own LEAK verdict is the leak itself. A surviving `ping.exe -n 300 127.0.0.1` orphan whose parent pid was already gone was observed in the live process list from that run and killed by hand afterwards. If Drop or KILL_ON_JOB_CLOSE were still reaching the tree there would be no leak and no red, which is precisely how trap 1 produces a false green. RESTORED AND RE-CONFIRMED: `git checkout -- owned_tree.rs`, file touched so cargo could not measure the mutated binary, rebuilt, and the under-kill test is 5/5 PASS again; `git status --porcelain` on that file is empty and `map(own_windows_tree)` is back at 1 call site. LINUX EVIDENCE STANDS UNCHANGED and is what grades the over-kill direction: with the guard mutated to walk from the child`s PPid, --retries 0, MUTATION-LIVE printed in 100 of 100 mutated runs, green arm 20/20 + 80/80 PASS and mutated arm 0/20 + 0/80, all 100 failures firing the same assertion at harness_guard_kills_only_its_own_tree.rs:173 (`left: Gone right: Ran`), never NoAnswer."
  - id: c5
    text: "The CI run that executed the Windows arm is cited by URL"
    state: met
    evidence: "test:crates/wcore-cli/tests/harness_owns_spawned_trees_windows.rs::dropping_the_guard_kills_a_detached_grandchild_on_windows"
    owner: core
    note: "https://github.com/FerroxLabs/wayland-core/actions/runs/33258852685 - job 99117201158, `CI (windows-latest, hosted)`, step `Run tests (nextest CI profile)`, on lane/f13-fin-windows-runs at bd184563. THE ARM EXECUTED AND PASSED, quoted from that job's log: `PASS [   0.406s] ( 7619/15962) wcore-cli::harness_owns_spawned_trees_windows dropping_the_guard_kills_a_detached_grandchild_on_windows`, with the four support::mock_llm self-tests in the same binary also PASS. `7619/15962` is the point: it ran as part of the ordinary workspace nextest, not a hand-picked invocation. THE JOB'S OVERALL CONCLUSION IS `failure` AND THAT IS NOT THIS ARM - stated plainly rather than left for a reader to trip over. Nine tests failed in that leg, all pre-existing at the branch point (this branch changed ZERO Rust code relative to ab6b602f - `git diff ab6b602f..HEAD -- \"*.rs\"` has no non-doc-comment line): two were desktop-contract-corpus staleness, since fixed on integ/f13 by e1f151a52 and green after merging it; one is FerroxLabs/wayland-core#374; three are FerroxLabs/wayland-core#387, the wl#1164 bash-resolution regression this same session found and A/B-proved. A criterion asking for the URL of the run that executed the arm is answered by the arm's own line, and the arm passed. Corroborated independently on hardware: c2 records the same test passing on a Windows 11 build 26200 workstation, and c3 the red arm on the same box."
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

## A red arm reached `integ/f13` and it is still there (found 2026-08-29)

Independent of c4, and larger than it. Commit `8d6add71` — subject
`RED ARM (throwaway, never merge): leaf-only OwnedTree on macOS [ci-darwin]` —
was merged into `integ/f13` by `d03a6e14` and is **still present at the
integration tip `e151392e`**. It puts

```rust
if std::hint::black_box(true) {
    return;
}
```

at the top of `OwnedTree::snapshot`, under `#[cfg(unix)]`. So on the
integration branch the descendant walk never runs on Linux **or** macOS, and
every one of the ~49 swept sites owns only the LEAF again — the exact state
FerroxLabs/wayland#1156 was filed about.

It is not a silent regression. The tree's own positive test says so, and has
been saying so:

```
# hetzner-dsm, working tree restored to e1f151a5 for the five files this lane
# touches, cargo nextest run --profile ci -p wcore-cli
TRY 1 FAIL [ 10.232s] wcore-cli::harness_owns_spawned_trees dropping_the_guard_kills_a_detached_grandchild_and_reaps_the_direct_child
TRY 2 FAIL [ 10.151s] ...same...
TRY 3 FAIL [ 10.106s] ...same...
   Summary [ 92.490s] 3693 tests run: 3692 passed, 1 failed, 20 skipped
```

Three tries, three failures — deterministic, not a flake, and `retries = 2`
cannot hide it. `integ/f13` is red on `wcore-cli` today.

It also blinds this ledger's own c4 evidence completely. With the early return
in place `descendants()` is never called, so the over-broad-kill mutation the
c4 note cites cannot execute: mutation applied with an `eprintln!` proving it
is in the binary, n=10 at `--retries 0` → **PASS=10, FAIL=0, and
`MUTATION-LIVE` printed in 0 of 10 runs**. The c4 measurements quoted in the
old note were taken in `/root/w-f13/win-owned-tree`, a lane cut *before*
`d03a6e14`, where the walk was still live — they were honest measurements of a
tree that is not the integration tree.

`lane/f13-fix-settle-race` reverts the ten lines (commit `f629fc0f`). After
the revert the same suite is green: `3693 passed, 20 skipped`, three
consecutive runs.

## State after lane `lane/f13-last3`

All six criteria are met. The mechanism, the two Windows tests and the
cross-platform negative control were built on `lane/f13-win-owned-tree`; the
Windows runs that graded them were taken on SeanDesktop, the last of them
(`c4`) at `f0060a2e`.

`c4` was the final one, and it was the awkward one: its two halves land in
different places. "A change that kills too much fails here" is graded on Linux,
where an over-broad descendant walk is reachable today and a mutation reddens
the control 100 times out of 100. "Passes in BOTH arms" could only ever be
graded on Windows, because every line `#358` changed is `cfg(windows)` and the
two arms compile to the same Linux binary.

Building an honest pre-fix arm on Windows is the part worth remembering. Two
obvious constructions are both wrong, and both fail quietly:

```
# FALSE GREEN: the job handle's Drop plus JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE
# still kill the tree, so the "pre-fix" guard still owns it.
-        job.terminate();

# DOES NOT BUILD: base owned_tree.rs has no `fn job`, and
# harness_owns_spawned_trees_windows.rs calls guard.job() -> E0599.
git show integ/f13-base:crates/wcore-cli/tests/support/owned_tree.rs > ...
```

What works is to stop the job being CREATED while keeping the accessor:

```
-        let job = child.pid().map(own_windows_tree);
+        let job: Option<wcore_types::job_object::WindowsJobObject> = None;
```

and then to prove the arm is real rather than assume it: in that arm the
UNDER-kill test must go red. It does, with nextest's own `FAIL + LEAK` verdict
and a surviving `ping.exe` orphan in the process list. A mutation whose only
evidence is the control still passing is not a mutation you have watched land.
