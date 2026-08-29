---
issue: 934
repo: FerroxLabs/wayland
title: "max_message_len is unverified across 8 adapters: the caps are asserted against themselves"
status: open
last_verified_commit: cfa89a9c
criteria:
  - id: c1
    text: "The five gates that could not fail now discriminate — a declaration that disagrees with the adapter is refused"
    state: met
    evidence: "test:crates/wcore-channels-registry/tests/delivery_semantics_declaration.rs::declaration_matches_every_adapter"
    owner: core
  - id: c2
    text: "That gate has a proven red arm, so it is not a second tautology"
    state: met
    evidence: "test:crates/wcore-channels-registry/tests/delivery_semantics_declaration.rs::comparator_rejects_a_flipped_row"
    owner: core
  - id: c3
    text: "Each adapter declares whether its cap was MEASURED or merely asserted, and an unknown verdict is refused outright"
    state: met
    evidence: "file:docs/delivery-semantics.md"
    owner: core
    note: "cap_measured is no / live; the tautology was hiding THREE wrong caps, not two"
  - id: c4
    text: "A boundary probe is COMMITTED for the adapters whose credentials we hold — a send at cap and at cap+1"
    state: not-met
    owner: core
    note: "Slack and Discord were measured live on 2026-08-27 and neither is credential-blocked, but the probes were never committed. Small and unblocked"
  - id: c5
    text: "Every adapter's declared cap is verified against the real platform limit"
    state: blocked
    owner: maintainer
    note: "seven adapters still declare cap_measured = no. There is no Twilio or Meta credential at all and the Matrix token was found dead on 2026-07-31; this needs credentials the core lane does not hold"
---

Partially fixed in v0.13.10.

The original complaint was that six adapters tested `max_message_len()` by
asserting the literal the function on the line above returns. That cannot
fail except by editing both halves, and it would keep passing if the number
were wrong about the platform — which is the only way it can be wrong that
matters.

The caps are now compared against a machine-readable declaration with a
proven red arm, and each adapter says whether its number was measured or
merely asserted. Visible-and-unmeasured is an improvement over
presented-as-fact; it is not measured. c4 is the cheap unblocked half and
should be done next; c5 needs credentials that are a maintainer decision.
