---
issue: 360
repo: FerroxLabs/wayland-core
kind: defect
title: "The WhatsApp bridge ships a message cap borrowed from Meta's docs, and the coverage guard structurally cannot see it"
status: open
last_verified_commit: 43848f75
criteria:
  - id: c1
    text: "The bridge's cap is measured against a real baileys or whatsapp-web.js backend, or the borrowed Some(4096) is replaced by something honest"
    state: met
    evidence: "symbol:crates/wcore-channel-whatsapp/src/bridge/mod.rs::BRIDGE_UNMEASURED_CHUNK_WIDTH"
    owner: core
    note: "Closed by the SECOND branch. The first is still not reachable here -- it needs Node, an operator's own bridge.js with the backend package installed, and a WhatsApp number QR-paired to it, and none of that is a credential anybody can issue. What changed: max_message_len() no longer returns a bare Some(4096) justified by Meta's Cloud API text.body page, which governs a surface this code never touches. It returns WhatsappBridgeConfig::max_message_chars when the operator set one and BRIDGE_UNMEASURED_CHUNK_WIDTH otherwise, and that constant's documentation says in as many words that it is a CHUNKING POLICY rather than a platform limit, with the asymmetry that chose it: too high loses messages (HIGH-6), too low only splits a reply that need not have been split, and None is unavailable because it disables chunking and sends an unbounded body at a limit nobody has published. The override is the honest half -- an operator who HAS driven their own bridge previously had nowhere to put the finding, so our unsourceable guess stayed load-bearing for everybody. Two tests, both green: the_chunk_width_is_the_operators_when_they_set_one_and_the_policy_default_otherwise drives both directions, and the_unmeasured_default_stays_finite_and_conservative reads through the ADAPTER rather than the constant and refuses a raise without a recorded boundary run. docs/delivery-semantics.md section 4.2 carries the row and says the number is a policy; crates/wcore-channel-whatsapp/schemas/whatsapp-bridge.json carries the knob."
  - id: c2
    text: "The coverage guard reaches backends selected by a CONFIG KEY, not only by platform string, so a ninth adapter in this shape cannot appear unprobed"
    state: met
    evidence: "test:crates/wcore-channels-registry/tests/live_message_cap_boundary.rs::every_capped_adapter_has_a_probe_cell"
    owner: core
    note: "The class is closed at the REGISTRY, not at the guard. wcore-channels-registry gained src/selector.rs: ChannelSelector { platform, options, key } plus constructible_selectors(), which enumerates what the registry can BUILD -- nine implementations, not seven platforms -- with the WhatsApp arm derived from WhatsappBackend::ALL_WIRE_NAMES so a fourth backend appears in every downstream gate with no second list to remember. Keys are the bare platform tag for a platform's default implementation and platform+<value> where a config key selects another, so every row written before this change keeps its name. Both gates walk it: shipped_caps() in live_message_cap_boundary.rs and measured_capabilities() in delivery_semantics_declaration.rs. CELLS is keyed by selector and carries whatsapp+baileys and whatsapp+whatsapp-web cells. every_capped_adapter_has_a_probe_cell additionally asserts that at least one config-keyed implementation reached it, so the widening cannot silently stop widening; the registry side is held by the_selector_list_is_wider_than_the_platform_list_by_the_config_keyed_backends, which reddens when a WhatsappBackend variant is added. RED ARM, run on this branch with both bridge cells deleted from tests/live_message_cap_boundary/cells.rs, restored and touched afterwards, verbatim: >>> thread 'every_capped_adapter_has_a_probe_cell' (857160) panicked at crates/wcore-channels-registry/tests/live_message_cap_boundary.rs:626:9: whatsapp+baileys declares a finite max_message_len() but has no cell in this file. Add one -- as NotMeasured naming the blocker if nobody can drive it -- so the gap is stated rather than absent. <<< The same test was green before the mutation and green after the restore. Line numbers in the quoted panics are the ones the run printed; the probe file was later split into live_message_cap_boundary/{cells,driver}.rs and the tests moved up, so grep the test name rather than the line."
  - id: c3
    text: "If the cap genuinely cannot be measured, that is recorded as a stated NOT-MEASURABLE with the reason, the way Matrix and MS Teams are"
    state: met
    evidence: "file:docs/delivery-semantics.md:418"
    owner: core
    note: "The prose at docs/delivery-semantics.md:417-426 states the number is UNVERIFIED and cannot be sourced, and says why. This is a disclosure, NOT a substitute for c1 or c2 - it is the honest interim only."
  - id: c4
    text: "A section 4.2 declaration row exists for the bridge, or the reason it cannot have one is enforced by a test rather than by a comment"
    state: met
    evidence: "test:crates/wcore-channels-registry/tests/delivery_semantics_declaration.rs::comparator_rejects_the_bridge_rows_going_missing"
    owner: core
    note: "The row exists, so the second branch is moot. docs/delivery-semantics.md gained whatsapp+baileys and whatsapp+whatsapp-web rows in the machine-readable block (guarantee, cap, cap_measured, cap_source), a section 4.2 cap-table row, and a blocked-probe row saying the blocker is a QR-paired account rather than a credential anybody can issue; the section 2 prose row already existed and is now COVERED rather than merely labelled. cap_source must be a URL and no vendor page governs this surface, so it names the decision -- wayland-core#360 -- and the block's prose says why that is the accountable answer: an unsourceable number must say so at a URL a reader can open, not carry a citation from the wrong vendor. The evidence test is not a duplicate of comparator_rejects_a_missing_row: that one proves the rule fires for a platform-keyed row, whereas the bridge is the row that could not EXIST until the harness stopped enumerating platform strings. It removes both bridge rows from the parsed declaration, requires exactly two NO-row-in reports naming the selector keys, and requires the Cloud API row NOT to be implicated -- the three WhatsApp implementations share one platform string, so a platform-keyed comparator would have reported the wrong one or none at all. the_two_whatsapp_row_labels_cannot_be_confused_by_a_prefix_match holds the neighbouring trap: every table lookup in that file is a starts_with, and the two labels are one closing asterisk pair apart."
  - id: c5
    text: "A red arm is quoted verbatim"
    state: met
    evidence: "test:crates/wcore-channels-registry/tests/live_message_cap_boundary.rs::every_capped_adapter_has_a_probe_cell"
    owner: core
    note: "Three, each run on this branch with the tree committed BEFORE the mutation and the file restored and touched afterwards, quoted verbatim from the run output. ARM 1 (c2), both bridge cells deleted from tests/live_message_cap_boundary/cells.rs: >>> thread 'every_capped_adapter_has_a_probe_cell' (857160) panicked at crates/wcore-channels-registry/tests/live_message_cap_boundary.rs:626:9: whatsapp+baileys declares a finite max_message_len() but has no cell in this file. Add one -- as NotMeasured naming the blocker if nobody can drive it -- so the gap is stated rather than absent. <<< ARM 2 (c4), the four whatsapp+baileys lines deleted from the machine-readable block: >>> thread 'declaration_matches_every_adapter' (868161) panicked at crates/wcore-channels-registry/tests/delivery_semantics_declaration.rs:506:5: assertion left == right failed: docs/delivery-semantics.md must carry a row for all twelve implementations (including the macOS-only iMessage and both bridged WhatsApp backends), found 11 / left: 11 / right: 12 <<< ARM 3 (wayland#934 c7), the matrix adapter mutated back to the 32,768 it shipped until 2026-08-28 at crates/wcore-channel-matrix/src/lib.rs:240: >>> thread 'a_derived_cap_is_exactly_what_its_budget_admits' (945519) panicked at crates/wcore-channels-registry/tests/live_message_cap_boundary.rs:375:9: matrix: the shipped cap disagrees with the budget it is derived from: a body of 32768 scalars costs up to 131072 bytes at 4 bytes per scalar, over the 65536 byte budget. The platform rejects the send and send_to_keyed does not re-send it -- this is HIGH-6, and it is exactly the shape matrix.cap shipped in when it was 32,768 <<< Each test was green before its mutation and green again after the restore, so the red is caused by the mutation and not by pre-existing drift."
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
