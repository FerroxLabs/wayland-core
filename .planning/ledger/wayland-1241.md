---
issue: 1241
repo: FerroxLabs/wayland
kind: defect
title: "The Write tool converts a refusal it could not roll back into an unchecked republish and reports success"
status: open
last_verified_commit: a268075fe
criteria:
  - id: c1
    text: "The direct Write path distinguishes \"atomic_write_checked never reached a verdict\" from \"the verdict REFUSED and the rollback could not be made\", and does not take the unchecked std::fs::write fallback in the second case"
    state: not-met
    owner: core
    note: "Filed 2026-08-30 by lane/f13-dur-atomic while closing wayland#1202. Nothing has been done. `atomic_write_checked` hands both meanings to the caller as a bare `std::io::Error`, so closing this probably means naming the distinction in wcore-config rather than sniffing the message text at write.rs:462."
  - id: c2
    text: "When the guard refused and could not roll back, the Write tool returns is_error: true and its text carries the path where the pre-image was preserved"
    state: not-met
    owner: core
    note: "Filed 2026-08-30 by lane/f13-dur-atomic. Nothing has been done. Today write.rs:462 turns that error into `Some(e)`, the fallback at write.rs:465-476 republishes unchecked, and the `it is preserved at <path>` string -- the only record of where the user's original went -- is discarded behind an `Updated <path> (N lines)` success. The ctx path (write.rs:262) and both EditTool arms (edit.rs:353, edit.rs:544) already do the right thing; only the direct `Tool::execute` path is in scope."
  - id: c3
    text: "A test drives the direct (no-ToolContext) Write path through a refused publish whose rollback exchanged nothing and asserts c2's observable; shown RED against today's `Err(e) => Some(e)` fallback"
    state: not-met
    owner: core
    note: "Filed 2026-08-30 by lane/f13-dur-atomic. Nothing has been done. The window is drivable the same way wayland#1202's test drives it -- unlink the destination from inside the check closure and then refuse -- so this needs no new primitive, only a fixture on the direct Write path."
  - id: c4
    text: "A genuine round-trip failure that never reached a verdict (a directory that will not hold a sibling temp file) still takes the fallback, still publishes, and still reports success -- with a test that fails if c1's new branch swallows it"
    state: not-met
    owner: core
    note: "Filed 2026-08-30 by lane/f13-dur-atomic. Nothing has been done. This is the anti-overcorrection criterion: c1 must narrow the fallback, not remove it."
---

`WriteTool`'s direct, no-`ToolContext` path (crates/wcore-tools/src/write.rs:452) treats every `Err` out of `atomic_write_checked` as "the tempfile round trip failed before any verdict" and falls into an unchecked `std::fs::write`, reporting success. Since wayland#1202 that error ALSO means "the guard ran, REFUSED, and the rollback exchanged nothing", in which case the error text is the only record of where the pre-image was preserved. The direct path discards it, so the tool reports success for a write its own guard refused and leaves the user's original under a `.tmpXXXXXX` sibling nothing will clean up.

Not new data loss -- in that state the new bytes are already published, so the fallback rewrites identical bytes. What is lost is the refusal signal and the pre-image's whereabouts.

Pre-existing in shape, widened in reach: before wayland#1202 the `keep_displaced` branch already returned `Err` when the restore exchange genuinely errored (EIO, EXDEV). wayland#1202 widens the trigger to the reachable `Swap::Vacant` / `Swap::Unsupported` case.

Distinct from wayland#1239, whose subject is a rollback that DID exchange and the concurrent save `discard_displaced` then unlinks. Criteria are taken verbatim from the issue's Acceptance section.
