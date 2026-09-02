---
issue: 392
repo: FerroxLabs/wayland-core
kind: task
title: "[Maintainer] Decide the permanent disposition of the WhatsApp BRIDGE chunk width (core#360 c1)"
status: open
last_verified_commit: 93ede3424
criteria:
  - id: c1
    text: "One of the two WhatsApp BRIDGE outcomes is chosen: accept BRIDGE_UNMEASURED_CHUNK_WIDTH as a documented chunking policy and reword core#360 c1 to the property actually wanted, or fund a measurement by naming who supplies a QR-paired WhatsApp number and a bridge host"
    state: blocked
    owner: maintainer
    note: "Blocked on a judgement no lane can make, which is why this ticket exists rather than the decision being taken in a ledger note. Filed 2026-08-30 by the 0.13.12 re-grade lane; carries wayland-core#360 c1. The constraint is not effort and not a credential request: measuring the bridge cap needs Node, an operator's own bridge.js with the baileys / whatsapp-web.js package installed, and a WhatsApp number QR-paired to it -- not something anybody can issue. The other branch the ticket offers, None, is proven harmful rather than merely disliked: ChannelManager::chunks_for (crates/wcore-channels/src/manager.rs:904-909) matches `Some(max) if max > 0` and falls everything else through to `vec![text.to_string()]`, so None disables chunking and sends an unbounded body at a limit no vendor publishes. That leaves accept-as-policy, which is the recommendation and is what the code already does, or fund-a-measurement. RECOMMENDATION, so this is not a bare choice: ACCEPT. It matches the wording wayland-core#364 already applies to the analogous Cloud outcome 3, the alternative is measurably worse, and the honest-disclosure scaffolding is already in place and graded (core#360 c2 the widened coverage guard, c3 the NOT-MEASURABLE disclosure, c4 the section 4.2 row, c6 the zero-cap refusal). What acceptance obliges is rewording core#360 c1 to the property actually wanted -- the number is not presented as measured, and an operator can override it -- rather than leaving a criterion nobody can satisfy. SEARCHED BEFORE FILING: gh search issues for 'whatsapp bridge cap' (nothing), 'baileys', 'max_message_len'; then wayland-core#364 c2 (Meta CLOUD cap; its own note distinguishes the bridge), wayland#1186 c5 ('whatsapp (Meta Cloud API)') and wayland#934 c6 (the bridge cap, but already `superseded` ONTO core#360) were each opened and ruled out. core#360 was the terminal owner with nothing downstream of it; that is the gap this fills."
---

Created 2026-08-30 by the 0.13.12 re-grade lane, together with the issue it tracks.

`kind: task` by the schema's own definition, and for the same reason `wayland-core#364`
carries it: the single remaining criterion is an act a human must perform — a product
judgement about what the bridge's unmeasurable cap should permanently be — with no code
behind it. The DEFECT it serves keeps blocking on its own row: `wayland-core#360` is
`kind: defect`, its c1 is `not-met` with `owner: maintainer`, and it names this ticket as
the carrier of the half no lane can do.

Classifying this `defect` would double-count one piece of work across two tickets, which is
the exact contention this release's re-grade pass exists to remove.
