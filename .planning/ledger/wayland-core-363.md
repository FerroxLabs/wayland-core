---
issue: 363
repo: FerroxLabs/wayland-core
title: "[Bug]: A Telegram forum-topic target is sent as reply_to_message_id, never as message_thread_id"
status: open
last_verified_commit: 0df4c47d
criteria:
  - id: c1
    text: "OutgoingMessage carries a destination thread distinct from the quoted message it replies to"
    state: met
    evidence: "symbol:crates/wcore-channels/src/outgoing.rs::OutgoingMessage"
    owner: core
    note: "e2d083c7. thread_id: Option<String> with serde(default, skip_serializing_if) under deny_unknown_fields; reply_to keeps its own doc line saying it is NOT a destination. Transcribed from wayland-core-253 c2, which this issue splits out."
  - id: c2
    text: "A send to a target of the form telegram:chat:topic sets message_thread_id and leaves reply_to_message_id unset"
    state: met
    evidence: "test:crates/wcore-channel-telegram/src/lib.rs::outbound_forum_topic_sets_message_thread_id_not_reply_to"
    owner: core
    note: "Asserts on the serialized wire body, needing no live credential. channel_send_transport.rs:242 routes target.thread_id into thread_id and leaves reply_to empty. Transcribed from wayland-core-253 c3."
  - id: c3
    text: "Every chunk of a chunked send carries the thread destination, and a threadless send does not invent one"
    state: met
    evidence: "test:crates/wcore-channels/src/manager.rs::a_chunked_send_carries_the_thread_destination_on_every_chunk"
    owner: core
    note: "Control: a_chunked_send_without_a_thread_does_not_invent_one, same file. Transcribed from wayland-core-253 c6."
  - id: c4
    text: "The inbound and default-agent-reply arm inherits the thread as a DESTINATION and the quote separately"
    state: met
    evidence: "test:crates/wcore-agent/src/channel_inbound.rs::a_reply_inherits_the_thread_as_a_destination_and_the_quote_separately"
    owner: core
    note: "The sibling outbound_quote_target_does_not_fall_back_to_thread closes the fallback. Transcribed from wayland-core-253 c5."
  - id: c5
    text: "Slack does not regress: thread_ts comes from the thread destination and a thread root supplied as reply_to is still honoured"
    state: met
    evidence: "test:crates/wcore-channel-slack/src/lib.rs::slack_takes_its_thread_ts_from_the_thread_destination"
    owner: core
    note: "Wrong-refusal twin: slack_still_honours_a_thread_root_supplied_as_reply_to. Transcribed from wayland-core-253 c7."
  - id: c6
    text: "Discord does not regress: a thread channel id never reaches message_reference"
    state: not-met
    owner: core
    note: "Filed 2026-08-29 with the issue. The removal of the outbound_reply_target thread fallback is what closes this in code - Discord reads reply_to as a genuine message_reference and routes threads by conversation_id, so the old fallback put a thread CHANNEL id into a message reference. There is NO Discord-specific test: the only graded arm is a_reply_inherits_the_thread_as_a_destination_and_the_quote_separately in wcore-agent, which grades the shared transport and names no connector. Not-met because the connector half is inferred from the shared path, and inferring a connector's behaviour from a shared helper is how the Telegram defect survived in the first place."
---

Split out of #253 on 2026-08-29 under that issue's c4, which required the buried
Telegram defect to be filed as its own defect-labelled issue rather than living
in a feature request's comment thread. Filtering every state of the tracker on
telegram / topic / thread had returned only #253 itself, so a defect that is
FIXED in code was invisible to any defect-labelled query - the exact condition
#253 c4 names.

The mechanism: `parse_target` reads the third segment of `platform:chat:thread`
as a destination thread, `channel_send_transport` put that value in
`OutgoingMessage::reply_to`, and the Telegram adapter forwards `reply_to` as
`reply_to_message_id`. A topic id is not a message id. Best case the API
refuses; worse, the topic id collides with a real unrelated message and the bot
quotes a stranger.

c1 through c5 are transcribed verbatim from the graded rows of
`.planning/ledger/wayland-core-253.md` and every cited test was re-checked in
this tree at 0df4c47d. c6 is new and deliberately open: the umbrella issue's
notes twice observe that Discord carries the same class, and nothing grades the
connector.

The umbrella feature request (#253) stays open and unscheduled - see Q6 in
`.planning/DECISIONS.md`. This file tracks only the defect.
