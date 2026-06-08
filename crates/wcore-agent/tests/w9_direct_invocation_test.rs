//! Wave W3 (closes B.1): direct invocation of W9 Curator + PUM at
//! `fire_on_session_end` for CLI-only flows.
//!
//! Background: the W9 plan ships two pipelines that read/write `MemoryApi`:
//!
//! 1. `wcore_skills::curate::Curator` — archives overlapping or stale P4
//!    procedures.
//! 2. `wcore_memory::partition::UserModelInferencer` (PUM) — infers user-model
//!    deltas from per-turn traces and writes them to P5.
//!
//! Both implement the `Hook` trait so a host (AionUI) can register them via
//! `register_rust_hook`. CLI-only flows have no host, so without explicit
//! engine-side invocation the pipelines silently never fire.
//!
//! This test pins the W3 invariant: with `observability.skills_lifecycle = true`,
//! `fire_on_session_end` invokes both pipelines directly against the engine's
//! `MemoryApi` handle, regardless of host hooks.
//!
//! Example: e2e_fixture — this file is the canonical demonstration of the
//! `wcore_agent::test_utils::e2e_fixture::E2eFixture` builder DSL. The W7
//! rewrite collapsed ~40 lines of per-test setup into ~6 lines of fixture
//! chain; the assertions themselves are unchanged.

use serde_json::json;
use wcore_agent::test_utils::e2e_fixture::E2eFixture;
use wcore_memory::v2_types::{AccessToken, ProcedureStatus, Tier};
use wcore_types::llm::LlmEvent;
use wcore_types::message::{FinishReason, StopReason, TokenUsage};

/// Single-turn script with one Grep tool call: enough to produce a
/// `TurnTrace` that PUM can consume (`UserModelInferencer::infer` returns
/// an empty vec on an empty trace slice).
fn one_tool_call_script() -> Vec<LlmEvent> {
    vec![
        LlmEvent::ToolUse {
            id: "call-0".into(),
            name: "Grep".into(),
            input: json!({ "pattern": "no-such-string-w3-direct", "path": "." }),
            extra: None,
        },
        LlmEvent::Done {
            stop_reason: StopReason::ToolUse,
            finish_reason: FinishReason::Stop,
            usage: TokenUsage::default(),
        },
    ]
}

#[tokio::test]
async fn fire_on_session_end_invokes_curator_and_pum_when_gate_on() {
    // Example: e2e_fixture — six-line setup (was ~40 lines hand-rolled).
    let mut fx = E2eFixture::new()
        .with_provider_script(one_tool_call_script())
        .with_skills_lifecycle(true)
        .with_max_turns(1)
        .with_in_memory_backend()
        .build()
        .await;
    fx.seed_overlapping_staged_procedures("auto-w3-overlap-", 5)
        .await;

    // Drive one synthetic turn. `max_turns = 1` causes the engine to fall
    // through to the `MaxTurns` return path on the next loop iteration,
    // which fires `fire_on_session_end` (the W3 invocation site).
    let _ = fx
        .send("invoke curator + pum at session end")
        .await
        .expect("synthetic run should not error");

    // --- Curator assertion -------------------------------------------------
    // Five overlapping staged drafts → at most 2 should remain non-archived;
    // at least 3 must be archived. Mirrors the contract pinned by
    // `wcore-skills/tests/curator_session_end.rs::
    //  curator_archives_overlapping_staged_drafts_keeping_at_most_two_active`.
    let procs = fx.list_procedures(Tier::Project).await;
    let archived_count = procs
        .iter()
        .filter(|p| {
            p.name.starts_with("auto-w3-overlap-") && matches!(p.status, ProcedureStatus::Archived)
        })
        .count();
    assert!(
        archived_count >= 3,
        "Curator should have archived ≥3 of the 5 overlapping drafts at \
         session end; got {archived_count}. Procedures: {:?}",
        procs
            .iter()
            .map(|p| (p.name.clone(), p.status))
            .collect::<Vec<_>>()
    );

    // --- PUM assertion -----------------------------------------------------
    // With `skills_lifecycle = true` and one tool call observed, PUM must
    // have written at least one stable key. W9 emits four
    // (`preferences.tool_order`, `tool_habits.recent_top5`,
    // `language.primary`, `working_hours.local_tz_window`). We assert the
    // weakest invariant — ≥1 — so future inferencer changes don't break
    // this test's intent.
    //
    // P5 reads require `AccessToken::System` (see gate.rs:94 — "P5
    // user_model requires SystemToken"). MainAgent token is denied.
    let user_model = fx.user_model().await;
    assert!(
        !user_model.entries.is_empty(),
        "UserModelInferencer should have written ≥1 user-model entry at \
         session end; got 0"
    );
    // Spot-check one stable key from the W9 contract.
    let has_tool_order = user_model
        .entries
        .iter()
        .any(|e| e.key == "preferences.tool_order");
    assert!(
        has_tool_order,
        "Expected `preferences.tool_order` key written by PUM; got keys \
         {:?}",
        user_model
            .entries
            .iter()
            .map(|e| e.key.clone())
            .collect::<Vec<_>>()
    );

    // Silence "unused import" for AccessToken — it's load-bearing
    // documentation for the SystemToken contract above even though the
    // fixture handles the read internally.
    let _ = AccessToken::System;
}

#[tokio::test]
async fn fire_on_session_end_skips_curator_and_pum_when_gate_off() {
    // Same setup but `skills_lifecycle = false`. The W3 invocation block
    // is gated, so neither Curator nor PUM should run — overlapping
    // drafts stay Staged, user-model stays empty.
    let mut fx = E2eFixture::new()
        .with_provider_script(one_tool_call_script())
        .with_skills_lifecycle(false)
        .with_max_turns(1)
        .with_in_memory_backend()
        .build()
        .await;
    fx.seed_overlapping_staged_procedures("auto-w3-overlap-", 5)
        .await;

    let _ = fx
        .send("gate off — no curator no pum")
        .await
        .expect("synthetic run should not error");

    // All 5 seeded drafts should remain Staged (Curator never ran).
    let procs = fx.list_procedures(Tier::Project).await;
    let still_staged = procs
        .iter()
        .filter(|p| {
            p.name.starts_with("auto-w3-overlap-") && matches!(p.status, ProcedureStatus::Staged)
        })
        .count();
    assert_eq!(
        still_staged, 5,
        "With skills_lifecycle = false, Curator must NOT run at session \
         end; expected 5 staged drafts intact, got {still_staged}"
    );

    // User model should also be empty (PUM never ran).
    let user_model = fx.user_model().await;
    assert!(
        user_model.entries.is_empty(),
        "With skills_lifecycle = false, PUM must NOT run at session end; \
         expected empty user model, got {:?}",
        user_model
            .entries
            .iter()
            .map(|e| e.key.clone())
            .collect::<Vec<_>>()
    );
}
