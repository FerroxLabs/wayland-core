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
    note: "SETTLED 2026-08-29 by measurement: Telegram counts CHARACTERS (Unicode scalars). The shipped 4,096 is correct and safe for non-BMP text; it does NOT drop to 2,048, so the HIGH-6 this criterion was hunting does not exist. Driven at the real bot (token sha256 prefix 335f2ce4427a) and the real chat with U+1F600 as the fill, verbatim: >>> LIVE_CAP_PROBE selector=telegram shipped_cap=4096 probing_at=4096 astral=true / LIVE_CAP_AT   scalars=4096 utf8_bytes=16384 utf16_units=8192 accepted id=18 / LIVE_CAP_OVER scalars=4097 utf8_bytes=16388 utf16_units=8194 REFUSED rejected by platform: 400: Bad Request: message is too long <<< BOTH rival readings die on the PAIR of runs and NEITHER dies on either run alone, which is why the astral arm was specified as an addition to the ASCII one rather than a replacement: a 4,096 UTF-16 CODE-UNIT limit would have refused the astral body, and it was accepted at 8,192 units; a 16,384 UTF-8 BYTE budget fits the astral body exactly and survives that run, but dies on the ASCII arm, which saw 4,097 characters -- 4,097 bytes -- refused. The previous note was right that the blocker was a DESTINATION rather than the credential, and it is now recorded at /root/wl-live-cap/credentials.toml (mode 700, outside every checkout) as [telegram] chat_id, verified by getChat. THE LIVE ARM NO LONGER MERELY PRINTS. A settled claim whose live arm can only print is a claim nothing can refute, so drive_boundary now ASSERTS the recorded verdict: a MeasuredScalars cell requires the send at the cap to be accepted and the one above it refused, a MeasuredUtf16CodeUnits cell requires the opposite, and a cell still UnsettledAsciiOnly keeps print-and-stop because a discovery run has no recorded verdict to disagree with. RED ARM, run on this branch with the recorded verdict mutated to CapUnit::MeasuredUtf16CodeUnits { limit_code_units: 4_096 }, restored and touched afterwards, verbatim: >>> thread 'live_boundary_at_real_telegram' (806519) panicked at crates/wcore-channels-registry/tests/live_message_cap_boundary/driver.rs:211:17: / telegram: the 2026-08-29 run recorded a UTF-16 CODE-UNIT limit, so 4096 astral scalars (8192 code units) must be refused -- they were accepted. The recorded verdict is wrong or the platform has changed; re-measure before trusting either number. <<< Green before the mutation (id=18), green after the restore (id=20), each printing LIVE_CAP_VERDICT selector=telegram unit=scalars boundary_at=4096. Every message the probe sends is deleted before it asserts. docs/delivery-semantics.md section 4.2 carries the two-encoding row, the rewritten unit-question subsection with the two-run argument, and a correction to the credential table, whose Telegram row still said 'No' on the strength of a 2026-07-30 grep.THE HERMETIC HALF, which is what the evidence token points at, because the live arm only runs with a credential: the unsettled census in a_settled_unit_verdict_is_enforced_against_the_shipped_cap is now a RATCHET against STILL_UNSETTLED = ["slack", "discord"] rather than a print, so a key may LEAVE the list when a run settles it and nothing may join it. Not vacuous -- the census prints count=2 and names both, so the list is exactly exercised rather than compared against an empty vector. RED ARM, telegram's cell reverted to UnsettledAsciiOnly, restored and touched afterwards, verbatim: >>> thread 'a_settled_unit_verdict_is_enforced_against_the_shipped_cap' (937979) panicked at crates/wcore-channels-registry/tests/live_message_cap_boundary.rs:539:5: / ["telegram"]: settled by a real run and now recorded UnsettledAsciiOnly again. A verdict that took a credential and a destination to obtain must not be withdrawn by an edit to a literal -- re-drive the arm or take the key out of STILL_UNSETTLED on purpose <<< 10/10 green before the mutation and after the restore. That is the gap this criterion could otherwise have shipped: the measurement lives in a literal, and nothing without a Telegram credential could tell whether the literal still matched a run."
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
