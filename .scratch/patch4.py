p = "crates/wcore-cli/tests/render_artifact_tui_surface.rs"
s = open(p).read()
old = s[s.index("/// Plain and HTML must not be re-interpreted"):]
new = '''/// Plain and HTML must not be re-interpreted as markdown on the way to the
/// terminal: the closed MIME vocabulary is a promise about how the bytes are to
/// be READ, and markdown-rendering `text/plain` eats its `#` and `*`.
#[test]
fn plain_text_is_fenced_rather_than_markdown_rendered() {
    let rendered = transcript_for(RenderMime::Plain, "# not a heading\\n* not a bullet\\n", false);
    assert!(
        rendered.contains("# not a heading"),
        "plain text must survive verbatim: {rendered:?}"
    );
    assert!(
        rendered.contains("```"),
        "plain text must be fenced, not handed to the markdown renderer: {rendered:?}"
    );
}

/// The converse control. Without it the fence assertion above is satisfiable by
/// fencing EVERYTHING, which would break the common case.
#[test]
fn markdown_is_not_fenced() {
    let rendered = transcript_for(RenderMime::Markdown, "# a real heading\\n", false);
    assert!(
        rendered.contains("# a real heading"),
        "markdown body must reach the transcript: {rendered:?}"
    );
    assert!(
        !rendered.contains("```"),
        "markdown must NOT be wrapped in a code fence: {rendered:?}"
    );
}

/// A plain-text artifact that itself contains a fence must not be able to break
/// out of the block the bridge wraps it in.
#[test]
fn a_content_fence_cannot_break_out_of_the_wrapper() {
    let rendered = transcript_for(RenderMime::Plain, "```\\nstill inside\\n```\\n", false);
    assert!(
        rendered.contains("````"),
        "the wrapper fence must be wider than the longest run in the content: {rendered:?}"
    );
}

/// The truncation flag must reach the user, not just the wire. A partial
/// artifact rendered as if it were whole is the silent-discard bug one step on.
#[test]
fn a_truncated_artifact_is_badged() {
    let rendered = transcript_for(RenderMime::Markdown, "half a file", true);
    assert!(
        rendered.to_lowercase().contains("truncated"),
        "a truncated artifact must say so: {rendered:?}"
    );
    let whole = transcript_for(RenderMime::Markdown, "half a file", false);
    assert!(
        !whole.to_lowercase().contains("truncated"),
        "an untruncated artifact must NOT be badged: {whole:?}"
    );
}

fn transcript_for(mime: RenderMime, content: &str, truncated: bool) -> String {
    let mut app = App::new();
    apply_event(
        &mut app,
        ProtocolEvent::RenderArtifact {
            msg_id: String::new(),
            call_id: "call-render".into(),
            title: "Raw".into(),
            mime,
            content: content.into(),
            truncated,
            critical: wcore_protocol::events::NonCritical,
        },
    );
    transcript(&app)
}

fn transcript(app: &App) -> String {
    app.session
        .turns
        .iter()
        .flat_map(|turn| turn.elements.iter())
        .filter_map(|element| match element {
            TurnElement::Markdown(text) => Some(text.clone()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\\n")
}
'''
s = s.replace(old, new)

# route the e2e assertion through the same helper
old_e2e = """    let mut app = App::new();
    apply_event(&mut app, frame);
    let rendered: String = app
        .session
        .turns
        .iter()
        .flat_map(|turn| turn.elements.iter())
        .filter_map(|element| match element {
            TurnElement::Markdown(text) => Some(text.clone()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\\n");
"""
new_e2e = """    let mut app = App::new();
    apply_event(&mut app, frame);
    let rendered = transcript(&app);
"""
assert s.count(old_e2e) == 1, "e2e anchor"
s = s.replace(old_e2e, new_e2e)
open(p, "w").write(s)
print("tests ok")
