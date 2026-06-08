//! F17 MCP curation: 50-tool fixture → ≤15-tool curated set.
//!
//! Behaviour pinned:
//! - Keyword overlap from the user message wins ties.
//! - Recency from the audit log breaks remaining ties.
//! - Specialized tools (e.g. Stripe MCP) absent when the task is "fix this
//!   Rust bug".
//! - Always-present "rescue" tools (Read, Grep, Glob) survive even when
//!   keyword score is zero.

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
    // Add some always-present rescues.
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
fn curator_always_keeps_rescue_tools_when_present() {
    let tools = synth_tools(50);
    let curated = McpCurator::new(15).curate(&CurationInput {
        user_message: "deploy stripe webhook handler",
        tools: &tools,
        recent_usage: &Default::default(),
    });
    let names: Vec<&str> = curated.iter().map(|r| r.tool_name.as_str()).collect();
    assert!(names.contains(&"Read"), "Read is always a rescue tool");
    assert!(names.contains(&"Grep"), "Grep is always a rescue tool");
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
