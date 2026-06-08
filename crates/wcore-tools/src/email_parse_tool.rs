//! T10 (Plan v2 Tier 2B): read-only email (`.eml` / `.mbox`) parsing tool.
//!
//! Parses an email file on the local filesystem and reports its structured
//! fields back to the model:
//!
//! - `.eml` — a single RFC 5322 message. Reports `From` / `To` / `Cc` /
//!   `Subject` / `Date`, the plain-text body, and attachment metadata.
//! - `.mbox` — a mailbox of many messages. Messages are split on the
//!   `From ` line separator (QMail-spec `>From ` quoting handled by the
//!   `mail-parser` mbox reader); each message gets a compact one-line
//!   summary plus its own attachment list.
//!
//! ## Backend
//!
//! Uses the pure-Rust [`mail-parser`](https://crates.io/crates/mail-parser)
//! crate — no C / native build deps, ~RFC 5322 + MIME conformant, and
//! liberal in what it accepts (Postel's law) so slightly malformed
//! real-world mail still parses.
//!
//! ## Safety posture
//!
//! **Read-only.** The LLM-supplied `file_path` is validated via
//! [`crate::path_validation::validate_user_path`] before any filesystem
//! touch (same discipline as `ReadTool` / `PdfTool`): absolute paths only,
//! no traversal, no null bytes, system-secret deny-list. Attachment
//! **contents are never read or returned** — only the attachment name and
//! byte size are reported. Body text is truncated to a byte cap so a
//! pathologically large email cannot blow the model's context window.

use std::fs;
use std::io::BufReader;
use std::path::Path;

use async_trait::async_trait;
use mail_parser::mailbox::mbox::MessageIterator;
use mail_parser::{Address, MessageParser, MimeHeaders};
use serde_json::{Value, json};

use wcore_protocol::events::ToolCategory;
use wcore_types::tool::{JsonSchema, ToolResult};

use crate::Tool;
use crate::path_validation::validate_user_path;
use crate::tool_output_limits::DEFAULT_MAX_BYTES;
use crate::truncate_utf8;

/// Marker appended when an email body is truncated to [`MAX_BODY_BYTES`].
const TRUNCATION_MARKER: &str = "\n\n... [email body truncated]";

/// Byte cap for an extracted email body before truncation kicks in.
///
/// Reuses the shared [`DEFAULT_MAX_BYTES`] terminal-output cap (50_000) so
/// email output is bounded consistently with other large-output tools.
pub const MAX_BODY_BYTES: usize = DEFAULT_MAX_BYTES;

/// Hard cap on the number of `.mbox` messages summarised in one call.
///
/// A mailbox can hold tens of thousands of messages; summarising all of
/// them would blow the context window regardless of per-body truncation.
/// The report notes when this cap clipped the output.
const MAX_MBOX_MESSAGES: usize = 500;

/// Read-only email parsing tool. See the module docs for behaviour.
#[derive(Debug, Default, Clone, Copy)]
pub struct EmailParseTool;

impl EmailParseTool {
    /// Construct a new `EmailParseTool`. Stateless — one instance serves all calls.
    pub fn new() -> Self {
        Self
    }
}

/// Apply the byte-cap truncation to an email body.
fn cap_body(text: &str) -> String {
    if text.len() <= MAX_BODY_BYTES {
        return text.to_string();
    }
    let head = truncate_utf8(text, MAX_BODY_BYTES);
    format!("{head}{TRUNCATION_MARKER}")
}

/// Render an optional [`Address`] header into a compact `Name <addr>` list.
///
/// Groups are flattened (`Address::iter` already does this). An entry with
/// only an address renders as the bare address; one with only a name
/// renders as the bare name. Returns `"(none)"` when the header is absent
/// or carries no usable entries.
fn render_address(addr: Option<&Address<'_>>) -> String {
    let Some(addr) = addr else {
        return "(none)".to_string();
    };
    let rendered: Vec<String> = addr
        .iter()
        .filter_map(|a| match (a.name(), a.address()) {
            (Some(name), Some(email)) => Some(format!("{name} <{email}>")),
            (None, Some(email)) => Some(email.to_string()),
            (Some(name), None) => Some(name.to_string()),
            (None, None) => None,
        })
        .collect();
    if rendered.is_empty() {
        "(none)".to_string()
    } else {
        rendered.join(", ")
    }
}

/// Format the parsed message's attachment metadata — name and byte size
/// only. **Attachment contents are never read.**
fn render_attachments(message: &mail_parser::Message<'_>) -> String {
    let mut lines: Vec<String> = Vec::new();
    for (idx, part) in message.attachments().enumerate() {
        let name = part.attachment_name().unwrap_or("(unnamed)").to_string();
        lines.push(format!("  [{idx}] {name} ({} bytes)", part.len()));
    }
    if lines.is_empty() {
        "  (none)".to_string()
    } else {
        lines.join("\n")
    }
}

/// Parse a single `.eml` message from raw bytes into a structured report.
///
/// Returns `Err` with a human-readable message when `mail-parser` cannot
/// make sense of the input at all.
fn format_eml(raw: &[u8]) -> Result<String, String> {
    let message = MessageParser::default()
        .parse(raw)
        .ok_or_else(|| "could not parse email message (malformed RFC 5322 input)".to_string())?;

    let from = render_address(message.from());
    let to = render_address(message.to());
    let cc = render_address(message.cc());
    let subject = message.subject().unwrap_or("(no subject)");
    let date = message
        .date()
        .map(|d| d.to_rfc3339())
        .unwrap_or_else(|| "(no date)".to_string());
    let body = message
        .body_text(0)
        .map(|b| cap_body(&b))
        .unwrap_or_else(|| "(no plain-text body)".to_string());
    let attachments = render_attachments(&message);

    Ok(format!(
        "From:    {from}\n\
         To:      {to}\n\
         Cc:      {cc}\n\
         Subject: {subject}\n\
         Date:    {date}\n\
         \n\
         Attachments:\n{attachments}\n\
         \n\
         --- Body ---\n{body}"
    ))
}

/// Parse a `.mbox` mailbox from raw bytes into a multi-message report.
///
/// Each message is split on the `From ` separator by `mail-parser`'s mbox
/// `MessageIterator`, then parsed individually for a compact one-line
/// summary plus its attachment list.
fn format_mbox(raw: &[u8]) -> Result<String, String> {
    let iter = MessageIterator::new(BufReader::new(raw));
    let parser = MessageParser::default();

    let mut sections: Vec<String> = Vec::new();
    let mut total = 0usize;
    let mut truncated = false;

    for entry in iter {
        let mbox_msg = entry.map_err(|e| format!("error reading mbox stream: {e}"))?;
        total += 1;
        if sections.len() >= MAX_MBOX_MESSAGES {
            truncated = true;
            continue;
        }

        let contents = mbox_msg.contents();
        let section = match parser.parse(contents) {
            Some(message) => {
                let from = render_address(message.from());
                let subject = message.subject().unwrap_or("(no subject)");
                let date = message
                    .date()
                    .map(|d| d.to_rfc3339())
                    .unwrap_or_else(|| "(no date)".to_string());
                let attachments = render_attachments(&message);
                format!(
                    "[{}] From: {from}\n     Subject: {subject}\n     Date: {date}\n     Attachments:\n{attachments}",
                    sections.len()
                )
            }
            None => format!(
                "[{}] (message could not be parsed — malformed RFC 5322 input)",
                sections.len()
            ),
        };
        sections.push(section);
    }

    if total == 0 {
        return Err("mbox file contains no messages".to_string());
    }

    let mut report = format!("Mailbox: {total} message(s)\n\n{}", sections.join("\n\n"));
    if truncated {
        report.push_str(&format!(
            "\n\n... [truncated: showing {MAX_MBOX_MESSAGES} of {total} messages]"
        ));
    }
    Ok(report)
}

/// Decide whether a path should be treated as a mailbox (`.mbox`) rather
/// than a single message. Case-insensitive on the extension.
fn is_mbox_path(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .map(|e| e.eq_ignore_ascii_case("mbox"))
        .unwrap_or(false)
}

#[async_trait]
impl Tool for EmailParseTool {
    fn name(&self) -> &str {
        "email_parse"
    }

    fn description(&self) -> &str {
        "Parses an email file on the local filesystem and reports its \
         structured fields. Read-only.\n\n\
         Usage:\n\
         - file_path must be an absolute path to a .eml or .mbox file.\n\
         - A .eml file is parsed as a single message: From, To, Cc, Subject,\n\
           Date, plain-text body, and attachment names/sizes.\n\
         - A .mbox file is parsed as a mailbox: a one-line summary per\n\
           message plus each message's attachment list.\n\
         - Attachment CONTENTS are never read or returned — only the\n\
           attachment name and byte size.\n\
         - Large bodies are truncated.\n\
         - This tool never modifies the file."
    }

    fn input_schema(&self) -> JsonSchema {
        json!({
            "type": "object",
            "properties": {
                "file_path": {
                    "type": "string",
                    "description": "Absolute path to the .eml or .mbox file to parse"
                }
            },
            "required": ["file_path"],
            "additionalProperties": false
        })
    }

    fn is_concurrency_safe(&self, _input: &Value) -> bool {
        // Read-only filesystem access — safe to run alongside other tools.
        true
    }

    async fn execute(&self, input: Value) -> ToolResult {
        let Some(file_path) = input.get("file_path").and_then(|v| v.as_str()) else {
            return ToolResult {
                content: "Missing required parameter: file_path".to_string(),
                is_error: true,
            };
        };

        // Same path discipline as ReadTool / PdfTool: absolute, no traversal,
        // no null bytes, system-secret deny-list.
        let validated = match validate_user_path(Path::new(file_path)) {
            Ok(p) => p,
            Err(e) => {
                return ToolResult {
                    content: format!("Refused to read {file_path}: {e}"),
                    is_error: true,
                };
            }
        };

        if !validated.is_file() {
            return ToolResult {
                content: format!("Email file not found or not a file: {file_path}"),
                is_error: true,
            };
        }

        let raw = match fs::read(&validated) {
            Ok(bytes) => bytes,
            Err(e) => {
                return ToolResult {
                    content: format!("Failed to read {file_path}: {e}"),
                    is_error: true,
                };
            }
        };

        let parsed = if is_mbox_path(&validated) {
            format_mbox(&raw)
        } else {
            format_eml(&raw)
        };

        match parsed {
            Ok(content) => ToolResult {
                content,
                is_error: false,
            },
            Err(msg) => ToolResult {
                content: format!("Failed to parse {file_path}: {msg}"),
                is_error: true,
            },
        }
    }

    fn max_result_size(&self) -> usize {
        // Headroom over MAX_BODY_BYTES so the truncation marker, header
        // block, and attachment list aren't clipped a second time by the
        // registry-level truncation.
        MAX_BODY_BYTES + TRUNCATION_MARKER.len() + 4_096
    }

    fn category(&self) -> ToolCategory {
        // Read-only file inspection — mirrors ReadTool / PdfTool.
        ToolCategory::Info
    }

    fn describe(&self, input: &Value) -> String {
        let path = input
            .get("file_path")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown");
        format!("Parse email file {path}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::io::Write;
    use tempfile::TempDir;

    /// Write `contents` to `<dir>/<name>` and return the absolute path string.
    fn write_fixture(dir: &TempDir, name: &str, contents: &[u8]) -> String {
        let path = dir.path().join(name);
        let mut file = std::fs::File::create(&path).unwrap();
        file.write_all(contents).unwrap();
        drop(file);
        path.to_str().unwrap().to_string()
    }

    /// A plain single-message `.eml` fixture (no attachments).
    const SIMPLE_EML: &[u8] = b"From: Alice Example <alice@example.com>\r\n\
To: Bob Builder <bob@example.com>\r\n\
Cc: carol@example.com\r\n\
Subject: Quarterly report\r\n\
Date: Sat, 20 Nov 2021 14:22:01 -0800\r\n\
Content-Type: text/plain; charset=\"utf-8\"\r\n\
\r\n\
Hello Bob,\r\n\
The numbers look great this quarter.\r\n";

    /// A multipart `.eml` fixture carrying one text body + one attachment.
    const EML_WITH_ATTACHMENT: &[u8] = b"From: Sender <sender@example.com>\r\n\
To: Receiver <receiver@example.com>\r\n\
Subject: With a file\r\n\
Date: Mon, 15 Jan 2018 15:30:00 +0000\r\n\
Content-Type: multipart/mixed; boundary=\"BOUND\"\r\n\
\r\n\
--BOUND\r\n\
Content-Type: text/plain; charset=\"us-ascii\"\r\n\
\r\n\
See the attached report.\r\n\
--BOUND\r\n\
Content-Type: text/plain; name=\"report.txt\"\r\n\
Content-Disposition: attachment; filename=\"report.txt\"\r\n\
\r\n\
COLUMN-A,COLUMN-B\r\n\
--BOUND--\r\n";

    #[tokio::test]
    async fn parses_eml_headers_and_body() {
        let dir = TempDir::new().unwrap();
        let path = write_fixture(&dir, "msg.eml", SIMPLE_EML);

        let tool = EmailParseTool::new();
        let result = tool.execute(json!({ "file_path": path })).await;

        assert!(!result.is_error, "unexpected error: {}", result.content);
        let c = &result.content;
        assert!(
            c.contains("Alice Example <alice@example.com>"),
            "From missing: {c}"
        );
        assert!(
            c.contains("Bob Builder <bob@example.com>"),
            "To missing: {c}"
        );
        assert!(c.contains("carol@example.com"), "Cc missing: {c}");
        assert!(c.contains("Quarterly report"), "Subject missing: {c}");
        assert!(c.contains("2021-11-20T14:22:01-08:00"), "Date missing: {c}");
        assert!(
            c.contains("The numbers look great this quarter."),
            "body missing: {c}"
        );
        assert!(
            c.contains("Attachments:\n  (none)"),
            "expected no attachments: {c}"
        );
    }

    #[tokio::test]
    async fn reports_attachment_metadata_not_content() {
        let dir = TempDir::new().unwrap();
        let path = write_fixture(&dir, "withfile.eml", EML_WITH_ATTACHMENT);

        let tool = EmailParseTool::new();
        let result = tool.execute(json!({ "file_path": path })).await;

        assert!(!result.is_error, "unexpected error: {}", result.content);
        let c = &result.content;
        // The attachment NAME and SIZE are reported...
        assert!(c.contains("report.txt"), "attachment name missing: {c}");
        assert!(c.contains("bytes)"), "attachment size missing: {c}");
        // ...but the attachment CONTENT must not leak into the report.
        assert!(
            !c.contains("COLUMN-A,COLUMN-B"),
            "attachment content leaked into report: {c}"
        );
        // The inline text body is still shown.
        assert!(c.contains("See the attached report."), "body missing: {c}");
    }

    #[tokio::test]
    async fn parses_mbox_with_multiple_messages() {
        let dir = TempDir::new().unwrap();
        // Two messages separated by the `From ` mbox separator line.
        let mbox = b"From alice@example.com Sat Jan  3 01:05:34 1996\r\n\
From: Alice <alice@example.com>\r\n\
Subject: First message\r\n\
Date: Sat, 03 Jan 1996 01:05:34 +0000\r\n\
\r\n\
This is message one.\r\n\
\r\n\
From bob@example.com Tue Jul 23 19:39:23 2002\r\n\
From: Bob <bob@example.com>\r\n\
Subject: Second message\r\n\
Date: Tue, 23 Jul 2002 19:39:23 +0000\r\n\
\r\n\
This is message two.\r\n";
        let path = write_fixture(&dir, "inbox.mbox", mbox);

        let tool = EmailParseTool::new();
        let result = tool.execute(json!({ "file_path": path })).await;

        assert!(!result.is_error, "unexpected error: {}", result.content);
        let c = &result.content;
        assert!(c.contains("Mailbox: 2 message(s)"), "count missing: {c}");
        assert!(c.contains("First message"), "msg 1 subject missing: {c}");
        assert!(c.contains("Second message"), "msg 2 subject missing: {c}");
        assert!(
            c.contains("Alice <alice@example.com>"),
            "msg 1 from missing: {c}"
        );
        assert!(
            c.contains("Bob <bob@example.com>"),
            "msg 2 from missing: {c}"
        );
    }

    #[tokio::test]
    async fn empty_mbox_is_an_error() {
        let dir = TempDir::new().unwrap();
        // A .mbox file with no `From ` separator lines yields zero messages.
        let path = write_fixture(&dir, "empty.mbox", b"not a real mailbox\r\n");

        let tool = EmailParseTool::new();
        let result = tool.execute(json!({ "file_path": path })).await;

        assert!(
            result.is_error,
            "expected error for empty mbox: {}",
            result.content
        );
        assert!(
            result.content.contains("no messages"),
            "expected 'no messages' error, got: {}",
            result.content
        );
    }

    #[tokio::test]
    async fn missing_file_returns_error() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("does_not_exist.eml");

        let tool = EmailParseTool::new();
        let result = tool
            .execute(json!({ "file_path": path.to_str().unwrap() }))
            .await;

        assert!(result.is_error);
        assert!(
            result.content.contains("not found"),
            "expected not-found error, got: {}",
            result.content
        );
    }

    #[tokio::test]
    async fn missing_file_path_param_returns_error() {
        let tool = EmailParseTool::new();
        let result = tool.execute(json!({})).await;
        assert!(result.is_error);
        assert!(result.content.contains("file_path"));
    }

    #[tokio::test]
    async fn relative_path_is_refused() {
        let tool = EmailParseTool::new();
        let result = tool
            .execute(json!({ "file_path": "relative/mail.eml" }))
            .await;
        assert!(result.is_error);
        assert!(result.content.contains("Refused"));
    }

    #[test]
    fn schema_and_metadata_are_well_formed() {
        let tool = EmailParseTool::new();
        assert_eq!(tool.name(), "email_parse");
        assert!(tool.is_concurrency_safe(&json!({})));
        assert_eq!(tool.category(), ToolCategory::Info);
        let schema = tool.input_schema();
        assert_eq!(schema["required"][0], "file_path");
        assert!(
            tool.describe(&json!({ "file_path": "/tmp/x.eml" }))
                .contains("/tmp/x.eml")
        );
    }

    #[test]
    fn cap_body_truncates_oversized_text() {
        let big = "a".repeat(MAX_BODY_BYTES + 5_000);
        let out = cap_body(&big);
        assert!(out.ends_with(TRUNCATION_MARKER));
        assert!(out.len() <= MAX_BODY_BYTES + TRUNCATION_MARKER.len());
        assert!(out.len() < big.len());
    }
}
