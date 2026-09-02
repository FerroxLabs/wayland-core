---
issue: 363
repo: FerroxLabs/wayland-core
title: "[Bug]: A Telegram forum-topic target is sent as reply_to_message_id, never as message_thread_id"
status: closed
kind: defect
last_verified_commit: 93ede3424
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
    state: met
    evidence: "test:crates/wcore-channel-discord/src/lib.rs::discord_never_sends_a_thread_destination_as_a_message_reference"
    owner: core
    note: "CLOSED 2026-08-29 (lane f13-fin-hetzner-residuals) by grading the CONNECTOR, which is what the entry said was missing: the only arm was a_reply_inherits_the_thread_as_a_destination_and_the_quote_separately in wcore-agent, which grades the shared transport and names no connector, and inferring a connector's behaviour from a shared helper is how the Telegram defect survived in the first place. Two tests now drive Discord's own send path (send_message_idempotent -> post_message -> rest::send_message) to a mockito endpoint and match the wire body EXACTLY. Exactness is load-bearing: message_reference is skip_serializing_if Option::is_none, so its ABSENCE is only observable against a whole-body match and a partial match would pass on a body that carried it; the derived nonce is what makes an exact match possible. The thread id used is a real-shaped channel snowflake, because a Discord thread id is indistinguishable in shape from a message id -- which is why the old fallback produced a well-formed reference to the WRONG OBJECT rather than an obvious error. WRONG-REFUSAL TWIN: discord_still_quotes_a_genuine_reply_to_message sets reply_to AND thread_id at once and requires message_reference to carry the reply_to; without it, `no message_reference` is satisfied by a connector that stopped sending replies at all. NOT VACUOUS -- MUTATION MEASURED. Restoring the exact fallback the fix removed (`msg.reply_to.as_deref().or(msg.thread_id.as_deref())` in post_message) reddens the first test and ONLY it, verbatim: `thread 'tests::discord_never_sends_a_thread_destination_as_a_message_reference' (898989) panicked at crates/wcore-channel-discord/src/lib.rs:1395:9: / Discord put something other than {content, nonce} on the wire: the thread DESTINATION was spent as a message_reference (FerroxLabs/wayland-core#363 c6). Send result: Err(Transport(\"server 501 Not Implemented\"))` -- `test result: FAILED. 1 passed; 1 failed`, the passing one being the twin, which is correct because it sets reply_to and the fallback cannot change it. The assertion is made on mock.matched_async(), not on the send: an unexpected body simply matches no mock, and the 501 that comes back names the symptom rather than the defect. Mutation reverted, file touched, both green: `test result: ok. 2 passed`."

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

## Re-graded at HEAD by lane f13-u-flake-chan, 2026-08-29

OWNED ELSEWHERE, NOT DUPLICATED. c6 is closed on
`lane/f13-fin-hetzner-residuals` (`d35ac0a0c` writes the two Discord connector
tests, `a612b07e3` grades the row). That branch is not merged into
`origin/integ/f13`, so the row above still reads `not-met` on THIS tree.

Independently re-checked here rather than taken on the note's word, on the
product code rather than on the test:
`crates/wcore-channel-discord/src/lib.rs::post_message` builds
`rest::MessageReference` from `msg.reply_to` and from nothing else -- there is
no `.or(msg.thread_id)` and no second construction site in the crate (one
`message_reference:` assignment at lib.rs:176, one field at rest.rs:43, one
`None` at rest.rs:608). The upstream halves hold too:
`channel_send_transport.rs:242` routes `target.thread_id` into
`OutgoingMessage::thread_id` and leaves `reply_to` empty, and
`channel_inbound.rs::outbound_reply_target` no longer falls back to the thread
(its doc comment names the Discord consequence explicitly). So c6 holds in code
on this tree; what was missing, and what that lane supplied, is a test that
grades the CONNECTOR instead of inferring it from the shared transport.

No work taken here: duplicating that test would race a lane that has already
measured its red arm.
