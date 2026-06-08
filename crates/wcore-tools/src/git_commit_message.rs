//! A2 commit-hygiene helper.
//!
//! Pure function: turns a `TurnTrace` plus a user-supplied intent string into
//! a commit message string. The Git tool's `commit` op consumes the result.
//! The helper NEVER runs git; the agent must explicitly invoke
//! `GitOp::Commit { message }` to actually commit.
//!
//! Style detection: callers pass `ProjectStyle` directly. The future
//! W6+1 enhancement can run `git log --format=%s -n 20` to auto-detect
//! conventional-commits adherence; v1 keeps the helper pure.

use wcore_observability::trace::TurnTrace;

#[derive(Debug, Clone, Copy)]
pub enum ProjectStyle {
    ConventionalCommits,
    Plain,
}

pub fn commit_message_from_trace(trace: &TurnTrace, intent: &str, style: ProjectStyle) -> String {
    // Edit/Write use `file_path`; older tools may use `path`. Be liberal.
    let touched: Vec<String> = trace
        .tool_calls
        .iter()
        .filter(|c| c.tool_name == "Edit" || c.tool_name == "Write")
        .filter_map(|c| {
            c.input
                .get("file_path")
                .or_else(|| c.input.get("path"))
                .and_then(|v| v.as_str())
                .map(|s| s.to_string())
        })
        .collect();

    let scope = infer_scope(&touched);
    let kind = classify_intent(intent);

    let subject = match style {
        ProjectStyle::ConventionalCommits => match scope {
            Some(s) => format!("{kind}({s}): {}", short_intent(intent)),
            None => format!("{kind}: {}", short_intent(intent)),
        },
        ProjectStyle::Plain => sentence_case(intent),
    };
    // Trim to ≤72 chars including the trailing ellipsis byte budget.
    // `…` is 3 bytes UTF-8, so leave a 3-byte head room.
    let subject = if subject.len() > 72 {
        let target = 72_usize.saturating_sub(3);
        let mut end = target;
        while end > 0 && !subject.is_char_boundary(end) {
            end -= 1;
        }
        format!("{}…", &subject[..end])
    } else {
        subject
    };

    let body = if touched.is_empty() {
        String::new()
    } else {
        format!(
            "\n\nFiles touched:\n{}",
            touched
                .iter()
                .map(|f| format!("  - {f}"))
                .collect::<Vec<_>>()
                .join("\n")
        )
    };
    format!("{subject}{body}")
}

fn classify_intent(intent: &str) -> &'static str {
    let lower = intent.to_lowercase();
    if lower.starts_with("fix") || lower.contains("bug") {
        "fix"
    } else if lower.starts_with("add") || lower.contains("new ") || lower.contains("introduce") {
        "feat"
    } else if lower.contains("refactor") {
        "refactor"
    } else if lower.contains("doc") {
        "docs"
    } else if lower.contains("test") {
        "test"
    } else {
        "chore"
    }
}

fn infer_scope(paths: &[String]) -> Option<String> {
    if paths.is_empty() {
        return None;
    }
    // crates/wcore-tools/src/git.rs -> wcore-tools
    let first = &paths[0];
    if let Some(rest) = first.strip_prefix("crates/")
        && let Some(slash) = rest.find('/')
    {
        return Some(rest[..slash].to_string());
    }
    None
}

fn short_intent(intent: &str) -> String {
    let trimmed = intent.trim();
    let trimmed = trimmed
        .strip_prefix("intent: ")
        .or_else(|| trimmed.strip_prefix("Intent: "))
        .unwrap_or(trimmed);
    if let Some(c) = trimmed.chars().next() {
        let lower = c.to_lowercase().next().unwrap_or(c);
        format!("{lower}{}", &trimmed[c.len_utf8()..])
    } else {
        String::new()
    }
}

fn sentence_case(s: &str) -> String {
    if let Some(c) = s.chars().next() {
        let upper = c.to_uppercase().next().unwrap_or(c);
        format!("{upper}{}", &s[c.len_utf8()..])
    } else {
        String::new()
    }
}
