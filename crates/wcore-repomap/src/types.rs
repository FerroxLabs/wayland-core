//! Public data types for `wcore-repomap`.
//!
//! Schema is forward-compatible: `#[non_exhaustive]` on the public structs
//! lets future waves add fields (e.g. `imports_resolved`, `doc_summary`)
//! without breaking downstream tool callers.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// Top-level result of `RepoMap::build`.
#[non_exhaustive]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RepoMap {
    /// Absolute path the build was rooted at. All `FileSummary.path`s are
    /// relative to this.
    pub root: PathBuf,
    /// `SystemTime` of when this map was built, serialized as an RFC-3339
    /// string via `chrono` is left to future work; for now we store the
    /// duration-since-epoch in seconds for portability.
    pub indexed_at_unix_secs: u64,
    /// One entry per indexed file. Sorted deterministically by `path`.
    pub files: Vec<FileSummary>,
}

impl RepoMap {
    /// Construct an empty map rooted at `root` and `indexed_at = 0`. Used
    /// in tests and as a placeholder before `build` populates it.
    pub fn empty(root: PathBuf) -> Self {
        Self {
            root,
            indexed_at_unix_secs: 0,
            files: Vec::new(),
        }
    }
}

/// Per-file summary.
#[non_exhaustive]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileSummary {
    /// Path relative to `RepoMap.root`. Cross-platform safe (built via
    /// `Path::strip_prefix`, never via string ops).
    pub path: PathBuf,
    /// Detected language.
    pub language: Language,
    /// Total line count (newline-delimited; final unterminated line counts as 1).
    pub lines: usize,
    /// Total size on disk in bytes.
    pub size_bytes: u64,
    /// Extracted symbols. Empty for `Language::Other`.
    pub symbols: Vec<Symbol>,
    /// Extracted import / use lines (raw, normalized whitespace). Empty
    /// for `Language::Other`.
    pub imports: Vec<String>,
    /// For `Language::Other`: the first non-blank, non-comment line of the
    /// file (truncated to 200 bytes on a char boundary). `None` for known
    /// languages — call `symbols` / `imports` for those.
    pub first_meaningful_line: Option<String>,
}

/// One extracted symbol.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Symbol {
    /// What kind of symbol this is.
    pub kind: SymbolKind,
    /// Symbol name as it appears in source (e.g. `Greeter`, `hello_world`).
    /// For `impl Trait for Type`, this is `"Trait for Type"`.
    pub name: String,
    /// 1-based line number where the declaration starts.
    pub line: usize,
}

/// Symbol kinds covered by W3 extractors.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SymbolKind {
    // Rust + TypeScript-shared
    /// Function declaration (`fn` in Rust, `function` in TS/JS).
    Function,
    /// Struct declaration (Rust only in W3).
    Struct,
    /// Enum declaration (Rust only in W3).
    Enum,
    // Rust-specific
    /// Trait declaration.
    Trait,
    /// `impl` block (inherent or `impl Trait for Type`).
    Impl,
    /// Module declaration.
    Module,
    /// `pub use` re-export.
    Use,
    // TypeScript-specific
    /// Class declaration.
    Class,
    /// Interface declaration.
    Interface,
    /// Type alias.
    TypeAlias,
    /// `export const/let/var` or `export { … }` re-export.
    Export,
}

/// Language tag derived from file extension.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Language {
    /// `.rs`
    Rust,
    /// `.ts`, `.tsx`
    TypeScript,
    /// `.js`, `.mjs`, `.cjs`, `.jsx`
    JavaScript,
    /// Anything else — first-line + size fallback only.
    Other,
}

impl Language {
    /// Map a path's extension to a `Language`. Case-insensitive on the
    /// extension. Returns `Language::Other` for unknown extensions.
    pub fn from_path(path: &Path) -> Self {
        let ext = match path.extension().and_then(|e| e.to_str()) {
            Some(e) => e.to_ascii_lowercase(),
            None => return Language::Other,
        };
        match ext.as_str() {
            "rs" => Language::Rust,
            "ts" | "tsx" => Language::TypeScript,
            "js" | "mjs" | "cjs" | "jsx" => Language::JavaScript,
            _ => Language::Other,
        }
    }
}

/// Options for `RepoMap::build_with_options`. Defaults are sensible.
#[derive(Debug, Clone)]
pub struct IndexOptions {
    /// Maximum file size to scan (bytes). Files larger than this become
    /// `Language::Other` with `first_meaningful_line = None`. Default 5 MB.
    pub max_file_bytes: u64,
    /// Maximum line count to scan. Files exceeding this are recorded with
    /// `lines`/`size_bytes` only; symbols are empty. Default 50_000.
    pub max_lines: usize,
    /// If true, respect `.gitignore` / `.git/info/exclude`. Default true.
    pub respect_gitignore: bool,
}

impl Default for IndexOptions {
    fn default() -> Self {
        Self {
            max_file_bytes: 5 * 1024 * 1024,
            max_lines: 50_000,
            respect_gitignore: true,
        }
    }
}

/// Public error type for the crate. Per AGENTS.md, public APIs use `thiserror`.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum RepoMapError {
    /// The provided root did not exist or could not be canonicalized.
    #[error("repo root not accessible: {path}: {source}")]
    Root {
        /// Path that failed to canonicalize.
        path: PathBuf,
        /// Underlying IO error.
        #[source]
        source: std::io::Error,
    },
    /// Walker reported an unrecoverable error.
    #[error("walker error: {0}")]
    Walk(#[from] ignore::Error),
    /// Reading a file failed in a way the indexer chose to surface
    /// (most IO errors are recorded per-file and skipped, not bubbled).
    #[error("io error at {path}: {source}")]
    Io {
        /// Path that failed.
        path: PathBuf,
        /// Underlying IO error.
        #[source]
        source: std::io::Error,
    },
}
