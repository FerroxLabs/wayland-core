//! #998 — the three states of an MCP server's per-tool selection must stay
//! distinguishable all the way from a config file to the registration decision.
//!
//! The Wayland desktop MCP Library exposes a switch per advertised tool. Core
//! had no tool dimension at all, so the switch appeared to work and did
//! nothing. The field that carries it is `Option<Vec<String>>`, and that type
//! is load-bearing:
//!
//!   * `None`      — no selection was made ⇒ every advertised tool is enabled;
//!   * `Some([a])` — a selection was made ⇒ only `a` is enabled;
//!   * `Some([])`  — "Disable all" ⇒ NO tool is enabled.
//!
//! The empty case is the one that silently inverts the feature. A `Vec<String>`
//! (or any decoding that folds empty into absent) cannot tell "the operator
//! disabled every tool" apart from "the operator expressed no preference", and
//! would enable everything at exactly the moment the operator asked for
//! nothing. Each state is asserted separately here for that reason.

use wcore_config::config::{McpServerConfig, tool_allows};

fn parse(extra: &str) -> McpServerConfig {
    let toml = format!("transport = \"stdio\"\ncommand = \"x\"\n{extra}");
    toml::from_str(&toml).expect("a valid MCP server config")
}

#[test]
fn an_absent_selection_deserializes_to_none_not_to_an_empty_list() {
    let config = parse("");
    assert!(
        config.allowed_tools.is_none(),
        "a config that predates #998 must decode as NO selection"
    );
    assert!(
        config.allows_tool("anything"),
        "and must therefore keep enabling every advertised tool"
    );
}

#[test]
fn a_named_selection_enables_only_what_it_names() {
    let config = parse(r#"allowed_tools = ["inventory_reserve"]"#);
    assert_eq!(
        config.allowed_tools,
        Some(vec!["inventory_reserve".to_string()])
    );
    assert!(config.allows_tool("inventory_reserve"));
    assert!(
        !config.allows_tool("payroll_wipe"),
        "within a declared list, silence means OFF"
    );
}

#[test]
fn an_empty_selection_is_disable_all_and_never_collapses_into_absent() {
    let config = parse("allowed_tools = []");
    assert_eq!(
        config.allowed_tools,
        Some(Vec::new()),
        "an empty array must survive as a real, empty selection"
    );
    assert!(
        !config.allows_tool("inventory_reserve"),
        "\"Disable all\" must disable all — folding empty into absent would \
         enable every tool the operator just switched off"
    );
}

/// Desktop's own model spells this field `allowedTools`. Accepting only the
/// snake_case spelling would drop the selection into `None` — the silent
/// inversion above, arriving through a naming mismatch instead of a type one.
#[test]
fn the_desktop_camel_case_spelling_is_accepted_losslessly() {
    assert_eq!(parse("allowedTools = []").allowed_tools, Some(Vec::new()));
    assert_eq!(
        parse(r#"allowedTools = ["a"]"#).allowed_tools,
        Some(vec!["a".to_string()])
    );
    assert!(!parse("allowedTools = []").allows_tool("a"));
}

/// The shared predicate, exercised directly: both registration seams in
/// `wcore-mcp` route through it, so its polarity is the whole feature.
#[test]
fn the_shared_predicate_has_the_allowlist_polarity() {
    assert!(tool_allows(None, "anything"));
    let named = vec!["a".to_string()];
    assert!(tool_allows(Some(&named), "a"));
    assert!(!tool_allows(Some(&named), "b"));
    assert!(!tool_allows(Some(&[]), "a"));
}
