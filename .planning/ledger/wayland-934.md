---
issue: 934
repo: FerroxLabs/wayland
kind: defect
title: "max_message_len is unverified across 8 adapters: the caps are asserted against themselves"
status: open
last_verified_commit: 43848f75
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
    evidence: "test:crates/wcore-channels-registry/tests/delivery_semantics_declaration.rs::comparator_rejects_a_cap_row_with_no_measured_verdict"
    owner: core
    note: "Upgraded from a bare doc pointer: the doc row is data, this is the gate that refuses a missing verdict. Unknown verdicts are refused in parse_declaration's matches!(v, 'no'|'live') assert."
  - id: c4
    text: "A boundary probe is COMMITTED for the adapters whose credentials we hold — a send at cap and at cap+1"
    state: met
    evidence: "test:crates/wcore-channels-registry/tests/live_message_cap_boundary.rs::every_capped_adapter_has_a_probe_cell"
    owner: core
    note: "All seven registry-constructible capped adapters now have a cell. MEASURED: slack 4,040/4,041 SilentlyReshaped (2026-08-27); discord 2,000/2,001 Refused 50035 (2026-08-27); telegram 4,096/4,097 Refused (2026-08-29); sms 1,600/1,601 Refused code 21617 (2026-08-29). NotMeasured with a named blocker: matrix, whatsapp, msteams. The slack/discord hardcoded exemption at delivery_semantics_declaration.rs:763 is still present and now also lists telegram and sms, but is no longer load-bearing - live_message_cap_boundary.rs:522 backstops it in both directions."
  - id: c5
    text: "Every adapter's declared cap is verified against the real platform limit"
    state: blocked
    owner: maintainer
    note: "Three caps still declare cap_measured = no, and the reasons are now DIFFERENT from each other. whatsapp (Meta Cloud API) is blocked on Meta's 15-app-per-developer cap against an account holding 44 apps - see wayland#1186 c5. matrix and msteams are NOT credential-blocked at all; see c7. The previous note said seven unmeasured and no Twilio credential; both are now false."
  - id: c6
    text: "The WhatsApp BRIDGE cap, the eighth max_message_len, is measured or made honest and is reachable by the coverage guard"
    state: superseded
    owner: core
    note: "Split out to wayland-core#360, which is open and carries the full contract including widening the guard to backends selected by a config key. bridge/mod.rs:596 still returns a borrowed Some(4096) and contributes no declaration row, so every_capped_adapter_has_a_probe_cell structurally cannot reach it."
  - id: c7
    text: "matrix and msteams: the two-point boundary probe is made capable of deciding a byte-budget cap, or the adapters are recorded NOT-MEASURABLE by construction"
    state: not-met
    owner: core
    note: "RESCOPE 2026-08-29, and this half is CORE'S, not the credentials request's. Both caps derive from byte budgets (65,536-byte PDU, 80 KB UTF-16 Activity), so an ASCII send at cap and cap+1 lands inside the accepted region at BOTH arms and enum Above (live_message_cap_boundary.rs:122) has only Refused and SilentlyReshaped - no variant for accepted normally. A credential would not close this; a probe-shape change would."
  - id: c8
    text: "Telegram's unit question is settled: the cap is characters or UTF-16 code units, measured rather than assumed"
    state: not-met
    owner: core
    note: "The 2026-08-29 probe was driven in ASCII, which cannot distinguish the two. The cell itself records the question as open. An astral-plane run would settle it and the credential to do so is already held, so this is unblocked."
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
