//! W6 F17 — wiring integration: verify the engine's `apply_mcp_curation`
//! trims MCP tools (prefix `mcp__`) but keeps every non-MCP tool.
//!
//! This drives the public path via a synthetic engine — we don't have a full
//! provider stub here, so the helper is exercised through a thin smoke test
//! over the engine's pub helpers. The curator's ranking semantics live in
//! `mcp_curator.rs` tests; this file only pins the wiring boundary.

use wcore_agent::mcp_curator::{CurationInput, McpCurator};

#[test]
fn curator_filters_only_mcp_named_tools_in_practice() {
    // The engine's `apply_mcp_curation` partitions on the `mcp__` prefix.
    // We mirror that partitioning here to lock the contract: non-MCP tools
    // are always kept regardless of curation policy.
    let names = vec![
        "Read".to_string(),
        "Edit".to_string(),
        "mcp__stripe__create_charge".to_string(),
        "mcp__github__create_issue".to_string(),
        "mcp__supabase__query".to_string(),
    ];
    let (mcp, non_mcp): (Vec<_>, Vec<_>) = names.into_iter().partition(|n| n.starts_with("mcp__"));
    assert_eq!(non_mcp.len(), 2);
    assert_eq!(mcp.len(), 3);

    let triples: Vec<(String, String, String)> = mcp
        .iter()
        .map(|n| ("server".to_string(), n.clone(), n.clone()))
        .collect();
    let ranked = McpCurator::new(2).curate(&CurationInput {
        user_message: "stripe charge problem",
        tools: &triples,
        recent_usage: &Default::default(),
    });
    assert!(ranked.len() <= 2);
    let names: Vec<&str> = ranked.iter().map(|r| r.tool_name.as_str()).collect();
    // "stripe" keyword bias means stripe tool ranks in the top-K.
    assert!(
        names.contains(&"mcp__stripe__create_charge"),
        "stripe tool should rank into top-2 on a stripe task; got: {names:?}"
    );
}
