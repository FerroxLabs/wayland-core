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
    state: not-met
    evidence: "test:crates/wcore-cli/tests/harness_guard_kills_only_its_own_tree.rs::dropping_one_guard_leaves_a_sibling_guards_tree_and_the_runner_alive"
    owner: core
    note: "DOWNGRADED FROM met. Split the criterion in two and the halves land in different places. (b) `a change that kills too much fails here` is now TRUE ON LINUX and deterministic. (a) `passes in BOTH arms` is NOT closable on Linux and never was: every line #358 changed in owned_tree.rs is cfg(windows) (the pre-fix file has 0 job_object occurrences), so the two arms compile to the SAME Linux binary and `both arms pass` on Linux is a tautology about one binary. It is needs-platform-run, and settles with c2/c3/c5/c6 on the same Windows host. WHAT WAS WRONG WITH THE OLD `met`. The control read process_is_alive(bystander_direct) in the statement after drop(victim). kill(pid, SIGKILL) returns while the signal is only PENDING, so an over-killed process still samples as live for a scheduling quantum, and the control missed the over-kill it exists to catch. MEASURED on hetzner-dsm (96 cores), --retries 0, guard mutated to start descendants() one level too high -- walk from the child's PPid so the guard reaps its sibling tree -- with an eprintln!(\"MUTATION-LIVE ...\") inside `pub fn descendants` proving the mutation was in the executed binary: sampled liveness sequential n=20 -> detected 19, MISSED 1 (5.0%); sampled liveness 8 concurrent n=80 -> detected 71, MISSED 9 (11.3%). At [profile.ci] retries = 2 an 11.3% per-attempt miss is a 30% chance the run CONCLUSION reports a detected over-kill as PASSED. THE FIX (commit f92d5007). The bystander is now PROBED, not sampled: the fixture parent answers `ack` to a line on stdin, and a task cannot return to user space with a pending SIGKILL, so an `ack` received after drop(victim) has returned proves no kill was ever aimed at the bystander whatever the scheduler did (support::process_tree_fixture::RunningProof). The grandchild is a `sleep` and cannot answer, so it keeps a liveness check, taken AFTER the round trip and required to hold across a 500ms settle window rather than at one instant; it is the second net -- every over-kill shape this control names reaps the bystander PARENT, which the round trip grades exactly. Anti-vacuity for the instrument itself: the same probe must return Ran BEFORE anything is dropped, so a mute fixture cannot masquerade as an over-broad kill. RE-MEASURED, --retries 0, same mutation, MUTATION-LIVE printed in 100 of 100 mutated runs: green arm n=20 sequential -> 20 PASS / 0 FAIL, n=80 at 8 concurrent -> 80 PASS / 0 FAIL; mutated arm n=20 sequential -> 0 PASS / 20 FAIL, n=80 at 8 concurrent -> 0 PASS / 80 FAIL. All 100 mutated failures fire the SAME assertion (harness_guard_kills_only_its_own_tree.rs:173, `left: Gone right: Ran`) -- never NoAnswer, so none of them is a wedged fixture read as a kill. RETRIES CANNOT LAUNDER IT ANY MORE. .config/nextest.toml gains `retries = 0` for the three binaries that render a RUNTIME process-containment verdict -- harness_guard_kills_only_its_own_tree (over-kill), harness_owns_spawned_trees and harness_owns_spawned_trees_windows (under-kill). Allowlisted by MECHANISM: every_spawn_site_owns_its_tree is deliberately excluded, it is a static source ratchet with no process in it. The override is MEASURED, not assumed -- a probe test in this binary that fails on attempt 1 and passes on any retry, --profile ci: WITH the block -> `error: test run failed`, exit 100; with the block deleted and nothing else changed -> `FLAKY 2/3`, exit 0. GATES on hetzner-dsm at f92d5007: cargo fmt --all --check clean; cargo check --workspace --all-targets --all-features --locked clean; cargo clippy -p wcore-cli --all-targets -- -D warnings clean; cargo clippy --target x86_64-pc-windows-gnu -p wcore-cli --all-targets -- -D warnings clean (gnu is NOT msvc -- see c6); cargo nextest run --profile ci --no-fail-fast -p wcore-cli -> 3693 passed, 20 skipped, three consecutive runs. WHAT IS STILL UNRUN. The Windows fixture gained the same probe/ack round trip (a ReadLine loop before the terminal Start-Sleep, falling through on stdin EOF exactly as before) and harness_owns_spawned_trees_windows.rs now calls spawn_detaching_parent().into_parts(). Neither has executed on a Windows host. It compiles for Windows (clippy -gnu, exit 0) and must be validated by the SAME run that settles c2/c3/c5."
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

## State after lane `lane/f13-win-owned-tree`

The mechanism is built and the tests are written; what is missing is a Windows
host to run them on -- and as of 2026-08-29 most of it is no longer missing. `c1`, `c2`, `c3` and `c6` are all closed on the SAME real Windows run (SeanDesktop, the only Windows machine, at lane commit d35ac0a0): the green arm, the red arm with the job's kill-time reach withdrawn, and clippy on the real msvc target with a positive control proving
`--all-targets` really compiled the cfg(windows) test binary. Two remain open. `c4` was closed on Linux evidence and
has been REOPENED: its Linux half is real and is now deterministic, but `passes in BOTH arms` is a tautology on Linux,
where every line #358 changed is cfg(windows) and the two arms are the same binary -- and the probe/ack round trip its
fix added to the WINDOWS fixture has still never executed. `c5` is open for want of a cited run URL. Both settle on the
next Windows run, which should re-run:

```
# on a Windows host, from this branch
cargo clippy --target x86_64-pc-windows-msvc -p wcore-cli --all-targets -- -D warnings
cargo nextest run -p wcore-cli --test harness_owns_spawned_trees_windows

# then, for c3, restore the pre-fix guard and repeat the nextest run
git show integ/f13-base:crates/wcore-cli/tests/support/owned_tree.rs \
    > crates/wcore-cli/tests/support/owned_tree.rs
touch crates/wcore-cli/tests/support/owned_tree.rs   # else cargo measures the OLD binary
```
