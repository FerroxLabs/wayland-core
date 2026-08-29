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
host to run them on. `c1` is closed with evidence measured on Linux (the code plus a Windows-target
clippy). `c4` was closed on the same basis and has been REOPENED: its Linux
half is real and is now deterministic, but `passes in BOTH arms` is a tautology
on Linux, where the two arms are the same binary. It joins the list below. `c2`, `c3`, `c5` and `c6` all reduce to the SAME missing run and
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
