//! F21-02 — the NO-CHANNEL canary, inverted.
//!
//! Phase 21 graded F21-02 ("nested children cannot exceed parent depth,
//! fan-out, concurrency, token, cost, or time reservations") NOT MET three
//! times, on the ground that it was VACUOUSLY true: no shipped surface carried
//! a child-fillable budget field, so the property held because nothing could
//! ask, not because anything refused. The corpus's canary at the time asserted
//! that absence — it went red the day a request channel appeared.
//!
//! That canary was the right instrument for the state it was written in and the
//! wrong one to keep. Its success condition was "nobody ever wires the seam",
//! which is green against an untouched tree and is directly opposed to the
//! sub-allocation capability the fleet-supervision work needs.
//!
//! This file inverts it. It asserts the channel EXISTS, is reachable from a
//! delegating actor, and is resolved by intersection at the spawn seam. It goes
//! red if F21-02 ever reverts to being satisfied by absence — which is the one
//! failure mode this phase's history says will actually happen.
//!
//! Every assertion reads PRODUCTION source under `crates/*/src`. Nothing here
//! greps a file the test itself writes, and none of it was green before the
//! sub-allocation seam landed: `sub_budget_narrowed` did not exist.

use std::path::{Path, PathBuf};

fn workspace_root() -> PathBuf {
    // CARGO_MANIFEST_DIR is <root>/crates/wcore-agent.
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("wcore-agent lives two levels below the workspace root")
        .to_path_buf()
}

/// Production `.rs` files only — never `tests/`, `benches/` or `examples/`, so
/// a fixture can never satisfy a claim about the shipped product.
fn production_sources() -> Vec<(PathBuf, String)> {
    fn walk(dir: &Path, out: &mut Vec<(PathBuf, String)>) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                walk(&path, out);
            } else if path.extension().is_some_and(|ext| ext == "rs")
                && let Ok(text) = std::fs::read_to_string(&path)
            {
                out.push((path, text));
            }
        }
    }

    let crates = workspace_root().join("crates");
    let mut out = Vec::new();
    for entry in std::fs::read_dir(&crates)
        .expect("crates/ is readable")
        .flatten()
    {
        let src = entry.path().join("src");
        if src.is_dir() {
            walk(&src, &mut out);
        }
    }
    assert!(
        out.len() > 100,
        "the production source scan collected {} files, which is too few to be \
         a real crawl — a broken walk would make every assertion below vacuous",
        out.len()
    );
    out
}

fn files_containing(needle: &str) -> Vec<String> {
    production_sources()
        .into_iter()
        .filter(|(_, text)| text.contains(needle))
        .map(|(path, _)| {
            path.strip_prefix(workspace_root())
                .unwrap_or(&path)
                .display()
                .to_string()
        })
        .collect()
}

/// The seam exists in production, not only in a test fixture.
///
/// RED WHEN: `sub_budget_narrowed` is deleted, or its only production caller is
/// reverted to `sub_budget(None)` — i.e. exactly the state that made F21-02
/// vacuous. The prior canary asserted the opposite of this line.
#[test]
fn f21_02_a_production_caller_forwards_a_requested_envelope_into_the_spawn_seam() {
    let definition = files_containing("pub fn sub_budget_narrowed");
    assert_eq!(
        definition.len(),
        1,
        "expected exactly one production definition of the narrowing seam, found {definition:?}"
    );

    let callers: Vec<String> = files_containing("sub_budget_narrowed(")
        .into_iter()
        .filter(|file| !file.contains("wcore-budget"))
        .collect();
    assert!(
        !callers.is_empty(),
        "NO PRODUCTION CALLER forwards a requested envelope into sub_budget_narrowed. \
         F21-02 has reverted to holding by the ABSENCE of a request channel: the \
         property would be satisfied because nothing can ask, which is the exact \
         vacuity this phase was graded NOT MET for three times."
    );
    assert!(
        callers.iter().any(|file| file.contains("wcore-agent")),
        "the spawn seam in wcore-agent must be the caller; found {callers:?}"
    );
}

/// The request is reachable by a delegating actor, not only by trusted
/// in-process orchestration.
///
/// This is the difference between "refused" and "never requested". If the field
/// is dropped from the Delegate schema, no delegated actor can express a budget
/// ask and every budget row in the phase corpus silently returns to being
/// NO-CHANNEL — passing, and proving nothing.
///
/// RED WHEN: `budget` leaves `DelegateTool::input_schema`, or `ForkOverrides`
/// loses the field that carries it to the seam.
#[test]
fn f21_02_the_budget_request_is_expressible_by_a_delegating_actor() {
    let root = workspace_root();

    let delegate = std::fs::read_to_string(root.join("crates/wcore-tools/src/delegate.rs"))
        .expect("the Delegate tool is a production file");
    let schema_start = delegate
        .find("fn input_schema")
        .expect("DelegateTool declares an input schema");
    let schema = &delegate[schema_start..];
    assert!(
        schema.contains("\"budget\""),
        "the Delegate tool no longer advertises a `budget` object, so no delegated \
         actor can request an envelope and F21-02 is back to vacuous"
    );
    for dimension in [
        "max_wall_time_secs",
        "max_tool_runtime_secs",
        "max_processes",
        "max_agent_depth",
        "max_tokens_in",
        "max_tokens_out",
        "max_cost_usd",
    ] {
        assert!(
            schema.contains(dimension),
            "the Delegate budget object no longer expresses `{dimension}`; that \
             dimension has silently returned to NOT-EXPRESSIBLE"
        );
    }

    let spawner_types = std::fs::read_to_string(root.join("crates/wcore-types/src/spawner.rs"))
        .expect("the spawn request types are a production file");
    assert!(
        spawner_types.contains("pub struct ChildBudgetRequest"),
        "ChildBudgetRequest is gone — the spawn request types carry no budget \
         field again, which is the precise finding that kept F21-02 open"
    );
    assert!(
        spawner_types.contains("pub budget: Option<ChildBudgetRequest>"),
        "ForkOverrides no longer carries the budget request, so a Delegate ask \
         cannot reach the spawn seam even if the schema still advertises it"
    );
}

/// The resolution is an INTERSECTION, so the channel above cannot be used to
/// widen. Adding a request channel without the clamp is the one change that
/// would make this phase's history strictly worse.
///
/// RED WHEN: the seam stops intersecting with the parent's effective caps and
/// starts installing the requested envelope verbatim.
#[test]
fn f21_02_the_requested_envelope_is_resolved_by_intersection_with_the_parent() {
    let execution =
        std::fs::read_to_string(workspace_root().join("crates/wcore-budget/src/execution.rs"))
            .expect("the execution budget is a production file");
    let seam_start = execution
        .find("pub fn sub_budget_narrowed")
        .expect("the narrowing seam exists");
    let seam_end = execution[seam_start..]
        .find("\n    /// Build a child view.")
        .map(|offset| seam_start + offset)
        .unwrap_or(execution.len());
    let seam = &execution[seam_start..seam_end];

    assert!(
        seam.contains("intersect_execution_budget"),
        "sub_budget_narrowed no longer intersects the request with the parent's \
         caps. A child-fillable budget channel WITHOUT the clamp converts a \
         currently-impossible widening into a possible one."
    );
    assert!(
        seam.contains("effective_budget()"),
        "the intersection must be taken against effective_budget() — the fold \
         over the WHOLE ancestor chain. Intersecting against the parent's leaf \
         caps alone lets a grandchild climb back out through an intermediate \
         that names a looser cap."
    );
}
