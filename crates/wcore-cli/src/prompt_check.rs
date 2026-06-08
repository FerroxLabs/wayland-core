// T3-6: Prompt vagueness check.
//
// Deterministic vague-prompt detector ported from
// `ijfw/mcp-server/src/prompt-check.js`. Pure functions, no I/O — safe to
// call from a CLI pre-dispatch hook or an MCP tool handler.
//
// Design constraints (mirrors the JS source):
//   - No LLM calls, no network. Pure regex.
//   - Fire only when >=2 signals trip AND prompt is short AND has no target.
//   - Single-signal trips are silent (low false-positive rate).
//   - Override: leading `*`, `/`, `#`, or substring "ijfw off" bypasses entirely.
//   - Positive framing in any user-visible suggestion.
//
// Public API:
//   - `check_prompt(text: &str) -> VaguenessReport`
//   - `bypass_reason(text: &str) -> Option<BypassReason>`
//   - `RULES` slice for introspection / tests.

use std::sync::OnceLock;

use regex::Regex;
use serde::{Deserialize, Serialize};

/// One of the 7 vagueness signals from the research-derived taxonomy.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Signal {
    BareVerb,
    UnresolvedAnaphora,
    AbstractGoal,
    NoTarget,
    ScopePlural,
    Polysemous,
    MissingConstraint,
}

impl Signal {
    /// Stable string id matching the JS source (used for serialization
    /// and human-readable reporting).
    pub fn as_id(&self) -> &'static str {
        match self {
            Signal::BareVerb => "bare_verb",
            Signal::UnresolvedAnaphora => "unresolved_anaphora",
            Signal::AbstractGoal => "abstract_goal",
            Signal::NoTarget => "no_target",
            Signal::ScopePlural => "scope_plural",
            Signal::Polysemous => "polysemous",
            Signal::MissingConstraint => "missing_constraint",
        }
    }
}

/// Why a prompt was bypassed entirely (no signals computed).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum BypassReason {
    Empty,
    AsteriskPrefix,
    SlashCommand,
    MemorizePrefix,
    OverrideKeyword,
    LongPrompt,
    FencedCode,
}

impl BypassReason {
    pub fn as_str(&self) -> &'static str {
        match self {
            BypassReason::Empty => "empty",
            BypassReason::AsteriskPrefix => "asterisk-prefix",
            BypassReason::SlashCommand => "slash-command",
            BypassReason::MemorizePrefix => "memorize-prefix",
            BypassReason::OverrideKeyword => "override-keyword",
            BypassReason::LongPrompt => "long-prompt",
            BypassReason::FencedCode => "fenced-code",
        }
    }
}

/// Structured report returned by [`check_prompt`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VaguenessReport {
    /// True when the prompt should be challenged (>=2 signals, short, no target).
    pub vague: bool,
    /// Every rule that tripped, in canonical order.
    pub signals: Vec<Signal>,
    /// Positive-framed one-liner the agent can surface.
    pub suggestion: String,
    /// ≤3 clarifying questions mapped from the tripped signals.
    pub rewrite: Vec<String>,
    /// Set when the prompt was bypassed (Override / empty / long / etc).
    pub bypass_reason: Option<BypassReason>,
}

impl VaguenessReport {
    /// Score = `signals.len()`, capped at the rule count. Useful for callers
    /// who want a single scalar (matches the JS port's implicit ordering).
    pub fn score(&self) -> usize {
        self.signals.len()
    }
}

/// The canonical id list, exposed for tests and downstream tooling.
pub const RULES: &[&str] = &[
    "bare_verb",
    "unresolved_anaphora",
    "abstract_goal",
    "no_target",
    "scope_plural",
    "polysemous",
    "missing_constraint",
];

// --- regex caches (compiled once) -------------------------------------------

struct Patterns {
    bare_verb: Regex,
    unresolved_anaphora: Regex,
    abstract_word: Regex,
    metric: Regex,
    path: Regex,
    line_number: Regex,
    dir_prefix: Regex,
    identifier: Regex,
    scope_plural: Regex,
    polysemous: Regex,
    constraint_word: Regex,
    number: Regex,
    fenced_code: Regex,
    override_kw: Regex,
}

fn patterns() -> &'static Patterns {
    static P: OnceLock<Patterns> = OnceLock::new();
    P.get_or_init(|| Patterns {
        // Bare imperative — operates on lowercased trimmed text.
        bare_verb: Regex::new(
            r"^(fix|refactor|improve|clean\s*up|optimi[sz]e|update|review|check|test|debug|analy[sz]e|handle|sort\s*out|tidy)\b",
        )
        .unwrap(),
        // Sentence-leading anaphoric reference (case-insensitive).
        unresolved_anaphora: Regex::new(
            r"(?i)^(this|that|it|these|those|the\s+(bug|issue|file|code|function|error|problem))\b",
        )
        .unwrap(),
        abstract_word: Regex::new(
            r"(?i)\b(better|cleaner|nicer|more\s+robust|production[\s\-]?ready|proper|correct|good|nice|right)\b",
        )
        .unwrap(),
        metric: Regex::new(r"(?i)\d+\s*(ms|%|x|kb|mb|sec|s\b|tests?\b|users?\b)").unwrap(),
        // File path like foo.rs / src/main.rs (1–5 char extension, followed
        // by word boundary or `:`).
        path: Regex::new(r"[\w./\-]+\.\w{1,5}(\b|:)").unwrap(),
        line_number: Regex::new(r":\d+").unwrap(),
        dir_prefix: Regex::new(r"(?i)\b(src|lib|app|tests?|spec|docs?)/").unwrap(),
        // snake_case (>=2 segments), UpperCamelCase (>=2 caps), or
        // lowerCamelCase (>=1 internal cap).
        identifier: Regex::new(
            r"\b([a-z]+_[a-z][\w_]*|[A-Z][a-z]+[A-Z]\w*|[a-z]+[A-Z]\w*)\b",
        )
        .unwrap(),
        scope_plural: Regex::new(
            r"(?i)\b(the\s+tests|all\s+the\s+(things|stuff|files)|everything|stuff|things)\b",
        )
        .unwrap(),
        polysemous: Regex::new(r"^(source|build|run|deploy|ship|release|setup|set\s*up)\.?\s*$")
            .unwrap(),
        constraint_word: Regex::new(
            r"(?i)\b(must|should|when|if|until|without|only|always|never|except)\b",
        )
        .unwrap(),
        number: Regex::new(r"\b\d+\b").unwrap(),
        fenced_code: Regex::new(r"(?m)^```").unwrap(),
        override_kw: Regex::new(r"(?i)\bijfw\s+off\b").unwrap(),
    })
}

// --- bypass -----------------------------------------------------------------

/// Mirrors `bypassReason` from prompt-check.js.
pub fn bypass_reason(text: &str) -> Option<BypassReason> {
    let t = text.trim();
    if t.is_empty() {
        return Some(BypassReason::Empty);
    }
    if t.starts_with('*') {
        return Some(BypassReason::AsteriskPrefix);
    }
    if t.starts_with('/') {
        return Some(BypassReason::SlashCommand);
    }
    if t.starts_with('#') {
        return Some(BypassReason::MemorizePrefix);
    }
    let p = patterns();
    if p.override_kw.is_match(t) {
        return Some(BypassReason::OverrideKeyword);
    }
    // Pasted code/stack trace — assume the user knows the target.
    if t.len() > 4000 {
        return Some(BypassReason::LongPrompt);
    }
    if p.fenced_code.is_match(t) {
        return Some(BypassReason::FencedCode);
    }
    None
}

// --- per-rule checks --------------------------------------------------------

fn check_bare_verb(text: &str) -> bool {
    let t = text.trim().to_lowercase();
    let tokens = t.split_whitespace().count();
    if tokens >= 6 {
        return false;
    }
    patterns().bare_verb.is_match(&t)
}

fn check_unresolved_anaphora(text: &str) -> bool {
    patterns().unresolved_anaphora.is_match(text.trim())
}

fn check_abstract_goal(text: &str) -> bool {
    let p = patterns();
    if !p.abstract_word.is_match(text) {
        return false;
    }
    // Mitigations: a metric or a file/dir reference.
    if p.metric.is_match(text) {
        return false;
    }
    if p.path.is_match(text) || p.dir_prefix.is_match(text) {
        return false;
    }
    true
}

fn check_no_target(text: &str) -> bool {
    let p = patterns();
    if p.path.is_match(text) {
        return false;
    }
    if p.line_number.is_match(text) {
        return false;
    }
    if p.dir_prefix.is_match(text) {
        return false;
    }
    if p.identifier.is_match(text) {
        return false;
    }
    true
}

fn check_scope_plural(text: &str) -> bool {
    patterns().scope_plural.is_match(text)
}

fn check_polysemous(text: &str) -> bool {
    let t = text.trim().to_lowercase();
    patterns().polysemous.is_match(&t)
}

fn check_missing_constraint(text: &str) -> bool {
    let token_count = text.split_whitespace().count();
    if token_count < 4 {
        return false;
    }
    let p = patterns();
    let has_constraint = p.constraint_word.is_match(text);
    let has_number = p.number.is_match(text);
    !has_constraint && !has_number
}

fn run_rules(text: &str) -> Vec<Signal> {
    let mut out = Vec::new();
    if check_bare_verb(text) {
        out.push(Signal::BareVerb);
    }
    if check_unresolved_anaphora(text) {
        out.push(Signal::UnresolvedAnaphora);
    }
    if check_abstract_goal(text) {
        out.push(Signal::AbstractGoal);
    }
    if check_no_target(text) {
        out.push(Signal::NoTarget);
    }
    if check_scope_plural(text) {
        out.push(Signal::ScopePlural);
    }
    if check_polysemous(text) {
        out.push(Signal::Polysemous);
    }
    if check_missing_constraint(text) {
        out.push(Signal::MissingConstraint);
    }
    out
}

// --- question pack ----------------------------------------------------------

fn build_question_pack(signals: &[Signal]) -> Vec<String> {
    let mut qs: Vec<String> = Vec::new();
    let push = |qs: &mut Vec<String>, q: &str| {
        if qs.len() < 3 && !qs.iter().any(|existing| existing == q) {
            qs.push(q.to_string());
        }
    };
    for sig in signals {
        if qs.len() >= 3 {
            break;
        }
        match sig {
            Signal::BareVerb | Signal::NoTarget => {
                push(
                    &mut qs,
                    "Which file, function, or line number is the target?",
                );
            }
            Signal::UnresolvedAnaphora => {
                push(
                    &mut qs,
                    "What does \"this/that\" refer to -- a file, a symptom, a prior message?",
                );
            }
            Signal::AbstractGoal => {
                push(
                    &mut qs,
                    "What specifically would \"done\" look like -- a metric, a test passing, or observable behavior?",
                );
            }
            Signal::ScopePlural => {
                push(
                    &mut qs,
                    "Which of \"all the X\" -- do you want every instance, or a specific subset?",
                );
            }
            Signal::MissingConstraint => {
                push(
                    &mut qs,
                    "Any constraints I should respect -- don't touch X, must run in <Y ms, preserve behavior Z?",
                );
            }
            Signal::Polysemous => {
                push(
                    &mut qs,
                    "Which meaning -- e.g. \"deploy\" could mean build, release, push, or run locally?",
                );
            }
        }
    }
    if qs.is_empty() {
        qs.push("What file, function, or acceptance criterion pins the target?".to_string());
    }
    qs.truncate(3);
    qs
}

// --- public entry point -----------------------------------------------------

/// Run the deterministic vagueness check on a user prompt.
///
/// Returns a [`VaguenessReport`]. When `bypass_reason` is set, no rules were
/// evaluated and `vague` is always `false`.
pub fn check_prompt(text: &str) -> VaguenessReport {
    if let Some(reason) = bypass_reason(text) {
        return VaguenessReport {
            vague: false,
            signals: Vec::new(),
            suggestion: String::new(),
            rewrite: Vec::new(),
            bypass_reason: Some(reason),
        };
    }

    let signals = run_rules(text);
    let token_count = text.split_whitespace().count();
    let short = token_count < 30;
    let no_target = signals.contains(&Signal::NoTarget);
    let vague = signals.len() >= 2 && short && no_target;

    let suggestion = if vague {
        if signals.contains(&Signal::BareVerb) && no_target {
            "Sharpening your aim -- which file, function, or symbol? e.g. src/auth.py:145, getUserById, the failing test name.".to_string()
        } else if signals.contains(&Signal::UnresolvedAnaphora) {
            "Anchoring the reference -- which file or recent code do you mean?".to_string()
        } else {
            "Pinning the target -- naming the file, symbol, or expected behavior will sharpen the edit.".to_string()
        }
    } else {
        String::new()
    };

    let rewrite = if vague {
        build_question_pack(&signals)
    } else {
        Vec::new()
    };

    VaguenessReport {
        vague,
        signals,
        suggestion,
        rewrite,
        bypass_reason: None,
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clear_prompt_is_not_vague() {
        // A specific target (file + line + behavior) — should trip none of
        // the firing conditions even if individual signals match.
        let r = check_prompt(
            "Update parse_message in src/agent/parser.rs:120 so it returns Err when len > 4096",
        );
        assert!(!r.vague, "expected clear prompt to be non-vague: {r:?}");
        assert!(r.bypass_reason.is_none());
        // Must not contain NoTarget — the firing gate.
        assert!(!r.signals.contains(&Signal::NoTarget));
    }

    #[test]
    fn vague_bare_verb_with_no_target_fires() {
        let r = check_prompt("fix it");
        assert!(r.vague, "expected vague to fire: {r:?}");
        assert!(r.signals.contains(&Signal::BareVerb));
        assert!(r.signals.contains(&Signal::NoTarget));
        // Suggestion is the bare-verb branch.
        assert!(r.suggestion.starts_with("Sharpening your aim"));
        // Question pack always at least 1, capped at 3.
        assert!(!r.rewrite.is_empty() && r.rewrite.len() <= 3);
    }

    #[test]
    fn anaphora_plus_no_target_fires_with_anaphora_suggestion() {
        // No bare verb (so bare-verb branch doesn't win), but unresolved
        // anaphora at sentence start + no target identifier.
        let r = check_prompt("this is broken somehow");
        assert!(r.vague, "expected vague: {r:?}");
        assert!(r.signals.contains(&Signal::UnresolvedAnaphora));
        assert!(r.signals.contains(&Signal::NoTarget));
        assert!(r.suggestion.starts_with("Anchoring the reference"));
    }

    #[test]
    fn empty_and_whitespace_bypass() {
        let r = check_prompt("");
        assert_eq!(r.bypass_reason, Some(BypassReason::Empty));
        assert!(!r.vague);

        let r2 = check_prompt("   \n\t ");
        assert_eq!(r2.bypass_reason, Some(BypassReason::Empty));
    }

    #[test]
    fn asterisk_slash_hash_bypass() {
        assert_eq!(
            check_prompt("* fix it").bypass_reason,
            Some(BypassReason::AsteriskPrefix)
        );
        assert_eq!(
            check_prompt("/help").bypass_reason,
            Some(BypassReason::SlashCommand)
        );
        assert_eq!(
            check_prompt("# remember this").bypass_reason,
            Some(BypassReason::MemorizePrefix)
        );
    }

    #[test]
    fn fenced_code_and_long_prompt_bypass() {
        let with_fence = "Look at this:\n```rust\nfn x() {}\n```";
        let r = check_prompt(with_fence);
        assert_eq!(r.bypass_reason, Some(BypassReason::FencedCode));

        // Long-prompt bypass — > 4000 chars.
        let long = "a ".repeat(2100);
        assert_eq!(
            check_prompt(&long).bypass_reason,
            Some(BypassReason::LongPrompt)
        );

        // Override keyword
        let r2 = check_prompt("fix it -- ijfw off please");
        assert_eq!(r2.bypass_reason, Some(BypassReason::OverrideKeyword));
    }

    #[test]
    fn single_word_does_not_fire_polysemous_alone() {
        // "deploy" trips polysemous + no_target, but not bare-verb (it's
        // not in the imperative regex) and is short. Two signals + short
        // + no_target => the firing gate fires.
        let r = check_prompt("deploy");
        assert!(r.signals.contains(&Signal::Polysemous));
        assert!(r.signals.contains(&Signal::NoTarget));
        assert!(r.vague);
        // Sanity: a single-word non-polysemous, non-imperative word does
        // NOT fire (only no_target trips, which is one signal).
        let r2 = check_prompt("hello");
        assert!(!r2.vague);
        assert_eq!(r2.signals, vec![Signal::NoTarget]);
    }

    #[test]
    fn abstract_goal_mitigated_by_metric_or_path() {
        // Plain abstract — fires the AbstractGoal rule.
        assert!(check_abstract_goal("make it better"));
        // Mitigated by a metric.
        assert!(!check_abstract_goal("make it better in under 200ms"));
        // Mitigated by a file path.
        assert!(!check_abstract_goal("make src/foo.rs cleaner"));
        // Mitigated by a directory prefix.
        assert!(!check_abstract_goal("make tests/ nicer"));
    }

    #[test]
    fn missing_constraint_skips_short_text() {
        // Token count < 4 => rule short-circuits to false.
        assert!(!check_missing_constraint("fix it now"));
        // >=4 tokens, no constraint word, no number => fires.
        assert!(check_missing_constraint(
            "please rewrite the helper function entirely"
        ));
        // >=4 tokens but contains a constraint word => does not fire.
        assert!(!check_missing_constraint(
            "please rewrite the helper function when needed"
        ));
        // >=4 tokens with a number => does not fire.
        assert!(!check_missing_constraint(
            "please rewrite the helper function under 200"
        ));
    }

    #[test]
    fn long_prompt_with_no_target_does_not_fire_short_gate() {
        // >=30 tokens => `short` is false, so vague=false even if signals trip.
        let long = "please go through everything and clean up stuff and things and make it nicer and better and cleaner because the code is bad and needs help and improvement all around the project".to_string();
        let r = check_prompt(&long);
        // Some signals likely trip, but the short gate fails.
        assert!(
            !r.vague,
            "expected vague=false due to short gate, got {r:?}"
        );
    }

    #[test]
    fn rules_list_canonical() {
        // Sanity check: RULES has 7 entries in stable order.
        assert_eq!(RULES.len(), 7);
        assert_eq!(RULES[0], "bare_verb");
        assert_eq!(RULES[6], "missing_constraint");
        // Signal::as_id matches RULES order.
        for (i, sig) in [
            Signal::BareVerb,
            Signal::UnresolvedAnaphora,
            Signal::AbstractGoal,
            Signal::NoTarget,
            Signal::ScopePlural,
            Signal::Polysemous,
            Signal::MissingConstraint,
        ]
        .iter()
        .enumerate()
        {
            assert_eq!(sig.as_id(), RULES[i]);
        }
    }
}
