---
issue: 360
repo: FerroxLabs/wayland-core
title: "The WhatsApp bridge ships a message cap borrowed from Meta's docs, and the coverage guard structurally cannot see it"
status: open
last_verified_commit: 43848f75
criteria:
  - id: c1
    text: "The bridge's cap is measured against a real baileys or whatsapp-web.js backend, or the borrowed Some(4096) is replaced by something honest"
    state: not-met
    owner: core
    note: "crates/wcore-channel-whatsapp/src/bridge/mod.rs:596-598 still returns Some(4096), carried over from Meta's Cloud API docs which do not govern either backend this channel drives. Not credential-blocked: it needs a running bridge, a Node subprocess driving a paired account."
  - id: c2
    text: "The coverage guard reaches backends selected by a CONFIG KEY, not only by platform string, so a ninth adapter in this shape cannot appear unprobed"
    state: not-met
    owner: core
    note: "This is the criterion that closes the class. CELLS in crates/wcore-channels-registry/tests/live_message_cap_boundary.rs is still keyed by platform tag, so every_capped_adapter_has_a_probe_cell structurally cannot reach the bridge."
  - id: c3
    text: "If the cap genuinely cannot be measured, that is recorded as a stated NOT-MEASURABLE with the reason, the way Matrix and MS Teams are"
    state: met
    evidence: "file:docs/delivery-semantics.md:418"
    owner: core
    note: "The prose at docs/delivery-semantics.md:417-426 states the number is UNVERIFIED and cannot be sourced, and says why. This is a disclosure, NOT a substitute for c1 or c2 - it is the honest interim only."
  - id: c4
    text: "A section 4.2 declaration row exists for the bridge, or the reason it cannot have one is enforced by a test rather than by a comment"
    state: not-met
    owner: core
    note: "The bridge contributes no declaration row because the harness enumerates platform strings; today the reason lives in a doc comment at bridge/mod.rs:654-658 and in prose, neither of which can go red."
  - id: c5
    text: "A red arm is quoted verbatim"
    state: not-met
    owner: core
    note: "Depends on c1 or c2 landing."
---

Found while measuring the WhatsApp cap for `wayland#934`. The Cloud API cell is a
separate matter; the BRIDGE backend cannot be measured the same way and the
number it ships is borrowed from the wrong vendor.

If the real backend limit is lower, sends fail or truncate. If it is higher, the
chunker splits messages nobody asked it to split. Neither has ever been observed.

The more important half of this ticket is the second criterion. This is the
eighth `max_message_len` in the product and the only one no test and no
declaration row touches, for two structural reasons already recorded in-tree: the
declaration harness enumerates platforms the registry builds FROM A PLATFORM
STRING, and the bridge is reached through `whatsapp` plus a `backend` key. The
guard that exists to make an unprobed cap impossible has a blind spot shaped
exactly like this adapter. Measuring one number without widening the guard just
moves the blind spot.
