p = "crates/wcore-cli/src/tui/protocol_bridge.rs"
s = open(p).read()

old_noop = """        | ProtocolEvent::HostSendMessageRequest { .. }
        // #1098 `RenderArtifact` is a json-stream host surface. The in-process
        // TUI never emits one — `RenderArtifactTool` is only registered under
        // a `ProtocolSink` — so this arm is unreachable in practice and must
        // stay a no-op rather than inventing a terminal rendering of it.
        | ProtocolEvent::RenderArtifact { .. }
"""
new_noop = """        | ProtocolEvent::HostSendMessageRequest { .. }
"""
assert s.count(old_noop) == 1, "noop arm anchor"
s = s.replace(old_noop, new_noop)

anchor = """        ProtocolEvent::CapabilityActivation { activation } => {
            app.capability_status
                .insert(activation.capability, activation);
        }
"""
new_arm = """        ProtocolEvent::CapabilityActivation { activation } => {
            app.capability_status
                .insert(activation.capability, activation);
        }

        // FerroxLabs/wayland#1138. This arm used to sit in the no-op group
        // below, under a comment asserting the TUI "never emits one because
        // `RenderArtifactTool` is only registered under a `ProtocolSink`". The
        // premise was wrong on both halves: registration is UNCONDITIONAL (it
        // has to be — `tool_inventory` is inside the recovery authority
        // digest), and the sink is what gates liveness. So the tool WAS
        // registered under the TUI; it refused every call, and once
        // `ChannelSink` stopped refusing, this arm silently ate the content.
        //
        // The artifact is content the model deliberately chose to SHOW the
        // user, so it belongs in the transcript. It is pushed as its own
        // system entry rather than appended to the assistant turn: it is not
        // the model's prose, and interleaving it would make a 1 MiB file read
        // look like something the model said.
        ProtocolEvent::RenderArtifact {
            title,
            mime,
            content,
            truncated,
            ..
        } => {
            push_system(app, render_artifact_body(&title, mime, &content, truncated));
        }
"""
assert s.count(anchor) == 1, "capability arm anchor"
s = s.replace(anchor, new_arm)

helper_anchor = """fn push_system(app: &mut App, text: String) {"""
helper = """/// #1138 — lay one `render_artifact` payload out as transcript markdown.
///
/// Only `text/markdown` is handed to the markdown renderer. `text/plain` and
/// `text/html` are fenced instead, because the closed MIME vocabulary is a
/// promise about how the bytes are to be READ: markdown-rendering a plain-text
/// artifact would eat its `#` and `*` characters, and markdown-rendering HTML
/// would strip the very tags the model asked to display. The fence is widened
/// past the longest backtick run in the content so a payload that itself
/// contains a code fence cannot break out of the block.
fn render_artifact_body(
    title: &str,
    mime: wcore_protocol::events::RenderMime,
    content: &str,
    truncated: bool,
) -> String {
    use wcore_protocol::events::RenderMime;

    let mut body = format!("**{title}**\\n\\n");
    match mime {
        RenderMime::Markdown => {
            body.push_str(content);
        }
        RenderMime::Plain | RenderMime::Html => {
            let longest_run = content
                .split(|c| c != '`')
                .map(str::len)
                .max()
                .unwrap_or(0);
            let fence = "`".repeat(longest_run.saturating_add(1).max(3));
            let language = if matches!(mime, RenderMime::Html) {
                "html"
            } else {
                ""
            };
            body.push_str(&fence);
            body.push_str(language);
            body.push('\\n');
            body.push_str(content);
            if !content.ends_with('\\n') {
                body.push('\\n');
            }
            body.push_str(&fence);
        }
    }
    if truncated {
        body.push_str(
            "\\n\\n_Truncated \\u{2014} the artifact was larger than the render cap._",
        );
    }
    body
}

fn push_system(app: &mut App, text: String) {"""
assert s.count(helper_anchor) == 1, "push_system anchor"
s = s.replace(helper_anchor, helper)

open(p, "w").write(s)
print("protocol_bridge ok")
