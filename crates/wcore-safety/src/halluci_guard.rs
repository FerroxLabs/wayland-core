//! Hallucination guard for assistant outputs.
//!
//! Wayland v0.6.2 Tier-2 lift **T2-A1**. Ports the deterministic claim
//! extractor + cross-ref shape from the prior Wayland Python engine
//! and generalises it from "delegation claims vs. Kanban DB" to
//! "factual claims vs. `ToolCallTrace::result_snippet`" (T2-A0).
//!
//! Detection is regex-based, not LLM-based — the trigger only needs to
//! catch the dominant false patterns (file paths the model fabricated,
//! identifiers it claims exist, numeric values it claims it saw). A
//! deterministic regex pipeline keeps the guard cheap and predictable;
//! richer NLP-level disambiguation is out of scope for v0.6.2.
//!
//! Cross-ref runs each extracted [`Claim`] against the snippet slice
//! supplied by the caller (typically extracted from
//! `wcore_observability::ToolCallTrace::result_snippet` upstream — kept
//! as `&[&str]` here to avoid an upward dep cycle on `wcore-observability`).
//! A strict substring hit is a [`CrossRefVerdict::Match`]; otherwise the
//! guard falls back to a fuzzy Levenshtein-ratio comparison against
//! fixed-width windows of the snippet. Claims of [`ClaimKind::Other`]
//! are always [`CrossRefVerdict::Unverifiable`].
//!
//! The [`HallucinationGuard::check`] entry-point aggregates the per-claim
//! verdicts into a [`GuardReport`] and tags it with the configured
//! [`GuardSeverity`]. The downstream caller (the agent loop) is
//! responsible for *acting* on the report — this module is pure
//! detection.

use std::sync::OnceLock;

use regex::Regex;

// ---------------------------------------------------------------------------
// Public dataclasses
// ---------------------------------------------------------------------------

/// Kind of factual claim extracted from an assistant turn.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ClaimKind {
    /// Filesystem path the model asserted exists or it read.
    FilePath,
    /// Code identifier — function, struct, method.
    Identifier,
    /// Numeric value the model surfaced ("got 42 results").
    NumericValue,
    /// Verbatim tool output snippet quoted by the model.
    ToolResult,
    /// Free-form prose that cannot be mechanically verified.
    Other,
}

/// A single claim extracted from output text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Claim {
    /// Verbatim text of the claim (the regex capture).
    pub text: String,
    /// Classification.
    pub kind: ClaimKind,
    /// Byte span `[start, end)` within the source text.
    pub span: (usize, usize),
}

/// Cross-ref outcome for a single claim.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CrossRefVerdict {
    /// Claim text appears in some trace's `result_snippet` (strict or fuzzy).
    Match,
    /// Claim text does not appear in any trace's `result_snippet`.
    Mismatch,
    /// Claim kind is structurally unverifiable (`ClaimKind::Other`) or
    /// emitted as a placeholder by the cascade-extension TODO path.
    Unverifiable,
}

/// Severity policy for the guard.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GuardSeverity {
    /// Mismatches reported; downstream caller decides.
    Warn,
    /// Report indicates the downstream caller should block emission.
    Block,
    /// Mismatched claims trigger a deterministic re-extraction cascade:
    /// the claim extractor is re-run over each mismatched claim's text
    /// to surface sub-claims (e.g. path segments, embedded identifiers),
    /// which are then cross-referenced against the original snippets.
    /// Surviving sub-claims are appended to the verdict buckets so the
    /// downstream caller sees concrete sub-claim verdicts instead of a
    /// TODO marker. v0.6.4 Task 3.2 replaced the prior
    /// `TODO(T2-A1-cascade)` placeholder path.
    Cascade,
}

/// Aggregated guard report.
///
/// # Field invariants
///
/// For modes `Warn` and `Block`:
///   `claims_total == verified + mismatched.len() + unverifiable.len()`.
///
/// For mode `Cascade`, this equality does NOT hold. Cascade re-extracts
/// sub-claims from each mismatched claim's text and routes them into the
/// same three buckets, so `verified + mismatched.len() + unverifiable.len()`
/// over-counts relative to `claims_total` (which still reflects the
/// top-level extraction count). Treat `claims_total` as authoritative for
/// the top-level extracted-claim count; iterate `mismatched` /
/// `unverifiable` for the per-claim and per-sub-claim verdicts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GuardReport {
    /// Total number of claims extracted from the input.
    pub claims_total: usize,
    /// Number of claims that received a [`CrossRefVerdict::Match`].
    pub verified: usize,
    /// Claims that received a [`CrossRefVerdict::Mismatch`].
    pub mismatched: Vec<Claim>,
    /// Claims that received a [`CrossRefVerdict::Unverifiable`].
    pub unverifiable: Vec<Claim>,
    /// Severity tag echoing the guard's configuration.
    pub severity: GuardSeverity,
}

// ---------------------------------------------------------------------------
// Patterns
// ---------------------------------------------------------------------------

static FILE_PATH_RE: OnceLock<Regex> = OnceLock::new();
static IDENTIFIER_RE: OnceLock<Regex> = OnceLock::new();
static NUMERIC_RE: OnceLock<Regex> = OnceLock::new();
static TOOL_RESULT_RE: OnceLock<Regex> = OnceLock::new();

fn file_path_re() -> &'static Regex {
    FILE_PATH_RE.get_or_init(|| {
        // Absolute or relative paths with at least one slash and a
        // file-ish suffix or path segment. Conservative — only matches
        // non-whitespace runs containing `/` plus a final segment.
        // Final segment requires at least one ASCII alpha char so that
        // pure-numeric/punct fragments like "1.2.3/4" (version strings)
        // are NOT extracted as file paths. The earlier permissive form
        // `[A-Za-z0-9_.-]+` flagged such version numbers as fabricated
        // paths.
        Regex::new(r"(?:[A-Za-z0-9_./-]*/)+[A-Za-z0-9_.-]*[A-Za-z][A-Za-z0-9_.-]*")
            .expect("wcore-safety: invalid FILE_PATH regex")
    })
}

fn identifier_re() -> &'static Regex {
    IDENTIFIER_RE.get_or_init(|| {
        // Path-like (`Foo::bar`) or call-like (`fn_name(`) identifiers.
        // The `::` form catches Rust/CPP qualified names; the trailing
        // `(` catches function-call references the model claims to have
        // invoked or to exist. Capture groups are NOT used — we slice
        // the full match minus the trailing `(` when present.
        Regex::new(
            r"\b[A-Za-z_][A-Za-z0-9_]*(?:::[A-Za-z_][A-Za-z0-9_]*)+|\b[A-Za-z_][A-Za-z0-9_]*\(",
        )
        .expect("wcore-safety: invalid IDENTIFIER regex")
    })
}

fn numeric_re() -> &'static Regex {
    NUMERIC_RE.get_or_init(|| {
        // Bare integers or decimals embedded in prose. We intentionally
        // skip percentages/units for v0.6.2 — the cross-ref does pure
        // substring matching so adding `%` would just narrow the
        // verifiable surface.
        Regex::new(r"\b\d+(?:\.\d+)?\b").expect("wcore-safety: invalid NUMERIC regex")
    })
}

fn tool_result_re() -> &'static Regex {
    TOOL_RESULT_RE.get_or_init(|| {
        // Backtick-delimited inline code blocks — these are the most
        // common surface where the model quotes a tool's output verbatim.
        Regex::new(r"`([^`\n]{2,})`").expect("wcore-safety: invalid TOOL_RESULT regex")
    })
}

// ---------------------------------------------------------------------------
// Claim extractor
// ---------------------------------------------------------------------------

/// Extract claims from `text` using the deterministic regex pipeline.
///
/// Returns an empty vec on empty input. Spans are byte offsets into
/// `text`; callers that need char indices must convert separately.
pub fn extract_claims(text: &str) -> Vec<Claim> {
    if text.is_empty() {
        return Vec::new();
    }

    let mut claims: Vec<Claim> = Vec::new();

    // File paths first — they look identifier-shaped to the identifier
    // regex (`src/lib.rs` contains `lib`), so we record their spans and
    // filter overlapping identifier hits.
    let mut path_spans: Vec<(usize, usize)> = Vec::new();
    for m in file_path_re().find_iter(text) {
        path_spans.push((m.start(), m.end()));
        claims.push(Claim {
            text: m.as_str().to_string(),
            kind: ClaimKind::FilePath,
            span: (m.start(), m.end()),
        });
    }

    // Identifiers — skip any match that falls inside a path span.
    for m in identifier_re().find_iter(text) {
        let (s, e) = (m.start(), m.end());
        if path_spans.iter().any(|(ps, pe)| s >= *ps && e <= *pe) {
            continue;
        }
        // Strip a trailing `(` from call-style matches so the recorded
        // text is just the identifier proper.
        let raw = m.as_str();
        let stripped = raw.strip_suffix('(').unwrap_or(raw);
        let end = if raw.ends_with('(') { e - 1 } else { e };
        claims.push(Claim {
            text: stripped.to_string(),
            kind: ClaimKind::Identifier,
            span: (s, end),
        });
    }

    // Numeric values.
    for m in numeric_re().find_iter(text) {
        let (s, e) = (m.start(), m.end());
        if path_spans.iter().any(|(ps, pe)| s >= *ps && e <= *pe) {
            continue;
        }
        claims.push(Claim {
            text: m.as_str().to_string(),
            kind: ClaimKind::NumericValue,
            span: (s, e),
        });
    }

    // Backticked tool-result quotes.
    for caps in tool_result_re().captures_iter(text) {
        if let Some(inner) = caps.get(1) {
            claims.push(Claim {
                text: inner.as_str().to_string(),
                kind: ClaimKind::ToolResult,
                span: (inner.start(), inner.end()),
            });
        }
    }

    claims
}

// ---------------------------------------------------------------------------
// Cross-ref
// ---------------------------------------------------------------------------

/// Cross-reference a single [`Claim`] against the supplied snippets using
/// strict substring matching (no fuzzy fallback). Use
/// [`HallucinationGuard::check`] for the full pipeline including
/// Levenshtein-ratio fuzzy match.
///
/// `snippets` is typically built by the caller from
/// `wcore_observability::ToolCallTrace::result_snippet` values — pass the
/// `Some(&str)` snippets only; traces with no captured snippet should be
/// filtered out before calling. Empty slice → `Mismatch`.
pub fn cross_ref(claim: &Claim, snippets: &[&str]) -> CrossRefVerdict {
    if matches!(claim.kind, ClaimKind::Other) {
        return CrossRefVerdict::Unverifiable;
    }

    if snippets.is_empty() {
        return CrossRefVerdict::Mismatch;
    }

    for snippet in snippets {
        if snippet.contains(&claim.text) {
            return CrossRefVerdict::Match;
        }
    }
    CrossRefVerdict::Mismatch
}

// ---------------------------------------------------------------------------
// Guard
// ---------------------------------------------------------------------------

/// Stateful hallucination guard. Carries severity policy + fuzzy
/// tolerance. Detection is otherwise pure.
#[derive(Debug, Clone)]
pub struct HallucinationGuard {
    severity: GuardSeverity,
    fuzzy_tolerance: f32,
}

impl Default for HallucinationGuard {
    fn default() -> Self {
        Self {
            severity: GuardSeverity::Warn,
            fuzzy_tolerance: 0.85,
        }
    }
}

impl HallucinationGuard {
    /// Build a guard with the given severity and default `0.85` fuzzy tolerance.
    pub fn new(severity: GuardSeverity) -> Self {
        Self {
            severity,
            fuzzy_tolerance: 0.85,
        }
    }

    /// Override the Levenshtein-ratio threshold (`0.0..=1.0`).
    pub fn with_fuzzy_tolerance(mut self, tolerance: f32) -> Self {
        self.fuzzy_tolerance = tolerance.clamp(0.0, 1.0);
        self
    }

    /// Run the full pipeline: extract claims from `output_text`, cross-ref
    /// each one against `snippets`, return an aggregated [`GuardReport`].
    pub fn check(&self, output_text: &str, snippets: &[&str]) -> GuardReport {
        let claims = extract_claims(output_text);
        let claims_total = claims.len();
        let mut verified: usize = 0;
        let mut mismatched: Vec<Claim> = Vec::new();
        let mut unverifiable: Vec<Claim> = Vec::new();

        for claim in claims {
            // Phase 1: strict substring cross-ref.
            let verdict = match cross_ref(&claim, snippets) {
                CrossRefVerdict::Match => CrossRefVerdict::Match,
                CrossRefVerdict::Unverifiable => CrossRefVerdict::Unverifiable,
                CrossRefVerdict::Mismatch => {
                    // Phase 2: fuzzy fallback when substring missed.
                    if self.fuzzy_match(&claim, snippets) {
                        CrossRefVerdict::Match
                    } else {
                        CrossRefVerdict::Mismatch
                    }
                }
            };
            match verdict {
                CrossRefVerdict::Match => verified += 1,
                CrossRefVerdict::Mismatch => mismatched.push(claim),
                CrossRefVerdict::Unverifiable => unverifiable.push(claim),
            }
        }

        // Cascade severity: re-extract sub-claims from each mismatched
        // claim's text and cross-ref them against the same snippets. This
        // surfaces concrete sub-claim verdicts (e.g. a fabricated full
        // path may contain an identifier that DOES appear in a snippet)
        // instead of a TODO marker. Sub-claims that exactly duplicate the
        // parent claim are skipped to avoid emitting the same verdict
        // twice.
        if matches!(self.severity, GuardSeverity::Cascade) {
            for parent in mismatched.clone() {
                let sub_claims = extract_claims(&parent.text);
                for sub in sub_claims {
                    if sub.text == parent.text {
                        // Same string the parent extractor already
                        // surfaced — re-emitting would just duplicate the
                        // mismatch verdict. Skip.
                        continue;
                    }
                    // Re-anchor sub-claim spans into the original text by
                    // offsetting from the parent's start.
                    let (ss, se) = sub.span;
                    let anchored = Claim {
                        text: sub.text,
                        kind: sub.kind,
                        span: (parent.span.0 + ss, parent.span.0 + se),
                    };
                    let verdict = match cross_ref(&anchored, snippets) {
                        CrossRefVerdict::Match => CrossRefVerdict::Match,
                        CrossRefVerdict::Unverifiable => CrossRefVerdict::Unverifiable,
                        CrossRefVerdict::Mismatch => {
                            if self.fuzzy_match(&anchored, snippets) {
                                CrossRefVerdict::Match
                            } else {
                                CrossRefVerdict::Mismatch
                            }
                        }
                    };
                    match verdict {
                        CrossRefVerdict::Match => verified += 1,
                        CrossRefVerdict::Mismatch => mismatched.push(anchored),
                        CrossRefVerdict::Unverifiable => unverifiable.push(anchored),
                    }
                }
            }
        }

        GuardReport {
            claims_total,
            verified,
            mismatched,
            unverifiable,
            severity: self.severity,
        }
    }

    /// Return `true` if `claim.text` is within `fuzzy_tolerance`
    /// Levenshtein ratio of any same-length window of any supplied
    /// snippet. Pure helper.
    fn fuzzy_match(&self, claim: &Claim, snippets: &[&str]) -> bool {
        if matches!(claim.kind, ClaimKind::Other) {
            return false;
        }
        let needle = claim.text.as_str();
        let n = needle.chars().count();
        if n == 0 {
            return false;
        }
        for snippet in snippets {
            // Slide a window the same char-length as the needle across
            // the snippet. We work on char boundaries to keep UTF-8 safe.
            let chars: Vec<char> = snippet.chars().collect();
            if chars.len() < n {
                // Compare the whole snippet against the needle once.
                let dist = levenshtein(needle, snippet);
                let max_len = n.max(chars.len()) as f32;
                if max_len > 0.0 {
                    let ratio = 1.0 - (dist as f32 / max_len);
                    if ratio >= self.fuzzy_tolerance {
                        return true;
                    }
                }
                continue;
            }
            for start in 0..=chars.len() - n {
                let window: String = chars[start..start + n].iter().collect();
                let dist = levenshtein(needle, &window);
                let ratio = 1.0 - (dist as f32 / n as f32);
                if ratio >= self.fuzzy_tolerance {
                    return true;
                }
            }
        }
        false
    }
}

// ---------------------------------------------------------------------------
// Levenshtein (inline — no new external dep, per T2-A1 brief)
// ---------------------------------------------------------------------------

/// Standard dynamic-programming Levenshtein edit distance with the
/// single-row optimisation. O(n*m) time, O(min(n,m)) space.
pub fn levenshtein(a: &str, b: &str) -> usize {
    let a_chars: Vec<char> = a.chars().collect();
    let b_chars: Vec<char> = b.chars().collect();
    if a_chars.is_empty() {
        return b_chars.len();
    }
    if b_chars.is_empty() {
        return a_chars.len();
    }
    // Ensure `b` is the shorter so the row is small.
    let (a_chars, b_chars) = if a_chars.len() < b_chars.len() {
        (b_chars, a_chars)
    } else {
        (a_chars, b_chars)
    };
    let m = b_chars.len();
    let mut prev: Vec<usize> = (0..=m).collect();
    let mut curr: Vec<usize> = vec![0; m + 1];
    for (i, ca) in a_chars.iter().enumerate() {
        curr[0] = i + 1;
        for (j, cb) in b_chars.iter().enumerate() {
            let cost = if ca == cb { 0 } else { 1 };
            curr[j + 1] = (curr[j] + 1) // insertion
                .min(prev[j + 1] + 1) // deletion
                .min(prev[j] + cost); // substitution
        }
        std::mem::swap(&mut prev, &mut curr);
    }
    prev[m]
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_claims_file_paths() {
        let text = "Read /foo/bar.rs and edited src/lib.rs cleanly.";
        let claims = extract_claims(text);
        let paths: Vec<&str> = claims
            .iter()
            .filter(|c| c.kind == ClaimKind::FilePath)
            .map(|c| c.text.as_str())
            .collect();
        assert!(paths.contains(&"/foo/bar.rs"), "got: {paths:?}");
        assert!(paths.contains(&"src/lib.rs"), "got: {paths:?}");
    }

    #[test]
    fn extract_claims_identifiers() {
        let text = "Called MyStruct::new() then let x = foo() to wire it up.";
        let claims = extract_claims(text);
        let idents: Vec<&str> = claims
            .iter()
            .filter(|c| c.kind == ClaimKind::Identifier)
            .map(|c| c.text.as_str())
            .collect();
        assert!(
            idents.contains(&"MyStruct::new"),
            "expected MyStruct::new in {idents:?}"
        );
        assert!(idents.contains(&"foo"), "expected foo in {idents:?}");
    }

    #[test]
    fn extract_claims_numeric_values() {
        let claims = extract_claims("got 42 results and 3.14 average");
        let nums: Vec<&str> = claims
            .iter()
            .filter(|c| c.kind == ClaimKind::NumericValue)
            .map(|c| c.text.as_str())
            .collect();
        assert!(nums.contains(&"42"), "got: {nums:?}");
        assert!(nums.contains(&"3.14"), "got: {nums:?}");
    }

    #[test]
    fn extract_claims_empty_input() {
        assert!(extract_claims("").is_empty());
    }

    #[test]
    fn cross_ref_match_substring_in_snippet() {
        let claim = Claim {
            text: "src/lib.rs".to_string(),
            kind: ClaimKind::FilePath,
            span: (0, 10),
        };
        let snippets = ["Read src/lib.rs successfully (123 lines)"];
        assert_eq!(cross_ref(&claim, &snippets), CrossRefVerdict::Match);
    }

    #[test]
    fn cross_ref_mismatch_when_absent() {
        let claim = Claim {
            text: "src/missing.rs".to_string(),
            kind: ClaimKind::FilePath,
            span: (0, 14),
        };
        let snippets = ["Read src/lib.rs successfully"];
        assert_eq!(cross_ref(&claim, &snippets), CrossRefVerdict::Mismatch);
    }

    #[test]
    fn cross_ref_unverifiable_for_other_kind() {
        let claim = Claim {
            text: "anything".to_string(),
            kind: ClaimKind::Other,
            span: (0, 8),
        };
        let snippets = ["anything goes here"];
        assert_eq!(cross_ref(&claim, &snippets), CrossRefVerdict::Unverifiable);
    }

    #[test]
    fn cross_ref_handles_empty_snippets() {
        // Mirrors the upstream "trace with result_snippet = None" case:
        // callers filter Nones, so an empty slice reaches us.
        let claim = Claim {
            text: "src/lib.rs".to_string(),
            kind: ClaimKind::FilePath,
            span: (0, 10),
        };
        let snippets: [&str; 0] = [];
        assert_eq!(cross_ref(&claim, &snippets), CrossRefVerdict::Mismatch);
    }

    #[test]
    fn levenshtein_basic_pairs() {
        assert_eq!(levenshtein("kitten", "sitting"), 3);
        assert_eq!(levenshtein("flaw", "lawn"), 2);
    }

    #[test]
    fn levenshtein_identical_and_empty() {
        assert_eq!(levenshtein("same", "same"), 0);
        assert_eq!(levenshtein("", ""), 0);
        assert_eq!(levenshtein("", "abc"), 3);
        assert_eq!(levenshtein("abc", ""), 3);
    }

    #[test]
    fn fuzzy_match_within_tolerance_counts_as_match() {
        // One-character typo in a 10-char file name → ratio 0.9 > 0.85.
        let guard = HallucinationGuard::new(GuardSeverity::Warn);
        let output = "Read `src/Iib.rs` carefully."; // "Iib" instead of "lib"
        // Snippet contains the canonical name.
        let snippets = ["opened src/lib.rs and inspected contents"];
        let report = guard.check(output, &snippets);
        // The backticked `src/Iib.rs` is a ToolResult claim — fuzzy
        // match against `src/lib.rs` in the snippet (distance 1, len 10
        // → ratio 0.9) flips Mismatch to Match.
        let tool_results_matched = report
            .mismatched
            .iter()
            .filter(|c| c.kind == ClaimKind::ToolResult)
            .count();
        assert_eq!(
            tool_results_matched, 0,
            "fuzzy match should have rescued the ToolResult claim, report={report:?}"
        );
        assert!(report.verified >= 1, "report={report:?}");
    }

    #[test]
    fn check_severity_block_marks_report_severity() {
        let guard = HallucinationGuard::new(GuardSeverity::Block);
        let report = guard.check("some text", &[]);
        assert_eq!(report.severity, GuardSeverity::Block);
    }

    #[test]
    fn check_aggregates_counts_correctly() {
        // 3 claims: 1 match, 1 mismatch, plus we wedge in an Other via
        // direct cross_ref. The aggregator only runs over extracted
        // claims, so to exercise "unverifiable" we use a backtick quote
        // that won't match (mismatch) and assert structurally.
        let guard = HallucinationGuard::new(GuardSeverity::Warn).with_fuzzy_tolerance(1.01);
        // tolerance > 1.0 is clamped to 1.0 → only exact strict matches
        // count as Match. Below we have one strict match (src/lib.rs)
        // and one mismatch (src/other.rs — has the required slash so the
        // FilePath regex extracts it as a claim).
        let output = "Touched src/lib.rs and not src/other.rs";
        let snippets = ["src/lib.rs is fine"];
        let report = guard.check(output, &snippets);
        assert!(report.claims_total >= 2, "report={report:?}");
        assert!(report.verified >= 1, "report={report:?}");
        // At least one path-shaped claim must be mismatched.
        assert!(
            report
                .mismatched
                .iter()
                .any(|c| c.text.contains("src/other.rs")),
            "report={report:?}",
        );
        // Counts agree with totals.
        assert_eq!(
            report.claims_total,
            report.verified + report.mismatched.len() + report.unverifiable.len(),
            "report={report:?}",
        );
    }

    #[test]
    fn cascade_severity_reextracts_real_subclaims() {
        // The parent claim `src/never_existed.rs` mismatches the snippet,
        // but its `never_existed.rs` segment doesn't appear either, so
        // cascade re-extraction should yield real sub-claims (NOT a TODO
        // sentinel). The asserts cover both halves:
        //   1. No TODO(T2-A1-cascade) marker text anywhere in the report.
        //   2. A concrete re-extracted sub-claim was added.
        let guard = HallucinationGuard::new(GuardSeverity::Cascade).with_fuzzy_tolerance(1.01);
        // The backticked tool-result quote forces a ToolResult parent
        // claim whose interior re-extracts into a FilePath sub-claim
        // when fed back through `extract_claims`.
        let output = "Touched `src/never_existed.rs` in the repo";
        let snippets = ["totally unrelated snippet"];
        let report = guard.check(output, &snippets);
        assert_eq!(report.severity, GuardSeverity::Cascade);

        // 1. Sentinel must NOT appear anywhere — the cascade path no
        //    longer emits the TODO(T2-A1-cascade) marker.
        let any_sentinel = report
            .mismatched
            .iter()
            .chain(report.unverifiable.iter())
            .any(|c| c.text.contains("TODO(T2-A1-cascade)"));
        assert!(!any_sentinel, "sentinel still present in report={report:?}");

        // 2. Re-extraction surfaced concrete sub-claims. The parent text
        //    `src/never_existed.rs` re-extracts into a FilePath sub-claim
        //    of the same text. We dedup exact duplicates, so the SUB
        //    bucket here is driven by the ToolResult parent
        //    `src/never_existed.rs`: extract_claims over that string
        //    returns a FilePath of the same text → deduped. To prove
        //    re-extraction ran, we ALSO seed an Identifier-shaped parent
        //    via a separate output and inspect that report.
        let guard2 = HallucinationGuard::new(GuardSeverity::Cascade).with_fuzzy_tolerance(1.01);
        let output2 = "Called `Mystery::missing()` repeatedly";
        let snippets2 = ["totally unrelated snippet"];
        let report2 = guard2.check(output2, &snippets2);
        let any_sentinel2 = report2
            .mismatched
            .iter()
            .chain(report2.unverifiable.iter())
            .any(|c| c.text.contains("TODO(T2-A1-cascade)"));
        assert!(!any_sentinel2, "sentinel present in report2={report2:?}");
        // The ToolResult `Mystery::missing()` re-extracts into an
        // Identifier sub-claim `Mystery::missing` (the trailing `()` is
        // stripped). Different text → not deduped → appears as a
        // mismatched sub-claim.
        let has_subclaim = report2
            .mismatched
            .iter()
            .any(|c| c.kind == ClaimKind::Identifier && c.text == "Mystery::missing");
        assert!(
            has_subclaim,
            "expected re-extracted Identifier sub-claim Mystery::missing in report2={report2:?}",
        );
    }

    #[test]
    fn fuzzy_match_short_snippet_path() {
        // Snippet shorter than the needle exercises the
        // `chars.len() < n` branch.
        let guard = HallucinationGuard::new(GuardSeverity::Warn);
        let claim = Claim {
            text: "alongname".to_string(),
            kind: ClaimKind::Identifier,
            span: (0, 9),
        };
        // Snippet is 8 chars; distance to "alongname" (9 chars) is 1
        // (just the trailing 'e'), max_len=9, ratio = 1 - 1/9 ≈ 0.888 ≥ 0.85.
        let snippets = ["alongnam"];
        assert!(guard.fuzzy_match(&claim, &snippets));
    }
}
