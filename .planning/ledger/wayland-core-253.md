---
issue: 253
repo: FerroxLabs/wayland-core
title: "[Feature]: Bind conversation topics and threads to authorized agents"
status: open
last_verified_commit: 43848f75
criteria:
  - id: c1
    text: "The bindings umbrella feature is either scheduled or explicitly deferred, in writing"
    state: met
    evidence: "file:.planning/DECISIONS.md"
    owner: maintainer
    note: "TAKEN 2026-08-29 as Q6: keep the umbrella OPEN and UNSCHEDULED, and split the buried Telegram defect out now. That is the explicit written deferral the criterion asks for. The reason is recorded with it: slice 2 carries a breaking migration (SHAPE_FIELDS 13 to 14, ADMISSION_SHAPE_VERSION admission-v2 to v3) that invalidates every acknowledge_open_admission token an operator has written."
  - id: c2
    text: "OutgoingMessage carries a destination thread distinct from the quoted message it replies to"
    state: met
    evidence: "symbol:crates/wcore-channels/src/outgoing.rs::OutgoingMessage"
    owner: core
    note: "e2d083c7. thread_id: Option<String> at outgoing.rs:23 with serde(default, skip_serializing_if) as the ledger required under deny_unknown_fields; reply_to keeps its own doc line saying it is NOT a destination. The ::text() constructor was updated so the silent-default path sets both to None."
  - id: c3
    text: "A send to a target of the form telegram:chat:topic sets message_thread_id and leaves reply_to_message_id unset"
    state: met
    evidence: "test:crates/wcore-channel-telegram/src/lib.rs::outbound_forum_topic_sets_message_thread_id_not_reply_to"
    owner: core
    note: "Asserts on the serialized wire body with message_thread_id set and no reply_to_message_id, needing no live credential. channel_send_transport.rs:242 routes target.thread_id into thread_id and leaves reply_to empty; the red arm topic_destination_never_occupies_the_reply_quote_slot and its control threadless_target_still_delivers_with_no_reply_quote sit at the transport."
  - id: c4
    text: "The Telegram topic defect is filed as its own defect-labelled issue rather than living in a feature request comment thread"
    state: not-met
    owner: core
    note: "VERIFIED ABSENT against all 200 issues in the tracker: filtering every state on telegram, topic and thread returns only #253 itself (open, no labels), #210 and #110 (both closed, unrelated). The defect is FIXED in code but is still tracked only inside a feature request's comment thread, so it stays invisible to any defect-labelled query - which is exactly the condition this criterion names. Discord's message_reference carries the same class of bug and is likewise unfiled."
  - id: c5
    text: "The inbound and default-agent-reply arm inherits the thread as a DESTINATION and the quote separately"
    state: met
    evidence: "test:crates/wcore-agent/src/channel_inbound.rs::a_reply_inherits_the_thread_as_a_destination_and_the_quote_separately"
    owner: core
    note: "Added 2026-08-29; the buried defect's acceptance has four halves and the ledger carried only the outbound one. The sibling outbound_quote_target_does_not_fall_back_to_thread closes the fallback that also put a Discord thread CHANNEL id into a message reference."
  - id: c6
    text: "Every chunk of a chunked send carries the thread destination, and a threadless send does not invent one"
    state: met
    evidence: "test:crates/wcore-channels/src/manager.rs::a_chunked_send_carries_the_thread_destination_on_every_chunk"
    owner: core
    note: "Added 2026-08-29. Control: a_chunked_send_without_a_thread_does_not_invent_one, in the same file."
  - id: c7
    text: "Slack does not regress: thread_ts comes from the thread destination and a thread root supplied as reply_to is still honoured"
    state: met
    evidence: "test:crates/wcore-channel-slack/src/lib.rs::slack_takes_its_thread_ts_from_the_thread_destination"
    owner: core
    note: "Added 2026-08-29. The wrong-refusal twin is slack_still_honours_a_thread_root_supplied_as_reply_to, so the new field cannot break the adapter that already worked."
---

Two different things are tracked under one number.

The umbrella is a genuine feature request - platform-neutral conversation and
thread bindings, eight sub-designs, a twelve-line acceptance matrix. Nothing in
it describes broken behaviour, so it stays open and unscheduled, and whether it
gets scheduled is a maintainer decision.

Buried in its comment thread is a standalone, shipped defect. The documented
three-segment target form parses a Telegram topic id and then hands it to the
reply-to field, so every send to a forum topic asks Telegram to quote a message
id that is really a topic id, and sets no thread destination. Best case the API
refuses it; worse, it collides with an unrelated real message. The branch that
fixed this never reached v0.13.10.

Criteria come from the cluster F verification note of 2026-08-29, which read
every line cited here at the shipped tag. c2 through c4 are the defect; c1 is
the feature.
