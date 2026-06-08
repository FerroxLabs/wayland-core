//! Reference corpus for the W10A eval harness.
//!
//! 60 cases — exactly 30 known-good + 30 known-bad. Each case is a
//! YAML file under `data/corpus/<name>.yaml` with frontmatter naming
//! its skill body + (optional) trace fixture + the `expected_outcome`.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

use wcore_observability::trace::TurnTrace;
use wcore_skills::types::SkillMetadata;

use crate::error::EvalError;

/// Binary label on each reference case.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ExpectedOutcome {
    Good,
    Bad,
}

/// What the scorer predicted for a candidate.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Verdict {
    Good,
    Bad,
}

/// Convenience enum used by the harness if/when paired-ranking cases
/// are added in the future. W10A's corpus is binary-classification
/// (no pairs), so `Winner` is exported for W10B's GEPA loop but is
/// not consumed by W10A scoring.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Winner {
    A,
    B,
}

/// On-disk frontmatter for a reference case.
#[derive(Debug, Clone, Deserialize)]
pub struct CaseFrontmatter {
    pub id: String,
    /// One of the 10 corruption families for bad cases; "healthy" for good cases.
    pub category: String,
    pub skill_body: String,
    /// Optional — present only on trace-paired cases.
    pub trace_fixture: Option<String>,
    pub expected_outcome: ExpectedOutcome,
    /// Human-readable rationale; not consumed by scoring.
    pub rationale: String,
}

/// One reference case = one Candidate + its expected outcome.
#[derive(Debug, Clone)]
pub struct ReferenceCase {
    pub frontmatter: CaseFrontmatter,
    /// Absolute path to the case file. Used for error messages.
    pub source: PathBuf,
}

/// A scoring input: a skill + an optional execution trace.
///
/// `Candidate` is the unit the GEPA loop (W10B) will mutate and
/// re-score; W10A consumes one `Candidate` per `ReferenceCase`.
#[derive(Debug, Clone)]
pub struct Candidate {
    pub skill: SkillMetadata,
    pub trace: Option<TurnTrace>,
    /// Origin filename of the skill body, used by the
    /// `name_matches_filename` structural check.
    pub source_filename: String,
}

/// The full 60-case reference corpus.
#[derive(Debug, Clone)]
pub struct Corpus {
    pub cases: Vec<ReferenceCase>,
}

impl Corpus {
    /// Load every case under `<root>/data/corpus/`. Returns the cases
    /// in stable alphabetical order so the harness is reproducible.
    ///
    /// Enforces the W10A invariant: exactly 30 good + 30 bad.
    pub fn load(root: &Path) -> Result<Self, EvalError> {
        let cases_dir = root.join("data").join("corpus");
        let mut entries: Vec<PathBuf> = std::fs::read_dir(&cases_dir)
            .map_err(|source| EvalError::Io {
                path: cases_dir.clone(),
                source,
            })?
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| p.extension().and_then(|e| e.to_str()) == Some("yaml"))
            .collect();
        entries.sort();

        let mut cases = Vec::with_capacity(entries.len());
        for path in entries {
            cases.push(parse_case_file(&path)?);
        }
        if cases.is_empty() {
            return Err(EvalError::CorpusEmpty);
        }
        let (good, bad) = cases.iter().fold((0usize, 0usize), |(g, b), c| {
            match c.frontmatter.expected_outcome {
                ExpectedOutcome::Good => (g + 1, b),
                ExpectedOutcome::Bad => (g, b + 1),
            }
        });
        if good != 30 || bad != 30 {
            return Err(EvalError::CorpusUnbalanced { good, bad });
        }
        Ok(Corpus { cases })
    }

    pub fn len(&self) -> usize {
        self.cases.len()
    }

    pub fn is_empty(&self) -> bool {
        self.cases.is_empty()
    }
}

fn parse_case_file(path: &Path) -> Result<ReferenceCase, EvalError> {
    let raw = std::fs::read_to_string(path).map_err(|source| EvalError::Io {
        path: path.to_owned(),
        source,
    })?;
    let frontmatter: CaseFrontmatter =
        serde_yaml::from_str(&raw).map_err(|source| EvalError::Yaml {
            path: path.to_owned(),
            source,
        })?;
    Ok(ReferenceCase {
        frontmatter,
        source: path.to_owned(),
    })
}
