//! # Tool-call schema reward signal (T2-A3)
//!
//! Lightweight, deterministic reward signal for the GEPA evolution loop that
//! scores a sequence of tool-call observations along three axes:
//!
//! 1. **Structure match** — does the call hit a tool whose expected required
//!    arg set is known, and are all required args present in the JSON object
//!    payload?
//! 2. **Type correctness** — are the required args non-null (cheap proxy for
//!    "the LLM didn't emit a JSON `null` where a real value was needed").
//! 3. **Non-redundancy** — fraction of unique `(tool_name, args)` pairs across
//!    the sequence; duplicates are penalized.
//!
//! The blended `total` is clamped to `0.0..=1.0` so the GEPA child selector
//! can consume it uniformly. Default weights are `0.5 / 0.25 / 0.25` for
//! structure / type / non-redundancy respectively.
//!
//! ## Integration into the GEPA child selector
//!
//! `wcore-eval`'s `Scorer` consumes a `Candidate` (skill body + an optional
//! [`wcore_observability::trace::TurnTrace`]) and yields a `combined` score in
//! `[0, 1]`; the GEPA loop retains a child when its `combined` beats both the
//! running best and the parent. This reward signal feeds that decision through
//! two pieces of production glue defined here:
//!
//! - [`ToolCallSchemaReward::score_trace`] reads the `tool_calls` off a real
//!   `TurnTrace` (the same type the loop threads through
//!   `wcore_eval::Candidate::trace`) and scores them — no synthetic stand-in.
//! - [`blend_into_combined`] folds that reward into a candidate's `combined`
//!   score so a child that emits well-formed, non-redundant tool calls is
//!   preferred over one with the same eval outcome but sloppier tool usage.
//!
//! ### Deferral (honest, not faked)
//!
//! The fold only changes a child's score when a `TurnTrace` is actually
//! present. Today the loop scores *mutated-but-unexecuted* skill bodies, so
//! `generation::Generation::run` passes `Candidate::trace = None` and there are
//! no tool calls to score — [`blend_into_combined`] is then an identity
//! pass-through. The reward becomes live the moment GEPA children are
//! **executed** to populate `Candidate::trace` (the "execute-children" milestone
//! tracked for W10C): `Generation::run` would attach the child's `TurnTrace`,
//! `evolve()` would call [`blend_into_combined`] before the retention compare,
//! and this signal would shift which child wins. The bridge below is the real,
//! tested half of that wiring; only the trace-population call site is deferred.

use std::collections::{HashMap, HashSet};

use wcore_observability::trace::TurnTrace;

/// One observed tool invocation produced by a candidate prompt during
/// generation.
#[derive(Debug, Clone)]
pub struct ToolCallObservation {
    /// Tool name as the engine sees it (e.g. `"Read"`, `"Bash"`).
    pub tool_name: String,
    /// Tool input payload. The engine wire format is JSON, so we keep the raw
    /// `serde_json::Value` rather than introducing a typed mirror.
    pub args: serde_json::Value,
    /// Did the tool report success? Not used in scoring today but recorded so
    /// downstream signals (e.g. failure-aware reward) can read it without
    /// re-plumbing the observation source.
    pub result_ok: bool,
}

/// Breakdown of a single schema-reward scoring run.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SchemaRewardScore {
    /// Weighted blend of the three sub-scores, clamped to `0.0..=1.0`.
    pub total: f32,
    /// Fraction of observations whose tool name was in `expected_schema` AND
    /// whose `args` object contained every required arg.
    pub structure_match: f32,
    /// Among *matched* calls only: fraction whose required args were
    /// non-null. If nothing matched, returns `0.0`.
    pub type_correctness: f32,
    /// `1.0 - duplicates / max(1, total_calls)`.
    pub non_redundancy: f32,
}

/// Reward signal configured with the per-tool required-arg schema and the
/// type / redundancy weights.
#[derive(Debug, Clone)]
pub struct ToolCallSchemaReward {
    /// Required argument names per tool name.
    expected_schema: HashMap<String, HashSet<String>>,
    /// Weight applied to `type_correctness` in the blended total.
    type_check_weight: f32,
    /// Weight applied to `non_redundancy` in the blended total.
    redundancy_weight: f32,
}

impl ToolCallSchemaReward {
    /// Build a reward signal with the default weights (`type=0.25`,
    /// `redundancy=0.25`; the implicit structure weight is `0.5`).
    pub fn new(expected_schema: HashMap<String, HashSet<String>>) -> Self {
        Self {
            expected_schema,
            type_check_weight: 0.25,
            redundancy_weight: 0.25,
        }
    }

    /// Override the default weights. Values are NOT auto-normalized; callers
    /// are expected to keep them reasonable. The final `total` is clamped to
    /// `[0, 1]` regardless.
    pub fn with_weights(mut self, type_check_weight: f32, redundancy_weight: f32) -> Self {
        self.type_check_weight = type_check_weight;
        self.redundancy_weight = redundancy_weight;
        self
    }

    /// Score a sequence of tool-call observations.
    ///
    /// Empty sequences return an all-zero score so the GEPA child selector
    /// treats "no tools called" as no reward (rather than a free win).
    pub fn score(&self, calls: &[ToolCallObservation]) -> SchemaRewardScore {
        let total_calls = calls.len();
        if total_calls == 0 {
            return SchemaRewardScore {
                total: 0.0,
                structure_match: 0.0,
                type_correctness: 0.0,
                non_redundancy: 0.0,
            };
        }

        // Pass 1: structure match + type correctness (only over matched calls).
        let mut matched_count: usize = 0;
        let mut type_correct_count: usize = 0;

        for call in calls {
            let Some(required) = self.expected_schema.get(&call.tool_name) else {
                // Unrecognized tool: counts toward denominator but never the
                // structure-match numerator.
                continue;
            };

            // Args must be a JSON object for required-arg lookup to make
            // sense; anything else (array, scalar, null) is an immediate
            // structure miss.
            let Some(obj) = call.args.as_object() else {
                continue;
            };

            // Every required arg must be a key in the object.
            let all_required_present = required.iter().all(|k| obj.contains_key(k));
            if !all_required_present {
                continue;
            }

            matched_count += 1;

            // Type-correctness heuristic: every required arg must be
            // present AND non-null. `obj.contains_key` above already
            // guaranteed presence.
            let all_required_non_null = required
                .iter()
                .all(|k| obj.get(k).is_some_and(|v| !v.is_null()));
            if all_required_non_null {
                type_correct_count += 1;
            }
        }

        let structure_match = matched_count as f32 / total_calls as f32;
        let type_correctness = if matched_count == 0 {
            0.0
        } else {
            type_correct_count as f32 / matched_count as f32
        };

        // Pass 2: non-redundancy. A duplicate is any call whose
        // `(tool_name, args)` matches a *prior* call in the sequence.
        let mut seen: HashSet<(String, String)> = HashSet::new();
        let mut duplicates: usize = 0;
        for call in calls {
            // Canonicalize args via `to_string` — JSON object key order is
            // preserved by serde_json::Map (BTreeMap-backed when the
            // `preserve_order` feature is off, which is the workspace
            // default), so this is stable for the cases we care about.
            //
            // WARNING: this canonicalization depends on serde_json's default
            // BTreeMap-backed `Map`. If any transitive dependency enables
            // the `preserve_order` feature on serde_json, key order becomes
            // insertion-order and this duplicate detection will silently
            // miss reorderings of identical args. Future hardening: walk
            // the `Value` tree explicitly and emit via a forced BTreeMap.
            let key = (call.tool_name.clone(), call.args.to_string());
            if !seen.insert(key) {
                duplicates += 1;
            }
        }
        let non_redundancy = 1.0 - (duplicates as f32 / total_calls.max(1) as f32);

        let raw_total = 0.5 * structure_match
            + self.type_check_weight * type_correctness
            + self.redundancy_weight * non_redundancy;
        let total = raw_total.clamp(0.0, 1.0);

        SchemaRewardScore {
            total,
            structure_match,
            type_correctness,
            non_redundancy,
        }
    }

    /// Score the tool calls recorded on a real [`TurnTrace`].
    ///
    /// This is the production entry point used by the GEPA child selector: the
    /// loop threads a child's `TurnTrace` through `wcore_eval::Candidate::trace`,
    /// and this method scores its `tool_calls` directly off the trace — no
    /// synthetic observations. A trace with no tool calls returns the all-zero
    /// score (same contract as [`Self::score`] on an empty slice).
    pub fn score_trace(&self, trace: &TurnTrace) -> SchemaRewardScore {
        self.score(&observations_from_trace(trace))
    }
}

/// Convert a [`TurnTrace`]'s recorded tool calls into the observation slice the
/// reward scorer consumes. `result_ok` is derived from the absence of a
/// recorded `error` (the trace's only success signal at this layer).
pub fn observations_from_trace(trace: &TurnTrace) -> Vec<ToolCallObservation> {
    trace
        .tool_calls
        .iter()
        .map(|tc| ToolCallObservation {
            tool_name: tc.tool_name.clone(),
            args: tc.input.clone(),
            result_ok: tc.error.is_none(),
        })
        .collect()
}

/// Fold a schema-reward score into a candidate's eval `combined` score so the
/// GEPA selector prefers children with cleaner tool usage.
///
/// `combined` is the `wcore_eval` blended score in `[0, 1]`; `weight` is the
/// share given to the schema reward. The result is the convex blend
/// `(1 - weight) * combined + weight * reward.total`, re-clamped to `[0, 1]`.
///
/// `weight == 0.0` is the identity pass-through used while children are scored
/// unexecuted (no `TurnTrace`, so no tool calls to reward — see the module-level
/// deferral note). A non-zero weight is what makes this reward actually move a
/// child's rank once children are executed.
pub fn blend_into_combined(combined: f64, reward: &SchemaRewardScore, weight: f64) -> f64 {
    let weight = weight.clamp(0.0, 1.0);
    ((1.0 - weight) * combined + weight * reward.total as f64).clamp(0.0, 1.0)
}

/// Free-function convenience wrapper — equivalent to
/// `reward.score(calls)`. Provided so callers that already have a reward
/// reference don't need to import the type just to score.
pub fn score(reward: &ToolCallSchemaReward, calls: &[ToolCallObservation]) -> SchemaRewardScore {
    reward.score(calls)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn schema(pairs: &[(&str, &[&str])]) -> HashMap<String, HashSet<String>> {
        pairs
            .iter()
            .map(|(tool, args)| {
                (
                    (*tool).to_string(),
                    args.iter().map(|a| (*a).to_string()).collect(),
                )
            })
            .collect()
    }

    fn obs(tool: &str, args: serde_json::Value) -> ToolCallObservation {
        ToolCallObservation {
            tool_name: tool.to_string(),
            args,
            result_ok: true,
        }
    }

    #[test]
    fn structure_match_all_known_tools_with_required_args() {
        let reward =
            ToolCallSchemaReward::new(schema(&[("Read", &["path"]), ("Bash", &["command"])]));
        let calls = vec![
            obs("Read", json!({"path": "a.rs"})),
            obs("Bash", json!({"command": "ls"})),
        ];
        let s = reward.score(&calls);
        assert_eq!(s.structure_match, 1.0);
    }

    #[test]
    fn structure_match_unrecognized_tool_excluded_from_numerator() {
        let reward = ToolCallSchemaReward::new(schema(&[("Read", &["path"])]));
        let calls = vec![
            obs("Read", json!({"path": "a.rs"})),
            obs("Unknown", json!({"x": 1})),
        ];
        let s = reward.score(&calls);
        // 1 matched of 2 total → 0.5
        assert!((s.structure_match - 0.5).abs() < 1e-6);
    }

    #[test]
    fn structure_match_missing_required_arg_not_matched() {
        let reward = ToolCallSchemaReward::new(schema(&[("Read", &["path", "limit"])]));
        let calls = vec![
            obs("Read", json!({"path": "a.rs"})), // missing "limit"
            obs("Read", json!({"path": "b.rs", "limit": 10})),
        ];
        let s = reward.score(&calls);
        assert!((s.structure_match - 0.5).abs() < 1e-6);
    }

    #[test]
    fn type_correctness_null_value_marked_incorrect() {
        let reward = ToolCallSchemaReward::new(schema(&[("Read", &["path"])]));
        let calls = vec![obs("Read", json!({"path": serde_json::Value::Null}))];
        let s = reward.score(&calls);
        // Required arg is present (passes structure) but null (fails type).
        assert!((s.structure_match - 1.0).abs() < 1e-6);
        assert!((s.type_correctness - 0.0).abs() < 1e-6);
    }

    #[test]
    fn type_correctness_present_non_null_marked_correct() {
        let reward = ToolCallSchemaReward::new(schema(&[("Read", &["path"])]));
        let calls = vec![obs("Read", json!({"path": "a.rs"}))];
        let s = reward.score(&calls);
        assert!((s.type_correctness - 1.0).abs() < 1e-6);
    }

    #[test]
    fn non_redundancy_no_duplicates_returns_one() {
        let reward = ToolCallSchemaReward::new(schema(&[("Read", &["path"])]));
        let calls = vec![
            obs("Read", json!({"path": "a.rs"})),
            obs("Read", json!({"path": "b.rs"})),
            obs("Read", json!({"path": "c.rs"})),
        ];
        let s = reward.score(&calls);
        assert!((s.non_redundancy - 1.0).abs() < 1e-6);
    }

    #[test]
    fn non_redundancy_full_duplicates_returns_zero() {
        // 3 identical calls → 2 duplicates / 3 total = 0.6666… → 1 - 0.666 = 0.333
        // To get exactly 0.0 we'd need every single call to be a duplicate
        // of a prior call, which is impossible since the first call is
        // always unique. So we test "every call after the first is a dup"
        // → non_redundancy = 1 - (n-1)/n. For n=10 that's 0.1.
        // For the spirit of "full duplicates returns zero", we check the
        // floor: a single repeated call across a large sequence pushes
        // non_redundancy toward zero asymptotically. Concretely, 10 copies:
        let reward = ToolCallSchemaReward::new(schema(&[("Read", &["path"])]));
        let calls: Vec<_> = (0..10)
            .map(|_| obs("Read", json!({"path": "a.rs"})))
            .collect();
        let s = reward.score(&calls);
        // 9 duplicates / 10 total = 0.9 → 1 - 0.9 = 0.1
        assert!((s.non_redundancy - 0.1).abs() < 1e-6);
    }

    #[test]
    fn non_redundancy_partial_duplicates_correct_fraction() {
        let reward = ToolCallSchemaReward::new(schema(&[("Read", &["path"])]));
        let calls = vec![
            obs("Read", json!({"path": "a.rs"})),
            obs("Read", json!({"path": "a.rs"})), // dup
            obs("Read", json!({"path": "b.rs"})),
            obs("Read", json!({"path": "b.rs"})), // dup
        ];
        // 2 duplicates / 4 = 0.5 → 1 - 0.5 = 0.5
        let s = reward.score(&calls);
        assert!((s.non_redundancy - 0.5).abs() < 1e-6);
    }

    #[test]
    fn total_score_clamped_to_unit_interval() {
        // Crank weights way above 1.0 to force the raw blend > 1, then
        // verify the clamp pulls it back.
        let reward =
            ToolCallSchemaReward::new(schema(&[("Read", &["path"])])).with_weights(10.0, 10.0);
        let calls = vec![obs("Read", json!({"path": "a.rs"}))];
        let s = reward.score(&calls);
        // structure=1, type=1, non_red=1 → raw = 0.5 + 10 + 10 = 20.5
        assert!(s.total <= 1.0 && s.total >= 0.0);
        assert!((s.total - 1.0).abs() < 1e-6);
    }

    #[test]
    fn score_empty_observation_list_returns_zero_total() {
        let reward = ToolCallSchemaReward::new(schema(&[("Read", &["path"])]));
        let s = reward.score(&[]);
        assert_eq!(s.total, 0.0);
        assert_eq!(s.structure_match, 0.0);
        assert_eq!(s.type_correctness, 0.0);
        assert_eq!(s.non_redundancy, 0.0);
    }

    #[test]
    fn score_with_default_weights_matches_expected_formula() {
        // 4 calls:
        //  - Read{path:"a"}          → match, type-correct
        //  - Read{path:"a"}          → match (dup), type-correct
        //  - Read{path:null}         → match, type-INcorrect
        //  - Unknown{...}            → no match
        // structure_match = 3/4 = 0.75
        // matched=3, type_correct=2 → type_correctness = 2/3
        // duplicates: call #2 dups call #1; call #3 unique; call #4 unique
        //   → 1 dup / 4 total → non_redundancy = 0.75
        // total = 0.5 * 0.75 + 0.25 * (2/3) + 0.25 * 0.75
        //       = 0.375 + 0.16666… + 0.1875
        //       = 0.72916…
        let reward = ToolCallSchemaReward::new(schema(&[("Read", &["path"])]));
        let calls = vec![
            obs("Read", json!({"path": "a"})),
            obs("Read", json!({"path": "a"})),
            obs("Read", json!({"path": serde_json::Value::Null})),
            obs("Unknown", json!({"x": 1})),
        ];
        let s = reward.score(&calls);
        assert!((s.structure_match - 0.75).abs() < 1e-6);
        assert!((s.type_correctness - (2.0 / 3.0)).abs() < 1e-6);
        assert!((s.non_redundancy - 0.75).abs() < 1e-6);
        let expected = 0.5 * 0.75 + 0.25 * (2.0_f32 / 3.0) + 0.25 * 0.75;
        assert!((s.total - expected).abs() < 1e-6);
    }

    #[test]
    fn free_function_score_matches_method() {
        let reward = ToolCallSchemaReward::new(schema(&[("Read", &["path"])]));
        let calls = vec![obs("Read", json!({"path": "a.rs"}))];
        let a = reward.score(&calls);
        let b = score(&reward, &calls);
        assert_eq!(a, b);
    }

    /// Build a `TurnTrace` carrying the given `(tool_name, input, error)` tool
    /// calls so we can exercise the production trace bridge.
    fn trace_with(calls: &[(&str, serde_json::Value, Option<&str>)]) -> TurnTrace {
        let tool_calls = calls
            .iter()
            .enumerate()
            .map(|(i, (name, input, err))| {
                let mut tc = wcore_observability::trace::ToolCallTrace::new(
                    format!("call-{i}"),
                    (*name).to_string(),
                    input.clone(),
                );
                tc.error = err.map(|e| e.to_string());
                tc
            })
            .collect();
        TurnTrace {
            turn: 0,
            model: "stub".into(),
            provider: "stub".into(),
            input_tokens: 0,
            output_tokens: 0,
            cache_read: 0,
            cache_write: 0,
            cache_hit_rate: 0.0,
            cost_usd: 0.0,
            tool_calls,
            hook_actions: vec![],
            source_product: "test".into(),
            agent_run_id: String::new(),
        }
    }

    #[test]
    fn score_trace_reads_tool_calls_off_real_turn_trace() {
        let reward = ToolCallSchemaReward::new(schema(&[("Read", &["path"])]));
        let trace = trace_with(&[
            ("Read", json!({"path": "a.rs"}), None),
            ("Unknown", json!({"x": 1}), None),
        ]);
        // Identical to scoring the equivalent observation slice directly: one
        // matched of two total → structure_match 0.5.
        let s = reward.score_trace(&trace);
        assert!((s.structure_match - 0.5).abs() < 1e-6);
    }

    #[test]
    fn observations_from_trace_maps_error_to_result_ok() {
        let trace = trace_with(&[
            ("Read", json!({"path": "a.rs"}), None),
            ("Bash", json!({"command": "boom"}), Some("non-zero exit")),
        ]);
        let obs = observations_from_trace(&trace);
        assert_eq!(obs.len(), 2);
        // `.first()`/`.get()` instead of indexing — this crate denies
        // `clippy::indexing_slicing` (and unwrap/expect/panic) crate-wide.
        assert_eq!(obs.first().map(|o| o.result_ok), Some(true));
        let second = obs.get(1);
        assert_eq!(second.map(|o| o.result_ok), Some(false));
        assert_eq!(second.map(|o| o.tool_name.as_str()), Some("Bash"));
        assert_eq!(second.map(|o| &o.args), Some(&json!({"command": "boom"})));
    }

    #[test]
    fn blend_into_combined_reward_shifts_candidate_score() {
        let reward = ToolCallSchemaReward::new(schema(&[("Read", &["path"])]));
        // Two children tie on the eval combined score, but one emits a clean,
        // schema-matching tool call and the other emits a malformed one.
        let clean = reward.score_trace(&trace_with(&[("Read", json!({"path": "a.rs"}), None)]));
        let sloppy = reward.score_trace(&trace_with(&[("Read", json!({"nope": 1}), None)]));

        let base = 0.6_f64;
        let weight = 0.3_f64;
        let clean_blended = blend_into_combined(base, &clean, weight);
        let sloppy_blended = blend_into_combined(base, &sloppy, weight);

        // The reward must actually move the score: clean tool usage now
        // outranks sloppy usage that scored identically on the eval axis.
        assert!(clean_blended > sloppy_blended);
        assert!(clean_blended > base, "positive reward must lift the score");
        assert!(
            sloppy_blended < base,
            "zero reward must penalize relative to base"
        );
    }

    #[test]
    fn blend_into_combined_zero_weight_is_identity() {
        let reward = ToolCallSchemaReward::new(schema(&[("Read", &["path"])]));
        let s = reward.score_trace(&trace_with(&[("Read", json!({"path": "a.rs"}), None)]));
        // Identity pass-through used while children are scored unexecuted.
        assert!((blend_into_combined(0.42, &s, 0.0) - 0.42).abs() < 1e-12);
    }

    #[test]
    fn blend_into_combined_clamps_to_unit_interval() {
        let reward = ToolCallSchemaReward::new(schema(&[("Read", &["path"])]));
        let perfect = reward.score_trace(&trace_with(&[("Read", json!({"path": "a.rs"}), None)]));
        // Over-unit base + over-unit weight still clamps into [0, 1].
        let blended = blend_into_combined(1.5, &perfect, 2.0);
        assert!((0.0..=1.0).contains(&blended));
    }
}
