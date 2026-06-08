//! T9 (v0.6.3 Tier 2B) — Markdown table format / lint tool.
//!
//! A pure-string tool: it scans Markdown text for GitHub-flavored tables
//! and either
//!
//! * **`format`** — reformats every well-formed table so columns are
//!   padded to a uniform width, the separator row is normalized, and pipe
//!   spacing is consistent; non-table text passes through unchanged.
//! * **`lint`** — reports malformed tables (a missing separator row, or a
//!   body/header row whose column count differs from the header) as a
//!   list of defects with 1-based line numbers.
//!
//! No I/O, no network, no sandbox — every code path is a deterministic
//! pure function over `&str`, which makes the logic trivially testable.
//!
//! ## What counts as a table
//!
//! A table is a header row, a separator row, and zero or more body rows,
//! all of which are non-blank lines containing at least one unescaped
//! `|`. The separator row's cells must each match `:?-+:?` (an optional
//! leading/trailing colon for alignment around one or more dashes). The
//! scanner is line-based: a table block ends at the first blank line or
//! the first line with no `|`.

use async_trait::async_trait;
use serde_json::{Value, json};

use wcore_protocol::events::ToolCategory;
use wcore_types::tool::{JsonSchema, ToolResult};

use crate::Tool;

/// Column alignment parsed from a separator-row cell.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Align {
    /// No colons — left-ish default, rendered with plain dashes.
    None,
    /// `:---` — left aligned.
    Left,
    /// `---:` — right aligned.
    Right,
    /// `:---:` — center aligned.
    Center,
}

/// A single defect found by [`lint_tables`].
#[derive(Debug, Clone, PartialEq, Eq)]
struct Defect {
    /// 1-based line number the defect points at.
    line: usize,
    /// Human-readable description.
    message: String,
}

/// Split one Markdown table row into its cell strings.
///
/// Honors backslash-escaped pipes (`\|`) inside a cell, and strips the
/// optional leading / trailing border pipes so `| a | b |` and `a | b`
/// both yield `["a", "b"]`. Cell text is trimmed.
fn split_row(line: &str) -> Vec<String> {
    let trimmed = line.trim();
    let mut cells: Vec<String> = Vec::new();
    let mut cur = String::new();
    let mut escaped = false;
    for ch in trimmed.chars() {
        if escaped {
            // Preserve the escape so round-tripping keeps `\|` literal.
            cur.push('\\');
            cur.push(ch);
            escaped = false;
        } else if ch == '\\' {
            escaped = true;
        } else if ch == '|' {
            cells.push(cur.trim().to_string());
            cur = String::new();
        } else {
            cur.push(ch);
        }
    }
    if escaped {
        cur.push('\\');
    }
    cells.push(cur.trim().to_string());
    // Drop the empty cells produced by leading / trailing border pipes.
    if cells.first().is_some_and(|c| c.is_empty()) {
        cells.remove(0);
    }
    if cells.last().is_some_and(|c| c.is_empty()) {
        cells.pop();
    }
    cells
}

/// True if `line` looks like a table row at all (non-blank, has a `|`).
fn is_table_line(line: &str) -> bool {
    let t = line.trim();
    !t.is_empty() && t.contains('|')
}

/// Parse a separator-row cell into an [`Align`], or `None` if the cell is
/// not a valid separator (`:?-+:?` with at least one dash).
fn parse_separator_cell(cell: &str) -> Option<Align> {
    let c = cell.trim();
    if c.is_empty() {
        return None;
    }
    let left = c.starts_with(':');
    let right = c.ends_with(':');
    let inner = &c[usize::from(left)..c.len() - usize::from(right && c.len() > 1)];
    if inner.is_empty() || !inner.chars().all(|ch| ch == '-') {
        return None;
    }
    Some(match (left, right) {
        (true, true) => Align::Center,
        (true, false) => Align::Left,
        (false, true) => Align::Right,
        (false, false) => Align::None,
    })
}

/// True if every cell of `cells` is a valid separator cell.
fn is_separator_row(cells: &[String]) -> bool {
    !cells.is_empty() && cells.iter().all(|c| parse_separator_cell(c).is_some())
}

/// Display width of a cell — char count, since this tool does not depend
/// on a Unicode-width crate (no new deps). Adequate for column padding.
fn cell_width(s: &str) -> usize {
    s.chars().count()
}

/// Minimum cell width a separator needs: a canonical separator always
/// carries at least 3 dashes, plus one column for each alignment colon.
fn separator_min_width(align: Align) -> usize {
    match align {
        Align::None => 3,
        Align::Left | Align::Right => 4, // ":" + 3 dashes
        Align::Center => 5,              // ":" + 3 dashes + ":"
    }
}

/// Render a separator cell occupying exactly `width` columns. The colons
/// (if any) count toward `width`; the remaining columns are dashes, with
/// a 3-dash minimum so output stays canonical. `width` is expected to be
/// `>= separator_min_width(align)` (the caller guarantees this).
fn render_separator(width: usize, align: Align) -> String {
    let width = width.max(separator_min_width(align));
    match align {
        Align::None => "-".repeat(width),
        Align::Left => format!(":{}", "-".repeat(width - 1)),
        Align::Right => format!("{}:", "-".repeat(width - 1)),
        Align::Center => format!(":{}:", "-".repeat(width - 2)),
    }
}

/// Pad `text` to `width` columns according to `align`.
fn pad_cell(text: &str, width: usize, align: Align) -> String {
    let w = cell_width(text);
    if w >= width {
        return text.to_string();
    }
    let pad = width - w;
    match align {
        Align::Right => format!("{}{}", " ".repeat(pad), text),
        Align::Center => {
            let left = pad / 2;
            let right = pad - left;
            format!("{}{}{}", " ".repeat(left), text, " ".repeat(right))
        }
        Align::None | Align::Left => format!("{}{}", text, " ".repeat(pad)),
    }
}

/// A contiguous run of table lines discovered in the source.
struct TableBlock {
    /// 0-based index of the first line of the block.
    start: usize,
    /// The raw source lines that make up the block.
    lines: Vec<String>,
}

/// Scan `text` for table blocks. A block is a maximal run of consecutive
/// table lines (non-blank, containing `|`). The header / separator
/// structure is validated later by the caller.
fn scan_blocks(text: &str) -> Vec<TableBlock> {
    let mut blocks: Vec<TableBlock> = Vec::new();
    let mut cur: Vec<String> = Vec::new();
    let mut cur_start = 0usize;
    for (idx, line) in text.split('\n').enumerate() {
        if is_table_line(line) {
            if cur.is_empty() {
                cur_start = idx;
            }
            cur.push(line.to_string());
        } else if !cur.is_empty() {
            blocks.push(TableBlock {
                start: cur_start,
                lines: std::mem::take(&mut cur),
            });
        }
    }
    if !cur.is_empty() {
        blocks.push(TableBlock {
            start: cur_start,
            lines: cur,
        });
    }
    blocks
}

/// Reformat every well-formed table in `text`, leaving everything else
/// (including malformed tables) byte-for-byte intact.
fn format_tables(text: &str) -> String {
    let mut out_lines: Vec<String> = text.split('\n').map(str::to_string).collect();
    for block in scan_blocks(text) {
        // A formattable table needs >= 2 lines and a separator at row 1.
        if block.lines.len() < 2 {
            continue;
        }
        let header = split_row(&block.lines[0]);
        let sep_cells = split_row(&block.lines[1]);
        if header.is_empty() || !is_separator_row(&sep_cells) {
            continue;
        }
        let cols = header.len();
        // A ragged table (any row's column count differs) is left for the
        // linter — `format` only touches tables it can render losslessly.
        let rows: Vec<Vec<String>> = block.lines[2..].iter().map(|l| split_row(l)).collect();
        if sep_cells.len() != cols || rows.iter().any(|r| r.len() != cols) {
            continue;
        }

        let aligns: Vec<Align> = sep_cells
            .iter()
            .map(|c| parse_separator_cell(c).unwrap_or(Align::None))
            .collect();

        // Column width = widest of: the header cell, every body cell, and
        // the column's own separator minimum (so colons never overflow).
        let mut widths: Vec<usize> = aligns.iter().map(|a| separator_min_width(*a)).collect();
        for (i, h) in header.iter().enumerate() {
            widths[i] = widths[i].max(cell_width(h));
        }
        for row in &rows {
            for (i, cell) in row.iter().enumerate() {
                widths[i] = widths[i].max(cell_width(cell));
            }
        }

        let render = |cells: &[String]| -> String {
            let inner: Vec<String> = cells
                .iter()
                .enumerate()
                .map(|(i, c)| pad_cell(c, widths[i], aligns[i]))
                .collect();
            format!("| {} |", inner.join(" | "))
        };
        let sep_line: Vec<String> = (0..cols)
            .map(|i| render_separator(widths[i], aligns[i]))
            .collect();

        let mut rebuilt: Vec<String> = Vec::with_capacity(block.lines.len());
        rebuilt.push(render(&header));
        rebuilt.push(format!("| {} |", sep_line.join(" | ")));
        for row in &rows {
            rebuilt.push(render(row));
        }
        for (offset, new_line) in rebuilt.into_iter().enumerate() {
            out_lines[block.start + offset] = new_line;
        }
    }
    out_lines.join("\n")
}

/// Lint every table block in `text`, returning a sorted list of defects.
fn lint_tables(text: &str) -> Vec<Defect> {
    let mut defects: Vec<Defect> = Vec::new();
    for block in scan_blocks(text) {
        let header = split_row(&block.lines[0]);
        // Single-line "table" — header with no separator row at all.
        if block.lines.len() < 2 {
            defects.push(Defect {
                line: block.start + 1,
                message: "table is missing its separator row (--- line)".to_string(),
            });
            continue;
        }
        let sep_cells = split_row(&block.lines[1]);
        if !is_separator_row(&sep_cells) {
            defects.push(Defect {
                line: block.start + 2,
                message: "expected a separator row (e.g. `| --- | --- |`) after the header"
                    .to_string(),
            });
            // Without a valid separator, column-count checks are noise.
            continue;
        }
        let cols = header.len();
        if sep_cells.len() != cols {
            defects.push(Defect {
                line: block.start + 2,
                message: format!(
                    "separator row has {} columns but the header has {cols}",
                    sep_cells.len()
                ),
            });
        }
        for (i, raw) in block.lines[2..].iter().enumerate() {
            let row = split_row(raw);
            if row.len() != cols {
                defects.push(Defect {
                    line: block.start + 3 + i,
                    message: format!("row has {} columns but the header has {cols}", row.len()),
                });
            }
        }
    }
    defects.sort_by_key(|d| d.line);
    defects
}

/// `markdown_table` — format or lint Markdown tables in a string.
///
/// Zero-state tool: holds no fields. Pure-string, concurrency-safe.
#[derive(Default)]
pub struct MarkdownTableTool;

impl MarkdownTableTool {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Tool for MarkdownTableTool {
    fn name(&self) -> &str {
        "markdown_table"
    }

    fn description(&self) -> &str {
        "Format or lint Markdown (GitHub-flavored) tables in a block of text. \
Two operations:\n\n\
1. format — reformats every well-formed table so columns are padded to a \
uniform width and the separator row is normalized. Non-table text, and any \
malformed table, passes through unchanged.\n\
2. lint — reports malformed tables (missing separator row, or a row whose \
column count differs from the header) as a list of defects with 1-based \
line numbers.\n\n\
Pure string processing — no files are read or written."
    }

    fn input_schema(&self) -> JsonSchema {
        json!({
            "type": "object",
            "properties": {
                "operation": {
                    "type": "string",
                    "enum": ["format", "lint"],
                    "description": "`format` returns the reformatted Markdown; \
        `lint` returns a list of table defects with line numbers."
                },
                "text": {
                    "type": "string",
                    "description": "The Markdown text to format or lint."
                }
            },
            "required": ["operation", "text"]
        })
    }

    fn is_concurrency_safe(&self, _input: &Value) -> bool {
        // Pure string transform: no shared state, no filesystem, no network.
        true
    }

    async fn execute(&self, input: Value) -> ToolResult {
        let operation = match input.get("operation").and_then(|v| v.as_str()) {
            Some(s) => s,
            None => {
                return ToolResult {
                    content: "`operation` is required (`format` or `lint`).".to_string(),
                    is_error: true,
                };
            }
        };
        let text = match input.get("text").and_then(|v| v.as_str()) {
            Some(s) => s,
            None => {
                return ToolResult {
                    content: "`text` is required.".to_string(),
                    is_error: true,
                };
            }
        };

        match operation {
            "format" => ToolResult {
                content: format_tables(text),
                is_error: false,
            },
            "lint" => {
                let defects = lint_tables(text);
                let payload = json!({
                    "defect_count": defects.len(),
                    "defects": defects
                        .iter()
                        .map(|d| json!({ "line": d.line, "message": d.message }))
                        .collect::<Vec<_>>(),
                });
                ToolResult {
                    content: payload.to_string(),
                    is_error: false,
                }
            }
            other => ToolResult {
                content: format!("unknown operation `{other}` (expected `format` or `lint`)."),
                is_error: true,
            },
        }
    }

    fn category(&self) -> ToolCategory {
        // Pure text transform — mutates nothing, executes nothing.
        ToolCategory::Info
    }

    fn describe(&self, input: &Value) -> String {
        let op = input
            .get("operation")
            .and_then(|v| v.as_str())
            .unwrap_or("(missing operation)");
        format!("markdown_table: {op}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format_aligns_a_ragged_table() {
        // A valid table whose pipes are unevenly spaced gets padded so every
        // body cell lines up under its header.
        let src = "| Name | Age |\n| --- | --- |\n| Alice | 30 |\n| Bob | 5 |";
        let out = format_tables(src);
        let lines: Vec<&str> = out.split('\n').collect();
        assert_eq!(lines[0], "| Name  | Age |");
        assert_eq!(lines[1], "| ----- | --- |");
        assert_eq!(lines[2], "| Alice | 30  |");
        assert_eq!(lines[3], "| Bob   | 5   |");
        // Every rendered line has identical width (columns aligned).
        let w = lines[0].chars().count();
        assert!(lines.iter().all(|l| l.chars().count() == w));
    }

    #[test]
    fn format_normalizes_the_separator_row() {
        // Tight separator with no spaces + alignment colons → canonical form
        // with >= 3 dashes per column and alignment preserved.
        let src = "|A|B|C|\n|:-|:-:|-:|\n|x|y|z|";
        let out = format_tables(src);
        let lines: Vec<&str> = out.split('\n').collect();
        // Left / center / right alignment markers survive normalization.
        assert_eq!(lines[1], "| :--- | :---: | ---: |");
    }

    #[test]
    fn lint_flags_a_row_with_wrong_column_count() {
        // Header has 2 columns; the second body row has 3.
        let src = "| A | B |\n| --- | --- |\n| 1 | 2 |\n| 3 | 4 | 5 |";
        let defects = lint_tables(src);
        assert_eq!(defects.len(), 1, "got: {defects:?}");
        assert_eq!(defects[0].line, 4);
        assert!(
            defects[0].message.contains("3 columns"),
            "msg: {}",
            defects[0].message
        );
    }

    #[test]
    fn lint_flags_a_missing_separator() {
        // Header row followed by body rows but no `--- ` separator line.
        let src = "| A | B |\n| 1 | 2 |\n| 3 | 4 |";
        let defects = lint_tables(src);
        assert_eq!(defects.len(), 1, "got: {defects:?}");
        assert_eq!(defects[0].line, 2);
        assert!(defects[0].message.contains("separator"));
    }

    #[test]
    fn non_table_markdown_passes_through_unchanged() {
        let src =
            "# Title\n\nA paragraph with a | stray pipe? No, blank line breaks it.\n\nMore prose.";
        // Format must not touch prose (no header+separator pair anywhere).
        assert_eq!(format_tables(src), src);
        // And lint must not flag a line that is not part of a real table.
        // The one line with a `|` is isolated (single-line block) — that is
        // reported as a missing-separator defect, which is correct; verify
        // a genuinely table-free document yields zero defects instead.
        let prose = "# Title\n\nJust words here.\n\nAnd more words.";
        assert_eq!(format_tables(prose), prose);
        assert!(lint_tables(prose).is_empty());
    }

    #[test]
    fn well_formed_table_has_no_lint_defects() {
        let src = "| A | B |\n| --- | --- |\n| 1 | 2 |";
        assert!(lint_tables(src).is_empty());
    }

    #[test]
    fn format_leaves_a_ragged_table_untouched() {
        // `format` only reformats tables it can render losslessly; a ragged
        // table is left for `lint` to report.
        let src = "| A | B |\n| --- | --- |\n| 1 | 2 | 3 |";
        assert_eq!(format_tables(src), src);
    }

    #[test]
    fn split_row_honors_escaped_pipes() {
        // `\|` inside a cell must not split it.
        let cells = split_row(r"| a \| b | c |");
        assert_eq!(cells, vec![r"a \| b".to_string(), "c".to_string()]);
    }

    #[tokio::test]
    async fn execute_format_returns_reformatted_markdown() {
        let tool = MarkdownTableTool::new();
        let input = json!({
            "operation": "format",
            "text": "| A | B |\n| --- | --- |\n| longvalue | x |"
        });
        let res = tool.execute(input).await;
        assert!(!res.is_error, "{}", res.content);
        // Column 0 widens to fit "longvalue" (9 chars); column 1 holds its
        // canonical 3-dash-separator minimum width of 3.
        assert!(
            res.content.contains("| A         | B   |"),
            "got: {}",
            res.content
        );
    }

    #[tokio::test]
    async fn execute_lint_returns_defect_payload() {
        let tool = MarkdownTableTool::new();
        let input = json!({
            "operation": "lint",
            "text": "| A | B |\n| 1 | 2 |"
        });
        let res = tool.execute(input).await;
        assert!(!res.is_error);
        let parsed: Value = serde_json::from_str(&res.content).unwrap();
        assert_eq!(parsed["defect_count"], json!(1));
        // The defect points at line 2 — where a separator row was expected.
        assert_eq!(parsed["defects"][0]["line"], json!(2));
    }

    #[tokio::test]
    async fn execute_rejects_missing_fields() {
        let tool = MarkdownTableTool::new();
        let res = tool.execute(json!({ "operation": "format" })).await;
        assert!(res.is_error);
        assert!(res.content.contains("`text`"));

        let res = tool.execute(json!({ "text": "x" })).await;
        assert!(res.is_error);
        assert!(res.content.contains("`operation`"));
    }

    #[tokio::test]
    async fn execute_rejects_unknown_operation() {
        let tool = MarkdownTableTool::new();
        let res = tool
            .execute(json!({ "operation": "wat", "text": "x" }))
            .await;
        assert!(res.is_error);
        assert!(res.content.contains("unknown operation"));
    }
}
