//! V4A Patch Format Parser (HELPER module).
//!
//! Ported from the prior Wayland Python engine (sub-wave 2 of
//! T3-3). This module only parses the V4A patch format used by codex / cline
//! and similar coding agents — application of the parsed operations lives
//! elsewhere (the higher-level edit tooling).
//!
//! V4A Format:
//!
//! ```text
//! *** Begin Patch
//! *** Update File: path/to/file.py
//! @@ optional context hint @@
//!  context line (space prefix)
//! -removed line (minus prefix)
//! +added line (plus prefix)
//! *** Add File: path/to/new.py
//! +new file content
//! +line 2
//! *** Delete File: path/to/old.py
//! *** Move File: old/path.py -> new/path.py
//! *** End Patch
//! ```
//!
//! `parse_v4a_patch` returns the parsed operations or a human-readable parse
//! error. The shape mirrors the Python source so downstream tooling can be
//! ported incrementally.

use std::fmt;

/// What kind of operation a [`PatchOperation`] represents.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OperationType {
    Add,
    Update,
    Delete,
    Move,
}

impl fmt::Display for OperationType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            OperationType::Add => "add",
            OperationType::Update => "update",
            OperationType::Delete => "delete",
            OperationType::Move => "move",
        })
    }
}

/// A single line inside a hunk.
///
/// `prefix` is one of `' '` (context), `'+'` (added), or `'-'` (removed).
/// `content` is the line body with the prefix stripped.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HunkLine {
    pub prefix: char,
    pub content: String,
}

impl HunkLine {
    pub fn new(prefix: char, content: impl Into<String>) -> Self {
        Self {
            prefix,
            content: content.into(),
        }
    }
}

/// A group of contiguous changes inside a file.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Hunk {
    /// Optional `@@ ... @@` context hint that introduced this hunk.
    pub context_hint: Option<String>,
    pub lines: Vec<HunkLine>,
}

/// A single operation in a V4A patch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PatchOperation {
    pub operation: OperationType,
    pub file_path: String,
    /// Destination path for `Move` operations.
    pub new_path: Option<String>,
    pub hunks: Vec<Hunk>,
}

impl PatchOperation {
    fn new(operation: OperationType, file_path: impl Into<String>) -> Self {
        Self {
            operation,
            file_path: file_path.into(),
            new_path: None,
            hunks: Vec::new(),
        }
    }
}

/// Strip a `*** Update File: <path>` style directive, returning the trimmed
/// payload after the colon if `line` starts with the given keyword.
///
/// # Accepted forms
///
/// Both of the following are accepted:
/// * `*** Update File: src/foo.rs` — space after `***`
/// * `***Update File: src/foo.rs` — no space after `***`
///
/// Whitespace between the keyword and `File:` is REQUIRED, so the form
/// `***UpdateFile:` is REJECTED (no whitespace separator). Conversely
/// `*** Update File:` and `***Update File:` both parse because the
/// initial `***`/`Update` boundary is tolerated either way.
fn match_directive<'a>(line: &'a str, keyword: &str) -> Option<&'a str> {
    // Accept `*** Update File:` or `***Update File:` (with/without space).
    let rest = line.strip_prefix("***")?.trim_start();
    let after_kw = rest.strip_prefix(keyword)?;
    // Require whitespace between keyword and `File:` for safety
    // (the original Python regex uses `\s+`).
    let after_ws = after_kw.strip_prefix(|c: char| c.is_whitespace())?;
    let after_file = after_ws.trim_start().strip_prefix("File:")?;
    Some(after_file.trim())
}

/// Parse `@@ optional hint @@` into the captured hint string, if any.
fn parse_hunk_marker(line: &str) -> Option<String> {
    let rest = line.strip_prefix("@@")?;
    let trimmed = rest.trim();
    // Trailing `@@` is optional in the Python implementation — the regex
    // requires it, but the outer branch falls back to `None` when missing.
    if let Some(inner) = trimmed.strip_suffix("@@") {
        let hint = inner.trim();
        if hint.is_empty() {
            None
        } else {
            Some(hint.to_string())
        }
    } else {
        None
    }
}

/// Parse a V4A format patch into a list of [`PatchOperation`]s.
///
/// Returns `Ok(ops)` (possibly empty) on success or `Err(message)` with a
/// human-readable description of the first validation failure.
pub fn parse_v4a_patch(patch_content: &str) -> Result<Vec<PatchOperation>, String> {
    let lines: Vec<&str> = patch_content.split('\n').collect();
    let mut operations: Vec<PatchOperation> = Vec::new();

    // Locate `*** Begin Patch` / `*** End Patch`. Missing markers are
    // tolerated (matches Python behavior).
    let mut start_idx: Option<usize> = None;
    let mut end_idx: Option<usize> = None;
    for (i, line) in lines.iter().enumerate() {
        if line.contains("*** Begin Patch") || line.contains("***Begin Patch") {
            start_idx = Some(i);
        } else if line.contains("*** End Patch") || line.contains("***End Patch") {
            end_idx = Some(i);
            break;
        }
    }

    // Python uses `start_idx = -1` to mean "start from line 0"; we model
    // that with a signed scan index.
    let mut i: i64 = match start_idx {
        Some(s) => s as i64 + 1,
        None => 0,
    };
    let end: i64 = end_idx.map(|e| e as i64).unwrap_or(lines.len() as i64);

    let mut current_op: Option<PatchOperation> = None;
    let mut current_hunk: Option<Hunk> = None;

    // Helper closure to flush a pending hunk into the current op.
    fn flush_hunk(op: &mut PatchOperation, hunk: &mut Option<Hunk>) {
        if let Some(h) = hunk.take()
            && !h.lines.is_empty()
        {
            op.hunks.push(h);
        }
    }

    while i < end {
        let line = lines[i as usize];

        if let Some(path) = match_directive(line, "Update") {
            if let Some(mut op) = current_op.take() {
                flush_hunk(&mut op, &mut current_hunk);
                operations.push(op);
            }
            current_op = Some(PatchOperation::new(OperationType::Update, path));
            current_hunk = None;
        } else if let Some(path) = match_directive(line, "Add") {
            if let Some(mut op) = current_op.take() {
                flush_hunk(&mut op, &mut current_hunk);
                operations.push(op);
            }
            current_op = Some(PatchOperation::new(OperationType::Add, path));
            current_hunk = Some(Hunk::default());
        } else if let Some(path) = match_directive(line, "Delete") {
            if let Some(mut op) = current_op.take() {
                flush_hunk(&mut op, &mut current_hunk);
                operations.push(op);
            }
            operations.push(PatchOperation::new(OperationType::Delete, path));
            current_op = None;
            current_hunk = None;
        } else if let Some(payload) = match_directive(line, "Move") {
            // Move uses `<src> -> <dst>` after the colon.
            if let Some(mut op) = current_op.take() {
                flush_hunk(&mut op, &mut current_hunk);
                operations.push(op);
            }
            let (src, dst) = match payload.split_once("->") {
                Some((s, d)) => (s.trim().to_string(), d.trim().to_string()),
                None => (payload.trim().to_string(), String::new()),
            };
            let mut op = PatchOperation::new(OperationType::Move, src);
            if !dst.is_empty() {
                op.new_path = Some(dst);
            }
            operations.push(op);
            current_op = None;
            current_hunk = None;
        } else if line.starts_with("@@") {
            if let Some(op) = current_op.as_mut() {
                // Flush prior hunk into this op.
                if let Some(h) = current_hunk.take()
                    && !h.lines.is_empty()
                {
                    op.hunks.push(h);
                }
                current_hunk = Some(Hunk {
                    context_hint: parse_hunk_marker(line),
                    lines: Vec::new(),
                });
            }
        } else if current_op.is_some() && !line.is_empty() {
            if current_hunk.is_none() {
                current_hunk = Some(Hunk::default());
            }
            let hunk = current_hunk.as_mut().expect("hunk just initialized");

            let first = line.as_bytes()[0];
            match first {
                b'+' => hunk.lines.push(HunkLine::new('+', &line[1..])),
                b'-' => hunk.lines.push(HunkLine::new('-', &line[1..])),
                b' ' => hunk.lines.push(HunkLine::new(' ', &line[1..])),
                b'\\' => { /* "\\ No newline at end of file" — skip */ }
                _ => {
                    // Implicit-context line — treat as space-prefixed.
                    hunk.lines.push(HunkLine::new(' ', line));
                }
            }
        }

        i += 1;
    }

    // Flush the trailing op.
    if let Some(mut op) = current_op.take() {
        flush_hunk(&mut op, &mut current_hunk);
        operations.push(op);
    }

    // Empty patch is not an error — callers get [] and can decide.
    if operations.is_empty() {
        return Ok(operations);
    }

    let mut parse_errors: Vec<String> = Vec::new();
    for op in &operations {
        if op.file_path.is_empty() {
            parse_errors.push("Operation with empty file path".to_string());
        }
        if op.operation == OperationType::Update && op.hunks.is_empty() {
            parse_errors.push(format!("UPDATE {:?}: no hunks found", op.file_path));
        }
        if op.operation == OperationType::Move && op.new_path.is_none() {
            parse_errors.push(format!(
                "MOVE {:?}: missing destination path (expected 'src -> dst')",
                op.file_path
            ));
        }
    }

    if !parse_errors.is_empty() {
        return Err(format!("Parse error: {}", parse_errors.join("; ")));
    }

    Ok(operations)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn add_file_collects_plus_lines() {
        let patch = "\
*** Begin Patch
*** Add File: hello.txt
+hello
+world
*** End Patch
";
        let ops = parse_v4a_patch(patch).expect("parse ok");
        assert_eq!(ops.len(), 1);
        assert_eq!(ops[0].operation, OperationType::Add);
        assert_eq!(ops[0].file_path, "hello.txt");
        assert_eq!(ops[0].hunks.len(), 1);
        let lines = &ops[0].hunks[0].lines;
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0], HunkLine::new('+', "hello"));
        assert_eq!(lines[1], HunkLine::new('+', "world"));
    }

    #[test]
    fn delete_file_has_no_hunks() {
        let patch = "\
*** Begin Patch
*** Delete File: stale.rs
*** End Patch
";
        let ops = parse_v4a_patch(patch).expect("parse ok");
        assert_eq!(ops.len(), 1);
        assert_eq!(ops[0].operation, OperationType::Delete);
        assert_eq!(ops[0].file_path, "stale.rs");
        assert!(ops[0].hunks.is_empty());
    }

    #[test]
    fn update_with_multi_hunk() {
        let patch = "\
*** Begin Patch
*** Update File: lib.rs
@@ fn one @@
 ctx
-old
+new
@@ fn two @@
 keep
-drop
+add
*** End Patch
";
        let ops = parse_v4a_patch(patch).expect("parse ok");
        assert_eq!(ops.len(), 1);
        let op = &ops[0];
        assert_eq!(op.operation, OperationType::Update);
        assert_eq!(op.hunks.len(), 2);
        assert_eq!(op.hunks[0].context_hint.as_deref(), Some("fn one"));
        assert_eq!(op.hunks[1].context_hint.as_deref(), Some("fn two"));
        assert_eq!(op.hunks[0].lines.len(), 3);
        assert_eq!(op.hunks[1].lines.len(), 3);
        assert_eq!(op.hunks[0].lines[0], HunkLine::new(' ', "ctx"));
        assert_eq!(op.hunks[0].lines[1], HunkLine::new('-', "old"));
        assert_eq!(op.hunks[0].lines[2], HunkLine::new('+', "new"));
    }

    #[test]
    fn malformed_update_without_hunks_errors() {
        // `Update File:` directive with no hunks at all must surface a
        // parse error per the Python validator contract.
        let patch = "\
*** Begin Patch
*** Update File: empty.rs
*** End Patch
";
        let err = parse_v4a_patch(patch).expect_err("should error");
        assert!(err.contains("UPDATE"), "got: {err}");
        assert!(err.contains("no hunks"), "got: {err}");
    }

    #[test]
    fn trailing_newline_does_not_break_parse() {
        // Trailing-newline edge: payload ends with `\n` and then nothing.
        let patch = "*** Begin Patch\n*** Add File: f.txt\n+a\n*** End Patch\n\n";
        let ops = parse_v4a_patch(patch).expect("parse ok");
        assert_eq!(ops.len(), 1);
        assert_eq!(ops[0].operation, OperationType::Add);
        assert_eq!(ops[0].hunks[0].lines, vec![HunkLine::new('+', "a")]);
    }

    #[test]
    fn mixed_context_and_implicit_space() {
        // A bare line with no prefix should be treated as context (space).
        let patch = "\
*** Begin Patch
*** Update File: m.rs
@@ @@
 explicit
bare_context_line
-gone
+kept
*** End Patch
";
        let ops = parse_v4a_patch(patch).expect("parse ok");
        let hunk = &ops[0].hunks[0];
        assert_eq!(hunk.lines.len(), 4);
        assert_eq!(hunk.lines[0], HunkLine::new(' ', "explicit"));
        assert_eq!(hunk.lines[1], HunkLine::new(' ', "bare_context_line"));
        assert_eq!(hunk.lines[2], HunkLine::new('-', "gone"));
        assert_eq!(hunk.lines[3], HunkLine::new('+', "kept"));
        // `@@ @@` with no inner text → no hint.
        assert!(hunk.context_hint.is_none());
    }

    #[test]
    fn move_file_captures_src_and_dst() {
        let patch = "\
*** Begin Patch
*** Move File: old/path.py -> new/path.py
*** End Patch
";
        let ops = parse_v4a_patch(patch).expect("parse ok");
        assert_eq!(ops.len(), 1);
        assert_eq!(ops[0].operation, OperationType::Move);
        assert_eq!(ops[0].file_path, "old/path.py");
        assert_eq!(ops[0].new_path.as_deref(), Some("new/path.py"));
    }

    #[test]
    fn no_newline_marker_is_skipped() {
        let patch = "\
*** Begin Patch
*** Update File: file.txt
@@ @@
 keep
-old
\\ No newline at end of file
+new
*** End Patch
";
        let ops = parse_v4a_patch(patch).expect("parse ok");
        let hunk = &ops[0].hunks[0];
        // The `\\ No newline...` line is dropped entirely; we keep the
        // surrounding three lines.
        assert_eq!(hunk.lines.len(), 3);
        assert_eq!(hunk.lines[0], HunkLine::new(' ', "keep"));
        assert_eq!(hunk.lines[1], HunkLine::new('-', "old"));
        assert_eq!(hunk.lines[2], HunkLine::new('+', "new"));
    }

    #[test]
    fn move_without_destination_errors() {
        // Sanity check on the MOVE validator branch (covers the
        // "missing destination" error path in addition to the basic
        // happy paths above).
        let patch = "\
*** Begin Patch
*** Move File: orphan.py
*** End Patch
";
        let err = parse_v4a_patch(patch).expect_err("should error");
        assert!(err.contains("MOVE"), "got: {err}");
        assert!(err.contains("missing destination"), "got: {err}");
    }
}
