//! Light symbol extractor for TypeScript and JavaScript.
//!
//! Strategy mirrors the Rust extractor: pre-strip comments, then scan
//! lines with compiled regex patterns. Covers function/class/interface/
//! type and the three `export` shapes (default / named decl / re-export).

use std::sync::OnceLock;

use regex::Regex;

use crate::extractor::strip_comments_rust_style;
use crate::types::{Symbol, SymbolKind};

/// Result of one extractor pass.
#[derive(Debug, Default)]
pub struct ExtractResult {
    /// Extracted symbols.
    pub symbols: Vec<Symbol>,
    /// Import paths captured from `import … from '…';` lines.
    pub imports: Vec<String>,
}

struct Patterns {
    func: Regex,
    func_export: Regex,
    class_: Regex,
    interface_: Regex,
    type_: Regex,
    export_const: Regex,
    export_named: Regex,
    import_: Regex,
}

/// Wave RB: helper that wraps `Regex::new(...).expect("static regex")`
/// with a single colocated SAFETY rationale. Every caller passes a
/// compile-checked string literal that the crate's regex unit tests
/// exercise, so failure here would be a checked-in-source bug.
fn re(pat: &'static str) -> Regex {
    // SAFETY: static string literal, exercised by the regex unit tests.
    Regex::new(pat).expect("compile-checked static regex")
}

fn patterns() -> &'static Patterns {
    static P: OnceLock<Patterns> = OnceLock::new();
    P.get_or_init(|| Patterns {
        // `function foo`, `async function foo`, `export function foo`,
        // `export async function foo`, `export default function foo`,
        // `export default async function foo`.
        func: re(r"^\s*(?:export\s+(?:default\s+)?)?(?:async\s+)?function\s*\*?\s*([A-Za-z_$][A-Za-z0-9_$]*)"),
        // `export default function () {}` — anonymous default; surface as `default`.
        func_export: re(r"^\s*export\s+default\s+(?:async\s+)?function\b"),
        class_: re(r"^\s*(?:export\s+(?:default\s+)?)?(?:abstract\s+)?class\s+([A-Za-z_$][A-Za-z0-9_$]*)"),
        interface_: re(r"^\s*(?:export\s+)?interface\s+([A-Za-z_$][A-Za-z0-9_$]*)"),
        type_: re(r"^\s*(?:export\s+)?type\s+([A-Za-z_$][A-Za-z0-9_$]*)"),
        // `export const X`, `export let X`, `export var X`.
        export_const: re(r"^\s*export\s+(?:const|let|var)\s+([A-Za-z_$][A-Za-z0-9_$]*)"),
        // `export { A, B };` and `export { A } from '…';`
        export_named: re(r"^\s*export\s*\{([^}]+)\}"),
        // `import … from '…';`
        import_: re(r#"^\s*import\s+.*?from\s+['"]([^'"]+)['"]"#),
    })
}

/// Extract TypeScript/JavaScript symbols and imports from `source`.
pub fn extract_typescript(source: &str) -> ExtractResult {
    let mut out = ExtractResult::default();
    if source.is_empty() {
        return out;
    }
    let stripped = strip_comments_rust_style(source);
    let p = patterns();

    for (idx, line) in stripped.lines().enumerate() {
        let lineno = idx + 1;

        if let Some(c) = p.func.captures(line) {
            out.symbols.push(Symbol {
                kind: SymbolKind::Function,
                name: c[1].to_string(),
                line: lineno,
            });
            continue;
        }
        if p.func_export.is_match(line) {
            out.symbols.push(Symbol {
                kind: SymbolKind::Function,
                name: "default".to_string(),
                line: lineno,
            });
            continue;
        }
        if let Some(c) = p.class_.captures(line) {
            out.symbols.push(Symbol {
                kind: SymbolKind::Class,
                name: c[1].to_string(),
                line: lineno,
            });
            continue;
        }
        if let Some(c) = p.interface_.captures(line) {
            out.symbols.push(Symbol {
                kind: SymbolKind::Interface,
                name: c[1].to_string(),
                line: lineno,
            });
            continue;
        }
        if let Some(c) = p.type_.captures(line) {
            out.symbols.push(Symbol {
                kind: SymbolKind::TypeAlias,
                name: c[1].to_string(),
                line: lineno,
            });
            continue;
        }
        if let Some(c) = p.export_const.captures(line) {
            out.symbols.push(Symbol {
                kind: SymbolKind::Export,
                name: c[1].to_string(),
                line: lineno,
            });
            continue;
        }
        if let Some(c) = p.export_named.captures(line) {
            let inner = c[1].trim().to_string();
            out.symbols.push(Symbol {
                kind: SymbolKind::Export,
                name: inner,
                line: lineno,
            });
            continue;
        }
        if let Some(c) = p.import_.captures(line) {
            out.imports.push(c[1].to_string());
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
    fn extracts_function_declarations() {
        let src = "\
function plain() {}
async function asyncFn(x) { return x; }
export function exported() {}
export default function defaultExport() {}
";
        let r = extract_typescript(src);
        let fns = names(&r.symbols, Function);
        assert!(fns.contains(&"plain"));
        assert!(fns.contains(&"asyncFn"));
        assert!(fns.contains(&"exported"));
        assert!(fns.contains(&"defaultExport"));
    }

    #[test]
    fn extracts_anonymous_default_function_as_default() {
        // `export default function () { … }` has no name; the extractor
        // should surface it as Function with name "default" (via the
        // func_export fallback regex). Without this test, the func_export
        // branch could be deleted accidentally and the named cases above
        // would still all pass.
        let src = "export default function () { return 1; }\n";
        let r = extract_typescript(src);
        let fns = names(&r.symbols, Function);
        assert!(
            fns.contains(&"default"),
            "anonymous default function not surfaced: fns = {fns:?}"
        );
    }

    #[test]
    fn extracts_classes_interfaces_type_aliases() {
        let src = "\
class Widget {}
export class App {}
interface Options { x: number }
export interface Public { y: string }
type Callback = (x: number) => void;
export type Result<T> = { ok: T };
";
        let r = extract_typescript(src);
        assert!(names(&r.symbols, Class).contains(&"Widget"));
        assert!(names(&r.symbols, Class).contains(&"App"));
        assert!(names(&r.symbols, Interface).contains(&"Options"));
        assert!(names(&r.symbols, Interface).contains(&"Public"));
        assert!(names(&r.symbols, TypeAlias).contains(&"Callback"));
        assert!(names(&r.symbols, TypeAlias).contains(&"Result"));
    }

    #[test]
    fn captures_export_const_and_named_reexports() {
        let src = "\
export const PI = 3.14;
export let counter = 0;
export { Widget, App };
export { default as MainApp } from './main';
";
        let r = extract_typescript(src);
        let exports = names(&r.symbols, Export);
        assert!(exports.iter().any(|e| e.contains("PI")));
        assert!(exports.iter().any(|e| e.contains("counter")));
        assert!(
            exports
                .iter()
                .any(|e| e.contains("Widget") && e.contains("App"))
        );
        assert!(
            exports
                .iter()
                .any(|e| e.contains("MainApp") || e.contains("default as MainApp"))
        );
    }

    #[test]
    fn captures_import_statements() {
        let src = "\
import { readFile } from 'node:fs/promises';
import * as path from 'node:path';
import React from 'react';
";
        let r = extract_typescript(src);
        assert!(r.imports.iter().any(|i| i.contains("node:fs/promises")));
        assert!(r.imports.iter().any(|i| i.contains("node:path")));
        assert!(r.imports.iter().any(|i| i.contains("react")));
    }

    #[test]
    fn ignores_keywords_in_comments() {
        let src = "\
// function fake() {}
/* class Fake {} */
function real() {}
";
        let r = extract_typescript(src);
        let fns = names(&r.symbols, Function);
        assert_eq!(fns, vec!["real"]);
        assert!(names(&r.symbols, Class).is_empty());
    }
}
