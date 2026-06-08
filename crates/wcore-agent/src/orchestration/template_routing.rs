//! v0.8.0 Task K — wire `wcore_dispatch::TemplateRouter` into the per-turn
//! template-selection path. Maps each `Template` variant to its existing
//! `GraphConfig` constructor so `multi_agent_consensus` and
//! `hierarchical_delegation` — both shipped at zero callers in C.2 — are
//! finally reachable in production.
//!
//! # Resolution order
//!
//! For every turn, the orchestrator asks `select_graph_config(...)` to
//! pick the `GraphConfig` to execute. The function applies, in order:
//!
//! 1. **Manual override** — if the task description contains
//!    `@@template=<name>` and `<name>` parses to a known [`Template`], that
//!    variant is selected unconditionally (bypasses router AND classifier).
//!    Mirrors the existing `@@skill=` override on the skill router.
//! 2. **TemplateRouter** — if a router is provided, call
//!    `TemplateRouter::choose(user_input)`. The router honours its own
//!    `@@template=` parse internally, then Thompson-picks from its arms.
//!    On `RouterError::NoCandidates` (cold-start with zero arms), fall
//!    through. Any other [`RouterError`] is also treated as "no opinion"
//!    and falls through — the deterministic classifier is the safe
//!    default.
//! 3. **IntentClassifier + LoopSelector** — keyword pass over the task
//!    string, mapping the inferred `Intent` to a `GraphConfig`. Same
//!    behaviour as pre-K. Honours the optional [`Mode`] override on
//!    the engine.
//!
//! Once a `Template` is picked (by override or router), it is mapped to
//! a `GraphConfig` via [`graph_for_template`]. Default agent names match
//! the existing C.3 [`LoopSelector`] choices so test expectations stay
//! aligned: `"main"` for Direct, fan-out workers `worker_a/b/c` etc.

use serde_json::json;

use wcore_dispatch::{DecisionRouter, RouterError, Template, TemplateRouter};

use super::graph::GraphConfig;
use super::intent::{IntentClassifier, LoopSelector, Mode};

/// Provenance of the picked `GraphConfig`. Useful for telemetry + tests.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TemplateDecisionSource {
    /// Manual `@@template=<name>` override in the task description.
    Override,
    /// `TemplateRouter::choose` returned a learned/Thompson-sampled arm.
    Router,
    /// Fell back to `IntentClassifier` + `LoopSelector`.
    Classifier,
}

/// One per-turn decision: which `GraphConfig` to execute and where the
/// choice came from. The caller (`engine::run`) uses `config` to drive
/// the graph walker and `source` for tracing / observability.
pub struct TemplateDecision {
    pub config: GraphConfig,
    pub source: TemplateDecisionSource,
    /// `Some(t)` whenever the decision was made by the override or
    /// router branch — i.e. a discrete `Template` enum value can be
    /// attributed. `None` when the classifier path produced a
    /// `GraphConfig` directly (the classifier is not a `Template`-
    /// returning router).
    pub template: Option<Template>,
}

/// Convert a `Template` enum variant into its canonical `GraphConfig`.
/// Each branch invokes the existing constructor in `templates.rs`.
///
/// Default agent names match the C.3 [`LoopSelector`] defaults so test
/// expectations and observability dashboards stay aligned across the
/// router and classifier paths.
///
/// `Adaptive` is special — the `AdaptiveConfig` wrapper carries a replan
/// closure and does not fit the per-turn `ExecutionGraph::execute(&GraphConfig)`
/// seam. The seam-aware entry point is [`graph_for_template_with_task`],
/// which resolves `Adaptive` to one of the four concrete variants via
/// [`adaptive_pick`]. This function preserves the historical signature
/// for callers that do not have a task string in hand and uses a Direct
/// safety fallback for `Adaptive`. New callers should prefer
/// [`graph_for_template_with_task`].
pub fn graph_for_template(t: Template) -> GraphConfig {
    graph_for_template_with_task(t, "")
}

/// Seam-aware variant of [`graph_for_template`]: when `t == Adaptive`,
/// uses the keyword-signal selector [`adaptive_pick`] to choose a
/// concrete template based on `task`, then materialises that concrete
/// template's `GraphConfig`. All other variants map identically to
/// [`graph_for_template`].
pub fn graph_for_template_with_task(t: Template, task: &str) -> GraphConfig {
    match t {
        Template::Direct => GraphConfig::direct("main", json!({})),
        Template::Consensus => GraphConfig::multi_agent_consensus(
            vec!["proposer_a", "proposer_b", "proposer_c"],
            "judge",
        ),
        Template::SelfCritique => GraphConfig::self_critique("doer", "critic", 3),
        Template::Hierarchical => {
            GraphConfig::hierarchical_delegation("planner", "worker", "integrator")
        }
        // Adaptive resolves to a concrete template at routing time via a
        // simple keyword-signal selector. The `AdaptiveConfig` wrapper
        // (templates::adaptive) wires replan-on-failure and is reachable
        // separately through its own `execute` method; the per-turn
        // routing seam can't carry it, so we project Adaptive down to a
        // concrete template that ExecutionGraph::execute can run.
        Template::Adaptive => {
            let concrete = adaptive_pick(task);
            // Guard against infinite recursion: `adaptive_pick` is
            // documented to never return Template::Adaptive, but defend
            // anyway so a future regression is a Direct fallback rather
            // than a stack overflow.
            match concrete {
                Template::Adaptive => GraphConfig::direct("main", json!({})),
                other => graph_for_template_with_task(other, task),
            }
        }
    }
}

/// Keyword-signal selector used by the `Adaptive` template path. Maps a
/// task description to a concrete (non-`Adaptive`) `Template` variant.
///
/// Signals (first match wins):
/// - `review` / `audit` / `critique` / `proofread` → `SelfCritique`
/// - `compare` / `research` / `versus` / `vs ` / `pros and cons` →
///   `Consensus`
/// - `delegate` / `plan` / `coordinate` / `orchestrate` / `break down` →
///   `Hierarchical`
/// - otherwise → `Direct`
///
/// The selector is intentionally trivial and deterministic: a learned
/// router lives one level up (`TemplateRouter`), and Adaptive's role
/// here is to give the router a *single* arm that still does
/// task-aware projection. Never returns `Template::Adaptive`.
pub fn adaptive_pick(task: &str) -> Template {
    let t = task.to_ascii_lowercase();
    let has_any = |needles: &[&str]| needles.iter().any(|n| t.contains(n));

    if has_any(&["review", "audit", "critique", "proofread"]) {
        Template::SelfCritique
    } else if has_any(&["compare", "research", "versus", "vs ", "pros and cons"]) {
        Template::Consensus
    } else if has_any(&[
        "delegate",
        "plan",
        "coordinate",
        "orchestrate",
        "break down",
    ]) {
        Template::Hierarchical
    } else {
        Template::Direct
    }
}

/// Parse a `@@template=<name>` override out of the task description.
/// Public so callers can short-circuit even when no router is wired.
/// Returns `None` if no override exists OR the name doesn't parse to a
/// known [`Template`] (silent fall-through, no panic).
pub fn parse_template_override(input: &str) -> Option<Template> {
    use std::str::FromStr;
    let lower = input.to_ascii_lowercase();
    let needle = "@@template=";
    let idx = lower.find(needle)?;
    let tail = &lower[idx + needle.len()..];
    let end = tail
        .find(|c: char| c.is_whitespace() || c == '"' || c == ',' || c == ')')
        .unwrap_or(tail.len());
    let name = &tail[..end];
    Template::from_str(name).ok()
}

/// Primary per-turn selection entry point. Combines override + router +
/// classifier per the module-level resolution order.
///
/// Arguments
/// - `task`: latest user input (used by both override-parse and router).
/// - `router`: optional learned router. `None` ⇒ skip the router branch
///   entirely and go override → classifier.
/// - `mode_override`: pre-existing `Mode` knob threaded into the
///   classifier path; preserves byte-identical pre-K behaviour for
///   engines that never wire a `TemplateRouter`.
pub fn select_graph_config(
    task: &str,
    router: Option<&mut TemplateRouter>,
    mode_override: Option<Mode>,
) -> TemplateDecision {
    // 1. Manual override always wins. We deliberately do NOT defer to
    // the router's own `@@template=` parse because the router only
    // honours overrides whose `Template` is in its configured arm set;
    // the engine-level override is unconditional.
    if let Some(t) = parse_template_override(task) {
        return TemplateDecision {
            config: graph_for_template_with_task(t, task),
            source: TemplateDecisionSource::Override,
            template: Some(t),
        };
    }

    // 2. Try the router if one is wired.
    if let Some(r) = router {
        match r.choose(task) {
            Ok(t) => {
                return TemplateDecision {
                    config: graph_for_template_with_task(t, task),
                    source: TemplateDecisionSource::Router,
                    template: Some(t),
                };
            }
            Err(RouterError::NoCandidates)
            | Err(RouterError::Declined { .. })
            | Err(RouterError::Internal(_)) => {
                // Fall through to classifier — the router has no
                // opinion this turn.
            }
        }
    }

    // 3. Classifier fallback (pre-K behaviour).
    let intent = IntentClassifier::classify(task);
    let cfg = LoopSelector::select(&intent, mode_override);
    TemplateDecision {
        config: cfg,
        source: TemplateDecisionSource::Classifier,
        template: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // 1. TemplateRouter with arms restricted to {Direct} → Direct graph.
    #[test]
    fn router_cold_start_with_direct_arm_picks_direct_template() {
        let mut r = TemplateRouter::with_seed_and_arms(7, vec![Template::Direct]);
        let d = select_graph_config("do a thing", Some(&mut r), None);
        assert_eq!(d.source, TemplateDecisionSource::Router);
        assert_eq!(d.template, Some(Template::Direct));
        // Direct → single AgentCall with start == "main"
        assert!(d.config.is_direct());
    }

    // 2. Learned bias toward Hierarchical → hierarchical_delegation
    //    constructor is selected (multi-agent constructor reachable).
    #[test]
    fn router_learned_hierarchical_bias_routes_to_hierarchical_graph() {
        use wcore_dispatch::TaskOutcome;

        let mut r =
            TemplateRouter::with_seed_and_arms(42, vec![Template::Direct, Template::Hierarchical]);
        // Train: Hierarchical strongly successful, Direct mostly fails.
        for _ in 0..60 {
            r.observe(&Template::Hierarchical, TaskOutcome::Success);
        }
        for _ in 0..60 {
            r.observe(&Template::Direct, TaskOutcome::Failure);
        }

        // Sample 200 selections; Hierarchical should dominate.
        let mut hier = 0usize;
        let mut direct = 0usize;
        for _ in 0..200 {
            let d = select_graph_config("a generic task", Some(&mut r), None);
            assert_eq!(d.source, TemplateDecisionSource::Router);
            match d.template.expect("router branch always sets template") {
                Template::Hierarchical => hier += 1,
                Template::Direct => direct += 1,
                _ => {}
            }
        }
        assert!(
            hier > direct,
            "Hierarchical should dominate after training; got hier={hier} direct={direct}"
        );
    }

    // 3. Manual `@@template=consensus` override → multi_agent_consensus
    //    graph regardless of router state. Router's arms exclude
    //    Consensus, proving the engine-level override is unconditional.
    #[test]
    fn manual_template_override_consensus_bypasses_router() {
        let mut r = TemplateRouter::with_seed_and_arms(99, vec![Template::Direct]);
        let d = select_graph_config(
            "please use @@template=consensus for this answer",
            Some(&mut r),
            None,
        );
        assert_eq!(d.source, TemplateDecisionSource::Override);
        assert_eq!(d.template, Some(Template::Consensus));
        // multi_agent_consensus starts at `__cons_root__` synthetic passthrough.
        assert!(!d.config.is_direct());
    }

    // 4. IntentClassifier fallback when the router has no candidates.
    //    Direct-shaped task should produce a Direct graph via the
    //    classifier path.
    #[test]
    fn router_no_candidates_falls_back_to_intent_classifier() {
        // Empty arms is normalised to "all" by `with_arms`; to simulate
        // a truly-empty router we route `None` for the router (the
        // documented way to disable routing).
        let d = select_graph_config("fix typo in README line 12", None, None);
        assert_eq!(d.source, TemplateDecisionSource::Classifier);
        assert_eq!(d.template, None);
        assert!(d.config.is_direct());
    }

    // 5. Unknown override (`@@template=nonsense`) → no panic; falls
    //    through to router/classifier.
    #[test]
    fn unknown_override_falls_through_no_panic() {
        let mut r = TemplateRouter::with_seed_and_arms(11, vec![Template::Direct]);
        let d = select_graph_config(
            "@@template=floopynoodle please do the thing",
            Some(&mut r),
            None,
        );
        assert_eq!(d.source, TemplateDecisionSource::Router);
        assert_eq!(d.template, Some(Template::Direct));
    }

    // Bonus: override beats router even when the router exists.
    #[test]
    fn override_beats_router_when_both_present() {
        let mut r = TemplateRouter::with_seed_and_arms(1, vec![Template::Direct]);
        let d = select_graph_config(
            "@@template=hierarchical the rest of the task",
            Some(&mut r),
            None,
        );
        assert_eq!(d.source, TemplateDecisionSource::Override);
        assert_eq!(d.template, Some(Template::Hierarchical));
    }

    // Bonus: every Template variant maps to a constructable GraphConfig.
    #[test]
    fn graph_for_template_covers_all_variants() {
        for t in Template::all() {
            // Each call must not panic and must produce a config with at
            // least one node (the start node).
            let cfg = graph_for_template(t);
            assert!(
                !cfg.entry.is_empty(),
                "template {:?} produced empty entry node",
                t
            );
            assert!(
                !cfg.nodes.is_empty(),
                "template {:?} produced zero nodes",
                t
            );
        }
    }

    // Bonus: mode_override threads through the classifier path.
    #[test]
    fn mode_override_threads_through_classifier_when_router_absent() {
        let d = select_graph_config("anything goes here", None, Some(Mode::Parallel));
        assert_eq!(d.source, TemplateDecisionSource::Classifier);
        assert!(!d.config.is_direct());
    }

    // ---- v0.8.1 U4: Adaptive variant resolves to a concrete template ----

    // U4.1: review-shaped task → self_critique graph.
    #[test]
    fn adaptive_review_task_routes_to_self_critique() {
        assert_eq!(adaptive_pick("review this code"), Template::SelfCritique);
        let cfg = graph_for_template_with_task(Template::Adaptive, "review this code");
        // self_critique uses the synthetic loop entry id.
        assert_eq!(cfg.entry, "__crit__");
        assert!(!cfg.is_direct());
    }

    // U4.2: compare-shaped task → multi_agent_consensus graph.
    #[test]
    fn adaptive_compare_task_routes_to_consensus() {
        assert_eq!(adaptive_pick("compare X and Y"), Template::Consensus);
        let cfg = graph_for_template_with_task(Template::Adaptive, "compare X and Y");
        assert_eq!(cfg.entry, "__cons_root__");
        assert!(!cfg.is_direct());
    }

    // U4.3: delegate-shaped task → hierarchical_delegation graph.
    #[test]
    fn adaptive_delegate_task_routes_to_hierarchical() {
        assert_eq!(
            adaptive_pick("delegate the audit"),
            // "audit" beats "delegate" by selector order — SelfCritique
            // wins. Use a delegate-only phrasing for the hierarchical
            // assertion to exercise the intended branch.
            Template::SelfCritique
        );
        assert_eq!(
            adaptive_pick("delegate the rollout"),
            Template::Hierarchical
        );
        let cfg = graph_for_template_with_task(Template::Adaptive, "delegate the rollout");
        // hierarchical_delegation is a sequential pipeline starting at
        // the planner.
        assert_eq!(cfg.entry, "planner");
        assert!(!cfg.is_direct());
    }

    // U4.4: generic question → Direct graph.
    #[test]
    fn adaptive_generic_task_routes_to_direct() {
        assert_eq!(adaptive_pick("what time is it?"), Template::Direct);
        let cfg = graph_for_template_with_task(Template::Adaptive, "what time is it?");
        assert_eq!(cfg.entry, "main");
        assert!(cfg.is_direct());
    }

    // U4.5: `select_graph_config` actually exercises adaptive_pick when
    // the router picks Adaptive. Train the router to prefer Adaptive,
    // then issue a review-shaped task and assert the materialised graph
    // is self_critique (not direct).
    #[test]
    fn router_picking_adaptive_resolves_via_adaptive_pick() {
        use wcore_dispatch::TaskOutcome;

        let mut r =
            TemplateRouter::with_seed_and_arms(5, vec![Template::Direct, Template::Adaptive]);
        for _ in 0..80 {
            r.observe(&Template::Adaptive, TaskOutcome::Success);
        }
        for _ in 0..80 {
            r.observe(&Template::Direct, TaskOutcome::Failure);
        }

        // Sample until we see an Adaptive pick, then check the
        // materialised config matches the keyword-selected template.
        let mut saw_adaptive_resolved = false;
        for _ in 0..200 {
            let d = select_graph_config("please review this PR", Some(&mut r), None);
            assert_eq!(d.source, TemplateDecisionSource::Router);
            if d.template == Some(Template::Adaptive) {
                // Adaptive must NOT route to Direct for a review task.
                assert!(
                    !d.config.is_direct(),
                    "Adaptive on review task fell through to direct"
                );
                assert_eq!(d.config.entry, "__crit__");
                saw_adaptive_resolved = true;
                break;
            }
        }
        assert!(
            saw_adaptive_resolved,
            "router never picked Adaptive across 200 samples"
        );
    }

    // U4.6: `@@template=adaptive` override projects via the task too.
    #[test]
    fn adaptive_override_resolves_via_task_signals() {
        let d = select_graph_config("@@template=adaptive please audit the API", None, None);
        assert_eq!(d.source, TemplateDecisionSource::Override);
        assert_eq!(d.template, Some(Template::Adaptive));
        // "audit" → SelfCritique.
        assert_eq!(d.config.entry, "__crit__");
        assert!(!d.config.is_direct());
    }

    // U4.7: case-insensitive matching.
    #[test]
    fn adaptive_pick_is_case_insensitive() {
        assert_eq!(adaptive_pick("REVIEW this"), Template::SelfCritique);
        assert_eq!(adaptive_pick("Compare A vs B"), Template::Consensus);
        assert_eq!(adaptive_pick("Coordinate the team"), Template::Hierarchical);
    }
}
