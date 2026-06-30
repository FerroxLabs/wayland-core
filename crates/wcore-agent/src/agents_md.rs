use std::collections::HashSet;
use std::path::{Path, PathBuf};

use wcore_config::config::app_config_dir;
use wcore_skills::paths::stop_boundary;

// ---------------------------------------------------------------------------
// Public types
// ---------------------------------------------------------------------------

pub struct AgentsMdFile {
    pub path: PathBuf,
    pub content: String,
    pub is_global: bool,
}

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

const MAX_INCLUDE_DEPTH: u8 = 5;

/// Per-file byte cap on collected/expanded project-context (a single AGENTS.md
/// plus its inlined @-includes). Large project docs get truncated here so one
/// file can't dominate the cached system prefix.
const MAX_PER_FILE_BYTES: usize = 32 * 1024;

/// Total byte cap across ALL collected project-context files (global + the
/// hierarchical AGENTS.md chain, with their @-includes inlined). This bounds the
/// session-permanent prefix so a large project (or @-included doc set) can't
/// bloat it to hundreds of thousands of tokens that a new chat never clears
/// (issue #115). Budget is spent nearest-first (cwd before ancestors/global).
const MAX_TOTAL_BYTES: usize = 64 * 1024;

const ALLOWED_EXTENSIONS: &[&str] = &[".md", ".txt", ".json", ".yaml", ".yml", ".toml"];

const INSTRUCTION_PREAMBLE: &str = "Codebase and user instructions are shown below. \
Be sure to adhere to these instructions. IMPORTANT: These instructions OVERRIDE any \
default behavior and you MUST follow them exactly as written.";

// ---------------------------------------------------------------------------
// Discovery
// ---------------------------------------------------------------------------

pub fn collect_agents_md(cwd: &str) -> Vec<AgentsMdFile> {
    let cwd_path = Path::new(cwd);

    // 1. Global: <config_dir>/wayland-core/AGENTS.md (least specific).
    let global_path = app_config_dir()
        .map(|d| d.join("AGENTS.md"))
        .filter(|p| p.is_file());

    // 2. Walk up from cwd to stop_boundary, collecting AGENTS.md paths.
    //    Collected deepest-first, i.e. the cwd's own file comes first.
    let boundary = stop_boundary(cwd_path);
    let mut project_paths = Vec::new();
    let mut current = cwd_path.to_path_buf();

    loop {
        let candidate = current.join("AGENTS.md");
        if candidate.is_file() {
            project_paths.push(candidate);
        }

        if Some(&current) == boundary.as_ref() || current.parent().is_none() {
            break;
        }

        match current.parent() {
            Some(parent) if parent != current.as_path() => {
                current = parent.to_path_buf();
            }
            _ => break,
        }
    }

    // Spend a shared total budget NEAREST-FIRST so the most relevant context
    // (the cwd's own AGENTS.md) survives when a project exceeds the cap. Files
    // are read cwd-first for budget priority, but tagged with their display
    // position (root-first) so output ordering is unchanged.
    let mut total_remaining = MAX_TOTAL_BYTES;

    let count = project_paths.len();
    let mut project_files: Vec<(usize, AgentsMdFile)> = Vec::new();
    for (i, path) in project_paths.iter().enumerate() {
        let display_pos = count - 1 - i; // reverse cwd-first -> root-first
        if let Some(file) = read_agents_md(path, false, &mut total_remaining) {
            project_files.push((display_pos, file));
        }
    }
    project_files.sort_by_key(|(pos, _)| *pos);

    // Global file is least specific -> lowest budget priority (read last).
    let global_file = global_path
        .as_ref()
        .and_then(|p| read_agents_md(p, true, &mut total_remaining));

    // Emit in display order: global first, then project root-first.
    let mut files = Vec::new();
    if let Some(g) = global_file {
        files.push(g);
    }
    files.extend(project_files.into_iter().map(|(_, f)| f));
    files
}

fn read_agents_md(
    path: &Path,
    is_global: bool,
    total_remaining: &mut usize,
) -> Option<AgentsMdFile> {
    if *total_remaining == 0 {
        return None;
    }
    let raw = std::fs::read_to_string(path).ok()?;
    if raw.trim().is_empty() {
        return None;
    }
    let base_dir = path.parent()?;
    let mut seen = HashSet::new();
    if let Ok(canonical) = path.canonicalize() {
        seen.insert(canonical);
    }

    // Per-file budget is the smaller of the per-file cap and what's left of the
    // total budget. expand_includes spends this budget as it inlines content.
    let start = (*total_remaining).min(MAX_PER_FILE_BYTES);
    let mut budget = start;
    let mut content = expand_includes(&raw, base_dir, 0, &mut seen, &mut budget);
    if budget == 0 {
        content.push_str(&truncation_marker());
    }
    *total_remaining = total_remaining.saturating_sub(start - budget);

    Some(AgentsMdFile {
        path: path.to_path_buf(),
        content,
        is_global,
    })
}

// ---------------------------------------------------------------------------
// Formatting
// ---------------------------------------------------------------------------

pub fn format_agents_md_section(files: &[AgentsMdFile]) -> String {
    if files.is_empty() {
        return String::new();
    }

    let mut parts = vec![INSTRUCTION_PREAMBLE.to_string()];

    for file in files {
        let description = if file.is_global {
            "(user's global instructions for all projects)"
        } else {
            "(project instructions)"
        };
        let header = format!("Contents of {} {}:", file.path.display(), description);
        parts.push(format!("{header}\n\n{}", file.content.trim()));
    }

    parts.join("\n\n")
}

// ---------------------------------------------------------------------------
// @include expansion
// ---------------------------------------------------------------------------

fn is_allowed_extension(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .map(|e| {
            let dotted = format!(".{e}");
            ALLOWED_EXTENSIONS.contains(&dotted.as_str())
        })
        .unwrap_or(false)
}

fn resolve_include_path(raw: &str, base_dir: &Path) -> Option<PathBuf> {
    let path_str = raw.trim();
    if path_str.is_empty() {
        return None;
    }

    let resolved = if let Some(rest) = path_str.strip_prefix("~/") {
        dirs::home_dir()?.join(rest)
    } else if let Some(rest) = path_str.strip_prefix("./") {
        base_dir.join(rest)
    } else if path_str.starts_with('/') {
        PathBuf::from(path_str)
    } else {
        base_dir.join(path_str)
    };

    Some(resolved)
}

/// Marker appended when project context is truncated by the byte budget.
fn truncation_marker() -> String {
    format!(
        "\n\n[... truncated: project context exceeds {} KB; trim AGENTS.md / its @-includes ...]",
        MAX_TOTAL_BYTES / 1024
    )
}

/// Append `line` (plus its joining newline) to `result` if `budget` allows.
/// Returns `false` once the budget is exhausted so the caller stops emitting.
fn push_within_budget(result: &mut Vec<String>, line: &str, budget: &mut usize) -> bool {
    let cost = line.len() + 1; // +1 for the "\n" join between lines
    if cost > *budget {
        *budget = 0;
        return false;
    }
    *budget -= cost;
    result.push(line.to_string());
    true
}

/// Inline `@path` includes, bounded by both recursion depth (`MAX_INCLUDE_DEPTH`)
/// and an accumulated byte `budget`. Once the budget is exhausted, expansion
/// stops and `*budget` is left at 0 so the caller can append a truncation marker.
fn expand_includes(
    content: &str,
    base_dir: &Path,
    depth: u8,
    seen: &mut HashSet<PathBuf>,
    budget: &mut usize,
) -> String {
    let mut result = Vec::new();
    let mut in_code_block = false;

    for line in content.lines() {
        if *budget == 0 {
            break;
        }

        let trimmed = line.trim_start();
        if trimmed.starts_with("```") {
            in_code_block = !in_code_block;
            if !push_within_budget(&mut result, line, budget) {
                break;
            }
            continue;
        }

        if in_code_block {
            if !push_within_budget(&mut result, line, budget) {
                break;
            }
            continue;
        }

        let standalone = line.trim();
        if standalone.starts_with('@') && !standalone.contains('`') {
            let path_str = &standalone[1..];
            // Strip fragment identifiers
            let path_str = match path_str.find('#') {
                Some(i) => &path_str[..i],
                None => path_str,
            };

            if let Some(resolved) = resolve_include_path(path_str, base_dir) {
                if !is_allowed_extension(&resolved) {
                    continue;
                }
                let canonical = resolved.canonicalize().unwrap_or_else(|_| resolved.clone());
                if seen.contains(&canonical) || depth >= MAX_INCLUDE_DEPTH {
                    continue;
                }
                if let Ok(included) = std::fs::read_to_string(&resolved) {
                    seen.insert(canonical);
                    // The recursive call spends the shared budget directly, so
                    // its result is already bounded — push it as-is.
                    let expanded = expand_includes(
                        &included,
                        resolved.parent().unwrap_or(base_dir),
                        depth + 1,
                        seen,
                        budget,
                    );
                    if !expanded.is_empty() {
                        result.push(expanded);
                    }
                }
                continue;
            }
        }

        if !push_within_budget(&mut result, line, budget) {
            break;
        }
    }

    result.join("\n")
}

/// Truncate `text` to at most `max_bytes` bytes on a UTF-8 char boundary,
/// appending a truncation marker when the limit is hit. Used to bound the custom
/// assistant prompt/preset so a giant preset can't bloat the cached prefix.
pub fn truncate_with_marker(text: &str, max_bytes: usize) -> String {
    if text.len() <= max_bytes {
        return text.to_string();
    }
    let mut end = max_bytes;
    while end > 0 && !text.is_char_boundary(end) {
        end -= 1;
    }
    let mut out = text[..end].to_string();
    out.push_str(&format!(
        "\n\n[... truncated: content exceeds {} KB; trim it to restore the full text ...]",
        max_bytes / 1024
    ));
    out
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    // Tests that don't exercise the include budget pass an effectively
    // unlimited one. Returns a fresh value so callers can take `&mut` without
    // mutating a `const` item (which would be a no-op and a CI lint error).
    fn unlimited_budget() -> usize {
        usize::MAX
    }

    // --- @include expansion tests ---

    #[test]
    fn test_no_includes_passthrough() {
        let tmp = TempDir::new().unwrap();
        let mut seen = HashSet::new();
        let input = "Hello world\nNo includes here.";
        let result = expand_includes(input, tmp.path(), 0, &mut seen, &mut unlimited_budget());
        assert_eq!(result, input);
    }

    #[test]
    fn test_simple_include() {
        let tmp = TempDir::new().unwrap();
        fs::write(tmp.path().join("other.md"), "INCLUDED_CONTENT").unwrap();
        let mut seen = HashSet::new();
        let input = "@other.md";
        let result = expand_includes(input, tmp.path(), 0, &mut seen, &mut unlimited_budget());
        assert!(result.contains("INCLUDED_CONTENT"));
        assert!(!result.contains("@other.md"));
    }

    #[test]
    fn test_include_relative_dot() {
        let tmp = TempDir::new().unwrap();
        fs::write(tmp.path().join("sub.md"), "SUB_CONTENT").unwrap();
        let mut seen = HashSet::new();
        let input = "@./sub.md";
        let result = expand_includes(input, tmp.path(), 0, &mut seen, &mut unlimited_budget());
        assert!(result.contains("SUB_CONTENT"));
    }

    #[test]
    fn test_include_inside_code_block_ignored() {
        let tmp = TempDir::new().unwrap();
        fs::write(tmp.path().join("skip.md"), "SHOULD_NOT_APPEAR").unwrap();
        let mut seen = HashSet::new();
        let input = "```\n@skip.md\n```";
        let result = expand_includes(input, tmp.path(), 0, &mut seen, &mut unlimited_budget());
        assert!(!result.contains("SHOULD_NOT_APPEAR"));
        assert!(result.contains("@skip.md"));
    }

    #[test]
    fn test_include_missing_file_silently_skipped() {
        let tmp = TempDir::new().unwrap();
        let mut seen = HashSet::new();
        let input = "before\n@nonexistent.md\nafter";
        let result = expand_includes(input, tmp.path(), 0, &mut seen, &mut unlimited_budget());
        assert!(result.contains("before"));
        assert!(result.contains("after"));
        assert!(!result.contains("@nonexistent.md"));
    }

    #[test]
    fn test_include_circular_reference() {
        let tmp = TempDir::new().unwrap();
        fs::write(tmp.path().join("a.md"), "A_CONTENT\n@b.md").unwrap();
        fs::write(tmp.path().join("b.md"), "B_CONTENT\n@a.md").unwrap();
        let mut seen = HashSet::new();
        let result = expand_includes("@a.md", tmp.path(), 0, &mut seen, &mut unlimited_budget());
        assert!(result.contains("A_CONTENT"));
        assert!(result.contains("B_CONTENT"));
        // @a.md in b.md should be skipped (circular)
    }

    #[test]
    fn test_include_max_depth() {
        let tmp = TempDir::new().unwrap();
        // Chain: d0 → d1 → d2 → d3 → d4 → d5 → d6
        // With MAX_INCLUDE_DEPTH=5, expansion from the outer call:
        // outer(0) expands @d0 at depth 0 → d0 content expanded at depth 1
        // depth 1 expands @d1 → d1 at depth 2 → ... → d3 at depth 4
        // depth 4 expands @d4 → d4 at depth 5 → depth 5 >= MAX, @d5 NOT expanded
        for i in 0..7 {
            let content = if i < 6 {
                format!("DEPTH_{i}\n@d{}.md", i + 1)
            } else {
                format!("DEPTH_{i}")
            };
            fs::write(tmp.path().join(format!("d{i}.md")), content).unwrap();
        }
        let mut seen = HashSet::new();
        let result = expand_includes("@d0.md", tmp.path(), 0, &mut seen, &mut unlimited_budget());
        assert!(result.contains("DEPTH_0"));
        assert!(result.contains("DEPTH_3"));
        assert!(result.contains("DEPTH_4"));
        // DEPTH_5 should NOT appear — depth limit reached
        assert!(!result.contains("DEPTH_5"));
    }

    #[test]
    fn test_include_disallowed_extension() {
        let tmp = TempDir::new().unwrap();
        fs::write(tmp.path().join("image.png"), "BINARY_DATA").unwrap();
        let mut seen = HashSet::new();
        let input = "@image.png";
        let result = expand_includes(input, tmp.path(), 0, &mut seen, &mut unlimited_budget());
        assert!(!result.contains("BINARY_DATA"));
    }

    #[test]
    fn test_include_with_surrounding_text() {
        let tmp = TempDir::new().unwrap();
        fs::write(tmp.path().join("inc.md"), "MIDDLE").unwrap();
        let mut seen = HashSet::new();
        let input = "TOP\n@inc.md\nBOTTOM";
        let result = expand_includes(input, tmp.path(), 0, &mut seen, &mut unlimited_budget());
        assert_eq!(result, "TOP\nMIDDLE\nBOTTOM");
    }

    #[test]
    fn test_is_allowed_extension() {
        assert!(is_allowed_extension(Path::new("file.md")));
        assert!(is_allowed_extension(Path::new("file.txt")));
        assert!(is_allowed_extension(Path::new("file.yaml")));
        assert!(is_allowed_extension(Path::new("file.yml")));
        assert!(is_allowed_extension(Path::new("file.toml")));
        assert!(is_allowed_extension(Path::new("file.json")));
        assert!(!is_allowed_extension(Path::new("file.png")));
        assert!(!is_allowed_extension(Path::new("file.rs")));
        assert!(!is_allowed_extension(Path::new("file")));
    }

    #[test]
    fn test_inline_code_span_not_expanded() {
        let tmp = TempDir::new().unwrap();
        fs::write(tmp.path().join("x.md"), "SHOULD_NOT_APPEAR").unwrap();
        let mut seen = HashSet::new();
        let input = "Use `@x.md` for config";
        let result = expand_includes(input, tmp.path(), 0, &mut seen, &mut unlimited_budget());
        assert!(!result.contains("SHOULD_NOT_APPEAR"));
    }

    #[test]
    fn test_home_path_expansion() {
        let tmp = TempDir::new().unwrap();
        let mut seen = HashSet::new();
        let input = "@~/nonexistent-test-file.md";
        let result = expand_includes(input, tmp.path(), 0, &mut seen, &mut unlimited_budget());
        assert!(!result.contains("@~/"));
    }

    // --- Discovery tests ---

    #[test]
    fn test_collect_no_agents_md_anywhere() {
        let tmp = TempDir::new().unwrap();
        let cwd = tmp.path();
        fs::create_dir(cwd.join(".git")).unwrap();
        let files = collect_agents_md(&cwd.to_string_lossy());
        assert!(files.is_empty());
    }

    #[test]
    fn test_collect_cwd_only() {
        let tmp = TempDir::new().unwrap();
        let cwd = tmp.path();
        fs::create_dir(cwd.join(".git")).unwrap();
        fs::write(cwd.join("AGENTS.md"), "CWD_RULES").unwrap();

        let files = collect_agents_md(&cwd.to_string_lossy());
        assert_eq!(files.len(), 1);
        assert!(files[0].content.contains("CWD_RULES"));
        assert!(!files[0].is_global);
    }

    #[test]
    fn test_collect_hierarchical_ordering() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        fs::create_dir(root.join(".git")).unwrap();
        fs::write(root.join("AGENTS.md"), "ROOT_RULES").unwrap();

        let sub = root.join("packages").join("server");
        fs::create_dir_all(&sub).unwrap();
        fs::write(sub.join("AGENTS.md"), "SUB_RULES").unwrap();

        let files = collect_agents_md(&sub.to_string_lossy());
        assert_eq!(files.len(), 2);
        assert!(files[0].content.contains("ROOT_RULES"));
        assert!(files[1].content.contains("SUB_RULES"));
    }

    #[test]
    fn test_collect_stops_at_git_root() {
        let tmp = TempDir::new().unwrap();
        let above_git = tmp.path();
        fs::write(above_git.join("AGENTS.md"), "ABOVE_GIT_SHOULD_NOT_APPEAR").unwrap();

        let repo = above_git.join("repo");
        fs::create_dir_all(&repo).unwrap();
        fs::create_dir(repo.join(".git")).unwrap();
        fs::write(repo.join("AGENTS.md"), "REPO_RULES").unwrap();

        let files = collect_agents_md(&repo.to_string_lossy());
        assert_eq!(files.len(), 1);
        assert!(files[0].content.contains("REPO_RULES"));
    }

    #[test]
    fn test_collect_skips_empty_agents_md() {
        let tmp = TempDir::new().unwrap();
        let cwd = tmp.path();
        fs::create_dir(cwd.join(".git")).unwrap();
        fs::write(cwd.join("AGENTS.md"), "   \n  ").unwrap();

        let files = collect_agents_md(&cwd.to_string_lossy());
        assert!(files.is_empty());
    }

    #[test]
    fn test_collect_with_include_expanded() {
        let tmp = TempDir::new().unwrap();
        let cwd = tmp.path();
        fs::create_dir(cwd.join(".git")).unwrap();
        fs::write(cwd.join("AGENTS.md"), "@rules.md").unwrap();
        fs::write(cwd.join("rules.md"), "INCLUDED_RULES").unwrap();

        let files = collect_agents_md(&cwd.to_string_lossy());
        assert_eq!(files.len(), 1);
        assert!(files[0].content.contains("INCLUDED_RULES"));
    }

    // --- Formatting tests ---

    #[test]
    fn test_format_empty() {
        let files: Vec<AgentsMdFile> = vec![];
        let result = format_agents_md_section(&files);
        assert!(result.is_empty());
    }

    #[test]
    fn test_format_single_project() {
        let files = vec![AgentsMdFile {
            path: PathBuf::from("/workspace/AGENTS.md"),
            content: "My rules".to_string(),
            is_global: false,
        }];
        let result = format_agents_md_section(&files);
        assert!(result.contains("Be sure to adhere to these instructions"));
        assert!(result.contains("Contents of /workspace/AGENTS.md (project instructions):"));
        assert!(result.contains("My rules"));
    }

    #[test]
    fn test_format_global_and_project() {
        let files = vec![
            AgentsMdFile {
                path: PathBuf::from("/home/user/.config/wayland-core/AGENTS.md"),
                content: "Global rules".to_string(),
                is_global: true,
            },
            AgentsMdFile {
                path: PathBuf::from("/workspace/AGENTS.md"),
                content: "Project rules".to_string(),
                is_global: false,
            },
        ];
        let result = format_agents_md_section(&files);
        let global_pos = result.find("Global rules").unwrap();
        let project_pos = result.find("Project rules").unwrap();
        assert!(global_pos < project_pos, "global before project");
        assert!(result.contains("(user's global instructions for all projects)"));
        assert!(result.contains("(project instructions)"));
    }

    // --- Size-budget / truncation tests (issue #115) ---

    #[test]
    fn test_small_agents_md_passthrough_unchanged() {
        // A small AGENTS.md is read verbatim with no truncation marker.
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("AGENTS.md");
        let body = "Short project rules.\nLine two.\nLine three.";
        fs::write(&path, body).unwrap();

        let mut total = MAX_TOTAL_BYTES;
        let file = read_agents_md(&path, false, &mut total).unwrap();
        assert_eq!(file.content, body);
        assert!(!file.content.contains("truncated"));
        // Budget consumed ~= body size, well under the cap.
        assert!(total < MAX_TOTAL_BYTES && total > MAX_TOTAL_BYTES - 1024);
    }

    #[test]
    fn test_oversized_agents_md_truncated_with_marker() {
        // An AGENTS.md far larger than the per-file cap is truncated near the
        // cap and carries the truncation marker.
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("AGENTS.md");
        // 200 KB of content (each line ~32 bytes) -> well over the 32 KB cap.
        let big: String = (0..6400)
            .map(|i| format!("rule line number {i:08} here\n"))
            .collect();
        fs::write(&path, &big).unwrap();

        let mut total = MAX_TOTAL_BYTES;
        let file = read_agents_md(&path, false, &mut total).unwrap();
        assert!(
            file.content.contains("truncated"),
            "expected truncation marker"
        );
        // Content is bounded to roughly the per-file cap (plus the small marker),
        // nowhere near the 200 KB original.
        assert!(
            file.content.len() <= MAX_PER_FILE_BYTES + 256,
            "content len {} exceeded cap+marker",
            file.content.len()
        );
        assert!(file.content.len() > MAX_PER_FILE_BYTES / 2);
    }

    #[test]
    fn test_includes_stop_once_budget_exceeded() {
        // Three large @-includes; a small budget lets the first through but cuts
        // off later ones once the accumulated size exceeds the budget.
        let tmp = TempDir::new().unwrap();
        let chunk_a: String = "AAAA".repeat(40); // 160 B
        let chunk_b: String = "BBBB".repeat(40);
        let chunk_c: String = "CCCC".repeat(40);
        fs::write(tmp.path().join("a.md"), &chunk_a).unwrap();
        fs::write(tmp.path().join("b.md"), &chunk_b).unwrap();
        fs::write(tmp.path().join("c.md"), &chunk_c).unwrap();

        let input = "@a.md\n@b.md\n@c.md";
        let mut seen = HashSet::new();
        let mut budget = 200usize; // enough for a.md, not all three
        let result = expand_includes(input, tmp.path(), 0, &mut seen, &mut budget);

        assert!(result.contains("AAAA"), "first include should be present");
        assert!(
            !result.contains("CCCC"),
            "later include must be dropped once budget is exhausted"
        );
        assert_eq!(budget, 0, "budget should be fully spent");
    }

    #[test]
    fn test_total_budget_prefers_nearest_file() {
        // When a deep (cwd) file already exhausts the total budget, the ancestor
        // file is dropped — nearest content wins.
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        fs::create_dir(root.join(".git")).unwrap();
        let ancestor: String = (0..3000).map(|i| format!("ANCESTOR {i:06}\n")).collect();
        fs::write(root.join("AGENTS.md"), &ancestor).unwrap();

        let sub = root.join("pkg").join("svc");
        fs::create_dir_all(&sub).unwrap();
        let near: String = (0..3000).map(|i| format!("NEAREST {i:06}\n")).collect();
        fs::write(sub.join("AGENTS.md"), &near).unwrap();

        let files = collect_agents_md(&sub.to_string_lossy());
        let combined: usize = files.iter().map(|f| f.content.len()).sum();
        assert!(
            combined <= MAX_TOTAL_BYTES + 512,
            "combined {combined} exceeded total cap"
        );
        // The nearest file's content must be present (it gets the budget first).
        assert!(files.iter().any(|f| f.content.contains("NEAREST")));
    }

    #[test]
    fn test_truncate_with_marker_passthrough_and_cap() {
        // Small input untouched; large input truncated on a char boundary.
        let small = "hello world";
        assert_eq!(truncate_with_marker(small, 1024), small);

        let big = "x".repeat(40 * 1024);
        let out = truncate_with_marker(&big, 16 * 1024);
        assert!(out.contains("truncated"));
        assert!(out.len() <= 16 * 1024 + 128);

        // Multi-byte char near the cut boundary stays valid UTF-8.
        let multibyte = "é".repeat(10_000); // 2 bytes each
        let capped = truncate_with_marker(&multibyte, 4097);
        assert!(capped.contains("truncated"));
        // No panic == boundary respected; string is valid UTF-8 by construction.
        assert!(capped.starts_with('é'));
    }
}
