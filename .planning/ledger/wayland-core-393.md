---
issue: 393
repo: FerroxLabs/wayland-core
kind: defect
title: "Windows: a quarantine git abort kills the leaf and leaves its descendants running (split from #379)"
status: open
last_verified_commit: d8b422fe3
criteria:
  - id: c1
    text: "On Windows, both quarantine abort paths terminate the child's descendants, not the direct process alone"
    state: not-met
    owner: core
    note: "Filed 2026-08-30 by lane/f13-w3-teardown while closing the unix arm of wayland-core#379. Not a regression: unlike unix, Windows never had a group teardown for the #338 hardening to take away, so this is a standing gap rather than a consequence of #338. Filed anyway because pre-existing is not a disposition. `terminate_hardened_tree` in crates/wcore-cli/src/plugin/quarantine.rs has a `#[cfg(not(unix))]` no-op arm and its doc comment says why: DETACHED_PROCESS is a creation-time console decision that creates no session, no process group and no job, so there is nothing for a group signal to address, and Child::kill is TerminateProcess on one pid."
  - id: c2
    text: "A test on real Windows spawns a quarantine child that backgrounds a descendant, trips an abort path, and asserts the descendant is gone; shown RED against today's kill-the-leaf code"
    state: not-met
    owner: core
    note: "Needs SeanDesktop; there is one Windows box. Not a credential request. The unix counterparts to copy are a_timed_out_quarantine_child_takes_its_whole_session_with_it and a_helper_that_outlives_the_drain_guard_is_torn_down_with_its_session in crates/wcore-cli/src/plugin/quarantine.rs."
  - id: c3
    text: "The change does not weaken #338: a test asserts the production build_git_command child still does not share the user's console after the fix"
    state: not-met
    owner: core
    note: "THE TRAP, found by reading the shared mechanism rather than assumed. wcore_types::job_object::WindowsJobObject::create_suspended is `command.creation_flags(CREATE_SUSPENDED)`, and creation_flags is a SETTER, not an OR. Composed naively with harden_against_credential_prompt's creation_flags(DETACHED_PROCESS) it silently drops one of them, and dropping DETACHED_PROCESS reopens wayland-core#338's Windows console reduction -- a fix that reproduces a defect it is adjacent to. attach_running(pid) dodges the flag conflict at the cost of a race window before the job assignment lands. Either way the composition must be PROVEN on the box, which is why this is a criterion and not a note."
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
