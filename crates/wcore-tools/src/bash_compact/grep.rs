//! Compaction for `grep` / `ripgrep` / `find` output.
//!
//! Search tools emit one result per line, and the volume is dominated by the
//! per-match content (`grep -n` / `rg` produce `path:line:content`) or by bare
//! paths (`find`). Once a search returns dozens of hits, the per-line content
//! stops being useful for orientation: what matters is *which files* matched
//! and *how many* times. We summarize to a match count, a unique-file count,
//! and the first K unique paths, keeping raw match lines only when the result
//! set is small enough to read in full.
//!
//! Returns `None` when the output does not look like grep/find output, so the
//! caller can fall through to a generic classifier.

/// Below this many matches we keep the raw lines (still adding a summary
/// header); above it we collapse to the file-set summary.
const KEEP_RAW_THRESHOLD: usize = 15;

/// How many unique paths to list in a summary before truncating.
const MAX_LISTED_FILES: usize = 20;

/// A parsed grep-shaped match line: the file path it belongs to.
struct Match<'a> {
    file: &'a str,
}

/// Compact verbose grep/rg/find output to a file-set summary.
///
/// Returns `Some(compacted)` when the output is recognizably grep/rg or find
/// output and was parsed; `None` when neither shape dominates (e.g. prose).
pub(super) fn compact(raw: &str, exit_code: i32) -> Option<String> {
    // The exit code carries no extra signal here: grep returns 1 on "no match"
    // and find returns 0 either way, so we classify purely on output shape.
    let _ = exit_code;

    let non_empty: Vec<&str> = raw.lines().filter(|l| !l.trim().is_empty()).collect();
    if non_empty.is_empty() {
        return None;
    }

    let grep_hits = non_empty
        .iter()
        .filter(|l| parse_grep_line(l).is_some())
        .count();
    let find_hits = non_empty.iter().filter(|l| looks_like_path(l)).count();

    // Require a clear majority of one shape; otherwise this isn't search output.
    let majority = non_empty.len() / 2;
    if grep_hits > majority && grep_hits >= find_hits {
        Some(compact_grep(&non_empty))
    } else if find_hits > majority {
        Some(compact_find(&non_empty))
    } else {
        None
    }
}

/// Summarize grep/rg match lines.
fn compact_grep(lines: &[&str]) -> String {
    let matches: Vec<Match> = lines.iter().filter_map(|l| parse_grep_line(l)).collect();
    let total = matches.len();
    let files = unique_in_order(matches.iter().map(|m| m.file));

    let header = format!("{total} matches in {} files:", files.len());

    if total <= KEEP_RAW_THRESHOLD {
        // Small result set: keep the raw match lines under the header.
        let mut out = String::with_capacity(header.len() + lines.len() * 16);
        out.push_str(&header);
        for line in lines {
            if parse_grep_line(line).is_some() {
                out.push('\n');
                out.push_str(line);
            }
        }
        out
    } else {
        summarize_paths(&header, &files)
    }
}

/// Summarize `find`-style bare paths.
fn compact_find(lines: &[&str]) -> String {
    let paths = unique_in_order(
        lines
            .iter()
            .filter(|l| looks_like_path(l))
            .map(|l| l.trim()),
    );
    let header = format!("{} paths:", paths.len());
    summarize_paths(&header, &paths)
}

/// Append the first `MAX_LISTED_FILES` paths to `header`, with a truncation
/// footer when the set is larger.
fn summarize_paths(header: &str, paths: &[&str]) -> String {
    let mut out = String::with_capacity(header.len() + paths.len().min(MAX_LISTED_FILES) * 32);
    out.push_str(header);
    for path in paths.iter().take(MAX_LISTED_FILES) {
        out.push('\n');
        out.push_str(path);
    }
    if paths.len() > MAX_LISTED_FILES {
        out.push_str(&format!("\n… and {} more", paths.len() - MAX_LISTED_FILES));
    }
    out
}

/// Parse a line as `path:line:content` (grep -n / ripgrep). The match key is
/// the file path. We split into at most three parts and require the middle
/// part to be all digits — that's what distinguishes a real `file:line:` hit
/// from an arbitrary line that merely contains a colon.
fn parse_grep_line(line: &str) -> Option<Match<'_>> {
    let mut parts = line.splitn(3, ':');
    let file = parts.next()?;
    let lineno = parts.next()?;
    // `content` may legitimately be empty (a blank matched line), but the
    // `file:line:` prefix must be present, so the third split must exist.
    parts.next()?;

    if file.is_empty() || lineno.is_empty() || !lineno.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    Some(Match { file })
}

/// Does this line look like a bare filesystem path (find output)? It must have
/// no colon (which would make it grep-shaped) and either contain a path
/// separator or a file-extension dot.
fn looks_like_path(line: &str) -> bool {
    let trimmed = line.trim();
    if trimmed.is_empty() || trimmed.contains(':') {
        return false;
    }
    trimmed.contains('/') || (trimmed.contains('.') && !trimmed.contains(' '))
}

/// Collect items preserving first-seen order, dropping later duplicates.
fn unique_in_order<'a, I: Iterator<Item = &'a str>>(items: I) -> Vec<&'a str> {
    let mut seen: Vec<&str> = Vec::new();
    for item in items {
        if !seen.contains(&item) {
            seen.push(item);
        }
    }
    seen
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grep_summarizes_to_unique_files() {
        let mut raw = String::new();
        for i in 0..200 {
            raw.push_str(&format!("src/file{}.rs:{}:    let x = foo();\n", i % 10, i));
        }
        let out = compact(&raw, 0).expect("grep compacts");
        assert!(out.to_lowercase().contains("match"));
        assert!(out.contains("file0.rs"));
        assert!(out.len() < raw.len());
        // 10 unique files, 200 matches.
        assert!(out.contains("10") && out.contains("200"));
    }

    #[test]
    fn returns_none_for_non_grep() {
        assert!(compact(&"plain text\n".repeat(60), 0).is_none());
    }

    #[test]
    fn small_grep_keeps_raw_lines() {
        let raw = "src/a.rs:1:foo\nsrc/b.rs:2:bar\nsrc/a.rs:9:baz";
        let out = compact(raw, 0).expect("compacts");
        assert!(out.starts_with("3 matches in 2 files:"));
        assert!(out.contains("src/a.rs:1:foo"));
        assert!(out.contains("src/b.rs:2:bar"));
    }

    #[test]
    fn find_summarizes_bare_paths() {
        let mut raw = String::new();
        for i in 0..50 {
            raw.push_str(&format!("src/dir{i}/file.rs\n"));
        }
        let out = compact(&raw, 0).expect("find compacts");
        assert!(out.starts_with("50 paths:"));
        assert!(out.contains("src/dir0/file.rs"));
        assert!(out.contains("… and 30 more"));
    }

    #[test]
    fn large_grep_truncates_file_list() {
        let mut raw = String::new();
        for i in 0..100 {
            raw.push_str(&format!("src/f{i}.rs:1:hit\n"));
        }
        let out = compact(&raw, 0).expect("compacts");
        assert!(out.starts_with("100 matches in 100 files:"));
        assert!(out.contains("… and 80 more"));
        // Raw content lines are dropped above the threshold.
        assert!(!out.contains(":1:hit"));
    }
}
