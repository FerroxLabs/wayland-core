//! T3-3.2.1 — Binary file extensions to skip for text-based operations.
//!
//! Ported from the prior Wayland Python engine, which is
//! itself a port of `free-code src/constants/files.ts`.
//!
//! These files can't be meaningfully compared as text and are often large.
//! Tools that operate on text content (e.g. text diff, line counting,
//! grep-style scanning) can use [`has_binary_extension`] as a cheap
//! pure-string pre-filter to skip well-known binary formats without
//! touching the filesystem.
//!
//! This is complementary to — not a replacement for — the null-byte
//! content sniff that `ReadTool` performs on actual file bytes. The two
//! checks answer different questions:
//!
//! * `has_binary_extension(path)` — "does this look binary by name?"
//!   (no I/O, may have false negatives on extensionless binaries and
//!   false positives if a text file is misnamed).
//! * `read.rs` null-byte sniff — "is this actually binary?" (requires
//!   reading bytes; authoritative).
//!
//! The list intentionally excludes `.pdf`: PDFs are text-structured and
//! agents may want to inspect them.

/// Lower-cased binary file extensions including the leading dot.
///
/// Kept sorted to enable `binary_search` lookup in [`has_binary_extension`].
/// When adding entries, preserve sort order — the test
/// `binary_extensions_list_is_sorted` enforces this at build time.
pub const BINARY_EXTENSIONS: &[&str] = &[
    ".3ds", ".7z", ".a", ".aac", ".ai", ".aiff", ".app", ".avi", ".bin", ".blend", ".bmp", ".bz2",
    ".class", ".dat", ".data", ".db", ".deb", ".dll", ".doc", ".docx", ".dylib", ".ear", ".eot",
    ".eps", ".exe", ".fig", ".fla", ".flac", ".flv", ".gif", ".gz", ".ico", ".idx", ".iso", ".jar",
    ".jpeg", ".jpg", ".lib", ".lockb", ".m4a", ".m4v", ".max", ".mdb", ".mkv", ".mov", ".mp3",
    ".mp4", ".mpeg", ".mpg", ".msi", ".node", ".o", ".obj", ".odp", ".ods", ".odt", ".ogg",
    ".opus", ".otf", ".png", ".ppt", ".pptx", ".psd", ".pyc", ".pyo", ".rar", ".rlib", ".rpm",
    ".sketch", ".so", ".sqlite", ".sqlite3", ".swf", ".tar", ".tgz", ".tif", ".tiff", ".ttf",
    ".war", ".wasm", ".wav", ".webm", ".webp", ".wma", ".wmv", ".woff", ".woff2", ".xd", ".xls",
    ".xlsx", ".xz", ".z", ".zip",
];

/// Check if a file path has a known binary extension.
///
/// Pure string check — performs no filesystem I/O. The comparison is
/// case-insensitive on the extension (`Foo.PNG` matches `.png`).
///
/// Returns `false` for paths with no extension, paths ending in `.`,
/// or extensions not in [`BINARY_EXTENSIONS`].
pub fn has_binary_extension(path: &str) -> bool {
    let Some(dot) = path.rfind('.') else {
        return false;
    };
    // Reject "trailing dot only" (".") and any dot that lives inside a
    // directory segment rather than the final component — e.g.
    // "foo.png/bar" has rfind('.')=3 but the ext logically belongs to
    // the directory, not the file. Rust's `rfind` already finds the
    // rightmost dot, but if a path separator comes AFTER it, treat the
    // path as extensionless.
    let after_dot = &path[dot..];
    if after_dot.contains('/') || after_dot.contains('\\') {
        return false;
    }
    // Case-insensitive ASCII match against the sorted table.
    let lowered = after_dot.to_ascii_lowercase();
    BINARY_EXTENSIONS.binary_search(&lowered.as_str()).is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn binary_extensions_list_is_sorted() {
        // Enforce the sort invariant `has_binary_extension` relies on.
        for window in BINARY_EXTENSIONS.windows(2) {
            assert!(
                window[0] < window[1],
                "BINARY_EXTENSIONS not sorted: {:?} >= {:?}",
                window[0],
                window[1],
            );
        }
    }

    #[test]
    fn matches_common_binary_extensions() {
        assert!(has_binary_extension("foo.exe"));
        assert!(has_binary_extension("image.png"));
        assert!(has_binary_extension("/abs/path/video.mp4"));
        assert!(has_binary_extension("archive.tar.gz")); // rightmost ext wins
        assert!(has_binary_extension("lib.dylib"));
        assert!(has_binary_extension("compiled.wasm"));
    }

    #[test]
    fn is_case_insensitive() {
        assert!(has_binary_extension("Foo.PNG"));
        assert!(has_binary_extension("BAR.Jpeg"));
        assert!(has_binary_extension("baz.EXE"));
    }

    #[test]
    fn rejects_text_extensions() {
        assert!(!has_binary_extension("main.rs"));
        assert!(!has_binary_extension("README.md"));
        assert!(!has_binary_extension("config.toml"));
        assert!(!has_binary_extension("data.json"));
        // PDF is deliberately excluded — agents may want to inspect.
        assert!(!has_binary_extension("manual.pdf"));
    }

    #[test]
    fn handles_edge_cases() {
        // No extension.
        assert!(!has_binary_extension("Makefile"));
        assert!(!has_binary_extension(""));
        assert!(!has_binary_extension("no_extension_here"));
        // Dotfile with no further extension.
        assert!(!has_binary_extension(".gitignore"));
        // Dot in directory but no extension on basename.
        assert!(!has_binary_extension("path.to/file"));
        assert!(!has_binary_extension("path.to\\file"));
        // Trailing dot — `path[dot..]` would be ".", not in table.
        assert!(!has_binary_extension("weird."));
    }

    #[test]
    fn matches_via_binary_search() {
        // Spot-check a few categories to confirm the sorted-slice
        // lookup works for entries across the alphabet.
        for ext in [".png", ".mp4", ".zip", ".exe", ".sqlite3", ".woff2"] {
            let path = format!("file{ext}");
            assert!(
                has_binary_extension(&path),
                "expected {ext} to be recognized",
            );
        }
    }
}
