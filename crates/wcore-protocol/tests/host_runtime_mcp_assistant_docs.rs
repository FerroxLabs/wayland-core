//! The `add_mcp_server` assistant requirement must be documented wherever a
//! host integrator reads.
//!
//! # Why this file exists
//!
//! `scope_host_runtime_mcp` in `crates/wcore-cli/src/main.rs` refuses a
//! wire-added (`add_mcp_server`) MCP server outright when the host supplied no
//! `--assistant` / `WAYLAND_ASSISTANT` identity. In `v0.12.25` the same command
//! built its `McpServerConfig` with `only_for_assistant: None` and connected
//! unconditionally, so this is a behaviour change on the 0.12.25 → 0.12.26 line
//! that a host cannot discover from its own code.
//!
//! It shipped undocumented. `docs/releases/v0.12.26.md` flags exactly one
//! breaking change (workspace trust) and never says "assistant"; neither the
//! protocol spec, the Desktop integration guide, nor `docs/mcp.md` mentioned
//! the requirement. The failure mode is not loud for a host that ignores the
//! refusal frames: Core emits an `error` and an `mcp_failed`, then simply does
//! not connect. A host that swallows both shows the user a session with zero
//! tools, which reads as "MCP is broken" rather than "you did not pass an
//! identity".
//!
//! # What is asserted
//!
//! The refusal string is a fact of the source, so this gate reads it OUT of
//! `crates/wcore-cli/src/main.rs` rather than restating it, and then requires
//! every host-facing document to quote that exact string and to name the
//! `--assistant` flag that satisfies it. Rewording the refusal reddens the
//! docs; dropping the paragraph from any one document reddens that document.
//!
//! Reading a sibling crate's source as text is deliberate: `wcore-cli` sits
//! ABOVE `wcore-protocol` in the crate graph, so the message cannot be imported
//! as a constant without inverting the dependency. The extraction is asserted
//! to succeed, so a moved/renamed function reddens rather than passing
//! vacuously.
//!
//! # It runs in BOTH directions
//!
//! A gate that cannot fail proves nothing, and a gate that cannot pass proves
//! less. [`audit`] and [`extract_refusal_message`] are pure functions of
//! strings, so the tests below feed them doctored inputs:
//!
//! - the negative controls delete the quote, delete the flag, and delete the
//!   function, and assert each is reported — the gate can fail;
//! - [`the_gate_passes_in_a_world_where_the_refusal_is_reworded`] renames the
//!   message in the source AND in every document and asserts zero problems, so
//!   the pass state is reachable under a changed fact rather than pinned to
//!   today's literal.
//!
//! Every doctoring step asserts that it actually changed the string: a
//! `str::replace` that matches nothing silently returns its input, which would
//! make the control vacuous.

use std::path::{Path, PathBuf};

/// The CLI flag (and its env var) that satisfies the requirement. Named in the
/// source at `crates/wcore-cli/src/main.rs`.
const ASSISTANT_FLAG: &str = "--assistant";

/// Documents a host integrator reads. Each must carry the refusal quote and the
/// flag.
const REQUIRED_DOCS: &[&str] = &[
    "docs/releases/v0.12.26.md",
    "docs/releases/v0.12.26-desktop-integration.md",
    "docs/json-stream-protocol.md",
    "docs/mcp.md",
];

fn repo_root() -> PathBuf {
    // crates/wcore-protocol → crates → repo root
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("wcore-protocol lives two levels below the repo root")
        .to_path_buf()
}

fn read(relative: &str) -> String {
    let path = repo_root().join(relative);
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("cannot read {}: {e}", path.display()))
}

/// Pull the refusal reason out of `scope_host_runtime_mcp`'s `.ok_or("...")`.
///
/// Returns `None` when the function or the `ok_or` is not found, so a rename
/// surfaces as a hard failure instead of an empty comparison that passes.
fn extract_refusal_message(main_rs: &str) -> Option<String> {
    let fn_start = main_rs.find("fn scope_host_runtime_mcp")?;
    let body = &main_rs[fn_start..];
    // Bound the search to the function body so a later `ok_or` cannot be picked
    // up if this one is deleted.
    let body_end = body.find("\n}\n").map(|i| i + 2).unwrap_or(body.len());
    let body = &body[..body_end];
    let ok_or = body.find(".ok_or(\"")? + ".ok_or(\"".len();
    let rest = &body[ok_or..];
    let close = rest.find('"')?;
    Some(rest[..close].to_string())
}

/// Compare the source fact against the documents. Returns one problem string
/// per disagreement; empty means the docs and the code agree.
fn audit(main_rs: &str, docs: &[(&str, String)]) -> Vec<String> {
    let Some(refusal) = extract_refusal_message(main_rs) else {
        return vec![
            "cannot locate the `.ok_or(\"...\")` refusal inside `fn scope_host_runtime_mcp` in \
             crates/wcore-cli/src/main.rs — if the runtime-MCP assistant requirement was moved or \
             removed, update this gate and the four host-facing documents together"
                .to_string(),
        ];
    };

    let mut problems = Vec::new();
    for (name, body) in docs {
        if !body.contains(&refusal) {
            problems.push(format!(
                "{name} does not quote the runtime-MCP refusal reason emitted by \
                 `scope_host_runtime_mcp`: {refusal:?}"
            ));
        }
        if !body.contains(ASSISTANT_FLAG) {
            problems.push(format!(
                "{name} quotes no `{ASSISTANT_FLAG}` — a host reading it is told a session can \
                 fail but not what satisfies the requirement"
            ));
        }
    }
    problems
}

fn real_docs() -> Vec<(&'static str, String)> {
    REQUIRED_DOCS.iter().map(|d| (*d, read(d))).collect()
}

/// Asserts a doctoring step actually changed the input. A `str::replace` that
/// matches nothing returns its argument unchanged and would make the control
/// that follows it meaningless.
fn doctored(original: &str, from: &str, to: &str) -> String {
    let out = original.replace(from, to);
    assert_ne!(
        out, original,
        "doctoring step matched nothing: {from:?} is not present, so the control below would be \
         vacuous"
    );
    out
}

#[test]
fn the_refusal_message_is_where_this_gate_expects_it() {
    let main_rs = read("crates/wcore-cli/src/main.rs");
    assert_eq!(
        extract_refusal_message(&main_rs).as_deref(),
        Some("active assistant identity is required for a runtime MCP declaration"),
        "the extractor no longer finds today's refusal message; the rest of this file would be \
         comparing against the wrong fact"
    );
}

#[test]
fn every_host_facing_document_states_the_runtime_mcp_assistant_requirement() {
    let main_rs = read("crates/wcore-cli/src/main.rs");
    let problems = audit(&main_rs, &real_docs());
    assert!(
        problems.is_empty(),
        "add_mcp_server refuses without an assistant identity, and these documents do not say so:\n\
         - {}",
        problems.join("\n- ")
    );
}

#[test]
fn the_gate_rejects_a_document_that_drops_the_refusal_quote() {
    let main_rs = read("crates/wcore-cli/src/main.rs");
    let refusal = extract_refusal_message(&main_rs).expect("refusal message");
    let mut docs = real_docs();
    let stripped = doctored(&docs[0].1, &refusal, "some other reason");
    docs[0].1 = stripped;

    let problems = audit(&main_rs, &docs);
    assert!(
        problems
            .iter()
            .any(|p| p.contains(docs[0].0) && p.contains("does not quote")),
        "removing the quote from {} must be reported; got {problems:?}",
        docs[0].0
    );
}

#[test]
fn the_gate_rejects_a_document_that_drops_the_flag() {
    let main_rs = read("crates/wcore-cli/src/main.rs");
    let mut docs = real_docs();
    // docs/mcp.md is the smallest; strip every mention of the flag from it.
    let idx = docs.len() - 1;
    let stripped = doctored(&docs[idx].1, ASSISTANT_FLAG, "--some-other-flag");
    docs[idx].1 = stripped;

    let problems = audit(&main_rs, &docs);
    assert!(
        problems
            .iter()
            .any(|p| p.contains(docs[idx].0) && p.contains("quotes no")),
        "removing {ASSISTANT_FLAG} from {} must be reported; got {problems:?}",
        docs[idx].0
    );
}

#[test]
fn the_gate_fails_loudly_when_the_refusal_leaves_the_source() {
    let main_rs = read("crates/wcore-cli/src/main.rs");
    // NOT a suffix rename: `find("fn scope_host_runtime_mcp")` would still match
    // `fn scope_host_runtime_mcp_OLD` on the prefix, and the control would pass
    // for the wrong reason.
    let gutted = doctored(
        &main_rs,
        "fn scope_host_runtime_mcp",
        "fn retired_runtime_scope",
    );
    assert!(
        extract_refusal_message(&gutted).is_none(),
        "the doctored source must genuinely defeat the extractor"
    );

    let problems = audit(&gutted, &real_docs());
    assert_eq!(
        problems.len(),
        1,
        "a missing refusal must produce exactly one, unmistakable problem; got {problems:?}"
    );
    assert!(problems[0].contains("cannot locate"));
}

#[test]
fn the_gate_passes_in_a_world_where_the_refusal_is_reworded() {
    let main_rs = read("crates/wcore-cli/src/main.rs");
    let refusal = extract_refusal_message(&main_rs).expect("refusal message");
    let reworded = "an assistant identity must be supplied before a runtime MCP declaration";

    let source = doctored(&main_rs, &refusal, reworded);
    let docs: Vec<(&str, String)> = real_docs()
        .into_iter()
        .map(|(name, body)| (name, doctored(&body, &refusal, reworded)))
        .collect();

    assert_eq!(
        extract_refusal_message(&source).as_deref(),
        Some(reworded),
        "the extractor must track the source, not a hardcoded literal"
    );
    assert!(
        audit(&source, &docs).is_empty(),
        "a consistently reworded world must be green — otherwise this gate pins today's wording \
         rather than tracking the criterion"
    );
}
