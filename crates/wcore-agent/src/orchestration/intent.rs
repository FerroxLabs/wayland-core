//! W8b.2.B Task C.3 — `IntentClassifier` + `LoopSelector`.
//!
//! Classifies a free-text task into a coarse `Intent` (complexity +
//! shape) using a keyword heuristic, then maps it to a graph template.
//! The selector accepts an optional user [`Mode`] override that wins
//! when set.
//!
//! ## Why a heuristic, not an LLM call?
//!
//! The plan's prose mentions a "cheap-model classification call." The
//! executor brief for this sub-wave explicitly downgrades that to a
//! keyword heuristic ("the keyword heuristic is the deliverable; the
//! LLM-backed variant is feature-gated and optional"). The heuristic
//! is deterministic, free, runs in <1µs, and gives the agent a
//! reasonable default. The contract is open for an LLM-backed
//! implementation to slot in later behind a feature flag without
//! breaking callers (the `classify` entry point is the seam).

use super::graph::{AggregationStrategy, GraphConfig};

/// How rich a coordination strategy the task warrants. Used by
/// [`LoopSelector`] to pick a template when no `Mode` override is
/// supplied.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Complexity {
    /// One-shot edit, typo, rename, log-line. Direct dispatch.
    Trivial,
    /// Multi-file edit, refactor of a single function, light search.
    /// Parallel fanout or sequential pipeline.
    Moderate,
    /// Cross-module refactor, system redesign, anything that benefits
    /// from a planner+workers loop.
    Complex,
}

/// The coordination shape the classifier infers. Distinct from
/// `Complexity` because a Trivial task can still benefit from
/// self-critique (e.g. a poem) and a Complex task may still be best
/// served by a single direct call (the user knows what they want).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Shape {
    /// Default: route to Direct unless an explicit override or other
    /// shape keyword fires.
    Default,
    /// Self-critique signaled (e.g. "critique your own draft").
    SelfCritique,
    /// Parallel multi-file search signaled (e.g. "search across all").
    ParallelSearch,
    /// Wave OR: explicit sequential dependency signaled (e.g.
    /// "find a, then do b"). Routes to `sequential_pipeline`.
    Sequential,
}

/// Output of the classifier. Cheap to clone.
#[derive(Debug, Clone)]
pub struct Intent {
    pub task: String,
    pub complexity: Complexity,
    pub shape: Shape,
}

/// Explicit user override that supersedes the classifier's decision.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// Force a single-agent direct call.
    Direct,
    /// Force a parallel fanout of 3 workers.
    Parallel,
    /// Force a sequential pipeline.
    Sequential,
    /// Force a self-critique loop.
    SelfCritique,
    /// Defer to the classifier (same as `None`).
    Auto,
}

pub struct IntentClassifier;

impl IntentClassifier {
    /// Classify a task description into an [`Intent`]. Synchronous and
    /// dependency-free — purely a keyword pass over the lowercased
    /// task string.
    pub fn classify(task: &str) -> Intent {
        let lower = task.to_lowercase();

        // Shape signals (checked first; they're high-confidence).
        let shape = if contains_any(
            &lower,
            &["critique", "self-review", "self review", "revise your"],
        ) {
            Shape::SelfCritique
        } else if contains_any(
            &lower,
            &["search across all", "find every", "fan out", "in parallel"],
        ) {
            Shape::ParallelSearch
        } else if contains_any(
            &lower,
            // Wave OR: explicit sequential dependency markers. We
            // require BOTH a sequencing word ("then", "after that")
            // and a preceding action verb to avoid false positives on
            // generic "and then ...". The heuristic below approximates
            // that by requiring the phrase ", then " or "first ... then"
            // — clear dependency signals.
            &[", then ", " first ", " then do ", "after that"],
        ) {
            Shape::Sequential
        } else {
            Shape::Default
        };

        // Complexity heuristics: highest-priority match wins, in order.
        let complexity = if contains_any(
            &lower,
            &[
                "refactor",
                "redesign",
                "rewrite",
                "migrate",
                "architecture",
                "cross-module",
                "across the codebase",
                "system-wide",
            ],
        ) {
            Complexity::Complex
        } else if contains_any(
            &lower,
            &[
                "search across",
                "find every",
                "scan all",
                "all files",
                "every file",
                "multi-file",
                "across files",
            ],
        ) {
            Complexity::Moderate
        } else if contains_any(
            &lower,
            &[
                "typo",
                "rename",
                "fix typo",
                "fix the typo",
                "log line",
                "log statement",
                "fix typo in",
                "one-liner",
            ],
        ) {
            Complexity::Trivial
        } else {
            // Default: short tasks → Trivial; longer or multi-clause → Moderate.
            let word_count = lower.split_whitespace().count();
            if word_count <= 6 {
                Complexity::Trivial
            } else {
                Complexity::Moderate
            }
        };

        Intent {
            task: task.to_string(),
            complexity,
            shape,
        }
    }
}

fn contains_any(haystack: &str, needles: &[&str]) -> bool {
    needles.iter().any(|n| haystack.contains(n))
}

/// Dynamic Workflows B3 — a telemetry-only signal that a turn *looks
/// like* something a Fleet workflow could handle (a fan-out, a
/// multi-step audit, a migration, a "be comprehensive / across all
/// files" task).
///
/// This is NOT a routing input. It is produced at the engine's existing
/// intent-telemetry seam, gated behind `observability.workflow_detection_enabled`
/// (default off), and consumed by nothing in v1 except telemetry. The
/// confirm gate that turns this into a user-facing proposal lands in B6.
#[derive(Debug, Clone, PartialEq)]
pub struct WorkflowCandidate {
    /// Heuristic confidence in `[0.0, 1.0]`. Higher means more / stronger
    /// workflow signals fired. Derived purely from keyword/pattern hits —
    /// it is a ranking aid, not a calibrated probability.
    pub confidence: f32,
    /// Human-readable explanation of which signals fired. Used in shadow
    /// logs (B4) and the eventual proposal card (B6).
    pub rationale: String,
}

/// Detect whether `task` looks like a workflow candidate.
///
/// # Heuristic
///
/// Pure, deterministic, allocation-light keyword/pattern pass over the
/// lowercased task. Two signal tiers:
///
/// - **Strong signals** (weight 2): unambiguous fan-out / breadth /
///   migration phrasing — `"every file"`, `"across all"`, `"all files"`,
///   `"fan out"`, `"for each"`, `"audit the entire"`, `"migrate all"`,
///   `"in parallel"`, `"comprehensively"`, etc. A single strong signal is
///   enough to flag a candidate.
/// - **Weak signals** (weight 1): phrasing that *co-occurs* with workflows
///   but is too generic to flag alone — a bare `"audit"`/`"migrate"`/
///   `"comprehensive"` verb, an explicit multi-step enumeration
///   (`"1. ... 2. ..."` or `"step 1"`), or breadth words like `"all of
///   the"` / `"each of"`. Two independent weak signals together cross the
///   bar.
///
/// `confidence` is `min(1.0, score / SATURATION)` where `score` is the
/// summed signal weights and `SATURATION = 4.0` (≈ two strong signals
/// reaches full confidence). A candidate is returned only when
/// `score >= 2` — i.e. one strong signal OR two weak signals. Ordinary
/// single-task turns score 0–1 and return `None`.
///
/// The function never panics, never allocates beyond the lowercased copy
/// and the rationale `String`, and is independent of `classify` /
/// `select` — it shares no state and cannot perturb routing.
pub fn workflow_candidate(task: &str) -> Option<WorkflowCandidate> {
    const SATURATION: f32 = 4.0;
    const STRONG_WEIGHT: f32 = 2.0;
    const WEAK_WEIGHT: f32 = 1.0;
    const THRESHOLD: f32 = 2.0;

    let lower = task.to_lowercase();

    // Strong, unambiguous workflow phrasing. Each hit is a near-certain
    // breadth / fan-out / migration marker on its own.
    const STRONG: &[&str] = &[
        "every file",
        "across all",
        "across every",
        "all files",
        "all of the files",
        "fan out",
        "fan-out",
        "for each",
        "audit the entire",
        "audit the whole",
        "scan the entire",
        "scan the whole",
        "migrate all",
        "migrate every",
        "in parallel",
        "comprehensively",
        "one by one",
        "across the whole codebase",
        "across the entire codebase",
        "throughout the codebase",
        "everywhere in the codebase",
    ];

    // Weak signals: workflow-adjacent but too generic to flag alone.
    const WEAK: &[&str] = &[
        "all of the",
        "each of",
        "comprehensive",
        " audit",
        "migrate ",
        "migration",
        "step 1",
        "step-by-step",
        "be thorough",
        "be exhaustive",
        "exhaustive",
    ];

    let mut score = 0.0_f32;
    let mut hits: Vec<&str> = Vec::new();

    for needle in STRONG {
        if lower.contains(needle) {
            score += STRONG_WEIGHT;
            hits.push(needle);
        }
    }
    for needle in WEAK {
        if lower.contains(needle) {
            score += WEAK_WEIGHT;
            hits.push(needle.trim());
        }
    }

    // Multi-step enumeration: an ordered list like "1. ... 2. ..." or
    // "first ... second ...". Detected structurally (not as a keyword) so
    // it complements the keyword tiers. Counts as one weak signal.
    if has_multi_step_enumeration(&lower) {
        score += WEAK_WEIGHT;
        hits.push("multi-step enumeration");
    }

    if score < THRESHOLD {
        return None;
    }

    let confidence = (score / SATURATION).min(1.0);
    let rationale = format!("workflow signals: {}", hits.join(", "));
    Some(WorkflowCandidate {
        confidence,
        rationale,
    })
}

/// Detect an ordered multi-step enumeration in already-lowercased text.
/// Returns true when at least two distinct ordinal markers appear in
/// sequence — e.g. `"1." … "2."`, or `"first," … "second,"`. Cheap and
/// deterministic; no regex.
fn has_multi_step_enumeration(lower: &str) -> bool {
    // Numbered list: needs both "1" and "2" as list markers ("1." / "1)").
    let numbered = (lower.contains("1.") || lower.contains("1)"))
        && (lower.contains("2.") || lower.contains("2)"));
    // Ordinal words in sequence.
    let ordinal = lower.contains("first") && (lower.contains("second") || lower.contains("then "));
    numbered || ordinal
}

pub struct LoopSelector;

impl LoopSelector {
    /// Pick a graph template for `intent`. The user `mode_override`
    /// (when `Some` and not `Auto`) wins over the classifier's
    /// decision.
    pub fn select(intent: &Intent, mode_override: Option<Mode>) -> GraphConfig {
        // Explicit overrides short-circuit the classifier.
        match mode_override {
            Some(Mode::Direct) => return GraphConfig::direct("main", serde_json::json!({})),
            Some(Mode::Parallel) => {
                return GraphConfig::parallel_fanout(
                    vec!["worker_a", "worker_b", "worker_c"],
                    AggregationStrategy::MergeObjects,
                );
            }
            Some(Mode::Sequential) => {
                return GraphConfig::sequential_pipeline(vec![
                    ("step1", super::graph::InputMapper::PassThrough),
                    ("step2", super::graph::InputMapper::PassThrough),
                    ("step3", super::graph::InputMapper::PassThrough),
                ]);
            }
            Some(Mode::SelfCritique) => {
                return GraphConfig::self_critique("doer", "critic", 3);
            }
            Some(Mode::Auto) | None => {} // fall through to intent-driven
        }

        // Shape signal can flip the default routing only when the
        // task carries a HIGH-confidence keyword (e.g. "critique your
        // own draft", "search across all"). These keywords are
        // exclusion-tested against existing fixtures so opting in is
        // explicit. Without an override AND without a shape keyword,
        // the selector returns Direct — preserving byte-identical
        // behaviour for every pre-Wave-OR caller (default path = single
        // agent call). The Complexity field remains useful for
        // telemetry and future routing experiments, but it does NOT
        // change the default template on its own. Routing on raw
        // complexity (e.g. "all long prompts go to a planner+worker
        // pipeline") would be a behavioural change every existing
        // engine consumer would have to opt out of; that breaks the
        // wave's "Direct = byte-identical" invariant.
        if intent.shape == Shape::SelfCritique {
            return GraphConfig::self_critique("doer", "critic", 3);
        }
        if intent.shape == Shape::ParallelSearch {
            return GraphConfig::parallel_fanout(
                vec!["search_a", "search_b", "search_c"],
                AggregationStrategy::MergeObjects,
            );
        }
        if intent.shape == Shape::Sequential {
            return GraphConfig::sequential_pipeline(vec![
                ("step1", super::graph::InputMapper::PassThrough),
                ("step2", super::graph::InputMapper::PassThrough),
            ]);
        }

        // Default: every task without an explicit override or shape
        // keyword routes through the Direct template — a single
        // AgentCall named `"main"` that wraps the engine's existing
        // tool-call dispatch. This is the "auto-classify -> Direct"
        // case the engine's `run` loop exercises every turn.
        GraphConfig::direct("main", serde_json::json!({}))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classify_short_task_defaults_trivial() {
        let i = IntentClassifier::classify("do thing");
        assert_eq!(i.complexity, Complexity::Trivial);
        assert_eq!(i.shape, Shape::Default);
    }

    #[test]
    fn classify_long_task_defaults_moderate() {
        let i =
            IntentClassifier::classify("do this thing and that thing and another thing too please");
        assert_eq!(i.complexity, Complexity::Moderate);
    }

    #[test]
    fn selector_direct_for_trivial() {
        let i = Intent {
            task: "x".into(),
            complexity: Complexity::Trivial,
            shape: Shape::Default,
        };
        assert!(LoopSelector::select(&i, None).is_direct());
    }

    #[test]
    fn selector_self_critique_shape_overrides_complexity() {
        let i = Intent {
            task: "x".into(),
            complexity: Complexity::Trivial,
            shape: Shape::SelfCritique,
        };
        // SelfCritique routes to a self-critique graph regardless of complexity.
        let cfg = LoopSelector::select(&i, None);
        assert!(!cfg.is_direct());
    }

    // ===================================================================
    // Dynamic Workflows B3 — `workflow_candidate` detection signal.
    // ===================================================================

    /// Labeled corpus: `(task, is_workflow)`. POSITIVE = a fan-out /
    /// multi-step audit / migration / "be comprehensive / across all
    /// files" task a workflow could handle. NEGATIVE = an ordinary
    /// single-task turn. Used to measure precision/recall and lock the
    /// heuristic's behaviour.
    const LABELED: &[(&str, bool)] = &[
        // --- POSITIVES (workflow-shaped) ---
        ("Audit the entire codebase for unwrap() calls", true),
        ("Add a docstring to every file in src/", true),
        ("Run the linter across all crates and fix warnings", true),
        ("Fan out a search for TODO markers in the repo", true),
        ("For each module, write a unit test", true),
        ("Migrate all callers from the old API to the new one", true),
        ("Comprehensively review the security of the auth flow", true),
        ("Scan the entire project for hardcoded secrets", true),
        ("Process all files in parallel and summarize each", true),
        ("Update every file that imports the deprecated helper", true),
        ("Do a comprehensive migration of the config format", true),
        (
            "Go through the modules one by one and add error handling",
            true,
        ),
        ("Rename the symbol across the whole codebase", true),
        (
            "First, list all the endpoints. Second, audit each for auth.",
            true,
        ),
        // --- NEGATIVES (ordinary single tasks) ---
        ("Fix the typo in README line 12", false),
        ("Rename the variable foo to bar in main.rs", false),
        ("Add a log line to the request handler", false),
        ("What does this function do?", false),
        ("Write a hello world program", false),
        ("Bump the version to 1.2.0", false),
        ("Explain the difference between Box and Rc", false),
        ("Format this file", false),
        ("Add a new field to the Config struct", false),
        ("Run the tests", false),
        ("Refactor this function to be shorter", false),
    ];

    #[test]
    fn workflow_candidate_precision_recall_on_labeled_set() {
        let mut tp = 0usize; // predicted workflow, is workflow
        let mut fp = 0usize; // predicted workflow, is NOT workflow
        let mut f_n = 0usize; // predicted not, IS workflow
        let mut tn = 0usize; // predicted not, is not

        for (task, is_workflow) in LABELED {
            let predicted = workflow_candidate(task).is_some();
            match (predicted, *is_workflow) {
                (true, true) => tp += 1,
                (true, false) => fp += 1,
                (false, true) => f_n += 1,
                (false, false) => tn += 1,
            }
        }

        let precision = tp as f32 / (tp + fp).max(1) as f32;
        let recall = tp as f32 / (tp + f_n).max(1) as f32;

        // The heuristic is conservative-by-design: zero false positives
        // (we never flag an ordinary turn) and high recall on the
        // workflow-shaped set. These thresholds lock the behaviour;
        // loosening them must be a deliberate edit.
        assert_eq!(
            fp, 0,
            "false positives on ordinary tasks: precision={precision}, tn={tn}"
        );
        assert!(
            precision >= 0.99,
            "precision below bar: {precision} (tp={tp} fp={fp})"
        );
        assert!(
            recall >= 0.85,
            "recall below bar: {recall} (tp={tp} fn={f_n})"
        );
    }

    #[test]
    fn workflow_candidate_none_for_ordinary_short_tasks() {
        for task in &[
            "fix typo",
            "rename x to y",
            "add a comment",
            "what time is it",
            "build the project",
        ] {
            assert!(
                workflow_candidate(task).is_none(),
                "ordinary task wrongly flagged: {task:?}"
            );
        }
    }

    #[test]
    fn workflow_candidate_strong_signal_alone_is_enough() {
        // A single strong signal crosses the threshold.
        let c = workflow_candidate("touch every file please").expect("strong signal");
        assert!(c.confidence > 0.0 && c.confidence <= 1.0);
        assert!(c.rationale.contains("every file"));
    }

    #[test]
    fn workflow_candidate_single_weak_signal_is_not_enough() {
        // A bare "audit" (one weak signal) must NOT flag — too generic.
        assert!(workflow_candidate("audit this function").is_none());
    }

    #[test]
    fn workflow_candidate_confidence_in_unit_range_and_monotone() {
        // More signals => higher (or equal) confidence, always in [0,1].
        let one = workflow_candidate("review every file").expect("strong");
        let many = workflow_candidate(
            "comprehensively audit the entire codebase across all files in parallel, one by one",
        )
        .expect("many strong");
        assert!((0.0..=1.0).contains(&one.confidence));
        assert!((0.0..=1.0).contains(&many.confidence));
        assert!(many.confidence >= one.confidence);
        assert_eq!(many.confidence, 1.0, "saturated signal should reach 1.0");
    }

    #[test]
    fn workflow_candidate_multi_step_enumeration_alone_is_weak() {
        // A numbered enumeration is one weak signal — not enough alone.
        assert!(workflow_candidate("1. open the file 2. read it").is_none());
        // ...but combined with a weak verb it crosses the bar.
        assert!(workflow_candidate("1. audit the file 2. migrate it").is_some());
    }

    /// No-regression lock: `workflow_candidate` is a pure side-channel.
    /// Computing it must not change what `IntentClassifier::classify`
    /// returns for the same input (they share no state). This guards the
    /// B3 invariant that the detection signal cannot perturb routing.
    #[test]
    fn workflow_candidate_does_not_perturb_classify() {
        for (task, _) in LABELED {
            let before = IntentClassifier::classify(task);
            let _ = workflow_candidate(task);
            let after = IntentClassifier::classify(task);
            assert_eq!(before.complexity, after.complexity, "task={task:?}");
            assert_eq!(before.shape, after.shape, "task={task:?}");
        }
    }
}
