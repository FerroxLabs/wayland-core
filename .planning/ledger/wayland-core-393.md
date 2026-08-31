---
issue: 393
repo: FerroxLabs/wayland-core
kind: defect
title: "Windows: a quarantine git abort kills the leaf and leaves its descendants running (split from #379)"
status: open
last_verified_commit: ca15a48bf
criteria:
  - id: c1
    text: "On Windows, both quarantine abort paths terminate the child's descendants, not the direct process alone"
    state: met
    evidence: "symbol:crates/wcore-cli/src/plugin/quarantine.rs::run_hardened"
    owner: core
    note: "MET on real Windows by lane/f13-s2-win-proc. Measured on real Windows 10.0.26200.9168 (SeanDesktop), x86_64-pc-windows-msvc, cargo 1.95.0 / nextest 0.9.138, isolated checkout D:\\\\s2winproc at ca15a48bf with a CLEAN tree (the run printed COMMIT and TREE=[] before testing), --retries 0 so nothing is laundered. A cross-compiled --target x86_64-pc-windows-gnu check compiles these arms and does not execute them, so it is not the evidence of record. NEGATIVE CONTROL in the same session: -E 'test(this_test_name_does_not_exist_anywhere)' -> 0 tests run, exit 4 -- an empty selection cannot read as a pass here. HOST LIMIT, stated because it matters where privilege does: this box has Developer Mode ON, so it is not representative of an ordinary Windows host for anything privilege-dependent. Nothing in this row is privilege-dependent: it is process creation flags, a Job Object, and file I/O under the calling user. GREEN: `cargo nextest run -p wcore-cli --test quarantine_process_tree_windows --test quarantine_console_authority_windows` -> 6 tests run: 6 passed, 0 skipped. BOTH abort paths are graded, not one: the wall-clock timeout and the drain-grace exit each get their own test. RED ARM, driven on THIS tree and not inherited from lane/f13-windows (a different tree, 200+ commits behind this base): `impl Drop for HardenedTree`'s `job.terminate();` replaced by `job.release();` at MUTATION_SITES=1. The job is still created, `attach`ed and resumed -- only the teardown's CLAIM on the tree is withdrawn, which is precisely kill-the-leaf: the leaf still dies and its descendants do not. That is why this mutation and not deleting the spawn flags, which would measure a frozen process instead. RED_TESTS_EXIT=100, `3 tests run: 1 passed, 2 failed` -- the_wall_clock_abort_... `#393 c1: after the wall-clock abort the descendant (pid 55712) is Live`, and the_drain_grace_abort_... `#393 c1: after the drain-grace abort the descendant (pid 47756) is Live. The leaf was reaped and its tree was left running.` CHECK_EXIT=0 first, so the red is behaviour and not a build break. THE ONE THAT STILL PASSED IS THE CONTROL: a_successful_quarantine_run_leaves_its_tree_standing_on_windows -- a run that FINISHED must leave its tree standing, because the job kills on CLOSE and git-credential-cache--daemon deliberately outlives the git that started it. A mutation that simply broke the spawn would have taken that one down too. Post-restore GREEN control re-run on the blob-verified tree: 3 tests run, 3 passed, exit 0. Restored blob-verified against the HEAD blob with an mtime touch after both the mutation and the restore, so cargo could not skip the rebuild and hand back a verdict about the other binary. PRODUCTION PATH, not a helper: the Job Object is armed in `run_hardened`, the single spawn site `run_git` uses, and torn down by `Drop for HardenedTree`, so the scope owns the teardown rather than the branches -- which is the completeness failure wayland-core#379 was reopened for."
  - id: c2
    text: "A test on real Windows spawns a quarantine child that backgrounds a descendant, trips an abort path, and asserts the descendant is gone; shown RED against today's kill-the-leaf code"
    state: met
    evidence: "test:crates/wcore-cli/tests/quarantine_process_tree_windows.rs::the_drain_grace_abort_takes_the_whole_process_tree_on_windows"
    owner: core
    note: "MET. The test the criterion asks for exists, RUNS ON REAL WINDOWS, and was SHOWN RED against kill-the-leaf code by this lane on this tree. Measured on real Windows 10.0.26200.9168 (SeanDesktop), x86_64-pc-windows-msvc, cargo 1.95.0 / nextest 0.9.138, isolated checkout D:\\\\s2winproc at ca15a48bf with a CLEAN tree (the run printed COMMIT and TREE=[] before testing), --retries 0 so nothing is laundered. A cross-compiled --target x86_64-pc-windows-gnu check compiles these arms and does not execute them, so it is not the evidence of record. NEGATIVE CONTROL in the same session: -E 'test(this_test_name_does_not_exist_anywhere)' -> 0 tests run, exit 4 -- an empty selection cannot read as a pass here. HOST LIMIT, stated because it matters where privilege does: this box has Developer Mode ON, so it is not representative of an ordinary Windows host for anything privilege-dependent. Nothing in this row is privilege-dependent: it is process creation flags, a Job Object, and file I/O under the calling user. The fixture spawns a quarantine child through the production `run_hardened`, that child backgrounds a descendant, the abort path is tripped, and the descendant's liveness is the assertion. RED ARM, driven on THIS tree and not inherited from lane/f13-windows (a different tree, 200+ commits behind this base): `impl Drop for HardenedTree`'s `job.terminate();` replaced by `job.release();` at MUTATION_SITES=1. The job is still created, `attach`ed and resumed -- only the teardown's CLAIM on the tree is withdrawn, which is precisely kill-the-leaf: the leaf still dies and its descendants do not. That is why this mutation and not deleting the spawn flags, which would measure a frozen process instead. RED_TESTS_EXIT=100 with both abort tests naming the surviving descendant BY PID. Restored blob-verified against the HEAD blob with an mtime touch after both the mutation and the restore, so cargo could not skip the rebuild and hand back a verdict about the other binary. A REVIEWED EXEMPTION rather than a laundered one: the fixture's own alias spawn at quarantine_process_tree_windows.rs:115 is entered in ALLOWED_UNOWNED, because the descendant outliving it IS the property under test and wrapping it in OwnedTree would destroy the measurement; it is bounded by a self-terminating sleep and a taskkill /T /F in the liveness control."
  - id: c3
    text: "The change does not weaken #338: a test asserts the production build_git_command child still does not share the user's console after the fix"
    state: met
    evidence: "symbol:crates/wcore-cli/src/plugin/quarantine.rs::QUARANTINE_SPAWN_FLAGS"
    owner: core
    note: "MET, and measured through the PRODUCTION spawn rather than a rebuilt one. Measured on real Windows 10.0.26200.9168 (SeanDesktop), x86_64-pc-windows-msvc, cargo 1.95.0 / nextest 0.9.138, isolated checkout D:\\\\s2winproc at ca15a48bf with a CLEAN tree (the run printed COMMIT and TREE=[] before testing), --retries 0 so nothing is laundered. A cross-compiled --target x86_64-pc-windows-gnu check compiles these arms and does not execute them, so it is not the evidence of record. NEGATIVE CONTROL in the same session: -E 'test(this_test_name_does_not_exist_anywhere)' -> 0 tests run, exit 4 -- an empty selection cannot read as a pass here. HOST LIMIT, stated because it matters where privilege does: this box has Developer Mode ON, so it is not representative of an ordinary Windows host for anything privilege-dependent. Nothing in this row is privilege-dependent: it is process creation flags, a Job Object, and file I/O under the calling user. The trap this ticket named is closed by construction: `CommandExt::creation_flags` is a SETTER, so the two flags are OR-ed ONCE in `QUARANTINE_SPAWN_FLAGS` (DETACHED_PROCESS | CREATE_SUSPENDED) and applied at the single spawn site in `run_hardened`; `WindowsJobObject::create_suspended`, which would have re-set the flags, is never called on this path. MEASURED BY THIS LANE with --no-capture so the probe's own stdout is the evidence: `[production_spawn] SHARES_USER_CONSOLE_BEFORE=false` and `[production_spawn] CONOUT_BEFORE=OPEN` -- the child driven through `run_hardened`, with the containment flag composed beside the console flag, still does not share the operator's console. NEGATIVE CONTROL ALIVE in the same run: `[plain] SHARES_USER_CONSOLE_BEFORE=true`, so the oracle can tell a shared console from an unshared one rather than answering false to everything. The oracle is GetConsoleProcessList from inside the child, NOT GetConsoleWindow -- #389 records why the window handle is not a usable oracle here."
---

Split out of `FerroxLabs/wayland-core#379` on 2026-08-30 while its unix arm was being closed,
so that #379's wording -- "the whole session/process group it created" -- cannot be read as a
claim about a platform that creates neither.

Searched before filing: the open quarantine issues in this repo are #338, #369, #379, #380,
#385 and #389. #380 and #389 are the Windows arms of #338 and both are about console and
prompt authority, not teardown; a keyword search for "quarantine Windows job object" and for
"descendant process tree Windows" returned nothing, against a control search for "quarantine"
that returned all six. There was no carrier.

## What is graded off Windows, and what is not (lane `f13-w3-win-393-linux-arm`, 2026-08-31)

Both of this ticket's test files -- `crates/wcore-cli/tests/quarantine_process_tree_windows.rs`
and `crates/wcore-cli/tests/quarantine_console_authority_windows.rs` -- are `#![cfg(windows)]`,
so on every host our gates execute today they compile to ZERO tests. While that holds, the
whole fix can be deleted and every green stays green.

`crates/wcore-cli/tests/issue_393_quarantine_spawn_flags_guard.rs` closes the part of that
which is decidable off Windows. It has no `#![cfg]` and runs on Linux, macOS and Windows
alike. It deliberately closes NO criterion here; c1, c2 and c3 are unchanged and still
`not-met`.

WHAT IT GRADES (each shown RED on hetzner against a mutation of the production file, with
`cargo check -p wcore-cli --tests` RC=0 first, and restored green afterwards):

* the composed VALUE -- `QUARANTINE_SPAWN_FLAGS` contains `DETACHED_PROCESS`, contains
  `CREATE_SUSPENDED`, is exactly their OR, and `DETACHED_PROCESS` is `0x8` and not `0x10`
  (`CREATE_NEW_CONSOLE`). That last one is the mutation no source scan can see: it reads
  identically and inverts #338. Both constants were ungated and made `pub` for this; a `u32`
  costs nothing where it is never applied.
* the WIRING -- `quarantine.rs` makes exactly two `creation_flags` calls, one per function;
  `harden_against_credential_prompt`'s is `DETACHED_PROCESS`, `run_hardened`'s is the composed
  constant and precedes the `.spawn()` it governs; and nothing here calls
  `WindowsJobObject::create_suspended`, which is a second writer of the same field under
  another name. That is c3's trap, read from source.
* the release/terminate SPLIT -- `HardenedTree::disarm` releases the job and does not
  terminate it; `Drop` terminates and does not release; both `take()` the handle; the unix
  group teardown is still on the `Drop` path.

WHAT IT DOES NOT GRADE, ON ANY HOST BUT WINDOWS:

* that the flags reach `CreateProcessW` or have their effect. `std::process::Command` has no
  `creation_flags` on unix, so off Windows they are never applied to anything.
* that the child has no console (#338 c1 / #393 c3's own wording).
* that the Job Object owns a DESCENDANT and kills it (c1, c2). A `release` that has stopped
  releasing, or a `terminate` that terminates nothing, is invisible to a source scan.

The wiring and split arms are source scans on purpose: nothing inside a unix process can
observe a Windows creation flag or a Job Object, so whether the calls are there is in the
source or nowhere -- the same argument `every_spawn_site_owns_its_tree.rs` makes for its
wrapping ratchet, and the same cost (a deliberate refactor of this path reds them and has to
be re-argued there). The scans blank comments and string literals first, because
`quarantine.rs` names `creation_flags` five times in prose before calling it twice in code,
and each proves both polarities of its reader on synthetic sources in the same test call.

Net: what the release buys from Linux is that the DECISION cannot be edited away silently.
What it still does not buy is any evidence the decision works, and #393 stays open for
SeanDesktop.

### The compile gate beside it (`just check-windows-compile`)

The guard above grades the DECISION but still cannot see the two
`#![cfg(windows)]` files themselves: they compile to nothing on Linux, so a type
error or a stale call inside them is invisible until a Windows runner picks it
up. `just check-windows-compile` closes that half by cross-checking the
workspace at `x86_64-pc-windows-gnu`, and is wired into `check-all`.

Measured on hetzner, 2026-08-31, with a deliberate type error appended to
`quarantine_process_tree_windows.rs` and then reverted -- the same mutation
graded against both instruments, so the hole and the fix are on one record:

| instrument | error present | reverted |
|---|---|---|
| `cargo check -p wcore-cli --tests` (Linux) | RC=0 | RC=0 |
| `cargo test -p wcore-cli --test issue_393_..._guard` (Linux) | RC=0 | RC=0 |
| `vx just check-windows-compile` | RC=101, `error[E0308]` | RC=0 |

The first two rows ARE the hole, measured rather than argued: the Linux gates
stay green through a break in the file. Cost: ~1m04s cold against a warm dep
graph, 1-3s warm, 668 MB of `target/x86_64-pc-windows-gnu`.

Two honesty notes the release should carry:

* **gnu is not msvc.** We ship `x86_64-pc-windows-msvc`. The `gnu` target shares
  the `cfg(windows)` arms -- the whole point -- but not the ABI, the linker or
  the C runtime. A green here means "the Windows source still compiles as
  source", never "Windows works", and it does not substitute for the msvc legs
  in `ci.yml`, which stay the release-blocking arm.
* **The recipe does not use `vx`, and as first written it could never pass.**
  `vx` keeps a private rustup store whose only installed target is
  `x86_64-unknown-linux-gnu`, while `rustup target add` resolves to the system
  rustup regardless of a `vx` prefix. `vx just check-windows-compile` therefore
  failed `error[E0463]: can't find crate for std` on a CLEAN tree. That was
  caught by running the recipe as CI runs it rather than the command its own
  comment described. Bare `cargo` honours `rust-toolchain.toml` and resolves to
  the pinned cargo 1.95.0 (`vx cargo` resolved to 1.97.0), so determinism is
  kept.

None of this closes c1, c2 or c3. They remain `not-met` and need SeanDesktop.
