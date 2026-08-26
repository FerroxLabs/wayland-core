//! Collapsed-reasoning projection for the TUI transcript.
//!
//! The streaming tag filter itself moved to
//! [`wcore_types::reasoning_filter`] in #1129 so the JSON-stream protocol
//! sink can share it (`wcore-agent` cannot depend on `wcore-cli`). It is
//! re-exported here so `crate::tui::render::ReasoningFilter` and
//! `crate::tui::render::reasoning_filter::ReasoningFilter` keep resolving —
//! the TUI's behaviour is unchanged.

pub use wcore_types::reasoning_filter::ReasoningFilter;

// ── S21: collapsed reasoning projection ───────────────────────────────────
//
// v0.9.2 W7 (SPEC §3 S21, variant A). A captured reasoning block renders
// collapsed-by-default as a single `▶ Thought: <title> · Ns · N tok`
// line. The user toggles it open (Tab to focus the block, Enter to
// expand) — keyed by turn index in `App::reasoning_expanded`. When
// expanded the marker flips to `▼` and the body follows on subsequent
// wrapped lines.
//
// The `reasoning_filter` STRIPS reasoning tags from the live streaming
// buffer (so reasoning never leaks into prose); this projection renders
// reasoning that the engine surfaced *as* a discrete block (e.g. an
// Anthropic `thinking` content block or a provider summary), which is a
// separate, deliberate-to-show payload.

use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};

use crate::tui::theme::Theme;

/// Max chars for the one-line collapsed title before we ellipsize.
const REASONING_TITLE_MAX: usize = 50;

/// Extract a short title from a reasoning summary: the first **bold**
/// span if the summary opens with one (`**Title** …`), else the first
/// sentence (up to the first `.`/`!`/`?`), else the leading text.
/// Whitespace-collapsed and truncated to [`REASONING_TITLE_MAX`] chars
/// with a trailing `…` when cut.
pub fn reasoning_title(summary: &str) -> String {
    let trimmed = summary.trim();
    // First bold span: `**...**` at the very start.
    let raw = if let Some(rest) = trimmed.strip_prefix("**") {
        if let Some(end) = rest.find("**") {
            rest[..end].trim().to_string()
        } else {
            first_sentence(trimmed)
        }
    } else {
        first_sentence(trimmed)
    };
    // Collapse internal whitespace (incl. newlines) to single spaces.
    let collapsed: String = {
        let mut s = String::with_capacity(raw.len());
        let mut in_ws = false;
        for ch in raw.chars() {
            if ch.is_whitespace() {
                if !in_ws {
                    s.push(' ');
                    in_ws = true;
                }
            } else {
                s.push(ch);
                in_ws = false;
            }
        }
        s.trim().to_string()
    };
    if collapsed.chars().count() > REASONING_TITLE_MAX {
        let head: String = collapsed
            .chars()
            .take(REASONING_TITLE_MAX.saturating_sub(1))
            .collect();
        format!("{head}…")
    } else {
        collapsed
    }
}

/// The first sentence of `s` — text up to (and excluding) the first
/// sentence-terminator (`.`/`!`/`?`). Falls back to the whole string if
/// none is present.
fn first_sentence(s: &str) -> String {
    match s.find(['.', '!', '?']) {
        Some(ix) => s[..ix].to_string(),
        None => s.to_string(),
    }
}

/// Project a reasoning block to its renderable lines, honoring the
/// per-turn expand state.
///
/// * Collapsed (`expanded == false`): one line
///   `▶ Thought: <title> · Ns · N tok`. The duration / token counts are
///   omitted when zero so a block with no timing reads cleanly.
/// * Expanded (`expanded == true`): a `▼ Thought: <title>` header line
///   followed by the wrapped body (one `Line` per source line of the
///   reasoning summary), indented two spaces to sit under the header.
///
/// The marker + "Thought:" label are `text_muted`; the title is
/// `text_dim`; the timing meta is `text_muted`. Reasoning is ancillary,
/// so nothing here uses the brand accent.
pub fn reasoning_collapsed_lines(
    summary: &str,
    secs: u64,
    tokens: u64,
    expanded: bool,
) -> Vec<Line<'static>> {
    reasoning_collapsed_lines_themed(summary, secs, tokens, expanded, &Theme::detect())
}

/// [`reasoning_collapsed_lines`] with an explicit theme (so callers that
/// already hold the resolved `Theme` don't re-detect, and tests can pin
/// a known palette).
pub fn reasoning_collapsed_lines_themed(
    summary: &str,
    secs: u64,
    tokens: u64,
    expanded: bool,
    theme: &Theme,
) -> Vec<Line<'static>> {
    let title = reasoning_title(summary);
    let marker = if expanded { "▼" } else { "▶" };
    let muted = Style::default().fg(theme.text_muted);
    let dim = Style::default().fg(theme.text_dim);

    let mut header: Vec<Span<'static>> = vec![
        Span::styled(format!("{marker} "), muted),
        Span::styled("Thought: ".to_string(), muted.add_modifier(Modifier::BOLD)),
        Span::styled(title, dim),
    ];
    // Timing meta — only the parts that carry information.
    let mut meta = String::new();
    if secs > 0 {
        meta.push_str(&format!(" · {secs}s"));
    }
    if tokens > 0 {
        meta.push_str(&format!(" · {tokens} tok"));
    }
    if !meta.is_empty() {
        header.push(Span::styled(meta, muted));
    }

    let mut out = vec![Line::from(header)];
    if expanded {
        for src_line in summary.lines() {
            out.push(Line::from(vec![Span::styled(format!("  {src_line}"), dim)]));
        }
    }
    out
}

// ── Tests ───────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // ── v0.9.2 W7 (S21) — collapsed reasoning projection ───────────────

    fn line_text(line: &Line<'_>) -> String {
        line.spans.iter().map(|s| s.content.as_ref()).collect()
    }

    #[test]
    fn reasoning_title_takes_first_bold_span_v092() {
        let title = reasoning_title("**Plan the refactor** then we proceed.");
        assert_eq!(title, "Plan the refactor");
    }

    #[test]
    fn reasoning_title_falls_back_to_first_sentence_v092() {
        let title = reasoning_title("Considering the edge cases. Then the happy path.");
        assert_eq!(title, "Considering the edge cases");
    }

    #[test]
    fn reasoning_title_truncates_long_titles_v092() {
        let long = "a".repeat(120);
        let title = reasoning_title(&long);
        assert!(
            title.chars().count() <= REASONING_TITLE_MAX,
            "title not truncated: {} chars",
            title.chars().count()
        );
        assert!(
            title.ends_with('…'),
            "truncated title must end with ellipsis"
        );
    }

    #[test]
    fn reasoning_collapsed_default_is_single_marker_line_v092() {
        let lines = reasoning_collapsed_lines_themed(
            "**Weighing options** in detail across the whole module.",
            4,
            128,
            /* expanded = */ false,
            &Theme::hearth(),
        );
        assert_eq!(lines.len(), 1, "collapsed reasoning must be one line");
        let text = line_text(&lines[0]);
        assert!(
            text.starts_with("▶ "),
            "collapsed marker must be ▶; got {text:?}"
        );
        assert!(
            text.contains("Thought: "),
            "missing Thought label; got {text:?}"
        );
        assert!(
            text.contains("Weighing options"),
            "missing title; got {text:?}"
        );
        assert!(text.contains("· 4s"), "missing seconds meta; got {text:?}");
        assert!(
            text.contains("· 128 tok"),
            "missing token meta; got {text:?}"
        );
    }

    #[test]
    fn reasoning_expanded_shows_marker_flip_and_body_v092() {
        let summary = "First line of thought\nSecond line of thought";
        let lines = reasoning_collapsed_lines_themed(
            summary,
            0,
            0,
            /* expanded = */ true,
            &Theme::hearth(),
        );
        // Header + one line per source line.
        assert_eq!(lines.len(), 3, "expanded must be header + 2 body lines");
        let header = line_text(&lines[0]);
        assert!(
            header.starts_with("▼ "),
            "expanded marker must be ▼; got {header:?}"
        );
        // No timing meta when both counts are zero.
        assert!(
            !header.contains(" · "),
            "zero-timing header must omit meta; got {header:?}"
        );
        assert!(line_text(&lines[1]).contains("First line of thought"));
        assert!(line_text(&lines[2]).contains("Second line of thought"));
    }

    /// The collapsed/expanded choice is driven by the per-turn flag the
    /// App stores in `reasoning_expanded` — model the lookup here (absent
    /// or `false` ⇒ collapsed) to lock the contract the render path uses.
    #[test]
    fn reasoning_expanded_map_semantics_v092() {
        let mut expanded: std::collections::HashMap<usize, bool> = Default::default();
        let summary = "Some reasoning here.";
        // Turn 0 absent ⇒ collapsed (one line).
        let collapsed = reasoning_collapsed_lines_themed(
            summary,
            0,
            0,
            expanded.get(&0).copied().unwrap_or(false),
            &Theme::hearth(),
        );
        assert_eq!(collapsed.len(), 1);
        // Toggle turn 0 open ⇒ expanded (header + body).
        expanded.insert(0, true);
        let open = reasoning_collapsed_lines_themed(
            summary,
            0,
            0,
            expanded.get(&0).copied().unwrap_or(false),
            &Theme::hearth(),
        );
        assert!(open.len() > 1, "expanded turn must render body");
    }
}
