//! `---`-fenced YAML frontmatter splitting, shared by every markdown-asset
//! adapter (Claude Code agents/skills, Codex skills). Lives here so the
//! adapters carry one implementation between them rather than a copy each.

use std::path::Path;

use serde::Deserialize;

/// Split a `---`-fenced YAML frontmatter block from the markdown body.
/// Returns `(Some(frontmatter), body)` when a complete fence is present.
pub fn split_frontmatter(content: &str) -> (Option<String>, String) {
    let mut lines = content.lines();
    if lines.next().map(str::trim_end) != Some("---") {
        return (None, content.to_string());
    }
    let mut fm = String::new();
    let mut body = String::new();
    let mut in_body = false;
    for line in lines {
        if !in_body && line.trim_end() == "---" {
            in_body = true;
            continue;
        }
        if in_body {
            body.push_str(line);
            body.push('\n');
        } else {
            fm.push_str(line);
            fm.push('\n');
        }
    }
    if !in_body {
        // No closing fence — treat the whole thing as body.
        return (None, content.to_string());
    }
    (Some(fm), body)
}

/// Read a markdown file's frontmatter `name`, if it has one.
pub fn frontmatter_name(md: &Path) -> Option<String> {
    let content = std::fs::read_to_string(md).ok()?;
    let (fm, _) = split_frontmatter(&content);
    #[derive(Deserialize)]
    struct N {
        name: Option<String>,
    }
    serde_yaml::from_str::<N>(&fm?).ok()?.name
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_a_complete_fence() {
        let (fm, body) = split_frontmatter("---\nname: x\n---\nhello\n");
        assert_eq!(fm.as_deref(), Some("name: x\n"));
        assert_eq!(body, "hello\n");
    }

    #[test]
    fn unterminated_fence_is_all_body() {
        let (fm, body) = split_frontmatter("---\nname: x\nhello\n");
        assert!(fm.is_none());
        assert_eq!(body, "---\nname: x\nhello\n");
    }

    #[test]
    fn no_fence_is_all_body() {
        let (fm, body) = split_frontmatter("hello\n");
        assert!(fm.is_none());
        assert_eq!(body, "hello\n");
    }
}
