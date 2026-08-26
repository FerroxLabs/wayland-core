import re, pathlib
p = pathlib.Path("crates/wcore-agent/src/channel_inbound.rs")
s = p.read_text()
anchor = """    #[test]
    fn outbound_reply_target_prefers_reply_id_over_thread() {"""
assert s.count(anchor) == 1, s.count(anchor)
red = """    // RED ARM (core#253 §5): a destination TOPIC id must never be handed to a
    // connector as a quoted-message id. Telegram stamps `thread_id` from
    // `message_thread_id` (a forum topic), and `reply_to_message_id` from a real
    // quoted message. Collapsing the two makes every in-topic reply quote a
    // message id that does not exist.
    #[test]
    fn a_topic_id_is_never_used_as_a_quoted_message_id() {
        let mut m = dm("tg");
        m.platform = Some("telegram".into());
        m.thread_id = Some("77".into()); // forum topic, NOT a message id
        m.reply_to_message_id = None;
        assert_eq!(
            outbound_reply_target(&m),
            None,
            "topic id leaked into the reply-to slot"
        );
        assert_eq!(outbound_thread_target(&m), Some("77".to_string()));
    }

"""
s = s.replace(anchor, red + anchor, 1)
p.write_text(s)
print("patched")
