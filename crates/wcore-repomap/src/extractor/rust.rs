//! Light symbol extractor for Rust.
//!
//! Strategy: pre-strip line/block comments to avoid false hits inside
//! doc comments, then scan each line with compiled regex patterns for
//! the symbol kinds the design contract names (§5.6 line 918).

use std::sync::OnceLock;

use regex::Regex;

use crate::extractor::strip_comments_rust_style;
use crate::types::{Symbol, SymbolKind};

/// Result of one extractor pass.
#[derive(Debug, Default)]
pub struct ExtractResult {
    /// Extracted symbols.
    pub symbols: Vec<Symbol>,
    /// Import lines (plain `use …;`); `pub use` is treated as `SymbolKind::Use`.
    pub imports: Vec<String>,
}

struct Patterns {
    func: Regex,
    struct_: Regex,
    enum_: Regex,
    trait_: Regex,
    impl_: Regex,
    impl_for: Regex,
    mod_: Regex,
    use_pub: Regex,
    use_plain: Regex,
}

/// Wave RB: helper that wraps `Regex::new(...).expect("static regex")`
/// with a single colocated SAFETY rationale. Every caller passes a
/// compile-checked string literal exercised by the crate's regex unit
/// tests, so failure here would be a checked-in-source bug.
fn re(pat: &'static str) -> Regex {
    // SAFETY: static string literal, exercised by the regex unit tests.
    Regex::new(pat).expect("compile-checked static regex")
}

fn patterns() -> &'static Patterns {
    static P: OnceLock<Patterns> = OnceLock::new();
    P.get_or_init(|| Patterns {
        // `fn`, `pub fn`, `pub(crate) fn`, `async fn`, `pub async fn`, etc.
        // `extern "C" fn` — `extern\s+` consumes `extern `, then the ABI
        // literal `"C"` is matched by `\S+\s+`. Edge case: works for any
        // non-space ABI token.
        func: re(r#"^\s*(?:pub(?:\([^)]*\))?\s+)?(?:async\s+|const\s+|unsafe\s+|extern\s+(?:"[^"]*"\s+)?)*fn\s+([A-Za-z_][A-Za-z0-9_]*)"#),
        struct_: re(r"^\s*(?:pub(?:\([^)]*\))?\s+)?struct\s+([A-Za-z_][A-Za-z0-9_]*)"),
        enum_: re(r"^\s*(?:pub(?:\([^)]*\))?\s+)?enum\s+([A-Za-z_][A-Za-z0-9_]*)"),
        trait_: re(r"^\s*(?:pub(?:\([^)]*\))?\s+)?(?:unsafe\s+)?trait\s+([A-Za-z_][A-Za-z0-9_]*)"),
        // `impl Trait for Type { … }` — captured FIRST so the inherent regex doesn't shadow it.
        impl_for: re(r"^\s*(?:unsafe\s+)?impl(?:\s*<[^>]*>)?\s+([A-Za-z_][A-Za-z0-9_:<>, ]*?)\s+for\s+([A-Za-z_][A-Za-z0-9_:<>, ]*)"),
        // Inherent impl `impl<…> Type { … }`.
        impl_: re(r"^\s*(?:unsafe\s+)?impl(?:\s*<[^>]*>)?\s+([A-Za-z_][A-Za-z0-9_:<>, ]*)"),
        mod_: re(r"^\s*(?:pub(?:\([^)]*\))?\s+)?mod\s+([A-Za-z_][A-Za-z0-9_]*)"),
        use_pub: re(r"^\s*pub(?:\([^)]*\))?\s+use\s+(.+?);"),
        use_plain: re(r"^\s*use\s+(.+?);"),
    })
}

/// Extract Rust symbols and imports from `source`.
pub fn extract_rust(source: &str) -> ExtractResult {
    let mut out = ExtractResult::default();
    if source.is_empty() {
        return out;
    }
    let stripped = strip_comments_rust_style(source);
    let p = patterns();

    for (idx, line) in stripped.lines().enumerate() {
        let lineno = idx + 1;

        // Order matters: more specific patterns first so the generic
        // `impl_` doesn't swallow `impl_for`, and `use_pub` is tried
        // before `use_plain`.
        if let Some(c) = p.impl_for.captures(line) {
            let trait_name = c
                .get(1)
                .map(|m| m.as_str().trim().to_string())
                .unwrap_or_default();
            let ty_name = c
                .get(2)
                .map(|m| m.as_str().trim().to_string())
                .unwrap_or_default();
            out.symbols.push(Symbol {
                kind: SymbolKind::Impl,
                name: format!("{trait_name} for {ty_name}"),
                line: lineno,
            });
            continue;
        }
        if let Some(c) = p.func.captures(line) {
            out.symbols.push(Symbol {
                kind: SymbolKind::Function,
                name: c[1].to_string(),
                line: lineno,
            });
            continue;
        }
        if let Some(c) = p.struct_.captures(line) {
            out.symbols.push(Symbol {
                kind: SymbolKind::Struct,
                name: c[1].to_string(),
                line: lineno,
            });
            continue;
        }
        if let Some(c) = p.enum_.captures(line) {
            out.symbols.push(Symbol {
                kind: SymbolKind::Enum,
                name: c[1].to_string(),
                line: lineno,
            });
            continue;
        }
        if let Some(c) = p.trait_.captures(line) {
            out.symbols.push(Symbol {
                kind: SymbolKind::Trait,
                name: c[1].to_string(),
                line: lineno,
            });
            continue;
        }
        if let Some(c) = p.impl_.captures(line) {
            // Inherent impl. Name = the type after `impl<…>`.
            let name = c
                .get(1)
                .map(|m| m.as_str().trim().to_string())
                .unwrap_or_default();
            out.symbols.push(Symbol {
                kind: SymbolKind::Impl,
                name,
                line: lineno,
            });
            continue;
        }
        if let Some(c) = p.mod_.captures(line) {
            out.symbols.push(Symbol {
                kind: SymbolKind::Module,
                name: c[1].to_string(),
                line: lineno,
            });
            continue;
        }
        if let Some(c) = p.use_pub.captures(line) {
            out.symbols.push(Symbol {
                kind: SymbolKind::Use,
                name: c[1].trim().to_string(),
                line: lineno,
            });
            continue;
        }
        if let Some(c) = p.use_plain.captures(line) {
            out.imports.push(c[1].trim().to_string());
            continue;
        }
    }

    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::SymbolKind::*;

    fn names(symbols: &[Symbol], kind: SymbolKind) -> Vec<&str> {
        symbols
            .iter()
            .filter(|s| s.kind == kind)
            .map(|s| s.name.as_str())
            .collect()
    }

    #[test]
    fn extracts_pub_and_priv_fns_with_line_numbers() {
        let src = "\
fn first() {}
pub fn second() {}

pub async fn third(x: i32) -> i32 { x }
";
        let r = extract_rust(src);
        let fns = names(&r.symbols, Function);
        assert_eq!(fns, vec!["first", "second", "third"]);
        let lines: Vec<usize> = r
            .symbols
            .iter()
            .filter(|s| s.kind == Function)
            .map(|s| s.line)
            .collect();
        assert_eq!(lines, vec![1, 2, 4]);
    }

    #[test]
    fn extracts_struct_enum_trait_impl() {
        let src = "\
pub struct Greeter { name: String }
enum Mood { Happy, Sad }
pub trait LlmProvider {}
impl Greeter {}
impl LlmProvider for Greeter {}
";
        let r = extract_rust(src);
        assert_eq!(names(&r.symbols, Struct), vec!["Greeter"]);
        assert_eq!(names(&r.symbols, Enum), vec!["Mood"]);
        assert_eq!(names(&r.symbols, Trait), vec!["LlmProvider"]);
        // Two impls: inherent and trait. Both surface.
        let impls = names(&r.symbols, Impl);
        assert_eq!(impls.len(), 2);
        assert!(impls.contains(&"Greeter"), "impls = {impls:?}");
        assert!(
            impls
                .iter()
                .any(|n| n.contains("LlmProvider") && n.contains("Greeter")),
            "trait impl missing: {impls:?}"
        );
    }

    #[test]
    fn extracts_mod_and_pub_use() {
        let src = "\
mod private;
pub mod public_one;
pub mod public_two { /* inline */ }
pub use crate::inner::Helper;
pub use foo::{Bar, Baz};
";
        let r = extract_rust(src);
        let modules = names(&r.symbols, Module);
        assert!(modules.contains(&"private"));
        assert!(modules.contains(&"public_one"));
        assert!(modules.contains(&"public_two"));
        let uses = names(&r.symbols, Use);
        assert!(uses.iter().any(|u| u.contains("crate::inner::Helper")));
        assert!(uses.iter().any(|u| u.contains("foo::")));
    }

    #[test]
    fn captures_imports_as_lines() {
        let src = "\
use std::path::PathBuf;
use serde::{Serialize, Deserialize};
pub use foo::Bar;
";
        let r = extract_rust(src);
        // `use` lines (non-pub) become imports. `pub use` is a Symbol::Use.
        assert!(r.imports.iter().any(|i| i.contains("std::path::PathBuf")));
        assert!(r.imports.iter().any(|i| i.contains("serde")));
        let uses = names(&r.symbols, Use);
        assert!(uses.iter().any(|u| u.contains("foo::Bar")));
    }

    #[test]
    fn ignores_keywords_inside_line_comments() {
        let src = "\
// fn pretend_fn() {}
fn real_fn() {}
// pub struct Pretend {}
pub struct Real {}
";
        let r = extract_rust(src);
        assert_eq!(names(&r.symbols, Function), vec!["real_fn"]);
        assert_eq!(names(&r.symbols, Struct), vec!["Real"]);
    }

    #[test]
    fn ignores_keywords_inside_block_comments() {
        let src = "\
/* fn pretend() {}
   pub struct Pretend {} */
fn real() {}
";
        let r = extract_rust(src);
        assert_eq!(names(&r.symbols, Function), vec!["real"]);
        assert_eq!(names(&r.symbols, Struct), Vec::<&str>::new());
    }

    #[test]
    fn handles_empty_input() {
        let r = extract_rust("");
        assert!(r.symbols.is_empty());
        assert!(r.imports.is_empty());
    }
}
