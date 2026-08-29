---
issue: 253
repo: FerroxLabs/wayland-core
title: "[Feature]: Bind conversation topics and threads to authorized agents"
status: open
last_verified_commit: cfa89a9c
criteria:
  - id: c1
    text: "The bindings umbrella feature is either scheduled or explicitly deferred, in writing"
    state: blocked
    owner: maintainer
    note: "this is absent behaviour, not broken behaviour - new config surface, new routing, new session identity. Slice 2 also carries a breaking migration that invalidates every acknowledge_open_admission token an operator has written. Not a defect-sweep item"
  - id: c2
    text: "OutgoingMessage carries a destination thread distinct from the quoted message it replies to"
    state: not-met
    owner: core
    note: "outgoing.rs declares conversation_id, text, reply_to and attachments and no thread_id, so a parsed destination thread has nowhere to go. Any new field needs serde default plus skip_serializing_if because the type is deny_unknown_fields"
  - id: c3
    text: "A send to a target of the form telegram:chat:topic sets message_thread_id and leaves reply_to_message_id unset"
    state: not-met
    owner: core
    note: "channel_send_transport.rs:236 assigns target.thread_id to reply_to, and the Telegram adapter maps reply_to to reply_to_message_id. message_thread_id is read inbound only - no outbound path in the tree sets it. Slack maps reply_to to thread_ts correctly and must not regress"
  - id: c4
    text: "The Telegram topic defect is filed as its own defect-labelled issue rather than living in a feature request comment thread"
    state: not-met
    owner: core
    note: "as filed today the defect is invisible to any defect-labelled query, which is exactly how it survived a release. Discord has the same class of bug through message_reference"
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
