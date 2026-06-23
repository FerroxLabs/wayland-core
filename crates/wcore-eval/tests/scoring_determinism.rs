//! Property: DefaultScorer::score is deterministic and pure.

use serde_json::json;
use wcore_eval::{Candidate, DefaultScorer, Scorer};
use wcore_observability::trace::{ToolCallTrace, TurnTrace};
use wcore_skills::types::{ExecutionContext, LoadedFrom, SkillMetadata, SkillSource};

fn baseline_skill() -> SkillMetadata {
    SkillMetadata {
        name: "hello".into(),
        display_name: None,
        description: "A friendly greeting skill.".into(),
        has_user_specified_description: true,
        allowed_tools: vec![],
        argument_hint: None,
        argument_names: vec![],
        when_to_use: Some("Greet the user when they start a session.".into()),
        version: None,
        model: Some("claude-sonnet-4-7".into()),
        disable_model_invocation: false,
        user_invocable: true,
        execution_context: ExecutionContext::Inline,
        agent: None,
        effort: None,
        shell: None,
        paths: vec![],
        artifacts: vec![],
        hooks_raw: None,
        source: SkillSource::Bundled,
        loaded_from: LoadedFrom::Bundled,
        content: "Hello! I am a friendly bundled greeting skill. $ARGUMENTS".into(),
        content_length: 57,
        skill_root: None,
        max_turns: None,
        max_tokens: None,
    }
}

fn baseline_trace() -> TurnTrace {
    TurnTrace {
        turn: 1,
        model: "claude-sonnet-4-7".into(),
        provider: "anthropic".into(),
        input_tokens: 100,
        output_tokens: 50,
        cache_read: 0,
        cache_write: 0,
        cache_hit_rate: 0.0,
        cost_usd: 0.001,
        tool_calls: vec![ToolCallTrace::new("c1".into(), "Read".into(), json!({}))],
        hook_actions: vec![],
        source_product: "wayland-core".into(),
        agent_run_id: String::new(),
    }
}

fn candidate_baseline() -> Candidate {
    Candidate {
        skill: baseline_skill(),
        trace: Some(baseline_trace()),
        source_filename: "hello".into(),
    }
}

#[test]
fn score_is_deterministic_across_two_invocations() {
    let s = DefaultScorer::default();
    let c = candidate_baseline();
    let a = s.score(&c);
    let b = s.score(&c);
    assert_eq!(
        a.dimensions.combined.to_bits(),
        b.dimensions.combined.to_bits(),
        "f64 bit-equality required for determinism"
    );
    assert_eq!(
        a.dimensions.outcome.to_bits(),
        b.dimensions.outcome.to_bits()
    );
    assert_eq!(
        a.dimensions.cost_penalty.to_bits(),
        b.dimensions.cost_penalty.to_bits()
    );
    assert_eq!(
        a.dimensions.size_penalty.to_bits(),
        b.dimensions.size_penalty.to_bits()
    );
    assert_eq!(a.predicted, b.predicted);
}

#[test]
fn score_is_bounded_zero_to_one() {
    let s = DefaultScorer::default();
    let c = candidate_baseline();
    let out = s.score(&c);
    assert!(out.dimensions.combined >= 0.0);
    assert!(out.dimensions.combined <= 1.0);
}

#[test]
fn missing_arguments_placeholder_lowers_outcome() {
    let s = DefaultScorer::default();
    let good = s.score(&candidate_baseline());
    let mut bad_c = candidate_baseline();
    bad_c.skill.content = bad_c.skill.content.replace("$ARGUMENTS", "");
    let bad = s.score(&bad_c);
    assert!(bad.dimensions.outcome < good.dimensions.outcome);
}

#[test]
fn name_mismatch_with_filename_fails_check() {
    let s = DefaultScorer::default();
    let good = s.score(&candidate_baseline());
    let mut bad_c = candidate_baseline();
    bad_c.skill.name = "calculator".into(); // filename is still "hello"
    let bad = s.score(&bad_c);
    assert!(bad.dimensions.outcome < good.dimensions.outcome);
}

#[test]
fn stale_model_pin_fails_check() {
    let s = DefaultScorer::default();
    let good = s.score(&candidate_baseline());
    let mut bad_c = candidate_baseline();
    bad_c.skill.model = Some("claude-haiku-3-20240306".into()); // not in allowlist
    let bad = s.score(&bad_c);
    assert!(bad.dimensions.outcome < good.dimensions.outcome);
}

#[test]
fn off_topic_description_fails_check() {
    let s = DefaultScorer::default();
    let good = s.score(&candidate_baseline());
    let mut bad_c = candidate_baseline();
    bad_c.skill.description = "Translates English to French.".into();
    let bad = s.score(&bad_c);
    assert!(bad.dimensions.outcome < good.dimensions.outcome);
}

#[test]
fn oversize_body_penalizes_size_component() {
    let s = DefaultScorer::default();
    let small = s.score(&candidate_baseline());
    let mut big_c = candidate_baseline();
    big_c.skill.content = big_c.skill.content.repeat(60);
    big_c.skill.content_length = big_c.skill.content.len();
    let big = s.score(&big_c);
    assert!(big.dimensions.size_penalty > small.dimensions.size_penalty);
}

#[test]
fn expensive_trace_increases_cost_penalty() {
    let s = DefaultScorer::default();
    let cheap = s.score(&candidate_baseline());
    let mut exp_c = candidate_baseline();
    if let Some(t) = exp_c.trace.as_mut() {
        t.cost_usd *= 50.0;
        t.output_tokens *= 80;
    }
    let exp = s.score(&exp_c);
    assert!(exp.dimensions.cost_penalty > cheap.dimensions.cost_penalty);
}
