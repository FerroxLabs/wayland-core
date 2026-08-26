//! F24-C3-H3 regression: two different people texting the same Twilio number
//! must NOT share one conversation, one session, or one reply address.
//!
//! ## What broke
//!
//! `pairs_to_incoming` set `conversation_id` to the Twilio `To` field — the
//! bot's own number — on the reasoning that this groups each `(From, To)` pair
//! under one conversation. A deployment has ONE Twilio number, so `To` is a
//! constant and every distinct human collapsed into the same conversation id.
//!
//! Measured live at an out-of-process sink on Linux at 27d24bef, driving the
//! shipped binary with a signed Twilio webhook: an inbound SMS from
//! `+15553330000` produced an outbound whose `To` was `+15550009999` — the
//! bot's own number, read off the sink's own arrivals journal. The human who
//! texted received nothing.
//!
//! The session consequence is the more serious one. `build_session_key` for
//! `ChatType::Direct` is `agent:main:{channel}:dm:{conversation_id}` with no
//! sender component — correct on every platform where a DM conversation id
//! identifies the peer, and a cross-person context leak on the one where it
//! did not.
//!
//! ## What this test asserts
//!
//! Both directions, so it fails whichever way the field is put back: the
//! conversation must be the peer, and two peers must produce two distinct
//! session keys. Reverting `pairs_to_incoming` reddens both assertions.

use wcore_channel_sms::inbound::{pairs_to_incoming, parse_form};
use wcore_channels::dispatch::access::InboundPolicy;
use wcore_channels::dispatch::session_key::{DEFAULT_AGENT, build_session_key};

const BOT_NUMBER: &str = "+15550009999";
const ALICE: &str = "+15553330000";
const BOB: &str = "+15554440000";

fn inbound_from(peer: &str, sid: &str) -> wcore_channels::event::IncomingMessage {
    let body = format!(
        "MessageSid={sid}&From={}&To={}&Body=hi&NumMedia=0",
        peer.replace('+', "%2B"),
        BOT_NUMBER.replace('+', "%2B")
    );
    pairs_to_incoming(&parse_form(&body)).expect("parse twilio form")
}

#[test]
fn the_conversation_is_the_peer_not_the_bots_own_number() {
    let msg = inbound_from(ALICE, "SM-alice-1");

    assert_eq!(
        msg.conversation_id, ALICE,
        "the reply is addressed to conversation_id; the bot's own number here means the bot \
         texts itself and the sender receives nothing"
    );
    assert_eq!(
        msg.account_id.as_deref(),
        Some(BOT_NUMBER),
        "which bot number the message arrived on must still be recorded"
    );
    assert_eq!(msg.sender_id, ALICE);
}

#[test]
fn two_people_texting_the_same_bot_number_do_not_share_a_session() {
    let policy = InboundPolicy::default();
    let alice = build_session_key(
        DEFAULT_AGENT,
        "smschannel",
        &inbound_from(ALICE, "SM-alice-1"),
        &policy,
    );
    let bob = build_session_key(
        DEFAULT_AGENT,
        "smschannel",
        &inbound_from(BOB, "SM-bob-1"),
        &policy,
    );

    assert_ne!(
        alice, bob,
        "distinct senders must get distinct sessions; a shared key means Bob's turn can see \
         Alice's history"
    );
    assert!(
        alice.contains(ALICE) && bob.contains(BOB),
        "each session key must carry its own peer: alice={alice} bob={bob}"
    );

    // Positive control: the SAME peer twice must still be ONE session, or the
    // assertion above would be satisfied by a key that was simply unique per
    // message and no conversation would ever have a history at all.
    let alice_again = build_session_key(
        DEFAULT_AGENT,
        "smschannel",
        &inbound_from(ALICE, "SM-alice-2"),
        &policy,
    );
    assert_eq!(
        alice, alice_again,
        "the same peer must resolve to the same session across messages"
    );
}
