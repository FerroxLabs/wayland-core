p = "crates/wcore-channel-discord/src/lib.rs"
s = open(p).read()
mutated = """        let reference = msg
            .reply_to
            .as_deref()
            .or(msg.thread_id.as_deref())
            .map(|m| rest::MessageReference { message_id: m });"""
clean = """        let reference = msg
            .reply_to
            .as_deref()
            .map(|m| rest::MessageReference { message_id: m });"""
assert mutated in s, "mutation not present"
s = s.replace(mutated, clean, 1)
open(p, "w").write(s)
print("REVERTED")
