//! FerroxLabs/wayland#1171 — a `ToolSearch` hydration must not rewrite the
//! `tools[]` prefix the provider actually sees.
//!
//! Split out of #559, where the tools array was the last remaining component
//! that changed mid-conversation. Measured on a real leader session, the turn
//! after `ToolSearch` ran re-serialized the WHOLE array (`tools_sha` changed
//! once, then stayed constant), pinning `cache_read` for that turn: the three
//! hydrated tools landed mid-array instead of appending, so every cached byte
//! after the first entry was discarded and re-billed.
//!
//! This test drives the engine's own per-turn tool pipeline
//! (`AgentEngine::apply_tool_deferral`, engine.rs) through the same public
//! `wcore_tools::registry` helpers it calls, then serializes the result with
//! the real Anthropic wire encoder. The assertion is on the SERIALIZED bytes,
//! not on an internal `Vec` order.

use wcore_types::tool::ToolDef;

fn def(name: &str) -> ToolDef {
    ToolDef {
        name: name.to_string(),
        description: format!("{name} does a thing"),
        input_schema: serde_json::json!({
            "type": "object",
            "properties": { "arg": { "type": "string" } }
        }),
        deferred: false,
        server: None,
    }
}

/// Registry order for the measured session: registration order, which is a
/// deterministic statement sequence in `bootstrap.rs`.
fn registry_defs() -> Vec<ToolDef> {
    [
        "Bash",
        "Delegate",
        "Edit",
        "Forge",
        "Glob",
        "Grep",
        "Read",
        "Spawn",
        "ToolSearch",
        "Workflow",
        "Write",
    ]
    .iter()
    .map(|n| def(n))
    .collect()
}

fn hot_allowlist() -> Vec<String> {
    ["Bash", "Edit", "Forge", "Glob", "Grep", "Read", "Write"]
        .iter()
        .map(|s| s.to_string())
        .collect()
}

/// One turn of `AgentEngine::apply_tool_deferral`, built from the public
/// helpers the engine itself calls.
fn turn(hydrated: &[&str]) -> Vec<ToolDef> {
    let hydrated: Vec<String> = hydrated.iter().map(|s| s.to_string()).collect();
    let mut defs = registry_defs();
    wcore_tools::registry::apply_cold_deferral(&mut defs, &hot_allowlist());
    wcore_tools::registry::admit_hydrated_tools(&mut defs, &hydrated);
    wcore_tools::registry::fold_deferred_into_catalog(defs, 1000)
}

fn names(wire: &[serde_json::Value]) -> Vec<String> {
    wire.iter()
        .map(|t| t["name"].as_str().unwrap_or_default().to_string())
        .collect()
}

#[test]
fn hydration_appends_and_leaves_the_tools_prefix_byte_identical() {
    let turn1 = wcore_providers::anthropic_shared::build_tools(&turn(&[]));
    let turn2 =
        wcore_providers::anthropic_shared::build_tools(&turn(&["Delegate", "Spawn", "Workflow"]));

    // Sanity: this reproduces the measured shape — 8 tools, then 11.
    assert_eq!(turn1.len(), 8, "turn 1 wire tools: {:?}", names(&turn1));
    assert_eq!(turn2.len(), 11, "turn 2 wire tools: {:?}", names(&turn2));

    // The one entry that legitimately changes on hydration is `ToolSearch`:
    // its description carries the deferred-tool catalog line, which shrinks
    // by exactly the hydrated names. It must therefore sit at the TAIL, so a
    // hydration never rewrites bytes ahead of it.
    assert_eq!(
        names(&turn1).last().map(String::as_str),
        Some("ToolSearch"),
        "turn 1 wire tools: {:?}",
        names(&turn1)
    );
    assert_eq!(
        names(&turn2).last().map(String::as_str),
        Some("ToolSearch"),
        "turn 2 wire tools: {:?}",
        names(&turn2)
    );

    // The stable base: every turn-1 entry except the volatile ToolSearch tail.
    let base = turn1.len() - 1;
    let prefix1 = serde_json::to_string(&turn1[..base]).unwrap();
    let prefix2 = serde_json::to_string(&turn2[..base]).unwrap();
    assert_eq!(
        prefix1,
        prefix2,
        "hydration rewrote the cached tools[] prefix\n turn 1: {:?}\n turn 2: {:?}",
        names(&turn1),
        names(&turn2)
    );

    // …and the hydrated tools land in FIRST-HYDRATION order at the tail,
    // ahead of ToolSearch, so the NEXT hydration appends again rather than
    // inserting between them.
    assert_eq!(
        &names(&turn2)[base..turn2.len() - 1],
        &[
            "Delegate".to_string(),
            "Spawn".to_string(),
            "Workflow".to_string()
        ],
        "turn 2 wire tools: {:?}",
        names(&turn2)
    );
}

#[test]
fn a_second_hydration_appends_after_the_first() {
    let turn2 = wcore_providers::anthropic_shared::build_tools(&turn(&["Workflow"]));
    let turn3 = wcore_providers::anthropic_shared::build_tools(&turn(&["Workflow", "Delegate"]));

    // Hydration order, not name order: `Workflow` hydrated first, so it holds
    // its slot and `Delegate` appends after it.
    let base = turn2.len() - 1;
    assert_eq!(
        serde_json::to_string(&turn2[..base]).unwrap(),
        serde_json::to_string(&turn3[..base]).unwrap(),
        "the second hydration rewrote the prefix the first one established\n \
         turn 2: {:?}\n turn 3: {:?}",
        names(&turn2),
        names(&turn3)
    );
    assert_eq!(
        names(&turn3)[base],
        "Delegate",
        "turn 3 wire tools: {:?}",
        names(&turn3)
    );
}
