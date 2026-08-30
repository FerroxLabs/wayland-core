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
