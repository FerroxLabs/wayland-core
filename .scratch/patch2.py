# 1. make the process-wide active-token scrub reachable from the other sink impl
p = "crates/wcore-agent/src/lib.rs"
s = open(p).read()
old = "mod output_redaction;\n"
new = """// #1138 - `pub` because the active-token scrub is a PROCESS-WIDE chokepoint and
// `wcore-cli`'s TUI `ChannelSink` is the second `OutputSink` implementation
// that has to pass render content through it. Keeping it crate-private would
// have meant the TUI sink emitting unscrubbed, which is the #584 gap one sink
// over.
pub mod output_redaction;
"""
assert s.count(old) == 1
open(p, "w").write(s.replace(old, new))

p = "crates/wcore-agent/src/output_redaction.rs"
s = open(p).read()
old = "pub(crate) fn redact_active_tokens(text: &str) -> String {"
new = "pub fn redact_active_tokens(text: &str) -> String {"
assert s.count(old) == 1
open(p, "w").write(s.replace(old, new))
print("redaction ok")
