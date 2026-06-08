//! M4.1 — Mini-bench v1 dataset for the GEPA learning loop.
//!
//! `BenchCorpus` is a **frozen 30-case** evaluation set that lives next
//! to the legacy 60-case W10A corpus (`crate::corpus`) without
//! disturbing it. The 60-case corpus enforces a 30+30 good/bad
//! invariant for structural skill grading; this mini-bench grades
//! whole-task *outcomes* on prompts that an agent will actually run.
//!
//! Categories and counts (locked at end of M4.1):
//!
//! | Category    | Count | Why                                            |
//! |-------------|-------|------------------------------------------------|
//! | ToolRouting | 8     | "Which tool should the agent reach for?"       |
//! | Arithmetic  | 8     | Cheap deterministic correctness signal.        |
//! | Recall      | 8     | Stable factual answers (no temporal drift).    |
//! | FileOps     | 6     | Sandboxed file-tree mutations; sha256-checked. |
//!
//! 8 + 8 + 8 + 6 = 30. The 8/8/8/6 split tilts away from FileOps
//! because each file-ops case needs a sandbox temp dir at run time
//! (M4.3); 6 is the upper bound the harness affords in <30s wall-clock
//! without parallelism.
//!
//! ## Public surface (consumed by M4.2 GEPA loop)
//!
//! - [`BenchCategory`], [`BenchMatchStrategy`]
//! - [`BenchCaseFrontmatter`], [`BenchCase`], [`BenchCorpus`]
//! - [`BenchOutcome`], [`BenchRunner`] trait
//! - [`CannedBenchRunner`] — deterministic, no-LLM runner used by CI
//! - [`BenchScorer`] (impls [`crate::Scorer`])
//!
//! ## Determinism
//!
//! `BenchScorer::score` is deterministic and pure given a deterministic
//! [`BenchRunner`]. It performs no I/O on the hot path: cases are
//! loaded once at construction, and the runner is the only source of
//! per-call variability. No randomness, no clock reads.
//!
//! ## Why not extend `Corpus`?
//!
//! `crate::corpus::Corpus::load` hard-fails unless it sees exactly 30
//! good + 30 bad YAML files (`corpus.rs:113`). The mini-bench is a
//! different shape (task outcomes, not skill grading), so it is its
//! own type living under `data/bench/`. The legacy corpus stays
//! untouched.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::corpus::{Candidate, Verdict};
use crate::error::EvalError;
use crate::scorer::{LOCKED, ScoreDimensions, ScoreOutcome, Scorer};

/// The four mini-bench task families.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BenchCategory {
    /// "Which tool should the agent pick?" Prompts where the right
    /// answer names a specific tool (Bash, Read, Grep, ...).
    ToolRouting,
    /// Single-shot arithmetic with a unique numeric answer.
    Arithmetic,
    /// Stable factual recall: prompts whose canonical answer does not
    /// drift over time (e.g. "Capital of France?").
    Recall,
    /// Sandboxed file-tree mutations validated by SHA-256 of the
    /// resulting tree.
    FileOps,
}

/// How [`BenchScorer`] decides whether a runner output passes a case.
///
/// Strategies are deserialized via serde's tagged-enum form
/// (`kind: exact` / `kind: contains_all` / `kind: numeric_equal` /
/// `kind: file_tree_matches`) in the YAML frontmatter.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum BenchMatchStrategy {
    /// Output (trimmed) must equal `expected` (trimmed).
    Exact { expected: String },
    /// Every token in `tokens` must appear as a substring of the
    /// output. Case-sensitive; tokens are matched verbatim.
    ContainsAll { tokens: Vec<String> },
    /// First numeric token parsed out of the output must equal
    /// `expected` within `tolerance` (absolute). Negative numbers,
    /// scientific notation, and embedded decimals are all parsed.
    NumericEqual {
        expected: f64,
        #[serde(default)]
        tolerance: f64,
    },
    /// Output must equal `expected_sha256` (case-insensitive hex).
    /// In M4.3 the runner will compute the sha of the sandbox tree
    /// post-run; in M4.1 this is a deterministic string-match contract
    /// so the scorer can be exercised end-to-end without a real
    /// filesystem.
    FileTreeMatches {
        expected_sha256: String,
        /// Human-readable description of the expected post-state.
        /// Not consumed by scoring; helps a maintainer audit a case.
        #[serde(default)]
        description: String,
    },
}

/// On-disk frontmatter for one mini-bench case.
#[derive(Debug, Clone, Deserialize)]
pub struct BenchCaseFrontmatter {
    pub id: String,
    pub category: BenchCategory,
    pub prompt: String,
    pub match_strategy: BenchMatchStrategy,
    pub rationale: String,
    #[serde(default)]
    pub allowed_tools: Vec<String>,
    pub timeout_secs: u32,
}

/// One mini-bench case — frontmatter + the YAML file it came from.
#[derive(Debug, Clone)]
pub struct BenchCase {
    pub frontmatter: BenchCaseFrontmatter,
    /// Absolute path to the YAML file. Used in error messages.
    pub source: PathBuf,
}

/// The 30-case frozen mini-bench corpus.
#[derive(Debug, Clone)]
pub struct BenchCorpus {
    pub cases: Vec<BenchCase>,
}

impl BenchCorpus {
    /// Load every case under `<root>/data/bench/`. Returns cases in
    /// stable alphabetical order (matches [`crate::corpus::Corpus::load`]
    /// behaviour) so the harness is reproducible.
    ///
    /// Enforces the M4.1 invariants:
    ///   - exactly 30 cases total
    ///   - 8 ToolRouting + 8 Arithmetic + 8 Recall + 6 FileOps
    ///   - case `id`s are globally unique
    pub fn load(root: &Path) -> Result<Self, EvalError> {
        let dir = root.join("data").join("bench");
        let mut entries: Vec<PathBuf> = std::fs::read_dir(&dir)
            .map_err(|source| EvalError::Io {
                path: dir.clone(),
                source,
            })?
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| p.extension().and_then(|e| e.to_str()) == Some("yaml"))
            .collect();
        entries.sort();

        let mut cases = Vec::with_capacity(entries.len());
        for path in entries {
            cases.push(parse_bench_case(&path)?);
        }

        validate_invariants(&cases)?;
        Ok(BenchCorpus { cases })
    }

    /// True if the corpus has no cases. (Should never be `true` in
    /// practice — `load` rejects empty corpora.)
    pub fn is_empty(&self) -> bool {
        self.cases.is_empty()
    }

    /// Number of cases. Always 30 for the v1 mini-bench.
    pub fn len(&self) -> usize {
        self.cases.len()
    }
}

fn parse_bench_case(path: &Path) -> Result<BenchCase, EvalError> {
    let raw = std::fs::read_to_string(path).map_err(|source| EvalError::Io {
        path: path.to_owned(),
        source,
    })?;
    let frontmatter: BenchCaseFrontmatter =
        serde_yaml::from_str(&raw).map_err(|source| EvalError::Yaml {
            path: path.to_owned(),
            source,
        })?;
    Ok(BenchCase {
        frontmatter,
        source: path.to_owned(),
    })
}

fn validate_invariants(cases: &[BenchCase]) -> Result<(), EvalError> {
    if cases.is_empty() {
        return Err(EvalError::CaseMalformed {
            path: PathBuf::from("data/bench"),
            reason: "mini-bench corpus is empty (expected 30 cases)".into(),
        });
    }
    if cases.len() != 30 {
        return Err(EvalError::CaseMalformed {
            path: PathBuf::from("data/bench"),
            reason: format!("mini-bench must have exactly 30 cases, got {}", cases.len()),
        });
    }
    let mut routing = 0usize;
    let mut arith = 0usize;
    let mut recall = 0usize;
    let mut fileops = 0usize;
    for c in cases {
        match c.frontmatter.category {
            BenchCategory::ToolRouting => routing += 1,
            BenchCategory::Arithmetic => arith += 1,
            BenchCategory::Recall => recall += 1,
            BenchCategory::FileOps => fileops += 1,
        }
    }
    if routing != 8 || arith != 8 || recall != 8 || fileops != 6 {
        return Err(EvalError::CaseMalformed {
            path: PathBuf::from("data/bench"),
            reason: format!(
                "M4.1 category split must be 8/8/8/6 (routing/arith/recall/fileops); \
                 got {routing}/{arith}/{recall}/{fileops}",
            ),
        });
    }

    // Globally-unique case ids. Trips on copy-paste authoring errors.
    let mut ids: Vec<&str> = cases.iter().map(|c| c.frontmatter.id.as_str()).collect();
    ids.sort_unstable();
    for w in ids.windows(2) {
        if w[0] == w[1] {
            return Err(EvalError::CaseMalformed {
                path: PathBuf::from("data/bench"),
                reason: format!("duplicate case id: {}", w[0]),
            });
        }
    }

    Ok(())
}

/// Per-case verdict produced by [`BenchScorer`]. Exposed so the GEPA
/// loop in M4.2 can surface which cases drove a score change.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct BenchOutcome {
    pub case_id: String,
    pub category: BenchCategory,
    pub passed: bool,
    /// Why the case failed (empty when `passed = true`). Bounded-size,
    /// safe to log.
    pub reason: String,
}

/// What [`BenchScorer`] uses to obtain candidate output for one case.
///
/// M4.1 ships only the trait + tests-side canned-output runners; the
/// real `wcore-agent::Session`-backed runner lands in M4.3.
pub trait BenchRunner: Send + Sync {
    /// Produce the candidate's response for `case`. Implementations
    /// MUST be deterministic if the caller wants
    /// [`BenchScorer::score`] to be deterministic.
    fn run(&self, case: &BenchCase) -> Result<String, EvalError>;
}

/// Deterministic, no-LLM [`BenchRunner`]. Derives its output for every
/// case directly from `case.frontmatter.match_strategy` so the runner
/// always produces a string the [`BenchScorer`] will accept (unless an
/// override is registered via [`Self::with_override`]).
///
/// **Why this exists.** CI does not have LLM API keys, but M4.2 still
/// needs a per-PR regression gate. The canned runner exercises the
/// whole bench scoring path — corpus load, per-case match logic,
/// aggregate pass-ratio — without any network or stochastic input. A
/// real `wcore-agent::Session`-backed runner ships in M4.3 and will
/// replace this binary's runner behind a `--runner=session` flag.
///
/// **Properties.** Pure given a `BenchCorpus`. Calling `run` twice on
/// the same case returns bit-identical output. The runner allocates
/// once per call; no I/O, no clock, no randomness.
pub struct CannedBenchRunner {
    /// Per-case-id overrides. Cases not present here fall back to the
    /// strategy-derived pass output.
    overrides: HashMap<String, String>,
}

impl Default for CannedBenchRunner {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Debug for CannedBenchRunner {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CannedBenchRunner")
            .field("override_count", &self.overrides.len())
            .finish()
    }
}

impl CannedBenchRunner {
    /// Construct an empty canned runner. Every case will be answered
    /// with [`Self::pass_output`].
    pub fn new() -> Self {
        Self {
            overrides: HashMap::new(),
        }
    }

    /// Override the output for a single case id. Tests use this to
    /// force a known failure on a specific case.
    pub fn with_override(mut self, case_id: &str, value: &str) -> Self {
        self.overrides
            .insert(case_id.to_string(), value.to_string());
        self
    }

    /// Produce a deterministic string that the case's match strategy
    /// will accept. Mirrors the scorer's per-strategy logic so a freshly
    /// added strategy variant fails to compile here (forcing the author
    /// to think about the canned output).
    pub fn pass_output(case: &BenchCase) -> String {
        match &case.frontmatter.match_strategy {
            BenchMatchStrategy::Exact { expected } => expected.clone(),
            BenchMatchStrategy::ContainsAll { tokens } => {
                // Concatenate every required token with spaces so the
                // substring check passes; prefix with the case id so the
                // runner output isn't trivially-identical across cases
                // (helps when debugging aggregate logs).
                let mut out = String::with_capacity(64);
                out.push_str("[runner:");
                out.push_str(&case.frontmatter.id);
                out.push_str("] ");
                for t in tokens {
                    out.push_str(t);
                    out.push(' ');
                }
                out
            }
            BenchMatchStrategy::NumericEqual { expected, .. } => {
                // Bare number, no trailing newline. NumericEqual parses
                // the first numeric token in the text.
                format!("{expected}")
            }
            BenchMatchStrategy::FileTreeMatches {
                expected_sha256, ..
            } => expected_sha256.clone(),
        }
    }
}

impl BenchRunner for CannedBenchRunner {
    fn run(&self, case: &BenchCase) -> Result<String, EvalError> {
        if let Some(forced) = self.overrides.get(&case.frontmatter.id) {
            return Ok(forced.clone());
        }
        Ok(Self::pass_output(case))
    }
}

/// Scorer that grades a [`Candidate`] by running every case in a
/// [`BenchCorpus`] through a [`BenchRunner`] and reporting the
/// fraction passed.
///
/// `combined` ∈ [0.0, 1.0] is the pass ratio `passed / total`.
/// `outcome` mirrors `combined` because the bench surface does not
/// score cost/size (those signals come from the structural scorer).
/// `cost_penalty` and `size_penalty` are reported as `0.0` so the
/// existing [`ScoreDimensions`] shape stays compatible with the
/// legacy [`crate::DefaultScorer`].
///
/// Predicted [`Verdict::Good`] iff `combined >= LOCKED.acceptance_cutoff`
/// (the same 0.65 cutoff the structural scorer uses).
pub struct BenchScorer {
    corpus: BenchCorpus,
    runner: Arc<dyn BenchRunner>,
}

impl std::fmt::Debug for BenchScorer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BenchScorer")
            .field("corpus_len", &self.corpus.cases.len())
            .field("runner", &"<dyn BenchRunner>")
            .finish()
    }
}

impl BenchScorer {
    pub fn new(corpus: BenchCorpus, runner: Arc<dyn BenchRunner>) -> Self {
        Self { corpus, runner }
    }

    /// Read-only view onto the corpus this scorer grades against.
    pub fn corpus(&self) -> &BenchCorpus {
        &self.corpus
    }

    /// Run every case through the runner and return per-case outcomes
    /// in the same order as `corpus.cases`. Useful for the M4.2 GEPA
    /// loop, which needs per-case telemetry, not just the aggregate.
    pub fn outcomes(&self) -> Vec<BenchOutcome> {
        self.corpus
            .cases
            .iter()
            .map(|c| score_one(&*self.runner, c))
            .collect()
    }
}

impl Scorer for BenchScorer {
    /// Score signature ignores `_candidate` — bench scoring is a
    /// property of the runner + corpus, not the candidate skill
    /// metadata. The argument is kept for trait conformance so the
    /// existing harness wiring works unchanged.
    fn score(&self, _candidate: &Candidate) -> ScoreOutcome {
        let outcomes = self.outcomes();
        let total = outcomes.len();
        // `total == 0` cannot happen — `BenchCorpus::load` enforces
        // 30 cases — but guard explicitly so `score` never divides
        // by zero even if a hand-rolled corpus is plumbed through.
        let passed = outcomes.iter().filter(|o| o.passed).count();
        let combined = if total == 0 {
            0.0
        } else {
            passed as f64 / total as f64
        };

        let predicted = if combined >= LOCKED.acceptance_cutoff() {
            Verdict::Good
        } else {
            Verdict::Bad
        };

        ScoreOutcome {
            dimensions: ScoreDimensions {
                outcome: combined,
                cost_penalty: 0.0,
                size_penalty: 0.0,
                combined,
            },
            predicted,
        }
    }
}

fn score_one(runner: &dyn BenchRunner, case: &BenchCase) -> BenchOutcome {
    let raw = match runner.run(case) {
        Ok(s) => s,
        Err(e) => {
            return BenchOutcome {
                case_id: case.frontmatter.id.clone(),
                category: case.frontmatter.category,
                passed: false,
                reason: format!("runner error: {e}"),
            };
        }
    };
    let (passed, reason) = match &case.frontmatter.match_strategy {
        BenchMatchStrategy::Exact { expected } => {
            let lhs = raw.trim();
            let rhs = expected.trim();
            if lhs == rhs {
                (true, String::new())
            } else {
                (true_only_if(false), format!("exact mismatch: got {lhs:?}"))
            }
        }
        BenchMatchStrategy::ContainsAll { tokens } => {
            let mut missing = Vec::new();
            for t in tokens {
                if !raw.contains(t.as_str()) {
                    missing.push(t.clone());
                }
            }
            if missing.is_empty() {
                (true, String::new())
            } else {
                (false, format!("missing tokens: {missing:?}"))
            }
        }
        BenchMatchStrategy::NumericEqual {
            expected,
            tolerance,
        } => match parse_first_number(&raw) {
            Some(found) => {
                if (found - expected).abs() <= *tolerance {
                    (true, String::new())
                } else {
                    (
                        false,
                        format!("numeric mismatch: got {found}, expected {expected}"),
                    )
                }
            }
            None => (
                false,
                format!("no numeric token found in output: {:?}", truncate(&raw, 80)),
            ),
        },
        BenchMatchStrategy::FileTreeMatches {
            expected_sha256, ..
        } => {
            if raw.trim().eq_ignore_ascii_case(expected_sha256.trim()) {
                (true, String::new())
            } else {
                (
                    false,
                    format!("sha256 mismatch: got {:?}", truncate(raw.trim(), 80)),
                )
            }
        }
    };

    BenchOutcome {
        case_id: case.frontmatter.id.clone(),
        category: case.frontmatter.category,
        passed,
        reason,
    }
}

/// Identity passthrough used to make the `passed = false` branch a
/// distinct line in coverage. Inlining a literal `false` here loses
/// the failed-branch coverage signal because clippy collapses the
/// match arm. Trivial; kept explicit for clarity.
#[inline]
fn true_only_if(b: bool) -> bool {
    b
}

/// Find the first signed-decimal number in `s`. Accepts:
/// `-12`, `0.5`, `-1.5e-3`, `42`. Returns `None` if none found.
fn parse_first_number(s: &str) -> Option<f64> {
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        let start = i;
        let mut has_digit = false;
        // optional leading sign
        if matches!(bytes[i], b'+' | b'-') {
            i += 1;
        }
        // integer part
        while i < bytes.len() && bytes[i].is_ascii_digit() {
            has_digit = true;
            i += 1;
        }
        // fractional part
        if i < bytes.len() && bytes[i] == b'.' {
            i += 1;
            while i < bytes.len() && bytes[i].is_ascii_digit() {
                has_digit = true;
                i += 1;
            }
        }
        // exponent
        if has_digit && i < bytes.len() && (bytes[i] == b'e' || bytes[i] == b'E') {
            let exp_start = i;
            i += 1;
            if i < bytes.len() && matches!(bytes[i], b'+' | b'-') {
                i += 1;
            }
            let mut exp_has_digit = false;
            while i < bytes.len() && bytes[i].is_ascii_digit() {
                exp_has_digit = true;
                i += 1;
            }
            if !exp_has_digit {
                // back up — not a valid exponent
                i = exp_start;
            }
        }

        if has_digit {
            let slice = std::str::from_utf8(&bytes[start..i]).ok()?;
            if let Ok(v) = slice.parse::<f64>() {
                return Some(v);
            }
        }
        // didn't match here; advance one byte and keep scanning
        i = start + 1;
    }
    None
}

fn truncate(s: &str, n: usize) -> String {
    if s.len() <= n {
        s.to_owned()
    } else {
        let mut out = s[..s.char_indices().nth(n).map(|(i, _)| i).unwrap_or(s.len())].to_owned();
        out.push('…');
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_number_basics() {
        assert_eq!(parse_first_number("42"), Some(42.0));
        assert_eq!(parse_first_number("the answer is 42"), Some(42.0));
        assert_eq!(parse_first_number("-1.5"), Some(-1.5));
        assert_eq!(parse_first_number("1e3"), Some(1000.0));
        assert_eq!(parse_first_number("price=$2.50 only"), Some(2.50));
        assert_eq!(parse_first_number("no number here"), None);
    }

    #[test]
    fn parse_number_picks_first_only() {
        assert_eq!(parse_first_number("first 7 then 99"), Some(7.0));
    }

    #[test]
    fn truncate_short_passthrough() {
        assert_eq!(truncate("abc", 10), "abc");
    }

    #[test]
    fn truncate_long_appends_ellipsis() {
        let t = truncate("abcdefghij", 5);
        assert!(t.starts_with("abcde"));
        assert!(t.ends_with("…"));
    }
}
