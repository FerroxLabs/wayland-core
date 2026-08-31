---
issue: 1248
repo: FerroxLabs/wayland
kind: defect
title: "The VFS write path discards the intercepted-save notice, so a preserved file is left on disk with nothing naming it"
status: closed
last_verified_commit: e581dda5a
criteria:
  - id: c1
    text: "FileMutationOutcome::Conflict (or an equivalent typed channel out of compare_exchange_file) carries the intercepted-save path instead of discarding the Refusal"
    state: met
    owner: core
    evidence: "file:crates/wcore-tools/src/vfs.rs:613:intercepted_save: refusal.intercepted_save().map(Path::to_path_buf)"
    note: "`FileMutationOutcome::Conflict` gained `intercepted_save: Option<PathBuf>` and the retraction arm changed from `Err(_)` to `Err(refusal)`, so the one fact a refusal knows and the filesystem does not now leaves the VFS layer. The enum lost `Copy` (a `PathBuf` is not `Copy`); `cargo clippy --workspace --all-targets -- -D warnings` exit 0 covers every consumer of that change workspace-wide. The field is `Option` and NOT defaulted: all six producers that refuse BEFORE a publish set it to `None` explicitly, which is what c4 grades. Carried rather than re-derived because it is NOT derivable -- after a successful retraction the destination holds exactly what it held either way, so re-observing it (which is what the Conflict renderers did) cannot tell a refusal that cost somebody something from one that cost nothing. Verified live: `the_vfs_path_names_a_save_the_refusal_displaced` asserts the destination is byte-identical to the pre-image at the moment the notice is surfaced. RED under R0 (the reconstructed unfixed tree, `Err(_) => Ok(Conflict { current })`, `cargo check -p wcore-tools --tests` exit 0) and under R2 (the two renderers ignore the field, check exit 0): nextest exit 100 in both. Logs: /root/lane-scratch-s4-1248/."
  - id: c2
    text: "The VFS Write and Edit paths render the same distinction the direct paths do: a refusal that displaced a save does not end \"Nothing was changed.\" and does name where those bytes are"
    state: met
    owner: core
    evidence: "test:crates/wcore-tools/src/edit.rs::the_vfs_edit_path_names_a_save_the_refusal_displaced"
    note: "SAME distinction means the same function, found in the tree first and then reused rather than re-worded: the direct paths call `unsaved_work::refusal_message`, whose whole body is now `conflict_message(display_path, refusal.why(), refusal.intercepted_save())`. `conflict_message` is the ONE decision site on every path, and write.rs and edit.rs's VFS arms call it with the outcome's field. The displacing wording is therefore byte-identical to the direct path's -- `changed_under_write_displacing_a_save`, which does not contain 'Nothing was changed.' and does end '...they are preserved at <path>. Read <file> as it stands now, reconcile it with <path>, and redo the change against that.' GRADED ON THE PUBLIC TOOL SURFACE, not on a helper: both tests drive `execute_with_ctx` on a real `WriteTool`/`EditTool` over a real `RealFs`, and every assertion is on the returned `ToolResult.content` and on the directory as it actually stands. Edit is graded separately from Write rather than by analogy because they render off SEPARATE match arms and the Edit arm did not even destructure the outcome before this. A THIRD consumer existed and also discarded the notice -- `wcore-agent`'s `RollbackTool` (rollback_tool.rs) -- found by enumerating consumers from the tree, not from the ticket, which names only two; it now names the preserved path in its suspension reason. RED under R2 (both renderers reverted to `changed_under_write` while c1's carrying stayed intact; `cargo check -p wcore-tools --tests` exit 0): nextest exit 100, both VFS tests failing at 'the user is not told where their save went'. Logs: /root/lane-scratch-s4-1248/r2-run.txt."
  - id: c3
    text: "A test drives compare_exchange_file through a refusal that displaced a save and asserts the SURFACED tool text against the preserved file on disk; shown RED against today's Err(_)"
    state: met
    owner: core
    evidence: "test:crates/wcore-tools/src/write.rs::the_vfs_path_names_a_save_the_refusal_displaced"
    note: "REGRESSION arm, not a mutation of finished code. R0 reconstructs the unfixed tree exactly: `git checkout` of the pre-lane vfs.rs, write.rs, edit.rs, rollback_tool.rs and vfs_compare_exchange.rs, with ONLY the test scaffolding re-inserted -- the `publish_window` module, its four-line consult, and the two tests. The production logic under test is then literally today's `Err(_) => Ok(FileMutationOutcome::Conflict { current })` with today's `changed_under_write` renderers. `cargo check -p wcore-tools --tests` exit 0 BEFORE the red was believed; `cargo nextest run` exit 100, and the panic is the defect's own sentence: 'the user is not told where their save went: Refused to overwrite /tmp/.tmpv4YSvH/f.txt: ... Nothing was changed. Read the file as it stands now and redo the change against that.' Both VFS tests failed the same way; the failure is at the OBSERVABLE, after the fixture controls passed, which proves the unfixed tree really did reach the retraction arm and really did preserve a file. The test asserts the surfaced text against the preserved file found by SCANNING the directory for the save's bytes (exactly one survivor, not the destination), so the assertion is on the file that is actually on disk rather than on a path the test computed. The window has one entrance: reaching this state needs a save to land on the published inode strictly BETWEEN the publish exchange and the restore exchange, the only vfs code in that window is the pure `precondition.matches(observation_of(displaced))`, and a racer on a microsecond window is a flake generator. `vfs::publish_window` is therefore a `#[cfg(test)]` module, keyed by DESTINATION so one probing test cannot contaminate another's compare-exchange, and it substitutes exactly one thing -- the REASON the publish is refused. The exchange, the restore, `keep_displaced`, the `Refusal`, what `compare_exchange_file` carries and what the tool renders are all production code. Fixture control in the test itself: the window was entered EXACTLY ONCE and was handed the pre-image, without which every other assertion would also pass on a pre-flight conflict, which is a different arm. Unix exchange platforms only -- see residual. Logs: /root/lane-scratch-s4-1248/red0-run.txt."
  - id: c4
    text: "A Conflict produced WITHOUT an atomic_write_checked refusal -- the pre-flight classification arms, the InMemoryFs backend, the containment wrapper -- still renders exactly the wording it renders today, with a test that fails if c1's new field is treated as always-present"
    state: met
    owner: core
    evidence: "test:crates/wcore-tools/tests/issue_1248_conflict_notice_test.rs::no_conflict_without_a_publish_names_an_intercepted_save"
    note: "Producers enumerated from the tree with a control in the same call (`git grep -n 'FileMutationOutcome::Conflict' -- crates` plus `FileMutationOutcome::Applied` as the known-positive): SEVEN construction sites, six of which refuse before any publish (vfs.rs RealFs postcondition-authority / precondition, InMemoryFs postcondition-authority / already-intended / precondition, SandboxedFs containment) and one of which is the retraction arm. Enumeration was not trusted: EVERY one of the six was fabricated to `Some(PathBuf::from(\"/fabricated\"))` one at a time, each confirmed to COMPILE (`cargo check -p wcore-tools --tests` exit 0) before any verdict, and the c4 tests asked whether they noticed. The FIRST sweep found three arms UNGRADED -- RealFs postcondition-authority, InMemoryFs postcondition-authority, InMemoryFs already-intended -- so three arms were added, reached by preparation rather than by racing (`IntendedFileMutation::from_observation` bound to a different path object, and a same-bytes rewrite that changes the in-memory generation). The second sweep reddens all 6/6. The RENDERER half is graded separately: making `conflict_message` treat the field as always-present (`unwrap_or_else(|| Path::new(display_path))`, check exit 0) reddens all three wording tests -- nextest exit 100, 3 failed of 5, run with `--no-fail-fast` so the count is the whole selection and not a cancellation. The wording tests assert BOTH ways: byte-equality against `changed_under_write(display_path, why)` and, independently, that the text still ends 'Nothing was changed. Read the file as it stands now and redo the change against that.' and does not contain 'preserved at'. The `is_compare_exchange_unsupported` re-read arms (write.rs, edit.rs) still render `changed_under_write` and are deliberately untouched: they are entered only when `compare_exchange_file` returned `Err` unsupported, so no `atomic_write_checked` ran and no `Refusal` exists. Logs: /root/lane-scratch-s4-1248/sweep.txt, r4b-run.txt."
---

The VFS path -- the one `write_through_vfs` takes whenever a `ToolContext` is present, which is
every dispatched tool call -- matched `atomic_write_checked`'s refusal as `Err(_)` and reported
`Conflict { current }`. The bytes a non-cooperating editor saved inside the exchange-to-verdict
window were preserved on disk under a `.tmpXXXXXX` sibling by wayland#1239, and then nothing
named them: the user was told "Nothing was changed."

All four criteria are met at the commit recorded above. The issue is closeable by whoever owns
closing it; this lane does not close issues.

## The shape, not the three instances

Three consumers of `Conflict` existed and all three discarded the notice. The ticket names two;
`wcore-agent`'s `RollbackTool` is the third, and it was found by enumerating consumers from the
tree rather than from the ticket. Fixing three sites is an enumeration, and an enumeration is
correct only until the fourth consumer is written -- the field is an `Option`, so ignoring it is
silent and compiles.

`no_production_site_names_a_conflict_without_naming_the_notice` asks the total form of the same
question instead: does any production site NAME `FileMutationOutcome::Conflict` without naming
`intercepted_save` inside its own brace group? A construction site cannot fail that -- the
compiler requires every field -- so what it decides is the CONSUMER question, over every
`crates/*/src` file in the workspace, including consumers that do not exist yet. `Conflict { .. }`
is precisely this defect one layer up, and it is now a test failure rather than a code review.

The checker carries its own known-positive control in the same test (a known-bad snippet must be
reported, a known-good one must not, a doc comment must not be read as code), refuses a run that
walked fewer than 100 sources, and refuses a run that examined fewer than 10 `Conflict` sites --
because an empty offender list off an empty scan reads exactly like a clean tree. Scope is
sound: the only workspace member outside `crates/` is `workspace-hack`, and the eight files in
the tree that mention `FileMutationOutcome` are all under `crates/`.

RED arm for the guard: restoring `rollback_tool.rs`'s consumer to `Ok(FileMutationOutcome::Conflict { .. })`
(`cargo check -p wcore-agent --tests` exit 0) reddens it, naming the offending file and pattern.

## Residual

Named, not silent:

* **Windows -- CLOSED 2026-08-31. The claim recorded here was FALSE, the correction stood, and
  the coverage gap it left is now MEASURED SHUT.** The previous note asserted `ReplaceFileW`
  "hands nothing back to judge" and that `intercepted_save` is "structurally always `None`" on
  Windows. That reproduced the exact reading `wcore_config::atomic_io` already records as
  "simply wrong about `lpBackupFileName`". Corrected 2026-08-30 and measured 2026-08-31 under
  FerroxLabs/wayland#1268: both c3 tests are now UNGATED and were RUN on real Windows
  (10.0.26200.9168, SeanDesktop, lane/f13-windows), 2 passed at `--retries 0`, with a
  deliberately non-existent filter in the same session returning `0 tests run` so the pass is a
  pass and not an empty selection. So `Refusal { intercepted_save: Some(..) }` is not merely
  reachable on Windows -- it is reached, and the surfaced text names the preserved file there.
  A source guard, `crates/wcore-config/tests/issue_1268_windows_impossibility_guard.rs`, now
  fails the build if any doc comment in `wcore-tools` or `wcore-config` re-asserts the
  impossibility.
* **`RollbackTool` renders its own sentence.** It names the preserved path, so the notice is not
  lost, but it composes bespoke prose instead of going through `conflict_message`, because its
  surface is a suspension reason and not a tool refusal. The source guard covers the
  discard-the-field failure there; it does not force the wording to converge.
* **The `publish_window` seam is `#[cfg(test)]` code inside `vfs.rs`.** Same shape as
  `WriteTool::publish_window_probe` from wayland#1241, and for the same measured reason.
