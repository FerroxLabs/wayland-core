p = "crates/wcore-channel-discord/src/lib.rs"
s = open(p).read()
old = """        let reference = msg
            .reply_to
            .as_deref()
            .map(|m| rest::MessageReference { message_id: m });"""
new = """        let reference = msg
            .reply_to
            .as_deref()
            .or(msg.thread_id.as_deref())
            .map(|m| rest::MessageReference { message_id: m });"""
assert old in s, "mutation anchor miss"
s = s.replace(old, new, 1)
open(p, "w").write(s)
print("MUTATED: thread_id fallback into message_reference restored")
