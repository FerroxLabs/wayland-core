//! Token-opt (diff-resend): line-level diff between the content the model last
//! read and the current file, used to answer a re-read with just the changed
//! lines instead of the full file.
//!
//! Correctness is independent of diff *quality*: every diff is verified to
//! reconstruct the current content byte-for-byte before it is emitted (see
//! [`build_read_diff`]). A poor diff just fails the size gate and falls back to
//! full content — it can never produce a wrong reconstruction.
//!
//! The gating that makes a diff *safe to send* (route enabled, single-agent,
//! base still visible in the transcript, full read only, ReadResult base) lives
//! in the Read tool; this module is pure text math with no I/O.

/// A single line-level edit operation in a base→current diff.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Op {
    /// A line present in both base and current (unchanged).
    Keep(String),
    /// A line present only in current (added).
    Insert(String),
    /// A line present only in base (removed).
    Delete(String),
}

/// Strip the `"{:>6}\t"` line-number prefix that Read/Write emit, recovering the
/// raw file lines. Splitting on the FIRST tab is robust to line numbers wider
/// than 6 digits and to raw content that itself contains tabs.
pub fn strip_line_numbers(numbered: &str) -> Vec<String> {
    if numbered.is_empty() {
        return Vec::new();
    }
    numbered
        .split('\n')
        .map(|line| match line.split_once('\t') {
            Some((_num, rest)) => rest.to_string(),
            // A line with no tab isn't in our numbered format; keep it verbatim
            // so reconstruction stays faithful rather than silently dropping it.
            None => line.to_string(),
        })
        .collect()
}

/// Longest-common-subsequence line diff. Returns the ordered edit script that
/// turns `base` into `cur`. O(n*m) time/space — bounded by the 100 KB Read
/// result cap. Runs on every route (diff-resend is no longer route-gated; see
/// #182), subject to the caller's soundness guards in `read.rs`.
fn diff_ops(base: &[String], cur: &[String]) -> Vec<Op> {
    let n = base.len();
    let m = cur.len();

    // lcs[i][j] = LCS length of base[i..] and cur[j..].
    let mut lcs = vec![vec![0usize; m + 1]; n + 1];
    for i in (0..n).rev() {
        for j in (0..m).rev() {
            lcs[i][j] = if base[i] == cur[j] {
                lcs[i + 1][j + 1] + 1
            } else {
                lcs[i + 1][j].max(lcs[i][j + 1])
            };
        }
    }

    let mut ops = Vec::new();
    let (mut i, mut j) = (0, 0);
    while i < n && j < m {
        if base[i] == cur[j] {
            ops.push(Op::Keep(base[i].clone()));
            i += 1;
            j += 1;
        } else if lcs[i + 1][j] >= lcs[i][j + 1] {
            ops.push(Op::Delete(base[i].clone()));
            i += 1;
        } else {
            ops.push(Op::Insert(cur[j].clone()));
            j += 1;
        }
    }
    while i < n {
        ops.push(Op::Delete(base[i].clone()));
        i += 1;
    }
    while j < m {
        ops.push(Op::Insert(cur[j].clone()));
        j += 1;
    }
    ops
}

/// Reconstruct the current content from `base` + `ops`. Used as the byte-exact
/// verification gate before any diff is emitted.
fn reconstruct(ops: &[Op]) -> Vec<String> {
    ops.iter()
        .filter_map(|op| match op {
            Op::Keep(l) | Op::Insert(l) => Some(l.clone()),
            Op::Delete(_) => None,
        })
        .collect()
}

/// Render the edit script as a compact, line-anchored hunk view the model can
/// read. Unchanged runs longer than `CONTEXT * 2` are elided to a `...` marker;
/// changed lines are prefixed `-`/`+` with the current 1-based line number.
fn render(ops: &[Op]) -> String {
    const CONTEXT: usize = 2;

    // Walk ops, tracking the current-file line number (advances on Keep/Insert).
    // First classify each op with its current line number (0 for deletes).
    let mut rows: Vec<(char, usize, &str)> = Vec::new();
    let mut cur_line = 0usize;
    for op in ops {
        match op {
            Op::Keep(l) => {
                cur_line += 1;
                rows.push((' ', cur_line, l));
            }
            Op::Insert(l) => {
                cur_line += 1;
                rows.push(('+', cur_line, l));
            }
            Op::Delete(l) => {
                rows.push(('-', 0, l));
            }
        }
    }

    // Keep only context around changes; collapse long unchanged runs.
    let changed: Vec<bool> = rows.iter().map(|(tag, _, _)| *tag != ' ').collect();
    let mut keep = vec![false; rows.len()];
    for (idx, is_changed) in changed.iter().enumerate() {
        if *is_changed {
            let lo = idx.saturating_sub(CONTEXT);
            let hi = (idx + CONTEXT + 1).min(rows.len());
            for k in keep.iter_mut().take(hi).skip(lo) {
                *k = true;
            }
        }
    }

    let mut out = String::new();
    let mut elided = false;
    for (idx, (tag, line_no, text)) in rows.iter().enumerate() {
        if keep[idx] {
            elided = false;
            if *line_no > 0 {
                out.push_str(&format!("{tag}{line_no:>6}\t{text}\n"));
            } else {
                // Deleted line: no current line number.
                out.push_str(&format!("{tag}      \t{text}\n"));
            }
        } else if !elided {
            out.push_str("       …\n");
            elided = true;
        }
    }
    out
}

/// Build a diff result body for a re-read, or `None` to fall back to full
/// content.
///
/// Returns `Some(rendered_diff)` only when:
///  1. the diff reconstructs `cur` byte-for-byte (correctness gate), AND
///  2. the rendered diff is smaller than `max_ratio` of the full numbered
///     current content (it must actually save tokens).
///
/// `base_raw` / `cur_raw` are the RAW (un-numbered) line vectors;
/// `cur_numbered_len` is the byte length of the full numbered content the diff
/// would replace.
pub fn build_read_diff(
    base_raw: &[String],
    cur_raw: &[String],
    cur_numbered_len: usize,
    max_ratio: f64,
) -> Option<String> {
    let ops = diff_ops(base_raw, cur_raw);

    // Correctness gate: the ops MUST rebuild the current content exactly.
    if reconstruct(&ops) != cur_raw {
        return None;
    }

    let body = render(&ops);

    // Size gate: only worth it if meaningfully smaller than the full content.
    if cur_numbered_len == 0 || (body.len() as f64) >= max_ratio * (cur_numbered_len as f64) {
        return None;
    }

    Some(body)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lines(s: &[&str]) -> Vec<String> {
        s.iter().map(|x| x.to_string()).collect()
    }

    #[test]
    fn strip_recovers_raw_lines() {
        let numbered = "     1\tfn main() {\n     2\t    let x = 1;\n     3\t}";
        assert_eq!(
            strip_line_numbers(numbered),
            lines(&["fn main() {", "    let x = 1;", "}"])
        );
    }

    #[test]
    fn strip_handles_tabs_in_content() {
        // Raw line itself contains a tab — split_once('\t') must only eat the
        // number prefix, keeping the embedded tab.
        let numbered = "     1\tcol1\tcol2";
        assert_eq!(strip_line_numbers(numbered), lines(&["col1\tcol2"]));
    }

    #[test]
    fn strip_handles_wide_line_numbers() {
        let numbered = "1000000\tdeep line";
        assert_eq!(strip_line_numbers(numbered), lines(&["deep line"]));
    }

    #[test]
    fn strip_empty_is_empty() {
        assert!(strip_line_numbers("").is_empty());
    }

    #[test]
    fn diff_reconstructs_exactly_on_single_line_change() {
        let base = lines(&["a", "b", "c", "d", "e"]);
        let cur = lines(&["a", "b", "CHANGED", "d", "e"]);
        let ops = diff_ops(&base, &cur);
        assert_eq!(reconstruct(&ops), cur);
    }

    #[test]
    fn diff_reconstructs_on_insert_and_delete() {
        let base = lines(&["keep1", "remove", "keep2"]);
        let cur = lines(&["keep1", "keep2", "added"]);
        let ops = diff_ops(&base, &cur);
        assert_eq!(reconstruct(&ops), cur);
    }

    #[test]
    fn small_change_in_large_file_yields_diff() {
        let base: Vec<String> = (0..200).map(|i| format!("line {i}")).collect();
        let cur = {
            let mut c = base.clone();
            c[100] = "line 100 CHANGED".to_string();
            c
        };
        // Full numbered content is large; a one-line change must diff.
        let numbered_len = cur
            .iter()
            .enumerate()
            .map(|(i, l)| format!("{:>6}\t{}\n", i + 1, l).len())
            .sum();
        let diff =
            build_read_diff(&base, &cur, numbered_len, 0.6).expect("one-line change should diff");
        // The diff shows the changed line with its current line number and elides
        // the rest.
        assert!(diff.contains("CHANGED"));
        assert!(diff.contains('…'));
        assert!(diff.len() < numbered_len);
    }

    #[test]
    fn wholesale_rewrite_falls_back_to_full() {
        let base = lines(&["a", "b", "c"]);
        let cur = lines(&["x", "y", "z", "w", "v"]);
        let numbered_len: usize = cur
            .iter()
            .enumerate()
            .map(|(i, l)| format!("{:>6}\t{}\n", i + 1, l).len())
            .sum();
        // Almost everything changed — diff isn't smaller, so fall back.
        assert!(build_read_diff(&base, &cur, numbered_len, 0.6).is_none());
    }

    #[test]
    fn identical_content_is_not_a_diff() {
        // build_read_diff is only called when base != cur, but guard anyway:
        // an all-Keep diff renders empty and is below ratio, so it returns Some
        // empty — callers must check inequality first. Here we assert the
        // reconstruct gate holds for identical input.
        let base = lines(&["a", "b"]);
        let ops = diff_ops(&base, &base);
        assert_eq!(reconstruct(&ops), base);
        assert!(ops.iter().all(|o| matches!(o, Op::Keep(_))));
    }

    #[test]
    fn empty_base_inserts_everything() {
        let base: Vec<String> = Vec::new();
        let cur = lines(&["new1", "new2"]);
        let ops = diff_ops(&base, &cur);
        assert_eq!(reconstruct(&ops), cur);
        assert!(ops.iter().all(|o| matches!(o, Op::Insert(_))));
    }
}
