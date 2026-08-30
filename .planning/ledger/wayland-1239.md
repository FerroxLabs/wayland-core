---
issue: 1239
repo: FerroxLabs/wayland
kind: defect
title: "atomic_write_checked destroys a concurrent save that lands inside the exchange-to-verdict window, and the refusal is byte-identical to one that lost nothing"
status: open
last_verified_commit: 21549260
criteria:
  - id: c1
    text: "A save that lands on the displaced inode inside the exchange-to-verdict window is not silently destroyed on the refusal path: it is preserved on disk, or the caller is told, or the write is not refused over it"
    state: not-met
    owner: core
    note: "FILED 2026-08-30 by lane/f13-n-window while answering the adversarial verifier objection misfiled as OBJECTIONS/window.md (its body verifies lane/f13-n-dataloss, not this lane). REPRODUCED, not modelled, at 8501e1cf3 in /root/w-f13/n-window with a three-arm probe test appended to crates/wcore-config/src/atomic_io.rs and run as `cargo test -p wcore-config --lib atomic_io -- --nocapture`; the probe was reverted with `git checkout HEAD --` + touch and the tree verified clean afterwards, so nothing here is committed. Measured: ARM_A (a non-cooperating editor saves in place inside the exchange->verdict window, verdict refuses) -> outcome Ok(Err(\"changed under write\")), dest=\"original\", dir=[(\"f.txt\", \"original\")], their_save_survives_anywhere=false, caller_sees_hard_error=false. ARM_B (CONTROL, nobody else writes) -> byte-identical on all three: same return value, same destination bytes, same directory listing. That identity IS the defect statement - nothing the caller can read separates a refusal that destroyed a save from one that destroyed nothing. ARM_C (CONTROL for probe sensitivity: same concurrent save, verdict ACCEPTS) -> Ok(Ok(())) with dest=\"THEIRSAVE\", so the save survives on the accept path and the ARM_A loss is attributable to the rollback, not to the harness or the exchange. SCOPE: distinct from wayland#1202, whose restore exchanges NOTHING (Swap::Vacant/Unsupported); here the restore exchanges correctly and the loss is in discard_displaced unlinking the displaced inode after a third party wrote into it. Present unchanged on origin/integ/f13 a278f8c3b - not introduced by any f13 lane. NOT ATTEMPTED here: the fix lives in crates/wcore-config/src/atomic_io.rs, which unmerged lane/f13-n-dataloss rewrites by +87 lines, so editing it from this lane would collide; and preserving a third party's bytes rather than the pre-image is a design decision, not a one-liner."
  - id: c2
    text: "The caller can distinguish the ARM_A outcome from the ARM_B outcome - a refusal that destroyed someone's save is not byte-identical to a refusal that destroyed nothing"
    state: not-met
    owner: core
    note: "Measured identical at 8501e1cf3; see c1."
  - id: c3
    text: "A test drives the ARM_A / ARM_B / ARM_C triple and is shown RED against today's discard_displaced, with ARM_C proving the probe can observe survival"
    state: not-met
    owner: core
    note: "The probe exists and its three arms are recorded verbatim in c1, but it was run as a MEASUREMENT and reverted, not committed as a test. This criterion is not met until the triple is checked in and shown red."
  - id: c4
    text: "The atomic_io.rs doc comment stops claiming the exchange closes the race, or the claim becomes true; the residual window is described in terms of what it actually loses, not only a crash"
    state: not-met
    owner: core
    note: "The claim under test is the module doc comment: RENAME_EXCHANGE `closes it instead of narrowing it`, with the residual window's cost named as `a crash`. Measured above, the residual window also loses a concurrent save on the ordinary no-crash refusal path. Narrowed (read->rename becomes exchange->verdict), not closed."
  - id: c5
    text: "The existing atomic_io tests, including wayland#1202's a_rollback_that_exchanged_nothing_is_not_a_clean_refusal where merged, stay green"
    state: not-met
    owner: core
    note: "Baseline at 8501e1cf3 with the probe applied: `test result: ok. 10 passed; 0 failed` for the atomic_io filter."
---

Decomposed out of the lane/f13-n-dataloss verifier objection rather than fixed in place.

The objection asserted that wayland#1155 c5's class enumeration was false. Read as written,
that note enumerates the cfg arms of `fn restore` -- there are exactly two, and it addresses
both -- and c5's criterion text is about a rollback that exchanged NOTHING, which is not what
the objection's probe exercises. The enumeration stands. What does not stand is the module's
own doc claim that the exchange CLOSES the race: the probe above is a real, unticketed second
loss on the refusal path, so it gets its own criteria here instead of being folded into a
criterion whose sentence it does not falsify.
