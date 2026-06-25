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

/// How much a convened council is worth — drives the member-count ladder and the
/// budget tier. `Low` never convenes (it routes Direct); a council carries `Med`
/// or `High`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stakes {
    /// Not worth a council — answer directly.
    Low,
    /// Analysis / design / comparison — a modest council.
    Med,
    /// Security / correctness / irreversible — the widest council.
    High,
}

/// Whether a task warrants a council, with the reason for the routing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CouncilDecision {
    /// Convene the cross-provider council, sized to `stakes`.
    Council { reason: String, stakes: Stakes },
    /// Answer with a single direct call — the task does not warrant a council.
    Direct { reason: String },
}

impl CouncilDecision {
    /// The reason string, regardless of arm.
    pub fn reason(&self) -> &str {
        match self {
            CouncilDecision::Council { reason, .. } | CouncilDecision::Direct { reason } => reason,
        }
    }

    /// Whether this decision convenes a council.
    pub fn is_council(&self) -> bool {
        matches!(self, CouncilDecision::Council { .. })
    }

    /// The stakes if this convenes a council; `Low` for a Direct decision.
    pub fn stakes(&self) -> Stakes {
        match self {
            CouncilDecision::Council { stakes, .. } => *stakes,
            CouncilDecision::Direct { .. } => Stakes::Low,
        }
    }
}

/// How many proposers a council of the given `stakes` targets, clamped to the
/// configured `max_proposers` cap and the number of available `n_candidates`.
/// `Low` → 0 (no council), `Med` → 3, `High` → 5.
pub fn member_count(stakes: Stakes, max_proposers: usize, n_candidates: usize) -> usize {
    let target = match stakes {
        Stakes::Low => 0,
        Stakes::Med => 3,
        Stakes::High => 5,
    };
    target.min(max_proposers).min(n_candidates)
}

/// Tunables for the heuristic gate. Defaults favor cost: a council convenes only
/// on a positive signal; absent any signal a task routes Direct.
#[derive(Debug, Clone)]
pub struct GateConfig {
    /// Lowercased substrings that signal council-worthy work. A match on any one
    /// routes the task to a council. (Union of high + med markers by default.)
    pub council_signals: Vec<String>,
    /// The subset of `council_signals` that signal HIGH stakes (security /
    /// correctness / irreversible). A match here classifies the council `High`;
    /// any other signal match (or length alone) classifies it `Med`.
    pub high_signals: Vec<String>,
    /// Word count at/above which a task is treated as complex enough to council
    /// even without a keyword signal.
    pub council_word_threshold: usize,
}

/// Curated HIGH-stakes markers — security / correctness / irreversible work
/// where getting it wrong is costly. *Defaults*, not policy; `GateConfig` is
/// overridable. Lowercase substrings (so `vulnerab` catches "vulnerability").
const DEFAULT_HIGH_SIGNALS: &[&str] = &[
    "audit",
    "security",
    "secure",
    "vulnerab",
    "threat",
    "exploit",
    "injection",
    "prove",
    "verify",
    "correctness",
    "race condition",
    "root cause",
    "root-cause",
    "migrate",
    "migration",
    // Destructive / irreversible operations — getting these wrong loses data.
    // Distinctive multi-word terms so they don't fire on benign "delete a line".
    "rm -rf",
    "force-push",
    "force push",
    "drop table",
    "drop database",
    "truncate table",
    "delete from",
    "data loss",
    "irreversible",
    "destructive",
    "wipe",
];

/// Curated MED-stakes markers — analysis / design / comparison / review.
const DEFAULT_MED_SIGNALS: &[&str] = &[
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
    "edge case",
    "edge-case",
];

/// Default word count above which an un-keyworded task is deemed complex enough
/// to council. A long prompt usually encodes a multi-part / nuanced task.
const DEFAULT_COUNCIL_WORD_THRESHOLD: usize = 40;

impl Default for GateConfig {
    fn default() -> Self {
        Self {
            council_signals: DEFAULT_HIGH_SIGNALS
                .iter()
                .chain(DEFAULT_MED_SIGNALS)
                .map(|s| s.to_string())
                .collect(),
            high_signals: DEFAULT_HIGH_SIGNALS.iter().map(|s| s.to_string()).collect(),
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
    let words = task.split_whitespace().count();

    if let Some(sig) = cfg.council_signals.iter().find(|s| lower.contains(*s)) {
        // Classify stakes from the INSTRUCTION SPAN — the leading words — not the
        // whole body. A high-stakes keyword that appears only deep in a long
        // pasted body (a log, a stack trace, code) is far more likely incidental
        // content than the user's actual instruction, so it must NOT escalate to
        // High. The span is the whole task when short, else its leading
        // `council_word_threshold` words. (Counting keyword *presence* across the
        // whole body is the wrong signal — it rises with body length, exactly
        // backwards from "ambiguity biases lower".)
        let span: String = task
            .split_whitespace()
            .take(cfg.council_word_threshold.max(1))
            .collect::<Vec<_>>()
            .join(" ")
            .to_lowercase();
        let high_in_span = cfg.high_signals.iter().any(|h| span.contains(h));
        let stakes = if high_in_span {
            Stakes::High
        } else {
            Stakes::Med
        };
        return CouncilDecision::Council {
            reason: format!("matched signal '{sig}' → {stakes:?} stakes"),
            stakes,
        };
    }

    if words >= cfg.council_word_threshold {
        return CouncilDecision::Council {
            reason: format!(
                "long task ({words} words ≥ {}) → Med stakes",
                cfg.council_word_threshold
            ),
            stakes: Stakes::Med,
        };
    }

    CouncilDecision::Direct {
        reason: format!("short, low-stakes task ({words} words, no council signal)"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn classify(task: &str) -> CouncilDecision {
        classify_task(task, &GateConfig::default())
    }

    fn stakes_of(task: &str) -> Option<Stakes> {
        match classify(task) {
            CouncilDecision::Council { stakes, .. } => Some(stakes),
            CouncilDecision::Direct { .. } => None,
        }
    }

    #[test]
    fn high_stakes_signals_classify_high() {
        for t in [
            "do a security audit of this deployment",
            "prove the correctness of this algorithm",
            "help me fix this race condition",
            "review for injection vulnerabilities",
        ] {
            assert_eq!(stakes_of(t), Some(Stakes::High), "{t:?}");
        }
    }

    #[test]
    fn analysis_signals_classify_med() {
        for t in [
            "compare postgres vs mysql for our workload",
            "analyze this function's complexity",
            "design the layout of the new dashboard",
            "refactor this module",
        ] {
            assert_eq!(stakes_of(t), Some(Stakes::Med), "{t:?}");
        }
    }

    #[test]
    fn length_only_classifies_med_and_none_routes_direct() {
        let long = "please take this list of grocery items and for each one tell me \
                    a single fun fact about where it tends to come from in the world \
                    and roughly how long it lasts in a normal home fridge or pantry \
                    so i can plan my weekly shopping list a bit better than usual now";
        assert!(long.split_whitespace().count() >= 40);
        assert_eq!(stakes_of(long), Some(Stakes::Med));
        assert_eq!(stakes_of("what time is it"), None);
    }

    #[test]
    fn high_keyword_outside_instruction_span_stays_med() {
        // A benign instruction followed by a long pasted body containing high
        // keywords must NOT escalate to High — the high signal lands outside the
        // leading instruction span, so it's treated as incidental content.
        let mut body = String::from("please summarize the following log output for me ");
        for _ in 0..60 {
            body.push_str("normal line ");
        }
        // High ("exploit", "migration") + incidental Med ("review", "compare")
        // keywords, ALL beyond the instruction span. The old n_hits guard would
        // have escalated this (>=2 distinct hits); span-based must keep it Med.
        body.push_str("running migration exploit scanner please review and compare baseline ");
        for _ in 0..20 {
            body.push_str("more lines ");
        }
        assert!(body.split_whitespace().count() > 2 * 40);
        assert_eq!(
            stakes_of(&body),
            Some(Stakes::Med),
            "high keywords only in a long pasted body must not escalate to High"
        );
    }

    #[test]
    fn high_keyword_in_instruction_span_classifies_high() {
        // The high signal in the LEADING span is a real instruction → High, even
        // when the task is long.
        assert_eq!(stakes_of("audit this for exploits"), Some(Stakes::High));
        let mut t = String::from("do a thorough security audit of the auth flow then ");
        for _ in 0..60 {
            t.push_str("here is some context ");
        }
        assert_eq!(stakes_of(&t), Some(Stakes::High));
    }

    #[test]
    fn destructive_irreversible_ops_classify_high() {
        for t in [
            "drop database production and recreate it",
            "run rm -rf on the build dir",
            "force-push to main after the rebase",
        ] {
            assert_eq!(stakes_of(t), Some(Stakes::High), "{t:?}");
        }
    }

    #[test]
    fn member_count_clamps_to_max_and_candidates() {
        assert_eq!(member_count(Stakes::High, 5, 10), 5);
        assert_eq!(member_count(Stakes::High, 5, 3), 3); // clamp to candidates
        assert_eq!(member_count(Stakes::High, 2, 10), 2); // clamp to max
        assert_eq!(member_count(Stakes::Med, 5, 10), 3);
        assert_eq!(member_count(Stakes::Low, 5, 10), 0);
        assert_eq!(member_count(Stakes::Med, 5, 1), 1); // candidates-limited
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
            high_signals: Vec::new(),
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
            high_signals: Vec::new(),
            council_word_threshold: 100,
        };
        // The default "audit" no longer triggers; only the custom signal does.
        assert!(!classify_task("audit this thing", &cfg).is_council());
        assert!(classify_task("inspect this banana", &cfg).is_council());
    }
}
