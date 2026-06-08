//! `wcore-repomap` — Aider-style light symbol extractor and codebase index.
//!
//! See the crate-level doc in `src/lib.rs` of the W3 plan + design contract
//! `docs/superpowers/specs/2026-05-14-wcore-super-agent-design.md` §5.6.
//!
//! **Isolated:** zero internal `wcore-*` dependencies, no protocol events.

#![warn(missing_docs)]
#![deny(unsafe_code)]

pub mod extractor;
pub mod render;
pub mod types;

pub use types::{FileSummary, IndexOptions, Language, RepoMap, RepoMapError, Symbol, SymbolKind};

use std::fs;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use ignore::WalkBuilder;

use crate::extractor::extract;

impl RepoMap {
    /// Build a repo map of `root` with default `IndexOptions`.
    pub fn build(root: &Path) -> Result<Self, RepoMapError> {
        Self::build_with_options(root, IndexOptions::default())
    }

    /// Build a repo map of `root` with custom options.
    pub fn build_with_options(root: &Path, opts: IndexOptions) -> Result<Self, RepoMapError> {
        let canonical = fs::canonicalize(root).map_err(|e| RepoMapError::Root {
            path: root.to_path_buf(),
            source: e,
        })?;

        let mut summaries: Vec<FileSummary> = Vec::new();

        let walker = WalkBuilder::new(&canonical)
            .standard_filters(opts.respect_gitignore)
            .hidden(false) // include dotfiles (e.g. .config), but .gitignore still applies
            .build();

        for entry in walker {
            let entry = match entry {
                Ok(e) => e,
                Err(_e) => continue, // per-entry errors are silently skipped (light tool stance)
            };
            let file_type = match entry.file_type() {
                Some(ft) => ft,
                None => continue,
            };
            if !file_type.is_file() {
                continue;
            }
            let abs_path = entry.path();
            let rel_path = match abs_path.strip_prefix(&canonical) {
                Ok(p) => p.to_path_buf(),
                Err(_) => continue,
            };

            let metadata = match fs::metadata(abs_path) {
                Ok(m) => m,
                Err(_) => continue,
            };
            let size_bytes = metadata.len();
            if size_bytes > opts.max_file_bytes {
                summaries.push(FileSummary {
                    path: rel_path,
                    language: Language::Other,
                    lines: 0,
                    size_bytes,
                    symbols: Vec::new(),
                    imports: Vec::new(),
                    first_meaningful_line: None,
                });
                continue;
            }

            let language = Language::from_path(&rel_path);
            let bytes = match fs::read(abs_path) {
                Ok(b) => b,
                Err(_) => continue,
            };
            let source = match std::str::from_utf8(&bytes) {
                Ok(s) => s,
                Err(_) => {
                    // non-UTF8 (binary or unusual encoding) — record size only.
                    summaries.push(FileSummary {
                        path: rel_path,
                        language: Language::Other,
                        lines: 0,
                        size_bytes,
                        symbols: Vec::new(),
                        imports: Vec::new(),
                        first_meaningful_line: None,
                    });
                    continue;
                }
            };

            let lines = if source.is_empty() {
                0
            } else {
                source.lines().count()
            };
            if lines > opts.max_lines {
                summaries.push(FileSummary {
                    path: rel_path,
                    language,
                    lines,
                    size_bytes,
                    symbols: Vec::new(),
                    imports: Vec::new(),
                    first_meaningful_line: None,
                });
                continue;
            }

            let (symbols, imports) = extract(language, source);
            let first_meaningful_line = match language {
                Language::Other => first_meaningful(source),
                _ => None,
            };

            summaries.push(FileSummary {
                path: rel_path,
                language,
                lines,
                size_bytes,
                symbols,
                imports,
                first_meaningful_line,
            });
        }

        // Deterministic order — `ignore` walker order is platform-dependent.
        summaries.sort_by(|a, b| a.path.cmp(&b.path));

        let indexed_at_unix_secs = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);

        Ok(RepoMap {
            root: canonical,
            indexed_at_unix_secs,
            files: summaries,
        })
    }
}

/// Return the first non-blank, non-comment line (trimmed, truncated to 200
/// bytes on a char boundary). `None` if the file has no such line.
///
/// **Markdown-friendly:** `#` headings are NOT skipped. The Step 5.1 test
/// asserts `Some("# Project")` for a README starting with `# Project`, and
/// Markdown headings are genuinely meaningful first-lines for repo-map
/// purposes. Only C-style line/block comments (`//`, `/*`, `*` continuation)
/// and SQL-style (`--`) are skipped.
fn first_meaningful(source: &str) -> Option<String> {
    for raw in source.lines() {
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            continue;
        }
        if trimmed.starts_with("//")
            || trimmed.starts_with("/*")
            || trimmed.starts_with('*')
            || trimmed.starts_with("--")
        {
            continue;
        }
        let mut end = trimmed.len().min(200);
        while end > 0 && !trimmed.is_char_boundary(end) {
            end -= 1;
        }
        return Some(trimmed[..end].to_string());
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn repomap_default_has_empty_files_and_a_root() {
        // Pure data smoke test: types compile, default is sane,
        // SymbolKind variants cover every kind the extractors will emit.
        let map = RepoMap::empty(std::path::PathBuf::from("/tmp/example"));
        assert_eq!(map.root, std::path::PathBuf::from("/tmp/example"));
        assert!(map.files.is_empty());
    }

    #[test]
    fn symbol_kinds_cover_every_extractor_target() {
        // Compile-time guard: every kind both extractors will produce must
        // be a variant here. Adding a new kind requires updating this list.
        use SymbolKind::*;
        for kind in [
            Function, Struct, Enum, Trait, Impl, Module, Use, Class, Interface, TypeAlias, Export,
        ] {
            // `serde_json` round-trip ensures Serialize/Deserialize stay in sync.
            let s = serde_json::to_string(&kind).expect("serialize");
            let back: SymbolKind = serde_json::from_str(&s).expect("deserialize");
            assert_eq!(kind, back, "round-trip failed for {kind:?}");
        }
    }

    #[test]
    fn language_detect_recognizes_rust_and_typescript_and_falls_back() {
        use std::path::Path;
        assert_eq!(Language::from_path(Path::new("foo.rs")), Language::Rust);
        assert_eq!(
            Language::from_path(Path::new("foo.ts")),
            Language::TypeScript
        );
        assert_eq!(
            Language::from_path(Path::new("foo.tsx")),
            Language::TypeScript
        );
        assert_eq!(
            Language::from_path(Path::new("foo.js")),
            Language::JavaScript
        );
        assert_eq!(
            Language::from_path(Path::new("foo.mjs")),
            Language::JavaScript
        );
        assert_eq!(Language::from_path(Path::new("README.md")), Language::Other);
        assert_eq!(Language::from_path(Path::new("noext")), Language::Other);
    }

    #[test]
    fn build_on_inline_tempdir_finds_rust_and_ts_files() {
        use std::fs;
        use std::io::Write;

        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path();

        // Layout:
        //   src/lib.rs    — Rust: fn foo, struct Bar
        //   web/app.ts    — TS:  function greet, export const PI
        //   README.md     — unknown
        fs::create_dir_all(root.join("src")).unwrap();
        fs::create_dir_all(root.join("web")).unwrap();

        let mut rs = fs::File::create(root.join("src/lib.rs")).unwrap();
        writeln!(rs, "pub fn foo() {{}}\npub struct Bar {{}}\n").unwrap();
        drop(rs);

        let mut ts = fs::File::create(root.join("web/app.ts")).unwrap();
        writeln!(
            ts,
            "export function greet() {{}}\nexport const PI = 3.14;\n"
        )
        .unwrap();
        drop(ts);

        let mut md = fs::File::create(root.join("README.md")).unwrap();
        writeln!(md, "# Project\n\nThis is the readme.\n").unwrap();
        drop(md);

        let map = RepoMap::build(root).expect("build");
        // On macOS, fs::canonicalize may resolve /var -> /private/var; compare
        // against the canonicalized root instead of the tempdir's raw path.
        let canonical_root = std::fs::canonicalize(root).unwrap();
        assert_eq!(map.root, canonical_root);
        let paths: Vec<String> = map
            .files
            .iter()
            .map(|f| f.path.to_string_lossy().replace('\\', "/"))
            .collect();
        assert!(
            paths.contains(&"src/lib.rs".to_string()),
            "paths = {paths:?}"
        );
        assert!(
            paths.contains(&"web/app.ts".to_string()),
            "paths = {paths:?}"
        );
        assert!(
            paths.contains(&"README.md".to_string()),
            "paths = {paths:?}"
        );

        let rs_summary = map
            .files
            .iter()
            .find(|f| f.path.to_string_lossy().ends_with("lib.rs"))
            .unwrap();
        assert_eq!(rs_summary.language, Language::Rust);
        assert_eq!(rs_summary.symbols.len(), 2);

        let md_summary = map
            .files
            .iter()
            .find(|f| f.path.to_string_lossy().ends_with("README.md"))
            .unwrap();
        assert_eq!(md_summary.language, Language::Other);
        assert!(md_summary.symbols.is_empty());
        assert_eq!(
            md_summary.first_meaningful_line.as_deref(),
            Some("# Project")
        );
    }

    #[test]
    fn build_paths_are_sorted_deterministically() {
        use std::fs;

        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path();
        for name in ["zzz.rs", "aaa.rs", "mmm.rs"] {
            fs::write(root.join(name), "fn x() {}\n").unwrap();
        }

        let map = RepoMap::build(root).expect("build");
        let paths: Vec<String> = map
            .files
            .iter()
            .map(|f| f.path.to_string_lossy().to_string())
            .collect();
        let mut sorted = paths.clone();
        sorted.sort();
        assert_eq!(paths, sorted, "files must be returned in sorted order");
    }

    #[test]
    fn build_respects_gitignore() {
        use std::fs;

        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path();
        fs::write(root.join(".gitignore"), "target/\nignored.rs\n").unwrap();
        fs::write(root.join("kept.rs"), "fn keep() {}\n").unwrap();
        fs::write(root.join("ignored.rs"), "fn skip() {}\n").unwrap();
        fs::create_dir_all(root.join("target")).unwrap();
        fs::write(root.join("target/inside.rs"), "fn skip2() {}\n").unwrap();

        // `.gitignore` only takes effect under a git repo or with explicit
        // `.git/info/exclude`; create a `.git` directory so ignore::WalkBuilder
        // treats `root` as a git root.
        fs::create_dir_all(root.join(".git")).unwrap();

        let map = RepoMap::build(root).expect("build");
        let paths: Vec<String> = map
            .files
            .iter()
            .map(|f| f.path.to_string_lossy().replace('\\', "/"))
            .collect();
        assert!(paths.contains(&"kept.rs".to_string()), "paths = {paths:?}");
        assert!(
            !paths.iter().any(|p| p == "ignored.rs"),
            "ignored.rs leaked into map: {paths:?}"
        );
        assert!(
            !paths.iter().any(|p| p.starts_with("target/")),
            "target/ leaked into map: {paths:?}"
        );
    }

    #[test]
    fn build_rejects_nonexistent_root() {
        let result = RepoMap::build(std::path::Path::new("/definitely/does/not/exist/abc123"));
        assert!(
            matches!(result, Err(RepoMapError::Root { .. })),
            "expected RepoMapError::Root, got {result:?}"
        );
    }
}
