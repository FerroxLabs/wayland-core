//! `github` tool formatter.
//!
//! Expected payload shape:
//! ```json
//! { "action": "Created", "repo": "user/repo", "id": 42,
//!   "html_url": "https://github.com/user/repo/issues/42" }
//! ```
//! `action` is a verb like `Created`/`Updated`/`Merged`/`Commented on`.
//! `id` is the issue or PR number.

use std::time::Duration;

use ratatui::style::Style;
use ratatui::text::{Line, Span};
use serde_json::Value;

use super::ToolResultFormatter;
use super::{join_facts, opt_str};
use crate::tui::theme::Theme;

pub struct GithubFormatter;

impl ToolResultFormatter for GithubFormatter {
    // UAT-T3. `GitHubApiTool` is a straight PASSTHROUGH of the GitHub REST
    // response (`wcore-tools/src/github_tool.rs`: `GitHubOutcome::Ok { payload }`
    // -> `content: payload.to_string()`). It never adds an `action` or a
    // `repo` field — a grep for either as a RESULT key returns nothing, while
    // the control grep for the input-schema use of `repo` returns 14 — so the
    // summary rendered a fabricated verb ("Did") and a fabricated repo ("?")
    // on every call. `id` is also the wrong number: GitHub's `id` is the
    // internal database id, and the human-facing issue/PR number is `number`.
    fn summary_line(&self, payload: &Value, _duration: Duration) -> String {
        let mut facts: Vec<String> = Vec::new();
        // `full_name` is the real repo key on a repo object; on an issue/PR
        // it hangs off the nested `repository`. Absent -> no clause.
        if let Some(repo) = opt_str(payload, "full_name").or_else(|| {
            payload
                .get("repository")
                .and_then(|r| r.get("full_name"))
                .and_then(Value::as_str)
        }) {
            facts.push(repo.to_string());
        }
        if let Some(n) = payload.get("number").and_then(Value::as_i64) {
            facts.push(format!("#{n}"));
        }
        if let Some(state) = opt_str(payload, "state") {
            facts.push(state.to_string());
        }
        if let Some(title) = opt_str(payload, "title") {
            facts.push(title.to_string());
        }
        // An array response (a list operation) has a countable size and no
        // fields at all; report the count rather than nothing.
        if facts.is_empty()
            && let Some(a) = payload.as_array()
        {
            let unit = if a.len() == 1 { "item" } else { "items" };
            facts.push(format!("{} {unit}", a.len()));
        }
        join_facts(&facts)
    }

    fn detail_lines(&self, payload: &Value, theme: &Theme) -> Vec<Line<'static>> {
        let mut lines = Vec::new();
        let style = Style::default().fg(theme.text_dim);

        // Title (if present) makes the card readable at a glance.
        if let Some(title) = payload.get("title").and_then(Value::as_str) {
            lines.push(Line::from(Span::styled(title.to_string(), style)));
        }
        if let Some(url) = payload.get("html_url").and_then(Value::as_str) {
            lines.push(Line::from(Span::styled(url.to_string(), style)));
        }
        lines
    }

    fn extract_urls(&self, payload: &Value) -> Vec<String> {
        match payload.get("html_url").and_then(Value::as_str) {
            Some(u) if !u.is_empty() => vec![u.to_string()],
            _ => Vec::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    // UAT-T3: these cases previously asserted payload shapes the tool has
    // never emitted, and in several places asserted the FABRICATED output as
    // though it were the specification. They are not weakened here — the
    // fixtures are replaced with the shapes read out of the tool source, and
    // every "renders `?` when the field is missing" case is INVERTED, because
    // rendering `?` as though it were a fact is the defect.

    /// The real shape: `GitHubApiTool` is a passthrough of the GitHub REST
    /// response. An issue object carries `number`, `title`, `state` and
    /// `html_url` — and NOT `action` or `repo`.
    #[test]
    fn github_summary_reads_the_real_api_response() {
        let f = GithubFormatter;
        let payload = json!({
            "id": 2847362819_i64,
            "number": 42,
            "state": "open",
            "title": "Tool card lies about exit status",
            "html_url": "https://github.com/FerroxLabs/wayland-core/issues/42",
            "repository": { "full_name": "FerroxLabs/wayland-core" },
        });
        let s = f.summary_line(&payload, Duration::from_secs(1));
        assert!(s.contains("FerroxLabs/wayland-core"), "repo lost: {s}");
        // `#42` is the human-facing number, NOT the internal `id`.
        assert!(s.contains("#42"), "issue number lost: {s}");
        assert!(!s.contains("2847362819"), "showed the internal db id: {s}");
    }

    /// INVERTED. The old case asserted the fabricated verb "Did" and repo "?"
    /// by supplying keys the tool does not emit.
    #[test]
    fn github_never_fabricates_a_verb_or_a_repo() {
        let f = GithubFormatter;
        let s = f.summary_line(&json!({}), Duration::from_secs(1));
        assert!(!s.contains('?'), "fabricated a repo: {s}");
        assert!(!s.contains("Did"), "fabricated a verb: {s}");
    }

    #[test]
    fn github_extracts_html_url() {
        let f = GithubFormatter;
        let payload = json!({ "html_url": "https://github.com/x/y/pull/1" });
        let urls = f.extract_urls(&payload);
        assert_eq!(urls, vec!["https://github.com/x/y/pull/1".to_string()]);
    }
}
