//! git output parser.
//!
//! Compacts verbose `git status` / `git log` / `git diff` output down to the
//! load-bearing signal so it costs fewer tokens in the agent context:
//!
//! * `git status` (long form) — collapse each section into a `<label>: <count>`
//!   header followed by a capped path list (`… N more` when truncated).
//! * `git log` (verbose form) — drop `Author:` / `Date:` / blank noise and keep
//!   the indented subject lines (oneline-ish), prefixed with the short hash.
//! * `git diff` — keep the `diff --git` / `@@` structure and drop unchanged
//!   context lines once the diff is large.
//!
//! Returns `Some(compacted)` only when the input confidently looks like git
//! output; otherwise `None` so the caller falls through to the raw text.

/// Maximum number of paths shown per `git status` group before truncating.
const STATUS_PATH_CAP: usize = 10;

/// Line count above which a `git diff` drops unchanged context lines.
const DIFF_CONTEXT_DROP_THRESHOLD: usize = 40;

pub(super) fn compact(raw: &str, exit_code: i32) -> Option<String> {
    // exit_code is informational here; the textual markers are authoritative.
    let _ = exit_code;

    if looks_like_log(raw) {
        return compact_log(raw);
    }
    if looks_like_diff(raw) {
        return compact_diff(raw);
    }
    if looks_like_status(raw) {
        return compact_status(raw);
    }
    None
}

/// A verbose `git log` entry starts with `commit <40 hex>` and usually carries
/// `Author:` / `Date:` lines.
fn looks_like_log(raw: &str) -> bool {
    raw.lines()
        .any(|line| line.strip_prefix("commit ").is_some_and(is_full_hash))
        && raw.lines().any(|line| line.starts_with("Author:"))
}

fn looks_like_diff(raw: &str) -> bool {
    raw.lines().any(|line| line.starts_with("diff --git"))
}

fn looks_like_status(raw: &str) -> bool {
    raw.lines().any(|line| {
        line.starts_with("On branch ")
            || line.starts_with("Changes not staged")
            || line.starts_with("Changes to be committed")
            || line.starts_with("Untracked files")
            || line.trim_start().starts_with("modified:")
            || line.trim_start().starts_with("new file:")
    })
}

fn is_full_hash(s: &str) -> bool {
    let hash = s.split_whitespace().next().unwrap_or(s);
    hash.len() == 40 && hash.bytes().all(|b| b.is_ascii_hexdigit())
}

// ----- git log ------------------------------------------------------------

fn compact_log(raw: &str) -> Option<String> {
    let mut out = Vec::new();
    let mut pending_hash: Option<String> = None;

    for line in raw.lines() {
        if let Some(rest) = line.strip_prefix("commit ")
            && is_full_hash(rest)
        {
            let hash = rest.split_whitespace().next().unwrap_or(rest);
            pending_hash = Some(hash.chars().take(7).collect());
            continue;
        }
        if line.starts_with("Author:")
            || line.starts_with("Date:")
            || line.starts_with("Merge:")
            || line.trim().is_empty()
        {
            continue;
        }
        // Indented body line — the first one after a commit is the subject.
        let trimmed = line.trim_start();
        if let Some(hash) = pending_hash.take() {
            out.push(format!("{hash} {trimmed}"));
        }
        // Subsequent body lines are dropped (oneline form keeps the subject only).
    }

    if out.is_empty() {
        return None;
    }
    Some(out.join("\n"))
}

// ----- git status ----------------------------------------------------------

/// One section of a `git status` long-form listing.
struct StatusGroup {
    label: String,
    paths: Vec<String>,
}

fn compact_status(raw: &str) -> Option<String> {
    let mut groups: Vec<StatusGroup> = Vec::new();
    let mut branch: Option<String> = None;

    for line in raw.lines() {
        if let Some(rest) = line.strip_prefix("On branch ") {
            branch = Some(rest.trim().to_string());
            continue;
        }

        // Section headers end with ':' and are not indented.
        if !line.starts_with('\t') && !line.starts_with(' ') && line.trim_end().ends_with(':') {
            let label = line.trim_end().trim_end_matches(':').to_string();
            groups.push(StatusGroup {
                label,
                paths: Vec::new(),
            });
            continue;
        }

        // Indented entries belong to the current (last) group.
        let entry = line.trim_start();
        if entry.is_empty() || !(line.starts_with('\t') || line.starts_with("    ")) {
            continue;
        }
        // Git annotates some lines with hints like "(use ...)"; skip those.
        if entry.starts_with('(') {
            continue;
        }
        if let Some(group) = groups.last_mut() {
            // Strip the change-type label ("modified:   path" -> "modified:" + path).
            let path = match entry.split_once(':') {
                Some((kind, rest)) if is_change_kind(kind) => {
                    format!("{}: {}", kind.trim(), rest.trim())
                }
                _ => entry.to_string(),
            };
            group.paths.push(path);
        }
    }

    // Require at least one group with content to claim a confident parse.
    if groups.iter().all(|g| g.paths.is_empty()) {
        return None;
    }

    let mut out = Vec::new();
    if let Some(b) = branch {
        out.push(format!("On branch {b}"));
    }

    for group in &groups {
        if group.paths.is_empty() {
            continue;
        }
        out.push(format!("{}: {}", group.label, group.paths.len()));

        for path in group.paths.iter().take(STATUS_PATH_CAP) {
            out.push(format!("  {path}"));
        }
        if group.paths.len() > STATUS_PATH_CAP {
            let more = group.paths.len() - STATUS_PATH_CAP;
            out.push(format!("  … {more} more"));
        }
    }

    Some(out.join("\n"))
}

/// Recognized `git status` change-type prefixes (the bit before the colon).
fn is_change_kind(kind: &str) -> bool {
    matches!(
        kind.trim(),
        "modified" | "new file" | "deleted" | "renamed" | "copied" | "typechange"
    )
}

// ----- git diff ------------------------------------------------------------

fn compact_diff(raw: &str) -> Option<String> {
    let line_count = raw.lines().count();
    let drop_context = line_count > DIFF_CONTEXT_DROP_THRESHOLD;

    let mut out = Vec::new();
    for line in raw.lines() {
        if line.starts_with("diff --git")
            || line.starts_with("@@")
            || line.starts_with("+++")
            || line.starts_with("---")
            || line.starts_with("index ")
            || line.starts_with("new file")
            || line.starts_with("deleted file")
            || line.starts_with("rename ")
        {
            out.push(line.to_string());
            continue;
        }
        // Added / removed lines are always signal.
        if line.starts_with('+') || line.starts_with('-') {
            out.push(line.to_string());
            continue;
        }
        // Unchanged context lines (leading single space) — drop when large.
        if drop_context && line.starts_with(' ') {
            continue;
        }
        out.push(line.to_string());
    }

    if out.is_empty() {
        return None;
    }
    Some(out.join("\n"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_groups_by_change_type() {
        let mut raw = String::from("On branch main\nChanges not staged for commit:\n");
        for i in 0..50 {
            raw.push_str(&format!("\tmodified:   file{i}.rs\n"));
        }
        raw.push_str("Untracked files:\n");
        for i in 0..30 {
            raw.push_str(&format!("\tnew{i}.rs\n"));
        }
        let out = compact(&raw, 0).expect("git status compacts");
        assert!(out.contains("modified"));
        assert!(out.to_lowercase().contains("50") || out.contains("modified: 50"));
        assert!(out.len() < raw.len());
    }

    #[test]
    fn log_is_oneline() {
        let mut raw = String::new();
        for i in 0..40 {
            raw.push_str(&format!(
                "commit {:040x}\nAuthor: a <a@b>\nDate: now\n\n    subject {i}\n\n",
                i
            ));
        }
        let out = compact(&raw, 0).expect("git log compacts");
        assert!(out.contains("subject 0"));
        assert!(!out.contains("Author: a"), "drop author/date noise");
        assert!(out.len() < raw.len());
    }

    #[test]
    fn returns_none_for_non_git() {
        assert!(compact(&"random\n".repeat(60), 0).is_none());
    }
}
