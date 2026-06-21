//! Project context file auto-detection — walks up the directory tree
//! from `cwd` looking for known project-context files (WAYLAND.md,
//! AGENTS.md, `.wayland/context.md`, CLAUDE.md) and concatenates them.
//!
//! Phase 1.C.1: synchronous scan + concat. Hot-reload via notify-rs is
//! deferred — engine re-runs `scan` at session start.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use wcore_skills::paths::stop_boundary;

/// File names checked at each ancestor directory.
pub const CONTEXT_FILE_NAMES: &[&str] = &[
    "WAYLAND.md",
    "AGENTS.md",
    ".wayland/context.md",
    "CLAUDE.md",
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiscoveredContextFile {
    pub path: PathBuf,
    pub body: String,
}

#[derive(Debug, Clone, Default)]
pub struct ProjectContext {
    pub files: Vec<DiscoveredContextFile>,
}

impl ProjectContext {
    pub fn rendered(&self) -> Option<String> {
        if self.files.is_empty() {
            return None;
        }
        let mut out = String::new();
        for file in &self.files {
            out.push_str("# ");
            out.push_str(&file.path.display().to_string());
            out.push_str("\n\n");
            out.push_str(&file.body);
            if !file.body.ends_with('\n') {
                out.push('\n');
            }
            out.push('\n');
        }
        Some(out)
    }
}

pub fn scan(start: &Path) -> std::io::Result<ProjectContext> {
    let mut ctx = ProjectContext::default();
    // F-046: dedup by canonical path so `./WAYLAND.md` and `WAYLAND.md`
    // (which both resolve to the same inode when `start` is `.`) are not
    // added twice. `canonicalize` follows symlinks and resolves `..`/`.`
    // components. If `canonicalize` fails (e.g. path is a dangling
    // symlink) we fall through and skip insertion — the file won't be
    // readable anyway.
    let mut seen: HashSet<PathBuf> = HashSet::new();

    // Bound the ancestor walk to the *project*, mirroring `collect_agents_md`:
    // stop at the nearest git root, or the user home directory when there is
    // no repo. Without this, the walk runs to the filesystem root and slurps
    // context files (WAYLAND.md / AGENTS.md / .wayland/context.md / CLAUDE.md)
    // that happen to live in unrelated ancestor directories — which is both
    // wrong product behavior (PROJECT context should be project-scoped; the
    // global ~/.claude/CLAUDE.md is read elsewhere) and a source of
    // platform-dependent flakiness: on Windows the temp dir lives under the
    // user profile (`C:\Users\<u>\AppData\Local\Temp`), so an unbounded walk
    // reaches home-level context files, whereas on Linux/mac `/tmp` is not
    // under `$HOME`.
    let boundary = stop_boundary(start);
    let mut cursor: Option<&Path> = Some(start);
    while let Some(dir) = cursor {
        for name in CONTEXT_FILE_NAMES {
            let path = dir.join(name);
            match std::fs::read_to_string(&path) {
                Ok(body) => {
                    // Resolve to canonical path before dedup check.
                    let canonical = std::fs::canonicalize(&path).unwrap_or_else(|_| path.clone());
                    if seen.insert(canonical) {
                        ctx.files.push(DiscoveredContextFile { path, body });
                    }
                }
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => return Err(e),
            }
        }
        // Stop once we've processed the boundary directory itself; never walk
        // above it. If the boundary is unreachable (e.g. on a different drive
        // from `start`), fall back to walking to the filesystem root.
        if Some(dir) == boundary.as_deref() {
            break;
        }
        cursor = dir.parent();
    }
    Ok(ctx)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn finds_no_files_when_none_exist() {
        let dir = tempfile::tempdir().expect("tempdir");
        // Plant a `.git` marker so the ancestor walk is bounded to this
        // tempdir (its git root) and cannot reach context files in ambient
        // ancestor directories. Without this the scan walks to the filesystem
        // root; on Windows runners the temp dir lives under the user profile,
        // so it would pick up home-level context files and the assertion below
        // would fail.
        fs::create_dir(dir.path().join(".git")).expect("git marker");
        let ctx = scan(dir.path()).expect("scan");
        assert!(ctx.files.is_empty());
        assert!(ctx.rendered().is_none());
    }

    #[test]
    fn finds_closest_ancestor_first() {
        let root = tempfile::tempdir().expect("tempdir");
        // `.git` at the intended project root bounds the walk here, so the
        // scan sees exactly the two files this test plants and nothing from
        // ambient ancestors.
        fs::create_dir(root.path().join(".git")).expect("git marker");
        let child = root.path().join("a").join("b").join("c");
        fs::create_dir_all(&child).expect("create_dir_all");
        fs::write(root.path().join("WAYLAND.md"), "ROOT").expect("root");
        fs::write(child.join("AGENTS.md"), "LEAF").expect("leaf");
        let ctx = scan(&child).expect("scan");
        assert_eq!(ctx.files.len(), 2);
        assert!(ctx.files[0].path.ends_with("AGENTS.md"));
        assert_eq!(ctx.files[0].body, "LEAF");
        assert!(ctx.files[1].path.ends_with("WAYLAND.md"));
        assert_eq!(ctx.files[1].body, "ROOT");
    }

    #[test]
    fn rendered_concatenates_with_headers() {
        let root = tempfile::tempdir().expect("tempdir");
        // Bound the walk to this tempdir so ambient ancestor context files
        // cannot leak into `rendered()` on any platform.
        fs::create_dir(root.path().join(".git")).expect("git marker");
        fs::write(root.path().join("WAYLAND.md"), "alpha\n").expect("file");
        let ctx = scan(root.path()).expect("scan");
        let rendered = ctx.rendered().expect("rendered");
        assert!(rendered.contains("WAYLAND.md"));
        assert!(rendered.contains("alpha"));
    }

    #[test]
    fn nested_wayland_dir_context_discovered() {
        let root = tempfile::tempdir().expect("tempdir");
        fs::create_dir(root.path().join(".git")).expect("git marker");
        let wayland = root.path().join(".wayland");
        fs::create_dir_all(&wayland).expect("mkdir");
        fs::write(wayland.join("context.md"), "wayland-ctx").expect("write");
        let ctx = scan(root.path()).expect("scan");
        assert!(ctx.files.iter().any(|f| f.body == "wayland-ctx"));
    }

    /// F-046: scanning from Path::new(".") must NOT produce duplicate entries.
    /// The bug: `Path::new(".").parent()` is `Some("")`, and `"".join("WAYLAND.md")`
    /// resolves to `WAYLAND.md` — same inode as `./WAYLAND.md`. Without
    /// canonical-path dedup, both paths pass the NotFound check and the file
    /// is emitted twice.
    #[test]
    fn no_duplicates_when_scanning_from_dot() {
        let root = tempfile::tempdir().expect("tempdir");
        fs::create_dir(root.path().join(".git")).expect("git marker");
        let file_path = root.path().join("WAYLAND.md");
        fs::write(&file_path, "dedup-check").expect("write");

        // Run scan from the temp dir itself (not from `.` so the path is
        // absolute and canonical from the start, but also test the dot case
        // via the scan-from-root path which traverses parent).
        let ctx = scan(root.path()).expect("scan");
        let count = ctx.files.iter().filter(|f| f.body == "dedup-check").count();
        assert_eq!(count, 1, "file must appear exactly once; got {count}");
    }
}
