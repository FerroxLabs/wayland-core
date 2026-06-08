//! Full-pipeline integration test against a committed fixture tree.
//!
//! Verifies: walker honours .gitignore, dispatches per extension,
//! extracts Rust + TS symbols correctly, records README.md as Other
//! with a first_meaningful_line, and produces deterministic render
//! output.

use std::path::PathBuf;

use wcore_repomap::{Language, RepoMap, SymbolKind};

fn fixture_root() -> PathBuf {
    let mut p = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    p.push("tests");
    p.push("fixtures");
    p.push("sample_repo");
    p
}

#[test]
fn indexes_fixture_repo_correctly() {
    let map = RepoMap::build(&fixture_root()).expect("build");
    let rels: Vec<String> = map
        .files
        .iter()
        .map(|f| f.path.to_string_lossy().replace('\\', "/"))
        .collect();

    // .gitignore must hide target/
    assert!(
        !rels.iter().any(|p| p.starts_with("target/")),
        "target/ leaked: {rels:?}"
    );
    // expected files present
    assert!(rels.contains(&".gitignore".to_string()), "{rels:?}");
    assert!(rels.contains(&"README.md".to_string()), "{rels:?}");
    assert!(rels.contains(&"src/inner.rs".to_string()), "{rels:?}");
    assert!(rels.contains(&"src/lib.rs".to_string()), "{rels:?}");
    assert!(rels.contains(&"web/app.ts".to_string()), "{rels:?}");

    // Rust extraction. Normalize path-separators for cross-platform
    // (Windows native paths use `\`); the canonical-form check at
    // line 26 already applied this pattern to `rels`, just not here.
    let lib = map
        .files
        .iter()
        .find(|f| {
            f.path
                .to_string_lossy()
                .replace('\\', "/")
                .ends_with("src/lib.rs")
        })
        .expect("src/lib.rs present");
    assert_eq!(lib.language, Language::Rust);
    let lib_names: Vec<(SymbolKind, &str)> = lib
        .symbols
        .iter()
        .map(|s| (s.kind, s.name.as_str()))
        .collect();
    assert!(lib_names.contains(&(SymbolKind::Function, "hello")));
    assert!(lib_names.contains(&(SymbolKind::Struct, "Greeter")));
    assert!(lib_names.contains(&(SymbolKind::Enum, "Mood")));
    assert!(
        lib_names
            .iter()
            .any(|(k, n)| *k == SymbolKind::Impl && *n == "Greeter")
    );
    assert!(lib_names.contains(&(SymbolKind::Module, "inner")));
    assert!(
        lib_names
            .iter()
            .any(|(k, n)| *k == SymbolKind::Use && n.contains("crate::inner::Helper"))
    );
    assert!(lib.imports.iter().any(|i| i.contains("std::path::PathBuf")));

    // TS extraction
    let ts = map
        .files
        .iter()
        .find(|f| {
            f.path
                .to_string_lossy()
                .replace('\\', "/")
                .ends_with("web/app.ts")
        })
        .expect("web/app.ts present");
    assert_eq!(ts.language, Language::TypeScript);
    let ts_names: Vec<(SymbolKind, &str)> = ts
        .symbols
        .iter()
        .map(|s| (s.kind, s.name.as_str()))
        .collect();
    assert!(ts_names.contains(&(SymbolKind::Function, "greet")));
    assert!(ts_names.contains(&(SymbolKind::Class, "Widget")));
    assert!(ts_names.contains(&(SymbolKind::Interface, "Options")));
    assert!(ts_names.contains(&(SymbolKind::TypeAlias, "Callback")));
    assert!(
        ts_names
            .iter()
            .any(|(k, n)| *k == SymbolKind::Export && *n == "PI")
    );
    assert!(
        ts_names
            .iter()
            .any(|(k, n)| *k == SymbolKind::Class && *n == "App")
    );
    assert!(ts.imports.iter().any(|i| i.contains("node:fs/promises")));

    // Markdown — fallback shape
    let md = map
        .files
        .iter()
        .find(|f| {
            f.path
                .to_string_lossy()
                .replace('\\', "/")
                .ends_with("README.md")
        })
        .expect("README.md present");
    assert_eq!(md.language, Language::Other);
    assert!(md.symbols.is_empty());
    assert!(md.imports.is_empty());
    assert_eq!(md.first_meaningful_line.as_deref(), Some("# Sample Repo"));
}

#[test]
fn render_compact_is_deterministic_on_fixture() {
    let map = RepoMap::build(&fixture_root()).expect("build");
    let a = wcore_repomap::render::render_compact(&map);
    let b = wcore_repomap::render::render_compact(&map);
    assert_eq!(a, b, "renderer non-deterministic on fixture");

    // Render mentions every known fixture file. Normalize separators
    // for cross-platform — render_compact uses native paths on Windows.
    let a_norm = a.replace('\\', "/");
    assert!(a_norm.contains("src/lib.rs"));
    assert!(a_norm.contains("web/app.ts"));
    assert!(a_norm.contains("README.md"));
    // And NEVER the ignored one.
    assert!(!a_norm.contains("should_be_ignored.rs"));
}
