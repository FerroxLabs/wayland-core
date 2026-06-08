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
use super::{i64_or, str_or, u64_or};
use crate::tui::theme::Theme;

pub struct FileOpsFormatter;

impl ToolResultFormatter for FileOpsFormatter {
    fn summary_line(&self, payload: &Value, _duration: Duration) -> String {
        let path = str_or(payload, "path", "?");
        match infer_action(payload) {
            Action::Read => {
                let lines = u64_or(payload, "lines", 0);
                format!("Read {} · {} lines", path, lines)
            }
            Action::Write => {
                let bytes = u64_or(payload, "bytes", 0);
                format!("Wrote {} · {} bytes", path, bytes)
            }
            Action::Edit => {
                let added = i64_or(payload, "added", 0);
                let removed = i64_or(payload, "removed", 0);
                format!("Edited {} · +{}/-{}", path, added, removed)
            }
            Action::Unknown => format!("file_ops {}", path),
        }
    }

    fn detail_lines(&self, payload: &Value, theme: &Theme) -> Vec<Line<'static>> {
        // For all three actions, a single-line restate of the summary is
        // enough; richer diffs/contents are out of scope for this card.
        let style = Style::default().fg(theme.text_dim);
        vec![Line::from(Span::styled(
            self.summary_line(payload, Duration::ZERO),
            style,
        ))]
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
