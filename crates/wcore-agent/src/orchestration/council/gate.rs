//! Council gating — the "should I even convene a council?" decision.
//!
//! A council costs N× a single call, so firing one for every request is just an
//! expensive worse-than-direct path. This is the Fugu **Conductor** principle:
//! a trivial ask ("what day is today?") should answer with one direct call,
//! while a high-stakes / complex ask ("cross-audit this security plan") warrants
//! the full cross-provider council.
//!
//! Slice-1 is a **cheap, deterministic heuristic** (no LLM, no token spend):
//! a council convenes only on a positive complexity / stakes signal — a curated
//! keyword or a long enough task. Everything else routes Direct, so the common
//! case never pays the council premium. The classifier returns a human-readable
//! reason on both arms so the CLI / desktop echo can explain the routing.
//!
//! A later slice can layer a learned router (the Thompson-sampling
//! `TemplateRouter` in `wcore-dispatch`) or a 1× cheap-model complexity score on
//! top of this floor; the heuristic stays as the zero-cost fast path.

/// Whether a task warrants a council, with the reason for the routing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CouncilDecision {
    /// Convene the full cross-provider council.
    Council { reason: String },
    /// Answer with a single direct call — the task does not warrant a council.
    Direct { reason: String },
}

impl CouncilDecision {
    /// The reason string, regardless of arm.
    pub fn reason(&self) -> &str {
        match self {
            CouncilDecision::Council { reason } | CouncilDecision::Direct { reason } => reason,
        }
    }

    /// Whether this decision convenes a council.
    pub fn is_council(&self) -> bool {
        matches!(self, CouncilDecision::Council { .. })
    }
}

/// Tunables for the heuristic gate. Defaults favor cost: a council convenes only
/// on a positive signal; absent any signal a task routes Direct.
#[derive(Debug, Clone)]
pub struct GateConfig {
    /// Lowercased substrings that signal high-stakes / complex work. A match on
    /// any one routes the task to a council.
    pub council_signals: Vec<String>,
    /// Word count at/above which a task is treated as complex enough to council
    /// even without a keyword signal.
    pub council_word_threshold: usize,
}

/// Curated high-stakes / complexity markers. These are *defaults*, not a
/// hardcoded policy — `GateConfig` is fully overridable. Kept lowercase; the
/// classifier lowercases the task before matching. Substrings (not whole words)
/// so e.g. `vulnerab` catches "vulnerable" / "vulnerability".
const DEFAULT_COUNCIL_SIGNALS: &[&str] = &[
    "audit",
    "security",
    "secure",
    "vulnerab",
    "threat",
    "exploit",
    "injection",
    "review",
    "cross-check",
    "cross check",
    "crosscheck",
    "double-check",
    "double check",
    "critique",
    "design",
    "architect",
    "refactor",
    "debug",
    "root cause",
    "root-cause",
    "trade-off",
    "tradeoff",
    "trade off",
    "compare",
    "evaluate",
    "assess",
    "analyze",
    "analyse",
    "comprehensive",
    "exhaustive",
    "thorough",
    "strategy",
    "prove",
    "verify",
    "correctness",
    "race condition",
    "edge case",
    "edge-case",
    "migrate",
    "migration",
];

/// Default word count above which an un-keyworded task is deemed complex enough
/// to council. A long prompt usually encodes a multi-part / nuanced task.
const DEFAULT_COUNCIL_WORD_THRESHOLD: usize = 40;

impl Default for GateConfig {
    fn default() -> Self {
        Self {
            council_signals: DEFAULT_COUNCIL_SIGNALS
                .iter()
                .map(|s| s.to_string())
                .collect(),
            council_word_threshold: DEFAULT_COUNCIL_WORD_THRESHOLD,
        }
    }
}

/// Classify whether `task` warrants a council. Deterministic and cheap (no LLM):
///
/// 1. **Keyword signal** — if the (lowercased) task contains any
///    `council_signals` substring, convene (the strongest signal).
/// 2. **Length** — else if the task has `council_word_threshold` words or more,
///    convene (long ⇒ likely complex / multi-part).
/// 3. Otherwise route **Direct** — a short, low-stakes ask the council premium
///    would be wasted on.
pub fn classify_task(task: &str, cfg: &GateConfig) -> CouncilDecision {
    let lower = task.to_lowercase();

    if let Some(sig) = cfg.council_signals.iter().find(|s| lower.contains(*s)) {
        return CouncilDecision::Council {
            reason: format!("matched high-stakes signal '{sig}'"),
        };
    }

    let words = task.split_whitespace().count();
    if words >= cfg.council_word_threshold {
        return CouncilDecision::Council {
            reason: format!("long task ({words} words ≥ {})", cfg.council_word_threshold),
        };
    }

    CouncilDecision::Direct {
        reason: format!("short, low-stakes task ({words} words, no high-stakes signal)"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn classify(task: &str) -> CouncilDecision {
        classify_task(task, &GateConfig::default())
    }

    #[test]
    fn trivial_factual_asks_route_direct() {
        for task in [
            "What day is today?",
            "what time is it",
            "hi",
            "ping",
            "what is 2 + 2",
            "convert 10 miles to km",
        ] {
            assert!(
                !classify(task).is_council(),
                "trivial ask should route Direct: {task:?}"
            );
        }
    }

    #[test]
    fn high_stakes_keywords_route_to_council() {
        for task in [
            "Do a detailed security audit of this deployment plan",
            "Cross-check the correctness of this migration",
            "Review this code for vulnerabilities",
            "Design the architecture for the new service",
            "Help me debug this race condition",
            "Compare Postgres vs MySQL for our workload",
        ] {
            let d = classify(task);
            assert!(d.is_council(), "should convene: {task:?} → {d:?}");
        }
    }

    #[test]
    fn keyword_match_is_case_insensitive() {
        assert!(classify("SECURITY AUDIT of the plan").is_council());
        assert!(classify("Threat-Model This Design").is_council());
    }

    #[test]
    fn long_task_without_keyword_routes_to_council() {
        // 40+ words, no signal keyword → council on length alone.
        let task = "please take this list of grocery items and for each one tell me \
                    a single fun fact about where it tends to come from in the world \
                    and roughly how long it lasts in a normal home fridge or pantry \
                    so i can plan my weekly shopping list a bit better than usual now";
        assert!(task.split_whitespace().count() >= 40);
        let d = classify(task);
        assert!(d.is_council(), "long task should convene: {d:?}");
        assert!(d.reason().contains("long task"));
    }

    #[test]
    fn medium_task_without_signal_routes_direct() {
        // Under the word threshold and no keyword → Direct.
        let task = "summarize the plot of this short paragraph in one sentence";
        assert!(task.split_whitespace().count() < 40);
        assert!(!classify(task).is_council());
    }

    #[test]
    fn word_threshold_boundary_is_inclusive() {
        // No signals (isolate the length rule) + a low threshold.
        let cfg = GateConfig {
            council_signals: Vec::new(),
            council_word_threshold: 5,
        };
        // Exactly 5 words → council (>= threshold).
        assert!(classify_task("one two three four five", &cfg).is_council());
        // 4 words → direct.
        assert!(!classify_task("one two three four", &cfg).is_council());
    }

    #[test]
    fn decision_reason_is_populated_on_both_arms() {
        assert!(!classify("audit this").reason().is_empty());
        assert!(!classify("hello").reason().is_empty());
    }

    #[test]
    fn custom_signals_override_defaults() {
        let cfg = GateConfig {
            council_signals: vec!["banana".to_string()],
            council_word_threshold: 100,
        };
        // The default "audit" no longer triggers; only the custom signal does.
        assert!(!classify_task("audit this thing", &cfg).is_council());
        assert!(classify_task("inspect this banana", &cfg).is_council());
    }
}
