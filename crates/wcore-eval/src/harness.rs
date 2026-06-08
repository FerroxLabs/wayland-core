//! Eval harness: walk the 60-case corpus, score each candidate,
//! tally precision/recall against expected_outcome.

use std::path::{Path, PathBuf};

use wcore_observability::trace::TurnTrace;
use wcore_skills::types::{ExecutionContext, LoadedFrom, SkillMetadata, SkillSource};

use crate::corpus::{Candidate, Corpus, ExpectedOutcome, ReferenceCase, Verdict};
use crate::error::EvalError;
use crate::report::{CaseResult, EvalReport};
use crate::scorer::{DefaultScorer, Scorer};

pub struct Harness {
    root: PathBuf,
    corpus: Corpus,
    scorer: Box<dyn Scorer>,
}

impl Harness {
    /// Load the corpus rooted at the crate manifest directory and
    /// install `DefaultScorer`.
    pub fn from_manifest_dir() -> Result<Self, EvalError> {
        Self::from_root(Path::new(env!("CARGO_MANIFEST_DIR")))
    }

    /// Load the corpus rooted at `root` and install `DefaultScorer`.
    pub fn from_root(root: &Path) -> Result<Self, EvalError> {
        let corpus = Corpus::load(root)?;
        Ok(Self {
            root: root.to_owned(),
            corpus,
            scorer: Box::new(DefaultScorer::default()),
        })
    }

    /// Construct with a caller-provided corpus and scorer. Useful for
    /// W10B's GEPA loop, which scores mutated candidates against the
    /// same corpus shape.
    pub fn new(root: PathBuf, corpus: Corpus, scorer: Box<dyn Scorer>) -> Self {
        Self {
            root,
            corpus,
            scorer,
        }
    }

    /// Test-only fixture: build a Harness from a hard-coded inline
    /// corpus of 2 cases (1 good, 1 bad). Gated behind the
    /// `test-utils` feature; consumed by `wcore-eval`'s own
    /// integration tests and by W10B's harness wiring tests.
    #[cfg(feature = "test-utils")]
    pub fn fixture_for_tests() -> Self {
        // Construction uses the same Corpus type; the fixture lives
        // entirely in-memory (no disk reads), so it is safe to call
        // from any test.
        let cases = vec![
            ReferenceCase {
                frontmatter: crate::corpus::CaseFrontmatter {
                    id: "fixture-good".into(),
                    category: "healthy".into(),
                    skill_body: "_inline_".into(),
                    trace_fixture: None,
                    expected_outcome: ExpectedOutcome::Good,
                    rationale: "in-memory fixture".into(),
                },
                source: PathBuf::from("<fixture>"),
            },
            ReferenceCase {
                frontmatter: crate::corpus::CaseFrontmatter {
                    id: "fixture-bad".into(),
                    category: "truncated-body".into(),
                    skill_body: "_inline_".into(),
                    trace_fixture: None,
                    expected_outcome: ExpectedOutcome::Bad,
                    rationale: "in-memory fixture".into(),
                },
                source: PathBuf::from("<fixture>"),
            },
        ];
        Self {
            root: PathBuf::from("<fixture>"),
            corpus: Corpus { cases },
            scorer: Box::new(DefaultScorer::default()),
        }
    }

    /// Walk every case in the corpus; score; tally; return report.
    pub fn run(&self) -> Result<EvalReport, EvalError> {
        let mut by_case = Vec::with_capacity(self.corpus.len());
        let mut tp = 0usize;
        let mut tn = 0usize;
        let mut fp = 0usize;
        let mut fn_ = 0usize;

        for case in &self.corpus.cases {
            let candidate = self.build_candidate(case)?;
            let outcome = self.scorer.score(&candidate);

            let agreed = match (case.frontmatter.expected_outcome, outcome.predicted) {
                (ExpectedOutcome::Good, Verdict::Good) => {
                    tp += 1;
                    true
                }
                (ExpectedOutcome::Bad, Verdict::Bad) => {
                    tn += 1;
                    true
                }
                (ExpectedOutcome::Bad, Verdict::Good) => {
                    fp += 1;
                    false
                }
                (ExpectedOutcome::Good, Verdict::Bad) => {
                    fn_ += 1;
                    false
                }
            };

            by_case.push(CaseResult {
                case_id: case.frontmatter.id.clone(),
                category: case.frontmatter.category.clone(),
                expected: case.frontmatter.expected_outcome,
                predicted: outcome.predicted,
                agreed,
                score: outcome,
            });
        }

        let total = by_case.len();
        let precision = if tp + fp == 0 {
            0.0
        } else {
            tp as f64 / (tp + fp) as f64
        };
        let recall = if tp + fn_ == 0 {
            0.0
        } else {
            tp as f64 / (tp + fn_) as f64
        };
        let f1 = if precision + recall == 0.0 {
            0.0
        } else {
            2.0 * precision * recall / (precision + recall)
        };
        let agreement_rate = (tp + tn) as f64 / total as f64;

        Ok(EvalReport {
            total,
            true_positive: tp,
            true_negative: tn,
            false_positive: fp,
            false_negative: fn_,
            precision,
            recall,
            f1,
            agreement_rate,
            by_case,
        })
    }

    fn build_candidate(&self, case: &ReferenceCase) -> Result<Candidate, EvalError> {
        let skill = self.load_skill_body(&case.frontmatter.skill_body, &case.frontmatter.id)?;
        let trace = match &case.frontmatter.trace_fixture {
            Some(f) => Some(self.load_trace(f, &case.frontmatter.id)?),
            None => None,
        };
        Ok(Candidate {
            skill,
            trace,
            source_filename: case.frontmatter.skill_body.clone(),
        })
    }

    fn load_skill_body(&self, name: &str, case_id: &str) -> Result<SkillMetadata, EvalError> {
        let path = self.root.join("data/skills").join(format!("{name}.md"));
        let raw = std::fs::read_to_string(&path).map_err(|_| EvalError::SkillBodyMissing {
            case: case_id.to_owned(),
            path: path.clone(),
        })?;
        let (fm, body) = split_frontmatter(&raw).ok_or_else(|| EvalError::CaseMalformed {
            path: path.clone(),
            reason: "skill body missing YAML frontmatter".into(),
        })?;
        #[derive(serde::Deserialize, Default)]
        struct SkillBodyFm {
            name: Option<String>,
            description: Option<String>,
            when_to_use: Option<String>,
            allowed_tools: Option<Vec<String>>,
            model: Option<String>,
        }
        let parsed: SkillBodyFm = serde_yaml::from_str(fm).map_err(|source| EvalError::Yaml {
            path: path.clone(),
            source,
        })?;
        let body_trimmed = body.trim_end_matches('\n').to_owned();
        let content_length = body_trimmed.len();
        Ok(SkillMetadata {
            name: parsed.name.unwrap_or_else(|| name.to_owned()),
            display_name: None,
            description: parsed.description.unwrap_or_default(),
            has_user_specified_description: true,
            allowed_tools: parsed.allowed_tools.unwrap_or_default(),
            argument_hint: None,
            argument_names: vec![],
            when_to_use: parsed.when_to_use,
            version: None,
            model: parsed.model,
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
            content: body_trimmed,
            content_length,
            skill_root: None,
        })
    }

    fn load_trace(&self, fixture: &str, case_id: &str) -> Result<TurnTrace, EvalError> {
        let path = self.root.join("data/traces").join(fixture);
        let raw = std::fs::read_to_string(&path).map_err(|_| EvalError::TraceMissing {
            case: case_id.to_owned(),
            path: path.clone(),
        })?;
        serde_json::from_str(&raw).map_err(|source| EvalError::Json { path, source })
    }
}

fn split_frontmatter(raw: &str) -> Option<(&str, &str)> {
    let raw = raw.strip_prefix("---\n")?;
    let end = raw.find("\n---\n")?;
    Some((&raw[..end], &raw[end + 5..]))
}
