//! Compact, deterministic text rendering of a `RepoMap` for low-token
//! consumption by an LLM.
//!
//! Format (stable):
//!
//! ```text
//! repo: <root>
//! indexed_at: <unix_secs>
//! files: <n>
//!
//! <rel_path>  [lang=<lang>  lines=<n>  size=<bytes>]
//!   <kind>: <name>@<line>
//!   ...
//!   imports: <a>, <b>, <c>
//!
//! ...
//! ```
//!
//! Determinism: the rendered bytes for a `RepoMap` are byte-identical
//! across re-renders of the same map. This matters for caching and for
//! diffing two maps.

use std::fmt::Write as _;

use crate::types::{Language, RepoMap, SymbolKind};

/// Render a `RepoMap` to a compact, deterministic string.
pub fn render_compact(map: &RepoMap) -> String {
    let mut out = String::new();
    let _ = writeln!(out, "repo: {}", map.root.display());
    let _ = writeln!(out, "indexed_at: {}", map.indexed_at_unix_secs);
    let _ = writeln!(out, "files: {}", map.files.len());
    let _ = writeln!(out);

    for f in &map.files {
        let _ = writeln!(
            out,
            "{}  [lang={}  lines={}  size={}]",
            f.path.display(),
            lang_tag(f.language),
            f.lines,
            f.size_bytes
        );
        for s in &f.symbols {
            let _ = writeln!(out, "  {}: {}@{}", kind_tag(s.kind), s.name, s.line);
        }
        if !f.imports.is_empty() {
            let mut imports = f.imports.clone();
            imports.sort(); // deterministic order even if extractor returned source order
            let _ = writeln!(out, "  imports: {}", imports.join(", "));
        }
        if let Some(line) = &f.first_meaningful_line {
            let _ = writeln!(out, "  first: {line}");
        }
        let _ = writeln!(out);
    }

    out
}

fn lang_tag(l: Language) -> &'static str {
    match l {
        Language::Rust => "rust",
        Language::TypeScript => "typescript",
        Language::JavaScript => "javascript",
        Language::Other => "other",
    }
}

fn kind_tag(k: SymbolKind) -> &'static str {
    match k {
        SymbolKind::Function => "fn",
        SymbolKind::Struct => "struct",
        SymbolKind::Enum => "enum",
        SymbolKind::Trait => "trait",
        SymbolKind::Impl => "impl",
        SymbolKind::Module => "mod",
        SymbolKind::Use => "use",
        SymbolKind::Class => "class",
        SymbolKind::Interface => "interface",
        SymbolKind::TypeAlias => "type",
        SymbolKind::Export => "export",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{FileSummary, Language, RepoMap, Symbol, SymbolKind};
    use std::path::PathBuf;

    fn sample_map() -> RepoMap {
        RepoMap {
            root: PathBuf::from("/tmp/repo"),
            indexed_at_unix_secs: 42,
            files: vec![FileSummary {
                path: PathBuf::from("src/lib.rs"),
                language: Language::Rust,
                lines: 3,
                size_bytes: 100,
                symbols: vec![
                    Symbol {
                        kind: SymbolKind::Function,
                        name: "foo".into(),
                        line: 1,
                    },
                    Symbol {
                        kind: SymbolKind::Struct,
                        name: "Bar".into(),
                        line: 2,
                    },
                ],
                imports: vec!["std::path::PathBuf".into()],
                first_meaningful_line: None,
            }],
        }
    }

    #[test]
    fn render_is_deterministic_across_runs() {
        let map = sample_map();
        let a = render_compact(&map);
        let b = render_compact(&map);
        assert_eq!(a, b, "renderer is non-deterministic");
    }

    #[test]
    fn render_contains_expected_shape() {
        let s = render_compact(&sample_map());
        assert!(s.contains("repo: /tmp/repo"));
        assert!(s.contains("indexed_at: 42"));
        assert!(s.contains("files: 1"));
        assert!(s.contains("[lang=rust"));
        assert!(s.contains("fn: foo@1"));
        assert!(s.contains("struct: Bar@2"));
        assert!(s.contains("imports: std::path::PathBuf"));
    }
}
