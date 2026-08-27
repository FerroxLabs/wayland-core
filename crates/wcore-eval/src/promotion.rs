//! The promotion gate: score an installed skill artifact and hand governance the
//! [`PromotionEvidence`] it refuses to promote without.
//!
//! # Why the gate lives here and not in `wcore-skills`
//!
//! `wcore-skills` owns the *rule* — nothing becomes model-facing without a score that
//! clears its threshold — and takes the evidence as a required argument. It cannot own the
//! *scorer*: `wcore-eval` already depends on `wcore-skills` for [`SkillMetadata`], so the
//! dependency cannot run the other way. That is the right way round anyway. The rule is
//! meant to be stable; the scorer is meant to be replaced.
//!
//! # What it scores, and why that is the bytes on disk
//!
//! [`evaluate_skill_dir`] reads `<skill_dir>/SKILL.md` and scores it with the same
//! [`DefaultScorer`] the W10A corpus is graded by. It deliberately does **not** score the
//! staged P4 procedure row, even on the procedure path: the grant binds a content digest of
//! the directory, so scoring anything other than those bytes would evaluate one artifact
//! and promote another.
//!
//! # It fails loud, never open
//!
//! A gate that can silently no-op on a missing input is worth nothing — it goes green
//! having checked nothing. So every way of not having something to score is an error:
//!
//! * no `SKILL.md`, or an unreadable one → [`EvalError::ArtifactMissing`];
//! * a `SKILL.md` with no YAML frontmatter → [`EvalError::ArtifactMalformed`].
//!
//! The second is not pedantry. Without frontmatter there is no declared name, no
//! `when_to_use` and no `description`, and `parse_frontmatter` returns an empty
//! [`FrontmatterData`] rather than failing — so the artifact would be scored as though the
//! author had simply written a terse skill, and a large enough body can carry that to a
//! passing score. Refusing to score it is the honest outcome; a promotion refusal for a
//! reason the file did not actually exhibit would not be.
//!
//! # Both directions
//!
//! The gate can pass and it can fail, on realistic input. A well-formed draft — the shape
//! `wcore_skills::draft::synth_skill_body` emits — misses only the `$ARGUMENTS` and
//! `when_to_use` structural checks and lands around 0.84, comfortably over the 0.65 cutoff.
//! A corrupted one (no description, no name, a body that reaches for tools it never
//! declared) falls under it. `tests/promotion_gate.rs` runs both arms.

use std::path::Path;

use wcore_skills::promote::PromotionEvidence;
use wcore_skills::types::{FrontmatterData, LoadedFrom, SkillMetadata, SkillSource};

use crate::corpus::{Candidate, Verdict};
use crate::error::EvalError;
use crate::scorer::{DefaultScorer, LOCKED, ScoreOutcome, Scorer};

/// Recorded verbatim in every grant this gate authorises, so a reader of the grant can tell
/// whose judgement they are inheriting. Versioned by the scorer, not by the crate: the
/// LOCKED constants are what the number means.
pub const EVALUATOR: &str = "wcore-eval/DefaultScorer(W10A-LOCKED)";

/// The filename every skill artifact is defined by.
const SKILL_FILE: &str = "SKILL.md";

/// What the gate produced for one artifact.
///
/// Both halves are returned because they answer different questions. `evidence` is what
/// governance consumes and what lands in the grant; `outcome` carries the per-dimension
/// breakdown, which is what an operator needs in order to fix a refused draft rather than
/// guess at it.
#[derive(Debug, Clone)]
pub struct GateResult {
    pub evidence: PromotionEvidence,
    pub outcome: ScoreOutcome,
}

impl GateResult {
    /// Would governance accept this? Mirrors [`PromotionEvidence::clears`] so callers can
    /// report the verdict before handing it over.
    pub fn clears(&self) -> bool {
        self.evidence.clears()
    }
}

/// The evidence an artifact that could **not** be scored carries.
///
/// A caller that hits an [`EvalError`] has two honest options: abort, or hand governance
/// evidence that cannot clear any threshold. It must not have a third, which is to carry on
/// with no evidence at all.
///
/// The second option exists because aborting at the call site reorders the refusals a user
/// sees: `promote_existing` refuses a **revoked** artifact before it looks at any score, and
/// user intent should not be pre-empted by "and also we could not parse it". Handing the
/// governance boundary failing evidence keeps that ordering while leaving the artifact
/// unpromotable by construction — the score is 0.0 against the real cutoff, so it fails the
/// same comparison every other refusal fails.
pub fn unscorable_evidence() -> PromotionEvidence {
    PromotionEvidence {
        evaluator: EVALUATOR.to_string(),
        score: 0.0,
        threshold: LOCKED.acceptance_cutoff(),
        verdict: "unscorable".to_string(),
    }
}

/// Score the artifact installed at `skill_dir`.
///
/// `skill_dir` is the directory the loader sees, and its file name is what the declared
/// `name:` is checked against — a draft whose frontmatter names one skill while living in
/// another's directory is exactly the mismatch the corpus's check 7 exists to catch.
pub fn evaluate_skill_dir(skill_dir: &Path) -> Result<GateResult, EvalError> {
    let path = skill_dir.join(SKILL_FILE);
    let raw = std::fs::read_to_string(&path).map_err(|source| EvalError::ArtifactMissing {
        path: path.clone(),
        source,
    })?;

    if !has_frontmatter(&raw) {
        return Err(EvalError::ArtifactMalformed {
            path,
            reason: "no YAML frontmatter. A skill with no frontmatter declares no name, no \
                     description and no when_to_use, and scoring it would grade the absence \
                     of a header as a terse skill"
                .into(),
        });
    }

    let dir_name = skill_dir
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();

    let parsed = wcore_skills::frontmatter::parse_frontmatter_with_source(&raw, path.to_str());
    let skill = to_metadata(&parsed.frontmatter, &parsed.content, &dir_name);

    let candidate = Candidate {
        skill,
        // No trace: promotion evaluates an artifact, not a run of it. The scorer's cost
        // axis contributes its full weight when no trace is present, which is the same
        // treatment the 30 trace-less corpus cases get.
        trace: None,
        source_filename: dir_name.clone(),
    };
    let outcome = DefaultScorer::new().score(&candidate);

    Ok(GateResult {
        evidence: PromotionEvidence {
            evaluator: EVALUATOR.to_string(),
            score: outcome.dimensions.combined,
            threshold: LOCKED.acceptance_cutoff(),
            verdict: match outcome.predicted {
                Verdict::Good => "good".to_string(),
                Verdict::Bad => "bad".to_string(),
            },
        },
        outcome,
    })
}

/// Does the file open with a closed YAML frontmatter block?
///
/// Deliberately stricter than `parse_frontmatter`, which falls back to an empty
/// `FrontmatterData` for anything it cannot read. Here the absence has to be detectable,
/// because it is a refusal rather than a default.
fn has_frontmatter(raw: &str) -> bool {
    let rest = match raw.strip_prefix("---\r\n") {
        Some(r) => r,
        None => match raw.strip_prefix("---\n") {
            Some(r) => r,
            None => return false,
        },
    };
    rest.contains("\n---\n") || rest.contains("\n---\r\n") || rest.trim_end().ends_with("\n---")
}

/// Normalise parsed frontmatter into the shape the scorer grades.
///
/// The one judgement call: `resolved_name` is the **declared** name, not the directory
/// name. `parse_skill_fields` is normally called by the loader with the directory name, so
/// `name` and the filename can never disagree and structural check 7 would be vacuous.
/// Passing the declared name — and the empty string when none is declared — keeps that
/// check live, which is the whole reason it was added.
fn to_metadata(fm: &FrontmatterData, content: &str, dir_name: &str) -> SkillMetadata {
    let declared = fm
        .name
        .as_deref()
        .map(str::trim)
        .filter(|n| !n.is_empty())
        .unwrap_or_default();
    let mut skill = wcore_skills::frontmatter::parse_skill_fields(
        fm,
        content,
        declared,
        SkillSource::User,
        LoadedFrom::Skills,
        Some(dir_name),
    );
    // `parse_skill_fields` records the body verbatim; the corpus loader trims the trailing
    // newlines a text editor leaves behind. Match it, so a file that differs from a corpus
    // case only by a final newline does not score differently.
    skill.content = skill.content.trim_end_matches('\n').to_owned();
    skill.content_length = skill.content.len();
    skill
}
