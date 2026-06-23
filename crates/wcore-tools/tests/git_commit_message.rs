//! A2 commit-hygiene helper tests.

use wcore_observability::trace::{ToolCallTrace, TurnTrace};
use wcore_tools::git_commit_message::{ProjectStyle, commit_message_from_trace};

fn synth_trace(touched: &[&str]) -> TurnTrace {
    let mut t = TurnTrace {
        turn: 1,
        model: "claude-opus-4-7".into(),
        provider: "anthropic".into(),
        input_tokens: 100,
        output_tokens: 50,
        cache_read: 0,
        cache_write: 0,
        cache_hit_rate: 0.0,
        cost_usd: 0.01,
        tool_calls: vec![],
        hook_actions: vec![],
        source_product: "wayland-core".into(),
        agent_run_id: String::new(),
    };
    for p in touched {
        t.tool_calls.push(ToolCallTrace::new(
            "c".into(),
            "Edit".into(),
            serde_json::json!({"file_path": p}),
        ));
    }
    t
}

#[test]
fn conventional_commits_style_when_detected() {
    let trace = synth_trace(&["crates/wcore-tools/src/git.rs"]);
    let msg = commit_message_from_trace(
        &trace,
        "intent: add git tool",
        ProjectStyle::ConventionalCommits,
    );
    let first = msg.lines().next().unwrap_or("");
    assert!(
        first.starts_with("feat")
            || first.starts_with("fix")
            || first.starts_with("chore")
            || first.starts_with("refactor"),
        "expected conventional-commits prefix, got: {msg}"
    );
}

#[test]
fn scope_extracted_from_crates_path() {
    let trace = synth_trace(&["crates/wcore-tools/src/git.rs"]);
    let msg = commit_message_from_trace(
        &trace,
        "introduce git tool",
        ProjectStyle::ConventionalCommits,
    );
    let first = msg.lines().next().unwrap_or("");
    assert!(
        first.contains("(wcore-tools)"),
        "expected (wcore-tools) scope, got: {first}"
    );
}

#[test]
fn plain_style_falls_back_to_sentence_case() {
    let trace = synth_trace(&["src/main.rs"]);
    let msg = commit_message_from_trace(&trace, "fix the parser", ProjectStyle::Plain);
    assert!(!msg.is_empty());
    let first = msg.lines().next().unwrap_or("");
    assert!(!first.starts_with("feat:"));
    assert!(
        first.starts_with("Fix"),
        "expected sentence-case capitalization, got: {first}"
    );
}

#[test]
fn message_subject_under_72_chars() {
    let trace = synth_trace(&["a.rs", "b.rs", "c.rs"]);
    let msg = commit_message_from_trace(
        &trace,
        "do many things including a very long intent that should be truncated past the 72 char limit",
        ProjectStyle::ConventionalCommits,
    );
    let subject = msg.lines().next().unwrap_or("");
    assert!(subject.len() <= 72, "subject too long: {}", subject.len());
}

#[test]
fn files_touched_in_body_when_present() {
    let trace = synth_trace(&["crates/wcore-tools/src/git.rs", "src/lib.rs"]);
    let msg = commit_message_from_trace(&trace, "add git tool", ProjectStyle::ConventionalCommits);
    assert!(msg.contains("Files touched:"));
    assert!(msg.contains("crates/wcore-tools/src/git.rs"));
    assert!(msg.contains("src/lib.rs"));
}

#[test]
fn no_body_when_no_files_touched() {
    let trace = synth_trace(&[]);
    let msg =
        commit_message_from_trace(&trace, "chore something", ProjectStyle::ConventionalCommits);
    assert!(
        !msg.contains("Files touched:"),
        "no body should appear when tool_calls is empty"
    );
}

#[test]
fn fix_intent_classifies_as_fix() {
    let trace = synth_trace(&["a.rs"]);
    let msg = commit_message_from_trace(&trace, "fix the bug", ProjectStyle::ConventionalCommits);
    assert!(msg.starts_with("fix"));
}

#[test]
fn supports_path_field_for_legacy_tools() {
    // Older tool-call traces may carry `path` instead of `file_path`.
    let mut t = synth_trace(&[]);
    t.tool_calls.push(ToolCallTrace::new(
        "c".into(),
        "Edit".into(),
        serde_json::json!({"path": "crates/wcore-config/src/lib.rs"}),
    ));
    let msg = commit_message_from_trace(&t, "fix config", ProjectStyle::ConventionalCommits);
    assert!(
        msg.contains("(wcore-config)"),
        "should derive scope from legacy `path` field: {msg}"
    );
}
