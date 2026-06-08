//! Per-language symbol extractors and the shared dispatcher.

pub mod rust;
pub mod typescript;

use crate::types::{Language, Symbol};

/// Dispatch to the right extractor for `language`. Returns `(symbols, imports)`.
/// For `Language::Other`, returns `(empty, empty)` — the indexer is expected
/// to record `first_meaningful_line` separately.
pub fn extract(language: Language, source: &str) -> (Vec<Symbol>, Vec<String>) {
    match language {
        Language::Rust => {
            let r = rust::extract_rust(source);
            (r.symbols, r.imports)
        }
        Language::TypeScript | Language::JavaScript => {
            let r = typescript::extract_typescript(source);
            (r.symbols, r.imports)
        }
        Language::Other => (Vec::new(), Vec::new()),
    }
}

/// Strip C-style line comments (`//…`) and block comments (`/* … */`)
/// from `source`, preserving the original line count and approximate
/// column positions so line-number reporting stays accurate.
///
/// String-literal awareness is intentionally NOT implemented — the
/// design contract specifies a "light" extractor, and false positives
/// inside string literals (e.g. a Rust file with the literal `"fn foo"`
/// in a doc test) are acceptable in exchange for the simplicity. The
/// fixture-index integration test asserts the behaviour on real code
/// shapes.
pub(crate) fn strip_comments_rust_style(source: &str) -> String {
    let mut out = String::with_capacity(source.len());
    let mut chars = source.chars().peekable();
    let mut in_block = false;
    while let Some(c) = chars.next() {
        if in_block {
            if c == '*' && chars.peek() == Some(&'/') {
                chars.next();
                out.push_str("  "); // preserve column width loosely
                in_block = false;
            } else if c == '\n' {
                out.push('\n');
            } else {
                out.push(' ');
            }
            continue;
        }
        if c == '/' && chars.peek() == Some(&'/') {
            // line comment — consume to next newline
            for nc in chars.by_ref() {
                if nc == '\n' {
                    out.push('\n');
                    break;
                }
            }
            continue;
        }
        if c == '/' && chars.peek() == Some(&'*') {
            chars.next();
            in_block = true;
            out.push_str("  ");
            continue;
        }
        out.push(c);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strip_comments_preserves_line_count() {
        let src = "fn a() {} // comment\n/* block\n   spans */ fn b() {}\n";
        let stripped = strip_comments_rust_style(src);
        assert_eq!(stripped.lines().count(), src.lines().count());
    }
}
