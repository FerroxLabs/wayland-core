//! FerroxLabs/wayland#1098 — the documented `render_artifact` MIME table must
//! be the code's enum.
//!
//! This matters more than the usual doc-drift check because the vocabulary is
//! deliberately CLOSED: a host is told to reject anything outside it. An
//! undocumented value is therefore one a host will never learn to render, and
//! a documented value the code cannot emit is a host feature built for nothing.
//!
//! Follows the `host_error_code_vocabulary_docs.rs` precedent: the spec is
//! checked against the type, not maintained by hand beside it.

use std::path::Path;

use wcore_protocol::events::RenderMime;

fn spec() -> String {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../docs/json-stream-protocol.md")
        .canonicalize()
        .expect("the protocol spec must exist");
    std::fs::read_to_string(path).expect("the protocol spec must be readable")
}

/// Pull the `| \`text/...\` | ... |` rows out of the §1.N+13 MIME table.
fn documented_mimes(spec: &str) -> Vec<String> {
    let section = spec
        .split("### 1.N+13 `render_artifact`")
        .nth(1)
        .expect("the spec must document render_artifact in §1.N+13");
    section
        .lines()
        .take_while(|line| !line.starts_with("### "))
        .filter_map(|line| {
            // The table lives inside a numbered list item, so rows are indented.
            let cell = line.trim_start().strip_prefix("| `")?;
            let (value, _) = cell.split_once('`')?;
            value.starts_with("text/").then(|| value.to_owned())
        })
        .collect()
}

#[test]
fn the_documented_mime_table_matches_the_code_enum() {
    let spec = spec();
    let documented = documented_mimes(&spec);
    assert_eq!(
        documented.iter().map(String::as_str).collect::<Vec<_>>(),
        RenderMime::all(),
        "the §1.N+13 MIME table and RenderMime disagree; a closed vocabulary that \
         is not documented is one a host can never learn to render"
    );
    // Every documented token must actually parse — a table row the type
    // rejects is a host feature built for a value Core cannot emit.
    for token in &documented {
        assert!(
            RenderMime::from_wire(token).is_some(),
            "{token} is documented but not in the closed vocabulary"
        );
    }
}

/// The host obligation for `text/html` is the one sentence whose absence would
/// hand Desktop a loaded gun. Core cannot enforce a sandboxed renderer, so the
/// spec saying it is the entire mitigation.
#[test]
fn the_html_renderer_obligation_is_stated() {
    let spec = spec();
    let section = spec
        .split("### 1.N+13 `render_artifact`")
        .nth(1)
        .expect("the spec must document render_artifact");
    let section = section
        .split("\n### ")
        .next()
        .expect("section must be delimited");
    for required in ["UNTRUSTED", "sandboxed renderer", "nodeIntegration: false"] {
        assert!(
            section.contains(required),
            "the render_artifact section must state the host's obligation: {required} missing"
        );
    }
}
