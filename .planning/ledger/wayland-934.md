---
issue: 934
repo: FerroxLabs/wayland
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
    note: "Closed 2026-08-29 by the FIRST branch, not the second: these caps ARE decidable, just not by the shape that was pointed at them, so recording them NOT-MEASURABLE BY CONSTRUCTION would have been false. Three changes. (1) enum Above gained AcceptedNormally, the variant the two-point probe had no way to write down -- it is what BOTH matrix and msteams ASCII arms actually do, and each cell records it in ByteBudget::ascii_two_point, so the undecidability is data rather than an argument. (2) Boundary::Derived(ByteBudget) carries the budget, the worst-case bytes per scalar, and what the budget covers beyond the body; derivation_faults() decides the cap hermetically with NO credential -- it asserts cap * worst <= budget AND (cap+1) * worst > budget, so the number must BE the derivation rather than merely sit under it. (3) the live arm owed is now a SATURATING one -- cap astral-plane scalars, which spends the budget exactly -- driven by drive_saturating() with an ASCII control at cap+1 whose job is to be accepted. RED ARM, run on this branch with the matrix adapter mutated back to the 32,768 it shipped until 2026-08-28 (crates/wcore-channel-matrix/src/lib.rs:240 Some(16_384) -> Some(32_768)), restored and touched afterwards, verbatim: >>> thread 'a_derived_cap_is_exactly_what_its_budget_admits' (945519) panicked at crates/wcore-channels-registry/tests/live_message_cap_boundary.rs:375:9: matrix: the shipped cap disagrees with the budget it is derived from: a body of 32768 scalars costs up to 131072 bytes at 4 bytes per scalar, over the 65536 byte budget. The platform rejects the send and send_to_keyed does not re-send it -- this is HIGH-6, and it is exactly the shape matrix.cap shipped in when it was 32,768 <<< The same test was green before the mutation and green after the restore. the_derivation_checker_rejects_the_two_caps_that_actually_shipped_wrong drives the same checker over 32,768 (matrix) and 28,000 (msteams) in-process on every build, plus the needlessly-low direction, so the rule keeps a red arm without a mutation. Also a_byte_budget_cell_states_why_the_two_point_probe_cannot_decide_it refuses a byte-budget cell whose ASCII over-arm is anything but AcceptedNormally. docs/delivery-semantics.md section 4.2 carries the byte-budget subsection and both rows now say why the two-point probe cannot measure them instead of naming a credential as the blocker."
  - id: c8
    text: "Telegram's unit question is settled: the cap is characters or UTF-16 code units, measured rather than assumed"
    state: blocked
    owner: maintainer
    note: "NOT SETTLED, and the blocker is narrower than the previous note implied. The credential IS held and works: the bot token at /root/wl-live-cap/credentials.toml (sha256 prefix 335f2ce4427a) answers getMe as WaylandTestBot. What is missing is a DESTINATION. No chat id was recorded by the 2026-08-29 run, getUpdates now returns zero rows (that run confirmed its offset and Telegram drops confirmed updates), and a bot cannot obtain a chat by itself. Both cheaper routes were MEASURED shut on this branch: sendMessage to a nonexistent chat_id answers Bad Request: chat not found at BOTH 4,096 and 4,097 characters, and to a public channel the bot has not joined it answers Forbidden: bot is not a member of the channel chat at 10, 4,096 and 4,097 -- so Telegram resolves the chat and the membership BEFORE it validates the length, and no destination means no verdict. EVERYTHING ELSE IS AUTHORED AND GREEN: CapUnit records which unit a cell has settled and telegram is UnsettledAsciiOnly with the reason; unit_safety_faults() refuses a scalar cap above limit/2 once a UTF-16 verdict is recorded; the_unit_rule_refuses_a_cap_a_utf16_verdict_makes_unsafe constructs exactly the verdict a Telegram astral run would produce and requires the checker to refuse today's 4,096; and the live arm is committed -- drive_boundary() fills the body with U+1F600 instead of x when WL_LIVE_CAP_TELEGRAM_ASTRAL=1 and prints LIVE_CAP_UNIT with both arms. ONE HUMAN STEP UNBLOCKS IT: message the bot once from any Telegram account (or add it to a group), read the chat id out of getUpdates, then run WL_LIVE_CAP_TELEGRAM_HOME=/root/wl-live-cap WL_LIVE_CAP_TELEGRAM_CHANNEL=cap-telegram WL_LIVE_CAP_TELEGRAM_TO=<chat id> WL_LIVE_CAP_TELEGRAM_ASTRAL=1 cargo test -p wcore-channels-registry --test live_message_cap_boundary -- --ignored --nocapture live_boundary_at_real_telegram. A REFUSAL at 4,096 astral scalars means Telegram counts UTF-16 code units, today's cap is unsafe for non-BMP text, and it must drop to 2,048 -- a shipped HIGH-6 defect, not a documentation gap. An ACCEPT means it counts scalars and the cap stands."
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
