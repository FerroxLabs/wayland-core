//! #650: read-only office-document text-extraction tool.
//!
//! Extracts clean text / markdown from `docx`, `xlsx`, `pptx`, and `csv`
//! files on the local filesystem, mirroring [`crate::pdf_tool::PdfTool`]'s
//! posture. This is the engine capability half of the document-ingestion
//! feature — it makes office docs *accessible* to the model on every surface
//! (headless, CLI, desktop). Auto-ingest-on-attach is a separate follow-up.
//!
//! ## Backends (all pure-Rust, no native build deps)
//!
//! - `xlsx` → [`calamine`], read via its STREAMING cell reader (never the dense
//!   `worksheet_range`, which would materialize the full declared bounding box —
//!   a two-cell `A1` + `XFD1048576` sheet is a decompression-cheap OOM through
//!   the dense path). Rendered as markdown tables, bounded cell count.
//! - `docx` → [`zip`] + [`quick_xml`] over `word/document.xml` (text + tables).
//! - `pptx` → [`zip`] + [`quick_xml`] over `ppt/slides/slideN.xml`, per slide.
//! - `csv` → the [`csv`] crate, rendered as a markdown table.
//!
//! Gated behind the **default-on** `doc-extract` cargo feature (mirroring
//! `pdf`). A `--no-default-features` build drops the parser tree and the tool
//! still registers — it just returns an honest "compiled without doc-extract"
//! error instead of silently disappearing.
//!
//! ## Safety posture (parsers over untrusted files)
//!
//! - **Path:** validated via [`crate::path_validation::validate_user_path`]
//!   then opened ONCE; every subsequent step works from that single handle /
//!   the `ZipArchive` built from it, so there is no validate-then-reopen TOCTOU.
//! - **On-disk cap:** files over [`MAX_ON_DISK_BYTES`] are rejected up front.
//! - **Zip-bomb:** entry-count / declared-total caps reject before extraction;
//!   docx/pptx part reads use a bounded `take()` that does not trust declared
//!   sizes; xlsx is read cell-by-cell (streaming), so a lying `<dimension>`
//!   cannot force a giant allocation.
//! - **Memory:** intermediate buffers are bounded and rendering early-stops at
//!   `max_chars`, so output never balloons before the final cap.
//! - **XXE / billion-laughs:** `quick_xml` never expands external or custom
//!   entities (they surface as ignored `GeneralRef`/`DocType` events); calamine
//!   parses its XML with `quick_xml` too, inheriting the same non-expansion.
//! - **No macro/formula execution:** cells and XML are parsed as data only.

use std::path::Path;

use async_trait::async_trait;
use serde_json::{Value, json};

use wcore_protocol::events::ToolCategory;
use wcore_types::tool::{JsonSchema, ToolResult};

use crate::Tool;
use crate::path_validation::validate_user_path;
use crate::tool_output_limits::DEFAULT_MAX_BYTES;
use crate::truncate_utf8;

/// Upper bound on the size of the truncation/continuation marker
/// [`window_text`] appends, used only for `max_result_size` accounting.
const MAX_MARKER_BYTES: usize = 200;

/// Hard ceiling on how far `offset + max_chars` can reach, so an absurd
/// `offset` cannot drive the extractor to its structural caps needlessly. 8 MB
/// of extracted text is far beyond any model context window, so no legitimate
/// paging session needs more.
#[cfg_attr(not(feature = "doc-extract"), allow(dead_code))]
const MAX_EXTRACT_BUDGET: usize = 8 * 1024 * 1024;

/// Default byte cap for extracted text before truncation. Reuses the shared
/// terminal-output cap (50_000) so document output is bounded consistently
/// with other large-output tools. Overridable downward via `max_chars`.
pub const MAX_DOC_TEXT_BYTES: usize = DEFAULT_MAX_BYTES;

/// Reject a source file larger than this on disk before opening it. Applies to
/// every format (not just csv).
#[cfg_attr(not(feature = "doc-extract"), allow(dead_code))]
const MAX_ON_DISK_BYTES: u64 = 50 * 1024 * 1024;

/// Read-only office-document text-extraction tool.
#[derive(Debug, Default, Clone, Copy)]
pub struct DocExtractTool;

impl DocExtractTool {
    /// Construct a new `DocExtractTool`. Stateless — one instance serves all
    /// calls.
    pub fn new() -> Self {
        Self
    }
}

/// Window `text` to the byte range starting at `offset`, at most `max_bytes`
/// long, appending an honest size + continuation marker (#650 contract: never
/// silently truncate — a truncated document must say how much was shown and how
/// to reach the rest).
///
/// `budget` is the internal extraction cap (`offset + max_bytes`). When the
/// extractor filled it exactly (`text.len() >= budget`) the document has more
/// content past this window, so the marker points the caller at the next
/// `offset`. When the extractor stopped short of `budget`, the true end was
/// reached and the total byte count is known and reported.
#[cfg_attr(not(feature = "doc-extract"), allow(dead_code))]
fn window_text(text: &str, offset: usize, max_bytes: usize, budget: usize) -> String {
    let extracted = text.len();
    // Did the extractor stop because it hit its cap (more may follow) or because
    // it reached the natural end of the document (so `extracted` is the total)?
    let hit_cap = extracted >= budget;

    // Snap the requested offset down to a UTF-8 char boundary and clamp.
    let mut start = offset.min(extracted);
    while start > 0 && !text.is_char_boundary(start) {
        start -= 1;
    }
    if start >= extracted {
        return if hit_cap {
            format!(
                "[no document text at byte offset {offset}; the previous window ended earlier — \
                 re-request with a smaller offset]"
            )
        } else {
            format!(
                "[end of document: {extracted} bytes total; byte offset {offset} is past the end]"
            )
        };
    }

    let shown = truncate_utf8(&text[start..], max_bytes);
    let shown_end = start + shown.len();
    let more = hit_cap || shown_end < extracted;

    if !more {
        // Reached the true end within this window — total size is known exactly.
        if start == 0 {
            return shown.to_string();
        }
        return format!(
            "{shown}\n\n... [end of document: bytes {start}\u{2013}{shown_end} of {extracted} total]"
        );
    }

    // More content remains — give an honest size note and the continuation offset.
    let total_note = if hit_cap {
        format!("{shown_end}+ bytes")
    } else {
        format!("{extracted} bytes")
    };
    format!(
        "{shown}\n\n... [document text truncated: showing bytes {start}\u{2013}{shown_end} of \
         {total_note}; pass offset={shown_end} to continue]"
    )
}

#[async_trait]
impl Tool for DocExtractTool {
    fn name(&self) -> &str {
        "doc_extract"
    }

    fn description(&self) -> &str {
        "Extracts clean text / markdown from an office document on the local \
         filesystem. Read-only. Supports .docx, .xlsx, .pptx, and .csv.\n\n\
         Usage:\n\
         - path must be an absolute path to the document.\n\
         - Spreadsheets render as markdown tables, one section per sheet; pass \
           sheet (a sheet name or 1-based index) to extract just one.\n\
         - Word docs render paragraphs and tables (as markdown tables).\n\
         - Presentations render one section per slide.\n\
         - Output is truncated if very large; lower max_chars to bound it.\n\
         - This tool never modifies the document and never runs macros/formulas."
    }

    fn input_schema(&self) -> JsonSchema {
        json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "Absolute path to the document to read (.docx/.xlsx/.pptx/.csv)"
                },
                "sheet": {
                    "type": ["string", "integer"],
                    "description": "Spreadsheets only: a sheet name or 1-based index. Omit to extract every sheet."
                },
                "max_chars": {
                    "type": "integer",
                    "minimum": 1,
                    "description": "Cap the extracted text length (bytes). Defaults to a bounded cap."
                },
                "offset": {
                    "type": "integer",
                    "minimum": 0,
                    "description": "Byte offset to start extraction from, for paging through a large document. Use the offset printed in a prior call's truncation marker to read the next chunk. Defaults to 0."
                }
            },
            "required": ["path"],
            "additionalProperties": false
        })
    }

    fn is_concurrency_safe(&self, _input: &Value) -> bool {
        // Read-only filesystem access — safe to run alongside other tools.
        true
    }

    async fn execute(&self, input: Value) -> ToolResult {
        let Some(path) = input.get("path").and_then(|v| v.as_str()) else {
            return ToolResult {
                content: "Missing required parameter: path".to_string(),
                is_error: true,
            };
        };

        // Same path discipline as ReadTool / PdfTool.
        let validated = match validate_user_path(Path::new(path)) {
            Ok(p) => p,
            Err(e) => {
                return ToolResult {
                    content: format!("Refused to read {path}: {e}"),
                    is_error: true,
                };
            }
        };

        if !validated.is_file() {
            return ToolResult {
                content: format!("Document not found or not a regular file: {path}"),
                is_error: true,
            };
        }

        // `sheet` may arrive as a string or a number per the schema.
        let sheet = input
            .get("sheet")
            .and_then(|v| match v {
                Value::String(s) => Some(s.clone()),
                Value::Number(n) => Some(n.to_string()),
                _ => None,
            })
            .filter(|s| !s.is_empty());

        let max_bytes = input
            .get("max_chars")
            .and_then(|v| v.as_u64())
            .map(|n| (n as usize).min(MAX_DOC_TEXT_BYTES))
            .unwrap_or(MAX_DOC_TEXT_BYTES);

        let offset = input
            .get("offset")
            .and_then(|v| v.as_u64())
            .map(|n| n as usize)
            .unwrap_or(0);

        extract(&validated, path, sheet.as_deref(), offset, max_bytes)
    }

    fn max_result_size(&self) -> usize {
        // Slightly above the cap so window_text's own marker is never clipped a
        // second time by the registry-level truncation.
        MAX_DOC_TEXT_BYTES + MAX_MARKER_BYTES
    }

    fn category(&self) -> ToolCategory {
        // Read-only file inspection — mirrors ReadTool / PdfTool.
        ToolCategory::Info
    }

    fn describe(&self, input: &Value) -> String {
        let path = input
            .get("path")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown");
        match input.get("sheet") {
            Some(Value::String(s)) => format!("Extract text from {path} (sheet {s})"),
            Some(Value::Number(n)) => format!("Extract text from {path} (sheet {n})"),
            _ => format!("Extract text from {path}"),
        }
    }
}

// ── Real backend — `doc-extract` feature ON ─────────────────────────────────

/// Zip-bomb caps for OOXML containers, enforced on DECLARED sizes as a fast
/// reject; the actual reads below (bounded `take` for docx/pptx, streaming for
/// xlsx) never trust the declared numbers.
#[cfg(feature = "doc-extract")]
const MAX_ENTRY_COUNT: usize = 10_000;
#[cfg(feature = "doc-extract")]
const MAX_ENTRY_UNCOMPRESSED: u64 = 100 * 1024 * 1024;
#[cfg(feature = "doc-extract")]
const MAX_TOTAL_UNCOMPRESSED: u64 = 300 * 1024 * 1024;
/// Cap the CSV rows / spreadsheet present-cells / rendered grid so a document
/// within the byte limits cannot balloon memory before the output cap trims.
#[cfg(feature = "doc-extract")]
const MAX_CSV_ROWS: usize = 100_000;
#[cfg(feature = "doc-extract")]
const MAX_CELLS: usize = 500_000;
/// Only build a dense render grid when the present cells' bounding box is at
/// most this many cells; a larger (sparse) box renders as a bounded list so a
/// two-cell `A1`+max-coordinate sheet can never allocate a giant grid.
#[cfg(feature = "doc-extract")]
const MAX_GRID_CELLS: u64 = 1_000_000;

#[cfg(feature = "doc-extract")]
fn extract(
    disk_path: &Path,
    display: &str,
    sheet: Option<&str>,
    offset: usize,
    max_bytes: usize,
) -> ToolResult {
    // Extract enough to cover the requested window [offset, offset+max_bytes],
    // clamped to a hard ceiling so an absurd offset can't drive the extractor to
    // its structural caps for nothing. The extractor's own byte early-stops use
    // this budget; window_text then slices out the [offset..] chunk.
    let budget = offset.saturating_add(max_bytes).min(MAX_EXTRACT_BUDGET);
    match extract_inner(disk_path, sheet, budget) {
        Ok(text) => ToolResult {
            content: window_text(&text, offset, max_bytes, budget),
            is_error: false,
        },
        Err(e) => ToolResult {
            content: format!("Failed to extract text from {display}: {e}"),
            is_error: true,
        },
    }
}

#[cfg(feature = "doc-extract")]
fn extract_inner(path: &Path, sheet: Option<&str>, max_bytes: usize) -> Result<String, String> {
    use std::fs::File;
    use std::io::{Read, Seek, SeekFrom};

    // Open ONCE: every step below works from this handle or the archive built
    // from it — no validate-then-reopen TOCTOU.
    let mut file = File::open(path).map_err(|e| e.to_string())?;
    let meta = file.metadata().map_err(|e| e.to_string())?;
    if !meta.is_file() {
        return Err("not a regular file".to_string());
    }
    if meta.len() > MAX_ON_DISK_BYTES {
        return Err(format!(
            "file too large ({} bytes > {MAX_ON_DISK_BYTES} limit)",
            meta.len()
        ));
    }

    let mut magic = [0u8; 4];
    let n = file.read(&mut magic).map_err(|e| e.to_string())?;
    file.seek(SeekFrom::Start(0)).map_err(|e| e.to_string())?;

    // OOXML (docx/xlsx/pptx) is a ZIP container (`PK\x03\x04`).
    if n >= 4 && &magic == b"PK\x03\x04" {
        let mut archive =
            zip::ZipArchive::new(file).map_err(|e| format!("not a valid zip archive: {e}"))?;
        check_zip_declared_limits(&mut archive)?;
        let names: Vec<String> = archive.file_names().map(str::to_string).collect();
        if names.iter().any(|x| x == "word/document.xml") {
            let xml = read_zip_text_part(&mut archive, "word/document.xml")?;
            return Ok(docx_xml_to_text(&xml, max_bytes));
        }
        if names.iter().any(|x| x == "ppt/presentation.xml") {
            return extract_pptx(&mut archive, max_bytes);
        }
        if names.iter().any(|x| x.starts_with("xl/")) {
            // Reuse the SAME handle (no reopen): stream cells from calamine.
            let file = archive.into_inner();
            return extract_xlsx(file, sheet, max_bytes);
        }
        return Err("unrecognized document: a ZIP that is not a .docx / .xlsx / .pptx".to_string());
    }

    // Non-zip → CSV / plain text, from the already-open handle.
    extract_csv(file, max_bytes)
}

/// Reject on entry-count / declared-total caps before extracting.
#[cfg(feature = "doc-extract")]
fn check_zip_declared_limits(archive: &mut zip::ZipArchive<std::fs::File>) -> Result<(), String> {
    if archive.len() > MAX_ENTRY_COUNT {
        return Err(format!(
            "archive has too many entries ({} > {MAX_ENTRY_COUNT}) — possible zip bomb",
            archive.len()
        ));
    }
    let mut total: u64 = 0;
    for i in 0..archive.len() {
        let entry = archive
            .by_index(i)
            .map_err(|e| format!("corrupt zip entry {i}: {e}"))?;
        if entry.size() > MAX_ENTRY_UNCOMPRESSED {
            return Err(format!(
                "zip entry {:?} exceeds the per-entry size limit — possible zip bomb",
                entry.name()
            ));
        }
        total = total.saturating_add(entry.size());
        if total > MAX_TOTAL_UNCOMPRESSED {
            return Err(
                "archive declared uncompressed size exceeds the limit — possible zip bomb"
                    .to_string(),
            );
        }
    }
    Ok(())
}

/// Read a named zip part as text with a bounded read that does NOT trust the
/// declared size (defends against a lying local-file header).
#[cfg(feature = "doc-extract")]
fn read_zip_text_part(
    archive: &mut zip::ZipArchive<std::fs::File>,
    name: &str,
) -> Result<String, String> {
    use std::io::Read;

    let entry = archive
        .by_name(name)
        .map_err(|e| format!("missing part {name}: {e}"))?;
    let mut buf = Vec::new();
    entry
        .take(MAX_ENTRY_UNCOMPRESSED + 1)
        .read_to_end(&mut buf)
        .map_err(|e| format!("reading {name}: {e}"))?;
    if buf.len() as u64 > MAX_ENTRY_UNCOMPRESSED {
        return Err(format!("part {name} exceeded the size limit during read"));
    }
    String::from_utf8(buf).map_err(|_| format!("part {name} is not valid UTF-8"))
}

/// Choose which sheet names to render: one by name (case-insensitive) or
/// 1-based index, or all when `sheet` is `None`.
#[cfg(feature = "doc-extract")]
fn select_sheets(names: &[String], sheet: Option<&str>) -> Result<Vec<String>, String> {
    match sheet {
        None => Ok(names.to_vec()),
        Some(s) => {
            if let Some(found) = names.iter().find(|n| n.eq_ignore_ascii_case(s)) {
                return Ok(vec![found.clone()]);
            }
            match s.parse::<usize>() {
                Ok(idx) if idx >= 1 && idx <= names.len() => Ok(vec![names[idx - 1].clone()]),
                Ok(_) => Err(format!(
                    "sheet index {s} out of range (1..={})",
                    names.len()
                )),
                Err(_) => Err(format!(
                    "sheet '{s}' not found; available: {}",
                    names.join(", ")
                )),
            }
        }
    }
}

/// xlsx → markdown tables, read via calamine's STREAMING cell reader so a
/// lying `<dimension>` cannot force a dense full-sheet allocation.
#[cfg(feature = "doc-extract")]
fn extract_xlsx<R: std::io::Read + std::io::Seek>(
    reader: R,
    sheet: Option<&str>,
    max_bytes: usize,
) -> Result<String, String> {
    use calamine::{Reader, Xlsx};

    let mut wb: Xlsx<R> = Xlsx::new(reader).map_err(|e| format!("cannot open xlsx: {e}"))?;
    let names = wb.sheet_names().to_owned();
    if names.is_empty() {
        return Err("spreadsheet has no sheets".to_string());
    }
    let selected = select_sheets(&names, sheet)?;

    let mut out = String::new();
    for name in &selected {
        out.push_str(&format!("## Sheet: {name}\n\n"));
        let mut cells: Vec<(u32, u32, String)> = Vec::new();
        let mut truncated = false;
        {
            let mut cr = wb
                .worksheet_cells_reader(name)
                .map_err(|e| format!("cannot read sheet '{name}': {e}"))?;
            let mut examined = 0usize;
            while let Some(cell) = cr
                .next_cell()
                .map_err(|e| format!("reading sheet '{name}': {e}"))?
            {
                // Count EVERY visited cell, not only populated ones. An xlsx
                // packed with a huge run of empty <c> elements (tiny + highly
                // compressible, staying under the zip-bomb byte caps) would
                // otherwise drive this loop unbounded: empty cells `continue`
                // before a populated-only counter, so MAX_CELLS never trips —
                // defeating the exact anti-DoS guard this loop exists for.
                examined += 1;
                if examined > MAX_CELLS {
                    truncated = true;
                    break;
                }
                let value = dataref_to_string(cell.get_value());
                if value.is_empty() {
                    continue;
                }
                let (row, col) = cell.get_position();
                cells.push((row, col, value));
            }
        }
        render_sparse_cells(&cells, &mut out, max_bytes);
        if truncated {
            out.push_str("\n_[sheet truncated: cell-scan limit reached]_\n");
        }
        out.push('\n');
        if out.len() >= max_bytes {
            break;
        }
    }
    Ok(out)
}

/// Convert a streamed `DataRef` cell to its display string. `DataRef` (unlike
/// `Data`) has no `Display`, so match explicitly.
#[cfg(feature = "doc-extract")]
fn dataref_to_string(d: &calamine::DataRef) -> String {
    use calamine::DataRef;
    match d {
        DataRef::Int(n) => n.to_string(),
        DataRef::Float(f) => f.to_string(),
        DataRef::String(s) => s.clone(),
        DataRef::SharedString(s) => s.to_string(),
        DataRef::Bool(b) => b.to_string(),
        DataRef::DateTime(dt) => dt.to_string(),
        DataRef::DateTimeIso(s) => s.clone(),
        DataRef::DurationIso(s) => s.clone(),
        DataRef::Error(e) => e.to_string(),
        DataRef::Empty => String::new(),
    }
}

/// Render sparse (row, col, value) cells. When the populated bounding box is
/// small enough, build a dense markdown table; otherwise (a sparse box spanning
/// a huge coordinate range) fall back to a bounded `(row, col): value` list so
/// no giant grid is ever allocated. Early-stops at `max_bytes`.
#[cfg(feature = "doc-extract")]
fn render_sparse_cells(cells: &[(u32, u32, String)], out: &mut String, max_bytes: usize) {
    if cells.is_empty() {
        out.push_str("_(empty)_\n");
        return;
    }
    let min_r = cells.iter().map(|c| c.0).min().unwrap();
    let max_r = cells.iter().map(|c| c.0).max().unwrap();
    let min_c = cells.iter().map(|c| c.1).min().unwrap();
    let max_c = cells.iter().map(|c| c.1).max().unwrap();
    let rows = (max_r - min_r) as u64 + 1;
    let cols = (max_c - min_c) as u64 + 1;

    if rows.saturating_mul(cols) <= MAX_GRID_CELLS {
        let nrows = rows as usize;
        let ncols = cols as usize;
        let mut grid = vec![vec![String::new(); ncols]; nrows];
        for (r, c, v) in cells {
            grid[(*r - min_r) as usize][(*c - min_c) as usize] = v.clone();
        }
        render_markdown_table(grid.into_iter(), out, max_bytes);
    } else {
        // Sparse box too large for a grid — list populated cells (1-based).
        for (r, c, v) in cells {
            out.push_str(&format!(
                "- ({}, {}): {}\n",
                r + 1,
                c + 1,
                v.replace(['\n', '\r'], " ")
            ));
            if out.len() >= max_bytes {
                break;
            }
        }
    }
}

/// Strip a namespace prefix (`w:tbl` → `tbl`) so element matching is robust to
/// the producer's chosen prefix. Uses only the proven `QName::as_ref()` bytes.
#[cfg(feature = "doc-extract")]
fn local_name(qname: &[u8]) -> &[u8] {
    match qname.iter().position(|&b| b == b':') {
        Some(i) => &qname[i + 1..],
        None => qname,
    }
}

/// Stream `word/document.xml` into text. Local element names (prefix stripped):
/// `t` = text run, `p` = paragraph, `tbl`/`tr`/`tc` = table/row/cell. Only the
/// OUTERMOST table is rendered as markdown; a nested table's text flows into the
/// containing cell (its rows never disturb the outer table's accumulation).
/// Appends stop once `max_bytes` is reached.
#[cfg(feature = "doc-extract")]
fn docx_xml_to_text(xml: &str, max_bytes: usize) -> String {
    use quick_xml::events::Event;
    use quick_xml::reader::Reader;

    let mut reader = Reader::from_str(xml);
    let mut out = String::new();
    let mut in_text = false;
    let mut table_depth = 0usize;
    let mut rows: Vec<Vec<String>> = Vec::new();
    let mut cur_row: Vec<String> = Vec::new();
    let mut cur_cell = String::new();
    let mut para = String::new();

    loop {
        if out.len() >= max_bytes {
            break;
        }
        match reader.read_event() {
            Ok(Event::Start(e)) => match local_name(e.name().as_ref()) {
                b"t" => in_text = true,
                b"tbl" => {
                    // Only reset row accumulation when entering the OUTERMOST
                    // table; a nested <tbl> must not wipe the outer rows.
                    if table_depth == 0 {
                        rows.clear();
                    }
                    table_depth += 1;
                }
                b"tr" if table_depth == 1 => cur_row.clear(),
                b"tc" if table_depth == 1 => cur_cell.clear(),
                _ => {}
            },
            Ok(Event::End(e)) => match local_name(e.name().as_ref()) {
                b"t" => in_text = false,
                b"p" => {
                    if table_depth > 0 {
                        cur_cell.push(' ');
                    } else {
                        let line = para.trim_end();
                        if !line.is_empty() {
                            out.push_str(line);
                        }
                        out.push('\n');
                        para.clear();
                    }
                }
                b"tc" if table_depth == 1 => cur_row.push(cur_cell.trim().to_string()),
                b"tr" if table_depth == 1 => rows.push(std::mem::take(&mut cur_row)),
                b"tbl" => {
                    table_depth = table_depth.saturating_sub(1);
                    if table_depth == 0 {
                        render_markdown_table(
                            std::mem::take(&mut rows).into_iter(),
                            &mut out,
                            max_bytes,
                        );
                        out.push('\n');
                    }
                }
                _ => {}
            },
            Ok(Event::Text(e)) => {
                if in_text && let Ok(t) = e.decode() {
                    if table_depth > 0 {
                        cur_cell.push_str(&t);
                    } else {
                        para.push_str(&t);
                    }
                }
            }
            Ok(Event::Eof) => break,
            Err(_) => break,
            _ => {}
        }
    }
    let tail = para.trim_end();
    if !tail.is_empty() && out.len() < max_bytes {
        out.push_str(tail);
        out.push('\n');
    }
    out
}

/// pptx → one section per slide, in numeric slide-file order. (v1 uses the
/// `slideN.xml` numeric order, a close approximation of PowerPoint's true
/// relationship-defined order; rel-order is a follow-up.) Early-stops at
/// `max_bytes`.
#[cfg(feature = "doc-extract")]
fn extract_pptx(
    archive: &mut zip::ZipArchive<std::fs::File>,
    max_bytes: usize,
) -> Result<String, String> {
    let mut slides: Vec<String> = archive
        .file_names()
        .filter(|n| n.starts_with("ppt/slides/slide") && n.ends_with(".xml"))
        .map(str::to_string)
        .collect();
    slides.sort_by_key(|n| slide_number(n));

    if slides.is_empty() {
        return Err("presentation has no slides".to_string());
    }

    let mut out = String::new();
    for (idx, name) in slides.iter().enumerate() {
        if out.len() >= max_bytes {
            break;
        }
        let xml = read_zip_text_part(archive, name)?;
        let text = pptx_slide_text(&xml);
        out.push_str(&format!("## Slide {}\n\n", idx + 1));
        out.push_str(text.trim());
        out.push_str("\n\n");
    }
    Ok(out)
}

/// Sort key for `ppt/slides/slideN.xml` by the numeric N.
#[cfg(feature = "doc-extract")]
fn slide_number(name: &str) -> usize {
    name.rsplit('/')
        .next()
        .unwrap_or(name)
        .trim_start_matches("slide")
        .trim_end_matches(".xml")
        .parse()
        .unwrap_or(usize::MAX)
}

/// Stream a slide XML into text: `a:t` = text run, `a:p` = paragraph/bullet.
#[cfg(feature = "doc-extract")]
fn pptx_slide_text(xml: &str) -> String {
    use quick_xml::events::Event;
    use quick_xml::reader::Reader;

    let mut reader = Reader::from_str(xml);
    let mut out = String::new();
    let mut in_text = false;

    loop {
        match reader.read_event() {
            Ok(Event::Start(e)) if local_name(e.name().as_ref()) == b"t" => in_text = true,
            Ok(Event::End(e)) => match local_name(e.name().as_ref()) {
                b"t" => in_text = false,
                b"p" => out.push('\n'),
                _ => {}
            },
            Ok(Event::Text(e)) => {
                if in_text && let Ok(t) = e.decode() {
                    out.push_str(&t);
                }
            }
            Ok(Event::Eof) => break,
            Err(_) => break,
            _ => {}
        }
    }
    out
}

/// csv → markdown table. Reads from the already-open handle, bounded.
#[cfg(feature = "doc-extract")]
fn extract_csv(file: std::fs::File, max_bytes: usize) -> Result<String, String> {
    use std::io::Read;

    let mut data = Vec::new();
    file.take(MAX_ON_DISK_BYTES + 1)
        .read_to_end(&mut data)
        .map_err(|e| format!("reading file: {e}"))?;
    if data.len() as u64 > MAX_ON_DISK_BYTES {
        return Err("file too large".to_string());
    }
    let text = String::from_utf8(data)
        .map_err(|_| "not a supported document type (not UTF-8 text)".to_string())?;

    let mut reader = csv::ReaderBuilder::new()
        .flexible(true)
        .has_headers(false)
        .from_reader(text.as_bytes());
    let mut rows: Vec<Vec<String>> = Vec::new();
    for record in reader.records() {
        let record = record.map_err(|e| format!("csv parse error: {e}"))?;
        rows.push(record.iter().map(str::to_string).collect());
        if rows.len() >= MAX_CSV_ROWS {
            break;
        }
    }
    let mut out = String::new();
    render_markdown_table(rows.into_iter(), &mut out, max_bytes);
    Ok(out)
}

/// Render rows as a GitHub-flavored markdown table. The first row is the
/// header. Cells have `|` escaped and embedded newlines flattened. Appends stop
/// once `out` reaches `max_bytes`.
#[cfg(feature = "doc-extract")]
fn render_markdown_table<I: Iterator<Item = Vec<String>>>(
    rows: I,
    out: &mut String,
    max_bytes: usize,
) {
    let rows: Vec<Vec<String>> = rows.collect();
    if rows.is_empty() {
        return;
    }
    let ncols = rows.iter().map(Vec::len).max().unwrap_or(0);
    if ncols == 0 {
        return;
    }
    let fmt_row = |cells: &[String]| -> String {
        let mut c: Vec<String> = cells
            .iter()
            .map(|s| s.replace('|', "\\|").replace(['\n', '\r'], " "))
            .collect();
        c.resize(ncols, String::new());
        format!("| {} |", c.join(" | "))
    };
    out.push_str(&fmt_row(&rows[0]));
    out.push('\n');
    out.push_str(&format!("|{}\n", " --- |".repeat(ncols)));
    for row in &rows[1..] {
        if out.len() >= max_bytes {
            break;
        }
        out.push_str(&fmt_row(row));
        out.push('\n');
    }
}

// ── Degraded backend — `doc-extract` feature OFF ────────────────────────────

/// The tool still registers and is schema-visible, but every call fails loudly
/// with an honest message (NO-STUBS: an honest blocker, not silent success).
#[cfg(not(feature = "doc-extract"))]
fn extract(
    _disk_path: &Path,
    display: &str,
    _sheet: Option<&str>,
    _offset: usize,
    _max_bytes: usize,
) -> ToolResult {
    ToolResult {
        content: format!(
            "Cannot extract text from {display}: this build of wcore-tools was \
             compiled without the `doc-extract` feature. Rebuild with the \
             default features (or `--features doc-extract`) to enable office-\
             document extraction."
        ),
        is_error: true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use tempfile::tempdir;

    // ── pure helper / schema tests (run regardless of the feature) ──────────

    #[test]
    fn window_text_passes_short_text_through() {
        // Whole doc fits, offset 0 → returned verbatim, no marker.
        let out = window_text("hello", 0, MAX_DOC_TEXT_BYTES, MAX_DOC_TEXT_BYTES);
        assert_eq!(out, "hello");
    }

    #[test]
    fn window_text_truncation_marks_size_and_continuation() {
        // Extractor hit its cap (text.len() >= budget) → the marker must carry a
        // size note AND a continuation offset (#650 contract: never silent).
        let big = "a".repeat(MAX_DOC_TEXT_BYTES + 5_000);
        let out = window_text(&big, 0, MAX_DOC_TEXT_BYTES, MAX_DOC_TEXT_BYTES);
        assert!(
            out.contains("document text truncated: showing bytes 0"),
            "size-annotated marker: {}",
            &out[out.len().saturating_sub(160)..]
        );
        assert!(
            out.contains(&format!("pass offset={MAX_DOC_TEXT_BYTES} to continue")),
            "continuation offset present"
        );
        assert!(out.len() <= MAX_DOC_TEXT_BYTES + MAX_MARKER_BYTES);
        assert!(out.len() < big.len());
    }

    #[test]
    fn window_text_offset_reads_next_chunk_and_reports_end() {
        // A 100-byte doc, offset 50, generous budget → returns the tail and
        // reports the exact total (end reached, not a silent stop).
        let text = "x".repeat(100);
        let out = window_text(&text, 50, 1_000, 1_050);
        assert!(out.contains("end of document"), "end marker: {out}");
        assert!(out.contains("of 100 total"), "exact total reported: {out}");
    }

    #[test]
    fn window_text_offset_past_end_is_honest() {
        // Offset beyond the extracted text must say so, not return empty-success.
        let text = "x".repeat(100);
        let out = window_text(&text, 200, 1_000, 1_200);
        assert!(out.contains("past the end"), "honest past-end note: {out}");
    }

    #[test]
    fn schema_and_metadata_are_well_formed() {
        let tool = DocExtractTool::new();
        assert_eq!(tool.name(), "doc_extract");
        assert!(tool.is_concurrency_safe(&json!({})));
        let schema = tool.input_schema();
        assert_eq!(schema["required"][0], "path");
        assert!(
            tool.describe(&json!({"path": "/x.docx"}))
                .contains("/x.docx")
        );
        assert!(
            tool.describe(&json!({"path": "/x.xlsx", "sheet": "Data"}))
                .contains("sheet Data")
        );
    }

    // ── error-path tests (run regardless of the feature) ────────────────────

    #[tokio::test]
    async fn missing_path_param_returns_error() {
        let tool = DocExtractTool::new();
        let result = tool.execute(json!({})).await;
        assert!(result.is_error);
        assert!(result.content.contains("path"));
    }

    #[tokio::test]
    async fn relative_path_is_refused() {
        let tool = DocExtractTool::new();
        let result = tool.execute(json!({ "path": "relative/doc.docx" })).await;
        assert!(result.is_error);
        assert!(result.content.contains("Refused"));
    }

    #[tokio::test]
    async fn missing_file_returns_error() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("nope.xlsx");
        let tool = DocExtractTool::new();
        let result = tool
            .execute(json!({ "path": path.to_str().unwrap() }))
            .await;
        assert!(result.is_error);
        assert!(result.content.contains("not found") || result.content.contains("not a regular"));
    }

    // ── backend tests (only meaningful with the feature on) ─────────────────

    #[cfg(feature = "doc-extract")]
    mod backend {
        use super::*;
        use std::io::Write;

        fn zip_bytes(parts: &[(&str, &str)]) -> Vec<u8> {
            let mut buf = Vec::new();
            {
                let mut zip = zip::ZipWriter::new(std::io::Cursor::new(&mut buf));
                let opts = zip::write::SimpleFileOptions::default();
                for (name, content) in parts {
                    zip.start_file(*name, opts).unwrap();
                    zip.write_all(content.as_bytes()).unwrap();
                }
                zip.finish().unwrap();
            }
            buf
        }

        fn write_tmp(name: &str, bytes: &[u8]) -> (tempfile::TempDir, std::path::PathBuf) {
            let dir = tempdir().unwrap();
            let path = dir.path().join(name);
            std::fs::write(&path, bytes).unwrap();
            (dir, path)
        }

        /// Minimal valid xlsx from (cellRef, inline-text) pairs on one sheet.
        /// `dimension` is the declared `<dimension ref>` (may lie about extent).
        fn make_xlsx(cells: &[(&str, &str)], dimension: &str) -> Vec<u8> {
            let content_types = r#"<?xml version="1.0"?><Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Default Extension="xml" ContentType="application/xml"/><Override PartName="/xl/workbook.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.sheet.main+xml"/><Override PartName="/xl/worksheets/sheet1.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.worksheet+xml"/></Types>"#;
            let root_rels = r#"<?xml version="1.0"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="xl/workbook.xml"/></Relationships>"#;
            let workbook = r#"<?xml version="1.0"?><workbook xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships"><sheets><sheet name="Sheet1" sheetId="1" r:id="rId1"/></sheets></workbook>"#;
            let wb_rels = r#"<?xml version="1.0"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/worksheet" Target="worksheets/sheet1.xml"/></Relationships>"#;
            let mut sd = String::new();
            for (r, v) in cells {
                // number cells if numeric, else inline string.
                if v.parse::<f64>().is_ok() {
                    sd.push_str(&format!(r#"<c r="{r}"><v>{v}</v></c>"#));
                } else {
                    sd.push_str(&format!(
                        r#"<c r="{r}" t="inlineStr"><is><t>{v}</t></is></c>"#
                    ));
                }
            }
            let sheet = format!(
                r#"<?xml version="1.0"?><worksheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"><dimension ref="{dimension}"/><sheetData><row>{sd}</row></sheetData></worksheet>"#
            );
            zip_bytes(&[
                ("[Content_Types].xml", content_types),
                ("_rels/.rels", root_rels),
                ("xl/workbook.xml", workbook),
                ("xl/_rels/workbook.xml.rels", wb_rels),
                ("xl/worksheets/sheet1.xml", &sheet),
            ])
        }

        #[test]
        fn render_markdown_table_shapes_header_and_rows() {
            let mut out = String::new();
            render_markdown_table(
                vec![
                    vec!["A".to_string(), "B".to_string()],
                    vec!["1".to_string(), "2".to_string()],
                ]
                .into_iter(),
                &mut out,
                MAX_DOC_TEXT_BYTES,
            );
            assert!(out.contains("| A | B |"));
            assert!(out.contains("| --- | --- |"));
            assert!(out.contains("| 1 | 2 |"));
        }

        #[test]
        fn render_markdown_table_escapes_pipes() {
            let mut out = String::new();
            render_markdown_table(
                vec![vec!["a|b".to_string()], vec!["c".to_string()]].into_iter(),
                &mut out,
                MAX_DOC_TEXT_BYTES,
            );
            assert!(out.contains("a\\|b"), "pipe must be escaped: {out}");
        }

        #[test]
        fn sparse_cells_huge_box_uses_list_not_grid() {
            // The xlsx OOM attack: two cells, A1 (0,0) and a max-coordinate cell.
            // A dense grid would be ~1.7e10 cells; the guard must fall to a list.
            let cells = vec![
                (0u32, 0u32, "corner".to_string()),
                (1_048_575u32, 16_383u32, "far".to_string()),
            ];
            let mut out = String::new();
            render_sparse_cells(&cells, &mut out, MAX_DOC_TEXT_BYTES);
            assert!(out.contains("corner"), "near cell present: {out}");
            assert!(out.contains("far"), "far cell present: {out}");
            assert!(out.contains("- ("), "must use list form, not a grid: {out}");
            assert!(!out.contains("| --- |"), "must NOT allocate a grid: {out}");
        }

        #[test]
        fn sparse_cells_small_box_uses_grid() {
            let cells = vec![
                (0u32, 0u32, "h1".to_string()),
                (0u32, 1u32, "h2".to_string()),
                (1u32, 0u32, "v1".to_string()),
            ];
            let mut out = String::new();
            render_sparse_cells(&cells, &mut out, MAX_DOC_TEXT_BYTES);
            assert!(out.contains("| h1 | h2 |"), "grid header: {out}");
        }

        #[test]
        fn docx_text_and_single_level_table() {
            let doc = r#"<?xml version="1.0"?>
                <w:document xmlns:w="x"><w:body>
                    <w:p><w:r><w:t>Hello Wayland</w:t></w:r></w:p>
                    <w:tbl>
                      <w:tr><w:tc><w:p><w:r><w:t>H1</w:t></w:r></w:p></w:tc>
                            <w:tc><w:p><w:r><w:t>H2</w:t></w:r></w:p></w:tc></w:tr>
                      <w:tr><w:tc><w:p><w:r><w:t>r1c1</w:t></w:r></w:p></w:tc>
                            <w:tc><w:p><w:r><w:t>r1c2</w:t></w:r></w:p></w:tc></w:tr>
                    </w:tbl>
                </w:body></w:document>"#;
            let text = docx_xml_to_text(doc, MAX_DOC_TEXT_BYTES);
            assert!(text.contains("Hello Wayland"), "para: {text}");
            assert!(text.contains("| H1 | H2 |"), "header: {text}");
            assert!(text.contains("| r1c1 | r1c2 |"), "row: {text}");
        }

        #[test]
        fn docx_nested_table_keeps_outer_rows() {
            // Outer table with a nested table inside its first cell. The outer
            // rows must survive (regression for the rows.clear() bug).
            let doc = r#"<?xml version="1.0"?>
                <w:document xmlns:w="x"><w:body><w:tbl>
                  <w:tr>
                    <w:tc><w:p><w:r><w:t>OUTER</w:t></w:r></w:p>
                      <w:tbl><w:tr><w:tc><w:p><w:r><w:t>inner</w:t></w:r></w:p></w:tc></w:tr></w:tbl>
                    </w:tc>
                    <w:tc><w:p><w:r><w:t>B</w:t></w:r></w:p></w:tc>
                  </w:tr>
                  <w:tr><w:tc><w:p><w:r><w:t>row2</w:t></w:r></w:p></w:tc>
                        <w:tc><w:p><w:r><w:t>C</w:t></w:r></w:p></w:tc></w:tr>
                </w:tbl></w:body></w:document>"#;
            let text = docx_xml_to_text(doc, MAX_DOC_TEXT_BYTES);
            assert!(text.contains("OUTER"), "outer cell content dropped: {text}");
            assert!(text.contains("row2"), "outer second row dropped: {text}");
            assert!(
                text.contains("| row2 | C |"),
                "outer table corrupted: {text}"
            );
        }

        #[tokio::test]
        async fn extracts_docx_end_to_end() {
            let doc = r#"<?xml version="1.0"?><w:document xmlns:w="x"><w:body>
                <w:p><w:r><w:t>Quarterly report</w:t></w:r></w:p></w:body></w:document>"#;
            let bytes = zip_bytes(&[("word/document.xml", doc)]);
            let (_d, path) = write_tmp("report.docx", &bytes);
            let result = DocExtractTool::new()
                .execute(json!({ "path": path.to_str().unwrap() }))
                .await;
            assert!(!result.is_error, "unexpected error: {}", result.content);
            assert!(result.content.contains("Quarterly report"));
        }

        #[tokio::test]
        async fn extracts_pptx_slides_in_order() {
            let slide = |t: &str| {
                format!(
                    r#"<?xml version="1.0"?><p:sld xmlns:p="x" xmlns:a="y"><p:cSld><p:spTree>
                    <a:p><a:r><a:t>{t}</a:t></a:r></a:p></p:spTree></p:cSld></p:sld>"#
                )
            };
            let bytes = zip_bytes(&[
                ("ppt/presentation.xml", "<p/>"),
                ("ppt/slides/slide2.xml", &slide("Second")),
                ("ppt/slides/slide10.xml", &slide("Tenth")),
            ]);
            let (_d, path) = write_tmp("deck.pptx", &bytes);
            let result = DocExtractTool::new()
                .execute(json!({ "path": path.to_str().unwrap() }))
                .await;
            assert!(!result.is_error, "unexpected error: {}", result.content);
            let second = result.content.find("Second").expect("slide2 text");
            let tenth = result.content.find("Tenth").expect("slide10 text");
            assert!(second < tenth, "slide2 before slide10: {}", result.content);
            assert!(result.content.contains("## Slide 1"));
            assert!(result.content.contains("## Slide 2"));
        }

        #[tokio::test]
        async fn extracts_csv_as_markdown_table() {
            let (_d, path) = write_tmp("data.csv", b"name,status\nalice,active\nbob,inactive");
            let result = DocExtractTool::new()
                .execute(json!({ "path": path.to_str().unwrap() }))
                .await;
            assert!(!result.is_error, "unexpected error: {}", result.content);
            assert!(result.content.contains("| name | status |"));
            assert!(result.content.contains("| alice | active |"));
        }

        #[tokio::test]
        async fn doc_pages_via_offset_continuation() {
            // #650 FIX-FIRST Finding 2: a doc larger than max_chars must truncate
            // WITH a continuation offset, and re-reading at that offset returns
            // the next chunk (never a silent tail-drop).
            let mut csv = String::from("col\n");
            for i in 0..200 {
                csv.push_str(&format!("row{i}\n"));
            }
            let (_d, path) = write_tmp("big.csv", csv.as_bytes());
            let p = path.to_str().unwrap().to_string();

            let first = DocExtractTool::new()
                .execute(json!({ "path": p, "max_chars": 40 }))
                .await;
            assert!(!first.is_error, "{}", first.content);
            assert!(
                first.content.contains("document text truncated")
                    && first.content.contains("pass offset="),
                "first chunk must truncate with a continuation offset: {}",
                first.content
            );

            // Parse the continuation offset and read the next chunk.
            let off: usize = first
                .content
                .rsplit("pass offset=")
                .next()
                .unwrap()
                .chars()
                .take_while(|c| c.is_ascii_digit())
                .collect::<String>()
                .parse()
                .expect("offset in marker");
            assert!(off > 0, "offset advances past the first chunk");

            let second = DocExtractTool::new()
                .execute(json!({ "path": p, "max_chars": 40, "offset": off }))
                .await;
            assert!(!second.is_error, "{}", second.content);
            assert_ne!(
                first.content, second.content,
                "the offset must advance to new content"
            );
            assert!(
                second.content.contains(&format!("bytes {off}")),
                "continuation chunk starts at the requested offset: {}",
                second.content
            );
        }

        #[tokio::test]
        async fn extracts_xlsx_cells_via_streaming() {
            let bytes = make_xlsx(
                &[
                    ("A1", "name"),
                    ("B1", "qty"),
                    ("A2", "widget"),
                    ("B2", "42"),
                ],
                "A1:B2",
            );
            let (_d, path) = write_tmp("book.xlsx", &bytes);
            let result = DocExtractTool::new()
                .execute(json!({ "path": path.to_str().unwrap() }))
                .await;
            assert!(!result.is_error, "unexpected error: {}", result.content);
            assert!(
                result.content.contains("## Sheet: Sheet1"),
                "sheet header: {}",
                result.content
            );
            assert!(
                result.content.contains("name"),
                "cell value: {}",
                result.content
            );
            assert!(
                result.content.contains("42"),
                "numeric cell: {}",
                result.content
            );
        }

        #[tokio::test]
        async fn extracts_xlsx_with_shared_strings() {
            // Real Excel stores text in xl/sharedStrings.xml referenced by
            // t="s" cells, resolved by calamine into DataRef::SharedString.
            // This verifies the streaming path end-to-end for the shape actual
            // Office files use (not just the inline-string fixtures above).
            let content_types = r#"<?xml version="1.0"?><Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Default Extension="xml" ContentType="application/xml"/><Override PartName="/xl/workbook.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.sheet.main+xml"/><Override PartName="/xl/worksheets/sheet1.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.worksheet+xml"/><Override PartName="/xl/sharedStrings.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.sharedStrings+xml"/></Types>"#;
            let root_rels = r#"<?xml version="1.0"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="xl/workbook.xml"/></Relationships>"#;
            let workbook = r#"<?xml version="1.0"?><workbook xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships"><sheets><sheet name="Sheet1" sheetId="1" r:id="rId1"/></sheets></workbook>"#;
            let wb_rels = r#"<?xml version="1.0"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/worksheet" Target="worksheets/sheet1.xml"/></Relationships>"#;
            let shared = r#"<?xml version="1.0"?><sst xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main" count="2" uniqueCount="2"><si><t>region</t></si><si><t>north</t></si></sst>"#;
            let sheet = r#"<?xml version="1.0"?><worksheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"><dimension ref="A1:A2"/><sheetData><row r="1"><c r="A1" t="s"><v>0</v></c></row><row r="2"><c r="A2" t="s"><v>1</v></c></row></sheetData></worksheet>"#;
            let bytes = zip_bytes(&[
                ("[Content_Types].xml", content_types),
                ("_rels/.rels", root_rels),
                ("xl/workbook.xml", workbook),
                ("xl/_rels/workbook.xml.rels", wb_rels),
                ("xl/sharedStrings.xml", shared),
                ("xl/worksheets/sheet1.xml", sheet),
            ]);
            let (_d, path) = write_tmp("shared.xlsx", &bytes);
            let result = DocExtractTool::new()
                .execute(json!({ "path": path.to_str().unwrap() }))
                .await;
            assert!(!result.is_error, "unexpected error: {}", result.content);
            assert!(
                result.content.contains("region"),
                "shared string 0: {}",
                result.content
            );
            assert!(
                result.content.contains("north"),
                "shared string 1: {}",
                result.content
            );
        }

        #[tokio::test]
        async fn xlsx_dimension_bomb_does_not_oom() {
            // Declares a full-sheet dimension but has only two real cells. The
            // streaming reader + sparse render must handle it without a giant
            // allocation. (If the dense path were used this would OOM.)
            let bytes = make_xlsx(&[("A1", "corner"), ("XFD1048576", "far")], "A1:XFD1048576");
            let (_d, path) = write_tmp("bomb.xlsx", &bytes);
            let result = DocExtractTool::new()
                .execute(json!({ "path": path.to_str().unwrap() }))
                .await;
            assert!(!result.is_error, "unexpected error: {}", result.content);
            assert!(result.content.contains("corner"));
            assert!(result.content.contains("far"));
        }

        // ── security tests ──────────────────────────────────────────────────

        #[tokio::test]
        async fn xlsx_empty_cell_flood_is_bounded() {
            // #650 FIX-FIRST Finding 1 (CRITICAL): a sheet packed with a huge run
            // of EMPTY cells (each tiny + highly compressible, so it stays under
            // the zip-bomb byte caps) must NOT drive the cell scan unbounded.
            // Empty cells previously `continue`d before the populated-only
            // MAX_CELLS counter, so the guard never tripped. The scan now counts
            // EVERY examined cell and must break at MAX_CELLS, reporting
            // truncation — proving bounded work on attacker input.
            let n = MAX_CELLS + 1_000;
            let mut sd = String::with_capacity(n * 40);
            let per_row = 16_000; // stay under Excel's 16_384 column limit
            let mut remaining = n;
            let mut row = 1;
            while remaining > 0 {
                let cells = remaining.min(per_row);
                sd.push_str(&format!("<row r=\"{row}\">"));
                for _ in 0..cells {
                    sd.push_str("<c t=\"inlineStr\"><is><t></t></is></c>");
                }
                sd.push_str("</row>");
                remaining -= cells;
                row += 1;
            }
            let content_types = r#"<?xml version="1.0"?><Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Default Extension="xml" ContentType="application/xml"/><Override PartName="/xl/workbook.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.sheet.main+xml"/><Override PartName="/xl/worksheets/sheet1.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.worksheet+xml"/></Types>"#;
            let root_rels = r#"<?xml version="1.0"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="xl/workbook.xml"/></Relationships>"#;
            let workbook = r#"<?xml version="1.0"?><workbook xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships"><sheets><sheet name="Sheet1" sheetId="1" r:id="rId1"/></sheets></workbook>"#;
            let wb_rels = r#"<?xml version="1.0"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/worksheet" Target="worksheets/sheet1.xml"/></Relationships>"#;
            let sheet = format!(
                r#"<?xml version="1.0"?><worksheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"><dimension ref="A1:A{n}"/><sheetData>{sd}</sheetData></worksheet>"#
            );
            let bytes = zip_bytes(&[
                ("[Content_Types].xml", content_types),
                ("_rels/.rels", root_rels),
                ("xl/workbook.xml", workbook),
                ("xl/_rels/workbook.xml.rels", wb_rels),
                ("xl/worksheets/sheet1.xml", &sheet),
            ]);
            let (_d, path) = write_tmp("flood.xlsx", &bytes);
            let result = DocExtractTool::new()
                .execute(json!({ "path": path.to_str().unwrap() }))
                .await;
            assert!(!result.is_error, "unexpected error: {}", result.content);
            assert!(
                result.content.contains("cell-scan limit reached"),
                "empty-cell flood must trip the MAX_CELLS scan bound: {}",
                &result.content[..result.content.len().min(400)]
            );
        }

        #[tokio::test]
        async fn xxe_external_entity_is_not_expanded() {
            let doc = r#"<?xml version="1.0"?>
                <!DOCTYPE w:document [ <!ENTITY xxe SYSTEM "file:///etc/passwd"> ]>
                <w:document xmlns:w="x"><w:body>
                <w:p><w:r><w:t>safe &xxe; text</w:t></w:r></w:p></w:body></w:document>"#;
            let bytes = zip_bytes(&[("word/document.xml", doc)]);
            let (_d, path) = write_tmp("evil.docx", &bytes);
            let result = DocExtractTool::new()
                .execute(json!({ "path": path.to_str().unwrap() }))
                .await;
            assert!(
                !result.content.contains("root:"),
                "XXE leaked passwd: {}",
                result.content
            );
            assert!(
                !result.content.contains("/bin/"),
                "XXE leaked shell path: {}",
                result.content
            );
        }

        #[tokio::test]
        async fn zip_bomb_entry_count_is_rejected() {
            let mut parts: Vec<(String, String)> = vec![(
                "word/document.xml".to_string(),
                "<w:document xmlns:w=\"x\"><w:body/></w:document>".to_string(),
            )];
            for i in 0..(MAX_ENTRY_COUNT + 5) {
                parts.push((format!("junk/{i}.bin"), String::new()));
            }
            let refs: Vec<(&str, &str)> = parts
                .iter()
                .map(|(a, b)| (a.as_str(), b.as_str()))
                .collect();
            let bytes = zip_bytes(&refs);
            let (_d, path) = write_tmp("bomb.docx", &bytes);
            let result = DocExtractTool::new()
                .execute(json!({ "path": path.to_str().unwrap() }))
                .await;
            assert!(result.is_error, "entry-count bomb must be rejected");
            assert!(
                result.content.contains("too many entries"),
                "got: {}",
                result.content
            );
        }

        #[tokio::test]
        async fn non_document_binary_fails_cleanly() {
            let (_d, path) = write_tmp("blob.bin", &[0x00, 0xFF, 0x00, 0xFE, 0x12, 0x34]);
            let result = DocExtractTool::new()
                .execute(json!({ "path": path.to_str().unwrap() }))
                .await;
            assert!(result.is_error);
            assert!(result.content.contains("Failed to extract"));
        }

        #[tokio::test]
        async fn sheet_index_zero_is_rejected() {
            let bytes = make_xlsx(&[("A1", "x")], "A1:A1");
            let (_d, path) = write_tmp("book.xlsx", &bytes);
            let result = DocExtractTool::new()
                .execute(json!({ "path": path.to_str().unwrap(), "sheet": 0 }))
                .await;
            assert!(result.is_error, "1-based contract: sheet 0 must error");
            assert!(
                result.content.contains("out of range"),
                "got: {}",
                result.content
            );
        }
    }

    /// Feature-off: the tool still registers and fails loudly.
    #[cfg(not(feature = "doc-extract"))]
    #[tokio::test]
    async fn degrades_gracefully_without_feature() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("doc.docx");
        std::fs::write(&path, b"PK\x03\x04").unwrap();
        let tool = DocExtractTool::new();
        let result = tool
            .execute(json!({ "path": path.to_str().unwrap() }))
            .await;
        assert!(result.is_error);
        assert!(result.content.contains("without the `doc-extract` feature"));
    }
}
