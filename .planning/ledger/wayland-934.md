---
issue: 934
repo: FerroxLabs/wayland
kind: defect
title: "max_message_len is unverified across 8 adapters: the caps are asserted against themselves"
status: open
last_verified_commit: be4467ed
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
    handoff: "FerroxLabs/wayland#1186"
    note: "AUDITED 2026-08-29. Three caps still declare cap_measured = no and every one of them is now a credential or an account, not a shape: matrix and msteams are decidable (c7 closed the probe-shape half hermetically) and owe one SATURATING send each, which needs a live homeserver token and a Bot Framework registration; whatsapp (Meta Cloud) is blocked on Meta's 15-app-per-developer cap against an account holding 44 apps. #1186 is the open ticket whose title names this criterion, and its 2026-08-29 comment records the rescope and the Meta reason on the ticket itself rather than only in this ledger. The two bridge rows are not counted here -- they are c6, superseded into wayland-core#360"
  - id: c6
    text: "The WhatsApp BRIDGE cap, the eighth max_message_len, is measured or made honest and is reachable by the coverage guard"
    state: superseded
    owner: core
    note: "Split out to wayland-core#360, which is open and carries the full contract including widening the guard to backends selected by a config key. UPDATE 2026-08-29: the successor's c1/c2/c4 are now met, so the last sentence of this note is stale -- the guard walks wcore_channels_registry::constructible_selectors() rather than platform strings, the bridge has probe cells and declaration rows keyed whatsapp+baileys / whatsapp+whatsapp-web, and the borrowed Some(4096) is now BRIDGE_UNMEASURED_CHUNK_WIDTH, documented as a chunking policy with a per-channel override. Measuring it at a real backend is still owed and is that issue's c1."
  - id: c7
    text: "matrix and msteams: the two-point boundary probe is made capable of deciding a byte-budget cap, or the adapters are recorded NOT-MEASURABLE by construction"
    state: met
    evidence: "test:crates/wcore-channels-registry/tests/live_message_cap_boundary.rs::a_derived_cap_is_exactly_what_its_budget_admits"
    owner: core
    note: "Closed 2026-08-29 by the FIRST branch, not the second: these caps ARE decidable, just not by the shape that was pointed at them, so recording them NOT-MEASURABLE BY CONSTRUCTION would have been false. Three changes. (1) enum Above gained AcceptedNormally, the variant the two-point probe had no way to write down -- it is what BOTH matrix and msteams ASCII arms actually do, and each cell records it in ByteBudget::ascii_two_point, so the undecidability is data rather than an argument. (2) Boundary::Derived(ByteBudget) carries the budget, the worst-case bytes per scalar, and what the budget covers beyond the body; derivation_faults() decides the cap hermetically with NO credential -- it asserts cap * worst <= budget AND (cap+1) * worst > budget, so the number must BE the derivation rather than merely sit under it. (3) the live arm owed is now a SATURATING one -- cap astral-plane scalars, which spends the budget exactly -- driven by drive_saturating() with an ASCII control at cap+1 whose job is to be accepted. RED ARM, run on this branch with the matrix adapter mutated back to the 32,768 it shipped until 2026-08-28 (crates/wcore-channel-matrix/src/lib.rs:240 Some(16_384) -> Some(32_768)), restored and touched afterwards, verbatim: >>> thread 'a_derived_cap_is_exactly_what_its_budget_admits' (945519) panicked at crates/wcore-channels-registry/tests/live_message_cap_boundary.rs:375:9: matrix: the shipped cap disagrees with the budget it is derived from: a body of 32768 scalars costs up to 131072 bytes at 4 bytes per scalar, over the 65536 byte budget. The platform rejects the send and send_to_keyed does not re-send it -- this is HIGH-6, and it is exactly the shape matrix.cap shipped in when it was 32,768 <<< The same test was green before the mutation and green after the restore. The quoted line number is the one that run printed; the probe file was later split into live_message_cap_boundary/{cells,driver}.rs, so grep the test name rather than the line. the_derivation_checker_rejects_the_two_caps_that_actually_shipped_wrong drives the same checker over 32,768 (matrix) and 28,000 (msteams) in-process on every build, plus the needlessly-low direction, so the rule keeps a red arm without a mutation. Also a_byte_budget_cell_states_why_the_two_point_probe_cannot_decide_it refuses a byte-budget cell whose ASCII over-arm is anything but AcceptedNormally. docs/delivery-semantics.md section 4.2 carries the byte-budget subsection and both rows now say why the two-point probe cannot measure them instead of naming a credential as the blocker."
  - id: c8
    text: "Telegram's unit question is settled: the cap is characters or UTF-16 code units, measured rather than assumed"
    state: met
    evidence: "test:crates/wcore-channels-registry/tests/live_message_cap_boundary.rs::a_settled_unit_verdict_is_enforced_against_the_shipped_cap"
    owner: core
    note: "SETTLED 2026-08-29 by running it, and the previous note was WRONG about the blocker: it said no destination existed, but the chat id resolves (getChat -> ok:true) and is recorded in the probe config outside every checkout, so this was core work all along and not a maintainer item. The run, verbatim from the log: LIVE_CAP_PROBE selector=telegram shipped_cap=4096 probing_at=4096 astral=true / LIVE_CAP_AT scalars=4096 utf8_bytes=16384 utf16_units=8192 accepted id=19 / LIVE_CAP_CLEAN deleted 19 / LIVE_CAP_OVER scalars=4097 utf8_bytes=16388 utf16_units=8194 REFUSED rejected by platform: 400: Bad Request: message is too long. 8,192 UTF-16 code units were ACCEPTED at the cap and 8,194 refused one scalar later, so a 4,096 code-unit limit is refuted: Telegram counts SCALARS, the shipped 4,096 stands, and it is safe for non-BMP text -- the benign one of the two outcomes. The cell now records CapUnit::MeasuredScalars with that evidence and docs/delivery-semantics.md says so in both the cap table and section 4. RED ARM, run: reverting the cell to UnsettledAsciiOnly reddened the cited test verbatim -- >>> telegram's unit question was settled in SCALARS by a live astral run on 2026-08-29 (4,096 scalars = 8,192 UTF-16 code units accepted at the cap; 4,097 refused). Got: UnsettledAsciiOnly { needs: \"REDARM\" } <<< Restored, touched, 10/10 green"
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
presented-as-fact; it is not measured.

**Updated 2026-08-29.** Seven of the registry-constructible capped adapters now
have a committed probe cell and four have been driven live. c8 — the unit
question, and the one with a shipped HIGH-6 hiding behind it — is SETTLED: the
Telegram astral arm was driven and the platform counts scalars, so the shipped
4,096 stands. It had been recorded as blocked on a maintainer for a destination
that already existed.

What is left on c5 is three rows that still declare `cap_measured = no`, and all
three are now credentials or an account rather than a probe shape: matrix and
msteams owe one saturating send each, and whatsapp is blocked behind Meta's
15-app-per-developer cap. #1186 carries them.
