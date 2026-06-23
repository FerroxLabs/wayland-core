//! T15: read-only PDF text-extraction tool.
//!
//! Ported in spirit from the prior Wayland Python engine. Extracts
//! plain text from a PDF file on the local filesystem — either the whole
//! document or a contiguous page range.
//!
//! ## Backend
//!
//! Uses the pure-Rust [`pdf-extract`](https://crates.io/crates/pdf-extract)
//! crate (no C / native build deps). Because that crate pulls a moderately
//! large transitive tree (~90 crates), it is gated behind the **default-on**
//! `pdf` cargo feature. Downstreams that don't need PDF support can build
//! `wcore-tools` with `--no-default-features` (or omit `pdf`) and `PdfTool`
//! still registers — it just returns an honest error explaining the tool
//! was compiled without PDF support, instead of silently disappearing.
//!
//! ## Safety posture
//!
//! Read-only. The LLM-supplied `file_path` is validated via
//! [`crate::path_validation::validate_user_path`] before any filesystem
//! touch (same discipline as `ReadTool`): absolute paths only, no traversal,
//! no null bytes, system-secret deny-list. Output is truncated to a byte
//! cap so a pathologically large PDF cannot blow the context window.

use std::path::Path;

use async_trait::async_trait;
use serde_json::{Value, json};

use wcore_protocol::events::ToolCategory;
use wcore_types::tool::{JsonSchema, ToolResult};

use crate::Tool;
use crate::path_validation::validate_user_path;
use crate::tool_output_limits::DEFAULT_MAX_BYTES;
use crate::truncate_utf8;

/// Marker appended when extracted text is truncated to [`MAX_PDF_TEXT_BYTES`].
const TRUNCATION_MARKER: &str = "\n\n... [PDF text truncated]";

/// Byte cap for extracted PDF text before truncation kicks in.
///
/// Reuses the shared [`DEFAULT_MAX_BYTES`] terminal-output cap (50_000) so
/// PDF output is bounded consistently with other large-output tools.
pub const MAX_PDF_TEXT_BYTES: usize = DEFAULT_MAX_BYTES;

/// Read-only PDF text-extraction tool.
///
/// Two modes, selected by the optional `start_page` / `end_page` inputs:
/// - both omitted → extract text from the entire document;
/// - `start_page` (and optionally `end_page`) present → extract only that
///   1-based, inclusive page range.
#[derive(Debug, Default, Clone, Copy)]
pub struct PdfTool;

impl PdfTool {
    /// Construct a new `PdfTool`. Stateless — one instance serves all calls.
    pub fn new() -> Self {
        Self
    }
}

/// Apply the byte-cap truncation to extracted text.
///
/// Returns the text unchanged when within budget, otherwise a
/// char-boundary-safe prefix with [`TRUNCATION_MARKER`] appended.
// Only the `#[cfg(feature = "pdf")]` `extract` path calls this; under
// `--no-default-features` it is exercised solely by unit tests, so allow
// dead_code for non-test feature-off builds to keep them warning-clean.
#[cfg_attr(not(feature = "pdf"), allow(dead_code))]
fn cap_text(text: &str) -> String {
    if text.len() <= MAX_PDF_TEXT_BYTES {
        return text.to_string();
    }
    let head = truncate_utf8(text, MAX_PDF_TEXT_BYTES);
    format!("{head}{TRUNCATION_MARKER}")
}

/// Resolve and validate a contiguous, 1-based, inclusive page range against
/// the actual page count.
///
/// `start`/`end` are the raw user inputs (`None` when omitted). Returns the
/// 0-based `[lo, hi)` slice bounds into a per-page text vector, or an error
/// string describing what was wrong.
// See `cap_text` above — feature-off non-test builds have no caller.
#[cfg_attr(not(feature = "pdf"), allow(dead_code))]
fn resolve_range(
    start: Option<u64>,
    end: Option<u64>,
    page_count: usize,
) -> Result<(usize, usize), String> {
    if page_count == 0 {
        return Err("PDF has no pages".to_string());
    }
    // Default: whole document.
    let start = start.unwrap_or(1);
    let end = end.unwrap_or(page_count as u64);
    if start == 0 || end == 0 {
        return Err("page numbers are 1-based; start_page/end_page must be >= 1".to_string());
    }
    if start > end {
        return Err(format!(
            "start_page ({start}) must not exceed end_page ({end})"
        ));
    }
    if start as usize > page_count {
        return Err(format!(
            "start_page ({start}) exceeds document page count ({page_count})"
        ));
    }
    // Clamp the upper bound to the real page count rather than erroring —
    // a caller asking for "pages 3-9999" of a 5-page doc gets pages 3-5.
    let lo = (start - 1) as usize;
    let hi = (end as usize).min(page_count);
    Ok((lo, hi))
}

#[async_trait]
impl Tool for PdfTool {
    fn name(&self) -> &str {
        "pdf_extract"
    }

    fn description(&self) -> &str {
        "Extracts plain text from a PDF file on the local filesystem. Read-only.\n\n\
         Usage:\n\
         - file_path must be an absolute path to a .pdf file.\n\
         - Omit start_page and end_page to extract the whole document.\n\
         - Provide start_page (1-based) to extract from that page onward;\n\
           add end_page (1-based, inclusive) to bound the range.\n\
         - Output is truncated if the extracted text is very large.\n\
         - This tool never modifies the PDF."
    }

    fn input_schema(&self) -> JsonSchema {
        json!({
            "type": "object",
            "properties": {
                "file_path": {
                    "type": "string",
                    "description": "Absolute path to the PDF file to read"
                },
                "start_page": {
                    "type": "integer",
                    "minimum": 1,
                    "description": "First page to extract (1-based, inclusive). Omit for whole document."
                },
                "end_page": {
                    "type": "integer",
                    "minimum": 1,
                    "description": "Last page to extract (1-based, inclusive). Defaults to the last page."
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

        // Same path discipline as ReadTool: absolute, no traversal, no
        // null bytes, system-secret deny-list.
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
                content: format!("PDF not found or not a file: {file_path}"),
                is_error: true,
            };
        }

        let start_page = input.get("start_page").and_then(|v| v.as_u64());
        let end_page = input.get("end_page").and_then(|v| v.as_u64());

        extract(&validated, file_path, start_page, end_page)
    }

    fn max_result_size(&self) -> usize {
        // Slightly above MAX_PDF_TEXT_BYTES so cap_text's own marker is
        // never clipped a second time by the registry-level truncation.
        MAX_PDF_TEXT_BYTES + TRUNCATION_MARKER.len() + 64
    }

    fn category(&self) -> ToolCategory {
        // Read-only file inspection — mirrors ReadTool's classification.
        ToolCategory::Info
    }

    fn describe(&self, input: &Value) -> String {
        let path = input
            .get("file_path")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown");
        match (
            input.get("start_page").and_then(|v| v.as_u64()),
            input.get("end_page").and_then(|v| v.as_u64()),
        ) {
            (Some(s), Some(e)) => format!("Extract text from {path} (pages {s}-{e})"),
            (Some(s), None) => format!("Extract text from {path} (pages {s}-end)"),
            _ => format!("Extract text from {path}"),
        }
    }
}

/// Real extraction body — `pdf` feature ON.
///
/// `disk_path` is the validated path; `display_path` is the original
/// user-supplied string used only for error messages.
#[cfg(feature = "pdf")]
fn extract(
    disk_path: &Path,
    display_path: &str,
    start_page: Option<u64>,
    end_page: Option<u64>,
) -> ToolResult {
    // Whole-document fast path: no page range requested.
    if start_page.is_none() && end_page.is_none() {
        return match pdf_extract::extract_text(disk_path) {
            Ok(text) => ToolResult {
                content: cap_text(&text),
                is_error: false,
            },
            Err(e) => ToolResult {
                content: format!("Failed to extract text from {display_path}: {e}"),
                is_error: true,
            },
        };
    }

    // Page-range path: extract per-page then slice.
    let pages = match pdf_extract::extract_text_by_pages(disk_path) {
        Ok(p) => p,
        Err(e) => {
            return ToolResult {
                content: format!("Failed to extract text from {display_path}: {e}"),
                is_error: true,
            };
        }
    };

    let (lo, hi) = match resolve_range(start_page, end_page, pages.len()) {
        Ok(bounds) => bounds,
        Err(msg) => {
            return ToolResult {
                content: format!("Invalid page range for {display_path}: {msg}"),
                is_error: true,
            };
        }
    };

    let joined = pages[lo..hi].join("\n");
    ToolResult {
        content: cap_text(&joined),
        is_error: false,
    }
}

/// Degraded extraction body — `pdf` feature OFF.
///
/// The tool still registers and is schema-visible to the LLM, but every
/// call fails loudly with an honest "compiled without PDF support" message
/// (NO-STUBS: an honest blocker, not silent success).
#[cfg(not(feature = "pdf"))]
fn extract(
    _disk_path: &Path,
    display_path: &str,
    _start_page: Option<u64>,
    _end_page: Option<u64>,
) -> ToolResult {
    ToolResult {
        content: format!(
            "Cannot extract text from {display_path}: this build of wcore-tools \
             was compiled without the `pdf` feature. Rebuild with the default \
             features (or `--features pdf`) to enable PDF text extraction."
        ),
        is_error: true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use tempfile::tempdir;

    /// Build a minimal, valid multi-page PDF whose pages each contain one
    /// line of Helvetica text (`page_texts[i]` on page `i+1`).
    ///
    /// Helvetica is a PDF base-14 font, so `pdf-extract` extracts the text
    /// from its built-in AFM metrics without needing an embedded font file.
    /// xref offsets are computed exactly so the file parses cleanly.
    // Only the `#[cfg(feature = "pdf")]` integration tests build PDFs;
    // under `--no-default-features` this helper has no caller.
    #[cfg_attr(not(feature = "pdf"), allow(dead_code))]
    fn make_pdf(page_texts: &[&str]) -> Vec<u8> {
        let n = page_texts.len();
        // Object numbering:
        //   1            = Catalog
        //   2            = Pages
        //   3            = Font (Helvetica)
        //   4 .. 4+n-1   = Page objects
        //   4+n .. 4+2n-1 = Content streams
        let first_page_obj = 4;
        let first_content_obj = 4 + n;
        let total_objs = 3 + 2 * n;

        let mut bodies: Vec<String> = Vec::with_capacity(total_objs);

        // 1: Catalog
        bodies.push("<< /Type /Catalog /Pages 2 0 R >>".to_string());

        // 2: Pages
        let kids: Vec<String> = (0..n)
            .map(|i| format!("{} 0 R", first_page_obj + i))
            .collect();
        bodies.push(format!(
            "<< /Type /Pages /Count {} /Kids [{}] >>",
            n,
            kids.join(" ")
        ));

        // 3: Font
        bodies.push("<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".to_string());

        // 4..: Page objects
        for i in 0..n {
            bodies.push(format!(
                "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] \
                 /Resources << /Font << /F1 3 0 R >> >> /Contents {} 0 R >>",
                first_content_obj + i
            ));
        }

        // Content streams
        for text in page_texts {
            // Escape the two PDF string metacharacters we might hit.
            let escaped = text
                .replace('\\', "\\\\")
                .replace('(', "\\(")
                .replace(')', "\\)");
            let stream = format!("BT /F1 24 Tf 72 700 Td ({escaped}) Tj ET");
            bodies.push(format!(
                "<< /Length {} >>\nstream\n{}\nendstream",
                stream.len(),
                stream
            ));
        }

        // Assemble file, tracking byte offsets for the xref table.
        let mut pdf = String::from("%PDF-1.4\n");
        let mut offsets: Vec<usize> = Vec::with_capacity(total_objs);
        for (idx, body) in bodies.iter().enumerate() {
            offsets.push(pdf.len());
            pdf.push_str(&format!("{} 0 obj\n{}\nendobj\n", idx + 1, body));
        }

        let xref_start = pdf.len();
        pdf.push_str(&format!("xref\n0 {}\n", total_objs + 1));
        pdf.push_str("0000000000 65535 f \n");
        for off in &offsets {
            pdf.push_str(&format!("{off:010} 00000 n \n"));
        }
        pdf.push_str(&format!(
            "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{}\n%%EOF",
            total_objs + 1,
            xref_start
        ));

        pdf.into_bytes()
    }

    // --- pure helper tests (run regardless of the `pdf` feature) ---

    #[test]
    fn cap_text_passes_short_text_through() {
        let s = "hello world";
        assert_eq!(cap_text(s), s);
    }

    #[test]
    fn cap_text_truncates_oversized_text() {
        let big = "a".repeat(MAX_PDF_TEXT_BYTES + 5_000);
        let out = cap_text(&big);
        assert!(out.ends_with(TRUNCATION_MARKER));
        assert!(out.len() <= MAX_PDF_TEXT_BYTES + TRUNCATION_MARKER.len());
        assert!(out.len() < big.len());
    }

    #[test]
    fn resolve_range_defaults_to_whole_document() {
        assert_eq!(resolve_range(None, None, 5), Ok((0, 5)));
    }

    #[test]
    fn resolve_range_handles_explicit_bounds_and_errors() {
        // Valid sub-range.
        assert_eq!(resolve_range(Some(2), Some(4), 6), Ok((1, 4)));
        // Upper bound clamps to page count rather than erroring.
        assert_eq!(resolve_range(Some(3), Some(9999), 5), Ok((2, 5)));
        // start > end is rejected.
        assert!(resolve_range(Some(4), Some(2), 6).is_err());
        // 0-based input is rejected (pages are 1-based).
        assert!(resolve_range(Some(0), None, 6).is_err());
        // start past the end of the document is rejected.
        assert!(resolve_range(Some(10), None, 5).is_err());
        // empty document is rejected.
        assert!(resolve_range(None, None, 0).is_err());
    }

    #[test]
    fn schema_and_metadata_are_well_formed() {
        let tool = PdfTool::new();
        assert_eq!(tool.name(), "pdf_extract");
        assert!(tool.is_concurrency_safe(&json!({})));
        let schema = tool.input_schema();
        assert_eq!(schema["required"][0], "file_path");
        assert!(
            tool.describe(&json!({"file_path": "/x.pdf"}))
                .contains("/x.pdf")
        );
        assert!(
            tool.describe(&json!({"file_path": "/x.pdf", "start_page": 2, "end_page": 4}))
                .contains("pages 2-4")
        );
    }

    // --- error-path tests (run regardless of the `pdf` feature) ---

    #[tokio::test]
    async fn missing_file_returns_error() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("does_not_exist.pdf");
        let tool = PdfTool::new();
        let result = tool
            .execute(json!({ "file_path": path.to_str().unwrap() }))
            .await;
        assert!(result.is_error);
        assert!(result.content.contains("not found"));
    }

    #[tokio::test]
    async fn missing_file_path_param_returns_error() {
        let tool = PdfTool::new();
        let result = tool.execute(json!({})).await;
        assert!(result.is_error);
        assert!(result.content.contains("file_path"));
    }

    #[tokio::test]
    async fn relative_path_is_refused() {
        let tool = PdfTool::new();
        let result = tool
            .execute(json!({ "file_path": "relative/doc.pdf" }))
            .await;
        assert!(result.is_error);
        assert!(result.content.contains("Refused"));
    }

    // --- backend tests (only meaningful with the `pdf` feature on) ---

    #[cfg(feature = "pdf")]
    #[tokio::test]
    async fn extracts_text_from_whole_document() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("doc.pdf");
        std::fs::write(&path, make_pdf(&["Hello Wayland", "Second page here"])).unwrap();

        let tool = PdfTool::new();
        let result = tool
            .execute(json!({ "file_path": path.to_str().unwrap() }))
            .await;

        assert!(!result.is_error, "unexpected error: {}", result.content);
        assert!(
            result.content.contains("Hello Wayland"),
            "missing page 1 text in: {:?}",
            result.content
        );
        assert!(
            result.content.contains("Second page here"),
            "missing page 2 text in: {:?}",
            result.content
        );
    }

    #[cfg(feature = "pdf")]
    #[tokio::test]
    async fn extracts_text_from_page_range() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("ranged.pdf");
        std::fs::write(
            &path,
            make_pdf(&["AlphaPage", "BetaPage", "GammaPage", "DeltaPage"]),
        )
        .unwrap();

        let tool = PdfTool::new();
        // Pages 2-3 only.
        let result = tool
            .execute(json!({
                "file_path": path.to_str().unwrap(),
                "start_page": 2,
                "end_page": 3
            }))
            .await;

        assert!(!result.is_error, "unexpected error: {}", result.content);
        assert!(result.content.contains("BetaPage"));
        assert!(result.content.contains("GammaPage"));
        assert!(
            !result.content.contains("AlphaPage"),
            "page 1 leaked into the 2-3 range: {:?}",
            result.content
        );
        assert!(
            !result.content.contains("DeltaPage"),
            "page 4 leaked into the 2-3 range: {:?}",
            result.content
        );
    }

    #[cfg(feature = "pdf")]
    #[tokio::test]
    async fn truncation_kicks_in_for_large_document() {
        // Build a PDF whose total extracted text far exceeds the cap.
        // ~600 chars per page line; 200 pages clears 50_000 bytes.
        let line = "X".repeat(600);
        let pages: Vec<&str> = (0..200).map(|_| line.as_str()).collect();

        let dir = tempdir().unwrap();
        let path = dir.path().join("huge.pdf");
        std::fs::write(&path, make_pdf(&pages)).unwrap();

        let tool = PdfTool::new();
        let result = tool
            .execute(json!({ "file_path": path.to_str().unwrap() }))
            .await;

        assert!(!result.is_error, "unexpected error: {}", result.content);
        assert!(
            result.content.ends_with(TRUNCATION_MARKER),
            "expected truncation marker, got tail: {:?}",
            &result.content[result.content.len().saturating_sub(40)..]
        );
        assert!(result.content.len() <= MAX_PDF_TEXT_BYTES + TRUNCATION_MARKER.len());
    }

    #[cfg(feature = "pdf")]
    #[tokio::test]
    async fn corrupt_non_pdf_file_returns_error() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("not_a.pdf");
        std::fs::write(&path, b"this is plain text, definitely not a PDF").unwrap();

        let tool = PdfTool::new();
        let result = tool
            .execute(json!({ "file_path": path.to_str().unwrap() }))
            .await;

        assert!(
            result.is_error,
            "corrupt file should fail: {}",
            result.content
        );
        assert!(result.content.contains("Failed to extract text"));
    }

    #[cfg(feature = "pdf")]
    #[tokio::test]
    async fn page_range_past_end_is_rejected() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("small.pdf");
        std::fs::write(&path, make_pdf(&["OnlyPage"])).unwrap();

        let tool = PdfTool::new();
        let result = tool
            .execute(json!({
                "file_path": path.to_str().unwrap(),
                "start_page": 5
            }))
            .await;

        assert!(result.is_error);
        assert!(result.content.contains("Invalid page range"));
    }

    /// When the crate is built without the `pdf` feature, the tool must
    /// still register and fail loudly with an honest message.
    #[cfg(not(feature = "pdf"))]
    #[tokio::test]
    async fn degrades_gracefully_without_pdf_feature() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("doc.pdf");
        std::fs::write(&path, b"%PDF-1.4\n").unwrap();

        let tool = PdfTool::new();
        let result = tool
            .execute(json!({ "file_path": path.to_str().unwrap() }))
            .await;

        assert!(result.is_error);
        assert!(result.content.contains("without the `pdf` feature"));
    }
}
