//! `file_ops` tool formatter — handles read, write, and edit.
//!
//! Expected payload shape (branches on `action`):
//! ```json
//! // read:
//! { "action": "read", "path": "/p/f", "lines": 42 }
//! // write:
//! { "action": "write", "path": "/p/f", "bytes": 1234 }
//! // edit:
//! { "action": "edit", "path": "/p/f", "added": 5, "removed": 3 }
//! ```
//! The dispatcher (`mod.rs::formatter_for`) also maps the bare
//! `"read"`/`"write"`/`"edit"` tool names onto this same formatter, so
//! a payload may not have an `action` field if the engine fired the
//! distinct tool. We infer the action from the presence of `bytes` vs
//! `lines` vs `added`/`removed` in that case.

use std::time::Duration;

use ratatui::style::Style;
use ratatui::text::{Line, Span};
use serde_json::Value;

use super::ToolResultFormatter;
use super::{elided_line_preview, join_facts, opt_str, opt_u64, raw_text};
use crate::tui::theme::Theme;

/// Max chars of a plain-text result echoed on the summary line.
const RAW_PREVIEW: usize = 60;

/// Max lines of a plain-text result shown in the detail view.
const MAX_DETAIL_LINES: usize = 20;

/// True when a `Read` result looks like the numbered file body rather than a
/// one-line status.
///
/// `ReadTool` prefixes each line with a right-aligned line number and a tab
/// (`read.rs` builds `numbered`), so the giveaway is several leading lines
/// that begin with digits. Deliberately conservative: a false negative just
/// means the first line is quoted instead of counted, which is still true;
/// a false positive would replace a real status with a line count.
fn looks_like_read_body(text: &str) -> bool {
    let mut numbered = 0usize;
    let mut checked = 0usize;
    for line in text.lines() {
        let t = line.trim_start();
        if t.is_empty() {
            continue;
        }
        checked += 1;
        if t.chars().next().is_some_and(|c| c.is_ascii_digit()) && t.contains('\t') {
            numbered += 1;
        }
        if checked >= 6 {
            break;
        }
    }
    numbered >= 2
}

pub struct FileOpsFormatter;

impl ToolResultFormatter for FileOpsFormatter {
    fn summary_line(&self, payload: &Value, _duration: Duration) -> String {
        // UAT-T3. `Read`, `Write` and `Edit` return PLAIN TEXT, not JSON:
        //
        //   Write -> "Created /tmp/out.txt (12 lines)"
        //   Edit  -> "Edited src/main.rs: replaced 3 occurrence(s)"
        //   Read  -> a header line followed by the numbered file body
        //
        // (`wcore-tools/src/{write,edit,read}.rs`.) None of them has ever
        // emitted the `{action, path, lines, bytes, added, removed}` object
        // this formatter was written against, so every field read below
        // returned `None` and the card rendered `file_ops ?`.
        //
        // Write's and Edit's own first line is already a better summary than
        // anything reassembled from parts, so use it verbatim. Read's body is
        // the file, so report its size instead of dumping it.
        if let Some(text) = raw_text(payload) {
            // Elide the MIDDLE, not the tail: these status lines put the
            // filename and the line/occurrence count LAST ("Created <path>
            // (12 lines)"), so head-truncation drops exactly the two facts the
            // card exists to show. Only Linux tempdir paths were short enough
            // to hide this; any real project path tripped it on every platform.
            let head = elided_line_preview(text, RAW_PREVIEW);
            // A `Read` result is the file itself; the first line is content,
            // not a status, so summarise rather than quote it.
            if looks_like_read_body(text) {
                return format!("{} lines read", text.lines().count());
            }
            return head;
        }

        // JSON path retained for any tool routed here that does emit an
        // object. Unknown fields are omitted, never defaulted.
        let path = opt_str(payload, "path");
        let verb = match infer_action(payload) {
            Action::Read => "Read",
            Action::Write => "Wrote",
            Action::Edit => "Edited",
            Action::Unknown => "file_ops",
        };
        // Each clause is pushed only when its field was actually read, so an
        // absent `lines` costs a missing clause instead of a fabricated `0`.
        let mut facts = vec![match path {
            Some(p) => format!("{verb} {p}"),
            None => verb.to_string(),
        }];
        if let Some(n) = opt_u64(payload, "lines") {
            facts.push(format!("{n} lines"));
        }
        if let Some(n) = opt_u64(payload, "bytes") {
            facts.push(format!("{n} bytes"));
        }
        match (opt_u64(payload, "added"), opt_u64(payload, "removed")) {
            (Some(a), Some(r)) => facts.push(format!("+{a}/-{r}")),
            (Some(a), None) => facts.push(format!("+{a}")),
            (None, Some(r)) => facts.push(format!("-{r}")),
            (None, None) => {}
        }
        join_facts(&facts)
    }

    fn detail_lines(&self, payload: &Value, theme: &Theme) -> Vec<Line<'static>> {
        let style = Style::default().fg(theme.text_dim);
        // Plain-text result: show the tool's real output. This branch used to
        // restate the (fabricated) summary, so a `Read` card showed the user
        // nothing about the file it had just read.
        if let Some(text) = raw_text(payload) {
            return text
                .lines()
                .filter(|l| !l.trim().is_empty())
                .take(MAX_DETAIL_LINES)
                .map(|s| Line::from(Span::styled(s.to_string(), style)))
                .collect();
        }
        let summary = self.summary_line(payload, Duration::ZERO);
        if summary.is_empty() {
            return Vec::new();
        }
        vec![Line::from(Span::styled(summary, style))]
    }
}

enum Action {
    Read,
    Write,
    Edit,
    Unknown,
}

/// Infer the action from the explicit `action` field (preferred) or
/// from the shape of the payload (the engine may fire `read`/`write`/
/// `edit` as distinct tool names without setting `action`).
fn infer_action(payload: &Value) -> Action {
    match payload.get("action").and_then(Value::as_str) {
        Some("read") => return Action::Read,
        Some("write") => return Action::Write,
        Some("edit") => return Action::Edit,
        _ => {}
    }
    if payload.get("added").is_some() || payload.get("removed").is_some() {
        Action::Edit
    } else if payload.get("bytes").is_some() {
        Action::Write
    } else if payload.get("lines").is_some() {
        Action::Read
    } else {
        Action::Unknown
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn read_summary() {
        let f = FileOpsFormatter;
        let payload = json!({ "action": "read", "path": "/etc/hosts", "lines": 42 });
        assert_eq!(
            f.summary_line(&payload, Duration::from_secs(1)),
            "Read /etc/hosts · 42 lines"
        );
    }

    #[test]
    fn write_summary() {
        let f = FileOpsFormatter;
        let payload = json!({ "action": "write", "path": "/tmp/out.txt", "bytes": 1024 });
        assert_eq!(
            f.summary_line(&payload, Duration::from_secs(1)),
            "Wrote /tmp/out.txt · 1024 bytes"
        );
    }

    #[test]
    fn edit_summary() {
        let f = FileOpsFormatter;
        let payload = json!({ "action": "edit", "path": "src/main.rs", "added": 5, "removed": 3 });
        assert_eq!(
            f.summary_line(&payload, Duration::from_secs(1)),
            "Edited src/main.rs · +5/-3"
        );
    }

    #[test]
    fn read_inferred_from_lines_field() {
        let f = FileOpsFormatter;
        // No `action` field; `lines` present → infer Read.
        let payload = json!({ "path": "/a/b", "lines": 7 });
        assert_eq!(
            f.summary_line(&payload, Duration::from_secs(1)),
            "Read /a/b · 7 lines"
        );
    }

    #[test]
    fn edit_inferred_from_added_removed() {
        let f = FileOpsFormatter;
        let payload = json!({ "path": "src/lib.rs", "added": 1, "removed": 0 });
        assert_eq!(
            f.summary_line(&payload, Duration::from_secs(1)),
            "Edited src/lib.rs · +1/-0"
        );
    }
}
