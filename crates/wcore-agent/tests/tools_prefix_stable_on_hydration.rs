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


// ---------------------------------------------------------------------------
// FerroxLabs/wayland#1209 — the same guarantee with the catalog fold OFF.
//
// `builtin_tools.defer_cold.catalog = false` is a documented knob that
// restores per-tool stub entries. Turning it off opts out of a TOKEN
// optimisation; it must not silently opt out of prompt-cache stability.
// Before the `sink_deferred_to_tail` pass it did: the stubs stayed at their
// registry slots interleaved with the hot tools, and `admit_hydrated_tools`
// pulled each hydrated one out of mid-array, shifting everything after it.
// ---------------------------------------------------------------------------

/// One turn of `AgentEngine::apply_tool_deferral` in either catalog mode,
/// built from the same public helpers the engine calls, in the same order.
fn turn_in_mode(hydrated: &[&str], catalog: bool) -> Vec<ToolDef> {
    let hydrated: Vec<String> = hydrated.iter().map(|s| s.to_string()).collect();
    let mut defs = registry_defs();
    wcore_tools::registry::apply_cold_deferral(&mut defs, &hot_allowlist());
    wcore_tools::registry::sink_deferred_to_tail(&mut defs);
    wcore_tools::registry::admit_hydrated_tools(&mut defs, &hydrated);
    if catalog {
        wcore_tools::registry::fold_deferred_into_catalog(defs, 1000)
    } else {
        defs
    }
}

/// The stable base both modes must preserve: every HOT tool, i.e. everything
/// ahead of the mutable region (the stub block in stub mode, the
/// catalog-carrying `ToolSearch` entry in catalog mode).
const HOT_WIRE_PREFIX: usize = 8;

fn wire(hydrated: &[&str], catalog: bool) -> Vec<serde_json::Value> {
    wcore_providers::anthropic_shared::build_tools(&turn_in_mode(hydrated, catalog))
}

fn first_differing_index(a: &[serde_json::Value], b: &[serde_json::Value]) -> Option<usize> {
    (0..a.len().min(b.len())).find(|&i| a[i] != b[i])
}

#[test]
fn hydration_leaves_the_tools_prefix_byte_identical_in_both_catalog_modes() {
    // --- Arm under test: catalog = false (per-tool stubs on the wire). ------
    let stub_turn1 = wire(&[], false);
    let stub_turn2 = wire(&["Delegate", "Spawn", "Workflow"], false);

    // The arm is genuinely stub mode, not catalog mode wearing its name: all
    // eleven tools are on the wire and the cold ones are `(Deferred)` stubs.
    assert_eq!(
        stub_turn1.len(),
        11,
        "stub mode must keep every tool on the wire: {:?}",
        names(&stub_turn1)
    );
    assert!(
        stub_turn1
            .iter()
            .any(|t| t["description"].as_str().unwrap_or_default().starts_with("(Deferred)")),
        "stub mode must emit per-tool stubs: {:?}",
        names(&stub_turn1)
    );

    // The measured defect, verbatim from wayland#1209: the arrays differed at
    // wire index 1 (`Delegate` -> `Edit`), so every cached byte from index 1
    // onward was re-billed on the hydration turn.
    assert_eq!(
        names(&stub_turn1)[1],
        names(&stub_turn2)[1],
        "wayland#1209: the hydration turn rewrote wire index 1\n \
         turn 1: {:?}\n turn 2: {:?}",
        names(&stub_turn1),
        names(&stub_turn2)
    );
    assert_eq!(
        serde_json::to_string(&stub_turn1[..HOT_WIRE_PREFIX]).unwrap(),
        serde_json::to_string(&stub_turn2[..HOT_WIRE_PREFIX]).unwrap(),
        "wayland#1209: the hydration turn rewrote the cached tools[] prefix\n \
         turn 1: {:?}\n turn 2: {:?}",
        names(&stub_turn1),
        names(&stub_turn2)
    );
    assert!(
        first_differing_index(&stub_turn1, &stub_turn2)
            .is_none_or(|i| i >= HOT_WIRE_PREFIX),
        "first differing wire index must be inside the tail-mutable region, got {:?}\n \
         turn 1: {:?}\n turn 2: {:?}",
        first_differing_index(&stub_turn1, &stub_turn2),
        names(&stub_turn1),
        names(&stub_turn2)
    );

    // A PARTIAL hydration is the harder case: two stubs stay behind, so the
    // admitted one cannot simply be "the whole tail".
    let stub_partial = wire(&["Spawn"], false);
    assert_eq!(
        serde_json::to_string(&stub_turn1[..HOT_WIRE_PREFIX]).unwrap(),
        serde_json::to_string(&stub_partial[..HOT_WIRE_PREFIX]).unwrap(),
        "a single-tool hydration rewrote the cached prefix: {:?}",
        names(&stub_partial)
    );
    assert_eq!(
        names(&stub_partial).last().map(String::as_str),
        Some("Spawn"),
        "the hydrated tool must append at the tail: {:?}",
        names(&stub_partial)
    );

    // --- Positive control: catalog = true, the path #1171 already fixed. ---
    // It holds the same property today, before and after this change, which
    // is what proves the assertion above is measuring the mode and not the
    // harness.
    let cat_turn1 = wire(&[], true);
    let cat_turn2 = wire(&["Delegate", "Spawn", "Workflow"], true);
    assert_eq!(
        cat_turn1.len(),
        HOT_WIRE_PREFIX,
        "control: catalog mode folds the stubs away: {:?}",
        names(&cat_turn1)
    );
    assert_eq!(
        serde_json::to_string(&cat_turn1[..HOT_WIRE_PREFIX - 1]).unwrap(),
        serde_json::to_string(&cat_turn2[..HOT_WIRE_PREFIX - 1]).unwrap(),
        "control arm broke: catalog mode rewrote its own prefix\n \
         turn 1: {:?}\n turn 2: {:?}",
        names(&cat_turn1),
        names(&cat_turn2)
    );
}

/// The `sink_deferred_to_tail` pass must be invisible to catalog mode — the
/// fold deletes exactly the defs the sink moved. Pinning the catalog-mode wire
/// bytes against the stub-mode HOT prefix proves both modes agree on the one
/// ordering discipline rather than each having its own.
#[test]
fn both_modes_agree_on_the_hot_prefix() {
    let stub = wire(&[], false);
    let catalog = wire(&[], true);
    // Catalog mode carries the deferred inventory on `ToolSearch`, so it moves
    // that ONE entry to the tail (wayland#1171); stub mode has no carrier and
    // leaves it in place. Modulo that documented carrier move, both modes emit
    // the same hot tools in the same registry order — one discipline, not two.
    let mut stub_hot = names(&stub)[..HOT_WIRE_PREFIX].to_vec();
    let carrier = stub_hot
        .iter()
        .position(|n| n == "ToolSearch")
        .expect("ToolSearch is never deferred, so it is in the hot prefix");
    let carrier = stub_hot.remove(carrier);
    stub_hot.push(carrier);
    assert_eq!(
        names(&catalog),
        stub_hot,
        "the two modes disagree on the hot prefix\n stub: {:?}\n catalog: {:?}",
        names(&stub),
        names(&catalog)
    );
}
