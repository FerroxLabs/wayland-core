//! Every `error.code` the json-stream protocol spec shows a host must be a code
//! the engine can actually emit.
//!
//! # Why this file exists
//!
//! `docs/json-stream-protocol.md` tells hosts, in its own words, to "branch on
//! `error.code`, not parse `error.message`". That instruction is only safe if
//! the codes the document names are real. They were not.
//!
//! The document shipped three wire examples and one catalogue row naming
//! `protocol_error`, a code that appears **nowhere** in the engine — a host that
//! followed the advice and wrote `case "protocol_error"` got dead code that
//! could never fire. `tool_error`, `config_error` and `internal_error` were
//! catalogued the same way. One of the three `protocol_error` examples was added
//! by the very change that documented the `add_mcp_server` assistant
//! requirement: the refusal frame it showed carried `protocol_error` when the
//! path provably emits the generic `engine_error`
//! (`ProtocolSink::emit_error` → `auth_error_code(msg).unwrap_or("engine_error")`).
//!
//! Prose review did not catch it, and the sibling docs gate could not: that gate
//! checks that the refusal *message* is quoted, and the message was correct. The
//! `code` beside it was wrong. Hence a second, code-anchored gate.
//!
//! # What is asserted
//!
//! Two independent properties, both read out of the source rather than restated:
//!
//! 1. [`undocumentable_codes`] — every `"code":"X"` literal in the protocol spec
//!    must appear in [`emittable_codes`], the vocabulary harvested from the
//!    engine's own source.
//! 2. [`refusal_frame_code_disagreement`] — the specific `add_mcp_server`
//!    assistant-refusal example must carry the exact fallback code that
//!    `ProtocolSink::emit_error` computes, extracted from its `unwrap_or("...")`.
//!    This is the assertion that would have failed on the shipped text.
//!
//! # The vocabulary is a floor, not a ceiling
//!
//! [`emittable_codes`] harvests string literals from `code: "..."` fields,
//! `auth_error_code`'s `Some("...")` arms, the `unwrap_or("...")` fallback, and
//! `*_CODE: &str = "..."` constants, skipping `tests/` directories and the
//! `#[cfg(test)]` tail of each file. That is a deliberately *generous* set: it
//! still contains codes that only a non-test fixture mentions (`provider_error`
//! is in a `WireSpec` fixture in `contract/spec.rs`, and no production path
//! emits it). So this gate proves a documented code EXISTS in the engine; it
//! does not prove some production path reaches it. It catches inventions, which
//! is the defect that occurred. The narrower property is noted in the spec's own
//! prose instead.
//!
//! # It runs in BOTH directions
//!
//! [`audit`] is a pure function of strings, so the controls below feed it
//! doctored inputs: reintroducing `protocol_error` must be reported, changing
//! the sink's fallback must be reported, and a consistently *renamed* world must
//! be green — so the gate tracks the source rather than pinning today's literal.
//! An instrument control asserts the harvester actually found a vocabulary, so
//! an empty scan can never read as "no problems".

use std::path::{Path, PathBuf};

/// Crate sources scanned for the emittable vocabulary. These are the crates that
/// construct `ProtocolEvent::Error`.
const EMITTER_CRATES: &[&str] = &["wcore-agent", "wcore-cli", "wcore-protocol"];

const PROTOCOL_DOC: &str = "docs/json-stream-protocol.md";
const SINK: &str = "crates/wcore-agent/src/output/protocol_sink.rs";
const CLI_MAIN: &str = "crates/wcore-cli/src/main.rs";

fn repo_root() -> PathBuf {
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

/// Every `.rs` file under the emitter crates' `src/`, as (path, contents).
///
/// `tests/` directories are skipped wholesale, and each file is truncated at its
/// first `#[cfg(test)]` so a code named only by a unit test does not enter the
/// vocabulary.
fn emitter_sources() -> Vec<(String, String)> {
    fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                walk(&path, out);
            } else if path.extension().is_some_and(|e| e == "rs") {
                out.push(path);
            }
        }
    }

    let root = repo_root();
    let mut sources = Vec::new();
    for krate in EMITTER_CRATES {
        let src = root.join("crates").join(krate).join("src");
        let mut files = Vec::new();
        walk(&src, &mut files);
        files.sort();
        for file in files {
            let Ok(body) = std::fs::read_to_string(&file) else {
                continue;
            };
            // Drop the unit-test tail; a literal only a test names is not part
            // of the emitted vocabulary.
            let production = match body.find("#[cfg(test)]") {
                Some(i) => body[..i].to_string(),
                None => body,
            };
            let name = file
                .strip_prefix(&root)
                .unwrap_or(&file)
                .to_string_lossy()
                .into_owned();
            sources.push((name, production));
        }
    }
    sources
}

/// Read the string literal that begins immediately after `haystack[from..]`
/// starts with a quote. Returns the literal and the index just past it.
fn literal_at(haystack: &str, from: usize) -> Option<String> {
    let rest = haystack.get(from..)?;
    let end = rest.find('"')?;
    Some(rest[..end].to_string())
}

/// Collect every literal introduced by `marker` in `body`.
fn literals_after(body: &str, marker: &str, out: &mut Vec<String>) {
    let mut cursor = 0;
    while let Some(hit) = body[cursor..].find(marker) {
        let start = cursor + hit + marker.len();
        if let Some(lit) = literal_at(body, start) {
            out.push(lit);
        }
        cursor = start;
    }
}

/// The set of error codes the engine's source names.
///
/// See the module header: this is a floor (existence), not a proof of
/// reachability.
fn emittable_codes(sources: &[(String, String)]) -> Vec<String> {
    let mut codes = Vec::new();
    for (_name, body) in sources {
        // `code: "engine_error"` / `code: "init_failed".to_string()`
        literals_after(body, "code: \"", &mut codes);
        // `auth_error_code`'s arms and any other `Some("...")` classification.
        literals_after(body, "Some(\"", &mut codes);
        // The `unwrap_or("engine_error")` fallback.
        literals_after(body, "unwrap_or(\"", &mut codes);
        // `pub const STARTUP_ERROR_CODE: &str = "init_failed";`
        literals_after(body, "_CODE: &str = \"", &mut codes);
    }
    codes.sort();
    codes.dedup();
    codes
}

/// Every distinct `"code":"X"` / `"code": "X"` literal the protocol doc shows.
fn documented_codes(doc: &str) -> Vec<String> {
    let mut codes = Vec::new();
    literals_after(doc, "\"code\":\"", &mut codes);
    literals_after(doc, "\"code\": \"", &mut codes);
    codes.sort();
    codes.dedup();
    codes
}

/// The fallback code `ProtocolSink::emit_error` applies to any non-auth error.
///
/// `None` when the expression is not found, so a refactor reddens loudly instead
/// of comparing against nothing.
fn extract_fallback_code(sink_rs: &str) -> Option<String> {
    let hit = sink_rs.find("auth_error_code(msg).unwrap_or(\"")?;
    literal_at(sink_rs, hit + "auth_error_code(msg).unwrap_or(\"".len())
}

/// The `add_mcp_server` refusal reason, read out of `scope_host_runtime_mcp`.
fn extract_refusal_message(main_rs: &str) -> Option<String> {
    let fn_start = main_rs.find("fn scope_host_runtime_mcp")?;
    let body = &main_rs[fn_start..];
    let body_end = body.find("\n}\n").map(|i| i + 2).unwrap_or(body.len());
    let body = &body[..body_end];
    let ok_or = body.find(".ok_or(\"")? + ".ok_or(\"".len();
    literal_at(body, ok_or)
}

/// Codes the document shows that the engine does not name.
fn undocumentable_codes(doc: &str, emittable: &[String]) -> Vec<String> {
    documented_codes(doc)
        .into_iter()
        .filter(|c| !emittable.iter().any(|e| e == c))
        .collect()
}

/// The refusal example's code, when it disagrees with the sink's fallback.
///
/// Returns `Err` when the anchors cannot be found — a moved refusal or a
/// refactored sink must redden, not pass silently.
fn refusal_frame_code_disagreement(
    doc: &str,
    sink_rs: &str,
    main_rs: &str,
) -> Result<Option<String>, String> {
    let fallback = extract_fallback_code(sink_rs).ok_or_else(|| {
        format!("cannot find `auth_error_code(msg).unwrap_or(\"...\")` in {SINK}")
    })?;
    let refusal = extract_refusal_message(main_rs).ok_or_else(|| {
        format!("cannot find the refusal in `scope_host_runtime_mcp` in {CLI_MAIN}")
    })?;

    let line = doc
        .lines()
        .find(|l| l.contains(&refusal) && l.contains("\"type\":\"error\""))
        .ok_or_else(|| {
            format!(
                "{PROTOCOL_DOC} shows no `error` frame quoting the refusal {refusal:?} — the \
                 worked example a host copies from is gone"
            )
        })?;

    let shown = literal_at(
        line,
        line.find("\"code\":\"").ok_or_else(|| {
            format!("{PROTOCOL_DOC}'s refusal example carries no \"code\" field at all")
        })? + "\"code\":\"".len(),
    )
    .ok_or_else(|| "unterminated code literal in the refusal example".to_string())?;

    Ok((shown != fallback).then_some(format!(
        "{PROTOCOL_DOC} shows the add_mcp_server refusal as code {shown:?}, but \
         `ProtocolSink::emit_error` emits {fallback:?} for this message. A host told to branch on \
         error.code would write an arm that never fires."
    )))
}

/// Full audit. Empty means the document and the engine agree.
fn audit(doc: &str, sink_rs: &str, main_rs: &str, sources: &[(String, String)]) -> Vec<String> {
    let emittable = emittable_codes(sources);
    let mut problems = Vec::new();

    for code in undocumentable_codes(doc, &emittable) {
        problems.push(format!(
            "{PROTOCOL_DOC} documents error code {code:?}, which appears nowhere in the engine \
             source — a host branching on it has dead code"
        ));
    }

    match refusal_frame_code_disagreement(doc, sink_rs, main_rs) {
        Ok(Some(problem)) => problems.push(problem),
        Ok(None) => {}
        Err(broken) => problems.push(broken),
    }

    problems
}

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
fn the_harvester_actually_finds_a_vocabulary() {
    // Instrument control. If the scan silently returned nothing, every other
    // test here would report "no problems" for the wrong reason.
    let sources = emitter_sources();
    assert!(
        sources.len() > 50,
        "expected to scan the emitter crates' sources; got {} files",
        sources.len()
    );
    let codes = emittable_codes(&sources);
    for expected in [
        "engine_error",
        "auth_required",
        "auth_invalid",
        "init_failed",
    ] {
        assert!(
            codes.iter().any(|c| c == expected),
            "the harvester lost {expected:?}; it is emitted by the engine, so the vocabulary scan \
             is broken and this gate is measuring nothing. Found: {codes:?}"
        );
    }
}

#[test]
fn the_fallback_code_is_where_this_gate_expects_it() {
    let sink = read(SINK);
    assert_eq!(
        extract_fallback_code(&sink).as_deref(),
        Some("engine_error"),
        "the extractor no longer finds the sink's fallback code; the refusal-frame assertion would \
         be comparing against the wrong fact"
    );
}

#[test]
fn every_error_code_the_protocol_spec_shows_is_one_the_engine_can_emit() {
    let problems = audit(
        &read(PROTOCOL_DOC),
        &read(SINK),
        &read(CLI_MAIN),
        &emitter_sources(),
    );
    assert!(
        problems.is_empty(),
        "the protocol spec names error codes the engine does not:\n- {}",
        problems.join("\n- ")
    );
}

#[test]
fn the_gate_rejects_a_reinvented_protocol_error() {
    let doc = doctored(
        &read(PROTOCOL_DOC),
        "\"code\":\"engine_error\"",
        "\"code\":\"protocol_error\"",
    );
    let problems = audit(&doc, &read(SINK), &read(CLI_MAIN), &emitter_sources());
    assert!(
        problems
            .iter()
            .any(|p| p.contains("protocol_error") && p.contains("appears nowhere")),
        "reintroducing protocol_error must be reported; got {problems:?}"
    );
}

#[test]
fn the_gate_rejects_a_refusal_example_that_drifts_from_the_sink() {
    // Keep the code emittable (so the vocabulary check stays quiet) but wrong
    // for THIS path — this isolates the second property from the first.
    let doc = doctored(
        &read(PROTOCOL_DOC),
        "\"code\":\"engine_error\",\"message\":\"AddMcpServer",
        "\"code\":\"auth_required\",\"message\":\"AddMcpServer",
    );
    let problems = audit(&doc, &read(SINK), &read(CLI_MAIN), &emitter_sources());
    assert!(
        problems
            .iter()
            .any(|p| p.contains("would write an arm that never fires")),
        "a refusal example disagreeing with the sink must be reported; got {problems:?}"
    );
}

#[test]
fn the_gate_fails_loudly_when_the_sink_fallback_moves() {
    let sink = doctored(
        &read(SINK),
        "auth_error_code(msg).unwrap_or(\"",
        "classify_error(msg).unwrap_or_default_code(\"",
    );
    assert!(
        extract_fallback_code(&sink).is_none(),
        "the doctored source must genuinely defeat the extractor"
    );
    let problems = audit(
        &read(PROTOCOL_DOC),
        &sink,
        &read(CLI_MAIN),
        &emitter_sources(),
    );
    assert!(
        problems.iter().any(|p| p.contains("cannot find")),
        "a moved fallback must be reported rather than passing; got {problems:?}"
    );
}

#[test]
fn the_gate_passes_in_a_world_where_the_fallback_is_renamed() {
    // A consistently renamed world must be green, or this gate pins today's
    // spelling instead of tracking the criterion.
    let renamed = "generic_engine_failure";
    let sink = doctored(
        &read(SINK),
        "unwrap_or(\"engine_error\")",
        &format!("unwrap_or(\"{renamed}\")"),
    );
    let doc = doctored(&read(PROTOCOL_DOC), "engine_error", renamed);

    assert_eq!(
        extract_fallback_code(&sink).as_deref(),
        Some(renamed),
        "the extractor must track the source, not a hardcoded literal"
    );

    // The renamed code must also enter the vocabulary, exactly as a real rename
    // would: the sink is one of the scanned sources.
    let mut sources = emitter_sources();
    for (name, body) in sources.iter_mut() {
        if name.replace('\\', "/").ends_with("output/protocol_sink.rs") {
            *body = body.replace(
                "unwrap_or(\"engine_error\")",
                &format!("unwrap_or(\"{renamed}\")"),
            );
        }
    }

    let problems = audit(&doc, &sink, &read(CLI_MAIN), &sources);
    assert!(
        problems.is_empty(),
        "a consistently renamed world must be green; got {problems:?}"
    );
}
