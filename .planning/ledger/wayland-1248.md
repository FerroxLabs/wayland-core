---
issue: 1248
repo: FerroxLabs/wayland
kind: defect
title: "The VFS write path discards the intercepted-save notice, so a preserved file is left on disk with nothing naming it"
status: open
last_verified_commit: 09d06e1ff
criteria:
  - id: c1
    text: "FileMutationOutcome::Conflict (or an equivalent typed channel out of compare_exchange_file) carries the intercepted-save path instead of discarding the Refusal"
    state: not-met
    owner: core
    note: "FILED 2026-08-30 by lane/f13-w2-dataloss as the DECOMPOSED remainder of wayland#1239 c2. Nothing has been done. Today `compare_exchange_file` matches the publish result with `Err(_)` and reports `Conflict { current }`, so the `Refusal` -- including `intercepted_save()`, which names where a save displaced by the rollback was preserved -- never leaves the VFS layer."
  - id: c2
    text: "The VFS Write and Edit paths render the same distinction the direct paths do: a refusal that displaced a save does not end \"Nothing was changed.\" and does name where those bytes are"
    state: not-met
    owner: core
    note: "FILED 2026-08-30. Nothing has been done. The direct paths already do this through `unsaved_work::refusal_message` (wayland#1239); write.rs:219 and edit.rs:318 reconstruct a `why` from the re-observed state instead and always take the `changed_under_write` wording, which ends 'Nothing was changed.' RE-MEASURED at HEAD while closing the lane/f13-w2-dataloss F2 objection, so this entry does not rot on line numbers and so the coverage question is answered by a check rather than by assumption: the two match arms are write.rs:219 and edit.rs:318, and the two `changed_under_write` renders inside them are write.rs:227 and edit.rs:321. Those are two of the FOUR production `changed_under_write` call sites; the other two (write.rs:249, edit.rs:336) are the `is_compare_exchange_unsupported` re-read arms, where no publish ran and no `Refusal` exists, and they are out of scope here for that reason -- c4 is what keeps them so. c2 as filed already covers both in-scope arms, so nothing was added to this issue and no further gap was filed."
  - id: c3
    text: "A test drives compare_exchange_file through a refusal that displaced a save and asserts the SURFACED tool text against the preserved file on disk; shown RED against today's Err(_)"
    state: not-met
    owner: core
    note: "FILED 2026-08-30. Nothing has been done. The window is drivable at the wcore-config level exactly as wayland#1239's triple drives it -- the check closure IS the exchange-to-verdict window -- but the VFS precondition closure is built inside `compare_exchange_file`, so this needs a fixture at the vfs level rather than a reused one."
  - id: c4
    text: "A Conflict produced WITHOUT an atomic_write_checked refusal -- the pre-flight classification arms, the InMemoryFs backend, the containment wrapper -- still renders exactly the wording it renders today, with a test that fails if c1's new field is treated as always-present"
    state: not-met
    owner: core
    note: "FILED 2026-08-30. Nothing has been done. This is the anti-overcorrection criterion: `Conflict` has construction sites where no publish was ever attempted, and the new field must be absent there rather than defaulted into a claim about a save nobody displaced."
---

Decomposed out of wayland#1239 rather than folded into it.

wayland#1239 makes `atomic_write_checked` preserve a save that landed inside its own
exchange-to-verdict window and name it in `Refusal::intercepted_save()`, and both DIRECT tool
paths render that. The VFS path -- the one taken whenever a `ToolContext` is present -- drops the
`Refusal` on the floor, so the preserved file sits under a `.tmpXXXXXX` sibling and the user is
told nothing was changed.

Not data loss: wayland#1239 c1 is a disjunction and the preservation half holds on both paths.
What is missing is the notice, which is the same shape wayland#1241 closed one layer down.

Criteria are taken verbatim from the issue's Acceptance section. Nothing has been done.
