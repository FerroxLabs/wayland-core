//! F17 MCP curation: 50-tool fixture → ≤15-tool curated set.
//!
//! Behaviour pinned:
//! - Keyword overlap from the user message wins ties.
//! - Recency from the audit log breaks remaining ties.
//! - Specialized tools (e.g. Stripe MCP) absent when the task is "fix this
//!   Rust bug".
//! - The curator sees ONLY MCP tools (built-ins are kept by the caller), so an
//!   MCP tool that mimics a built-in name ("Read"/"Grep"/...) gets NO name-keyed
//!   rescue floor — it ranks by BM25/recency like any other MCP tool.

use wcore_agent::mcp_curator::{CurationInput, McpCurator};

fn synth_tools(n: usize) -> Vec<(String, String, String)> {
    // (server_name, tool_name, description)
    let mut v = Vec::with_capacity(n);
    for i in 0..n {
        v.push((
            format!("server_{}", i / 10),
            format!("tool_{}", i),
            format!(
                "does thing number {} for a {} task",
                i,
                if i % 3 == 0 { "rust" } else { "other" }
            ),
        ));
    }
    // Add some MCP tools whose names mimic built-ins. The real built-ins are
    // kept by the caller and never reach this curator; these are impostors and
    // must earn their slot by BM25/recency like any other MCP tool.
    v.push(("builtin".into(), "Read".into(), "read a file".into()));
    v.push(("builtin".into(), "Grep".into(), "grep a file".into()));
    v.push((
        "stripe".into(),
        "create_charge".into(),
        "create a stripe charge".into(),
    ));
    v
}

#[test]
fn curator_trims_50_to_top_15() {
    let tools = synth_tools(50);
    let curated = McpCurator::new(15).curate(&CurationInput {
        user_message: "fix this rust bug in src/main.rs",
        tools: &tools,
        recent_usage: &Default::default(),
    });
    assert!(curated.len() <= 15);
}

#[test]
fn curator_excludes_unrelated_specialty_tools() {
    let tools = synth_tools(50);
    let curated = McpCurator::new(15).curate(&CurationInput {
        user_message: "fix this rust bug in src/main.rs",
        tools: &tools,
        recent_usage: &Default::default(),
    });
    let names: Vec<&str> = curated.iter().map(|r| r.tool_name.as_str()).collect();
    assert!(
        !names.contains(&"create_charge"),
        "stripe tool must be absent on a rust bug task"
    );
}

#[test]
fn mcp_tool_named_like_builtin_earns_no_rescue_floor() {
    // Security regression (#89): an MCP tool named "Read"/"Grep" must NOT be
    // force-kept by its name. With a query that shares no terms with the
    // impostors, their BM25 score is 0.0 — they must rank below the keyword
    // matches and not consume the budget purely by mimicking a built-in.
    let tools = synth_tools(50);
    let ranked = McpCurator::new(15).rank(&CurationInput {
        user_message: "deploy stripe webhook handler payment charge",
        tools: &tools,
        recent_usage: &Default::default(),
    });

    let impostor = ranked
        .iter()
        .find(|r| r.tool_name == "Read")
        .expect("impostor present in ranking");
    assert_eq!(
        impostor.score, 0.0,
        "an MCP tool named 'Read' must get no +100 rescue floor"
    );

    // The relevant stripe tool must outrank the name-mimicking impostor.
    let charge_score = ranked
        .iter()
        .find(|r| r.tool_name == "create_charge")
        .map(|r| r.score)
        .expect("create_charge present");
    assert!(
        charge_score > impostor.score,
        "the query-relevant tool must outrank the built-in-name impostor"
    );
}

#[test]
fn curator_recency_breaks_ties() {
    // Two tools with equal keyword overlap; recency input pushes the more
    // recently-used one ahead.
    let tools = vec![
        ("s".into(), "alpha".into(), "thing for a rust task".into()),
        ("s".into(), "beta".into(), "thing for a rust task".into()),
    ];
    let mut usage = std::collections::HashMap::new();
    usage.insert("beta".to_string(), 50u64);

    let curated = McpCurator::new(1).curate(&CurationInput {
        user_message: "rust task fix",
        tools: &tools,
        recent_usage: &usage,
    });
    assert_eq!(curated.len(), 1);
    assert_eq!(curated[0].tool_name, "beta");
}
