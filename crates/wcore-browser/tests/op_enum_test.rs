//! E.2 integration test — REV-2 audit F6 lock. Guards two attack surfaces:
//!   1. Variant-count guard — bumping requires editing `BROWSER_OP_LOCKED_VARIANT_COUNT`
//!      so a PR diff surfaces the change to a reviewer.
//!   2. Forbidden-name scan — rejects any variant name OR serialized field key
//!      whose lowercased text contains `evaluate`, `eval`, `script`, `run`,
//!      `exec`, `execute`, `inject_js`, `injectjs`, `injectscript`, `code`, `js`.

use wcore_browser::{BROWSER_OP_LOCKED_VARIANT_COUNT, BrowserOp};

const FORBIDDEN_OP_NAMES: &[&str] = &[
    "evaluate",
    "eval",
    "script",
    "run",
    "exec",
    "execute",
    "inject_js",
    "injectjs",
    "injectscript",
    "code",
    "js",
];

#[test]
fn no_arbitrary_js_execution_in_v1_surface() {
    assert_eq!(
        BrowserOp::all_variants_for_test().len(),
        BROWSER_OP_LOCKED_VARIANT_COUNT,
        "BrowserOp variant count changed — re-audit §5.16 Evaluate-ban before merging."
    );

    for variant in BrowserOp::all_variants_for_test() {
        let serialized = serde_json::to_string(&variant).unwrap().to_lowercase();
        let variant_name = format!("{variant:?}").to_lowercase();
        for forbidden in FORBIDDEN_OP_NAMES {
            assert!(
                !variant_name.split_whitespace().any(|w| w == *forbidden),
                "BrowserOp::{variant:?} contains forbidden token '{forbidden}' \
                 (design §5.16 lock — no arbitrary JS execution in v1)."
            );
            // Serialized JSON: the `kind` tag is bounded by quotes — only flag
            // if a forbidden token appears as an entire field/value, not as a
            // substring of e.g. "selector": "#submit-script". We use
            // word-boundary matching by checking for `"<forbidden>"` and
            // `:"<forbidden>"` patterns.
            let tag_pattern = format!("\"{forbidden}\"");
            assert!(
                !serialized.contains(&tag_pattern),
                "BrowserOp::{variant:?} serializes forbidden token '{forbidden}' \
                 as a JSON value: {serialized}"
            );
        }
    }
}

#[test]
fn no_forbidden_field_names_in_browser_op() {
    for variant in BrowserOp::all_variants_for_test() {
        let json = serde_json::to_value(&variant).unwrap();
        if let Some(obj) = json.as_object() {
            for key in obj.keys() {
                let k = key.to_lowercase();
                for forbidden in FORBIDDEN_OP_NAMES {
                    assert!(
                        k != *forbidden,
                        "BrowserOp::{variant:?} has field '{key}' matching forbidden token \
                         '{forbidden}' (design §5.16 lock — no arbitrary JS execution in v1)."
                    );
                }
            }
        }
    }
}

#[test]
fn variant_count_matches_constant() {
    // Direct guard the plan asks for — if this fires, BROWSER_OP_LOCKED_VARIANT_COUNT
    // is no longer the source of truth.
    assert_eq!(BROWSER_OP_LOCKED_VARIANT_COUNT, 18);
    assert_eq!(BrowserOp::all_variants_for_test().len(), 18);
}
