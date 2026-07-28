//! Phase 30 scorecard types (F30-01, F30-02).
//!
//! **The verifier is deliberately ASYMMETRIC, and that is not a bug.** Grading a
//! criterion `MET` or `MET_WITH_STATED_EXCEPTIONS` requires a non-empty evidence
//! set in which every reference resolves and none is marked unproven; grading it
//! `NOT_MET`, `PARTIAL` or `UNPROVEN` requires nothing at all and always
//! succeeds. An agent under time pressure reaches for the grade that ends the
//! task, so the cheap grade is made the honest one.
//!
//! Both enums are CLOSED — no catch-all, no default, no untagged fallback — so a
//! grade or a maturity state nobody declared fails at DESERIALIZATION, before any
//! logic runs. An agent on this program once invented an extra termination state
//! to dodge an artifact it wrongly believed unobtainable; the Phase 30 shape of
//! that move is to invent a sixth verdict meaning ready without meaning proved.
//! `tests/scorecard_contract.rs` feeds the exact invented string and asserts the
//! refusal, so a future reader finds the reason in a test rather than a comment.
//!
//! Every struct crossing a trust boundary sets `deny_unknown_fields`: a truth
//! that was silently ignored reads exactly like a truth that was supplied.

use std::path::Path;
use std::process::Command;

use serde::{Deserialize, Serialize};

/// Errors from scorecard verification. `thiserror` per AGENTS.md — every
/// refusal NAMES the reference or field that caused it, because a refusal a
/// reader cannot locate is only marginally better than a silent pass.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum ScorecardError {
    #[error("criterion `{criterion}` is graded {grade} but carries no evidence at all")]
    MetWithoutEvidence { criterion: String, grade: String },

    #[error(
        "criterion `{criterion}` is graded {grade} but evidence reference `{reference}` \
         does not resolve ({detail})"
    )]
    EvidenceDoesNotResolve {
        criterion: String,
        grade: String,
        reference: String,
        detail: String,
    },

    #[error(
        "criterion `{criterion}` is graded {grade} but evidence reference `{reference}` \
         is marked UNPROVEN"
    )]
    EvidenceUnproven {
        criterion: String,
        grade: String,
        reference: String,
    },

    #[error("surface row `{surface}`: {detail}")]
    InvalidSurfaceRow { surface: String, detail: String },

    #[error("command-tree walk: {0}")]
    Walk(String),
}

/// The eight maturity states CTRL-01 declares, in the ledger's own order.
///
/// CLOSED. A row carrying a state the ledger never declared fails to
/// deserialize. Tokens are spelled out per variant rather than derived by a
/// `rename_all`, so the wire vocabulary is greppable in this file and cannot
/// drift from the ledger silently.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum MaturityV1 {
    #[serde(rename = "ABSENT")]
    Absent,
    #[serde(rename = "SOURCE")]
    Source,
    #[serde(rename = "CONFIGURED")]
    Configured,
    #[serde(rename = "CONSTRUCTED")]
    Constructed,
    #[serde(rename = "REACHED")]
    Reached,
    #[serde(rename = "EFFECTIVE")]
    Effective,
    #[serde(rename = "OPERATOR_COMPLETE")]
    OperatorComplete,
    #[serde(rename = "PACKAGED_PROVEN")]
    PackagedProven,
}

/// Exhaustive list of the declared states. The COMPILER checks the count via the
/// array length, so a variant added without updating this constant fails to
/// build rather than relying on a reviewer counting variants by eye. Later plans
/// enumerate the states from here.
pub const ALL_MATURITY_STATES: [MaturityV1; 8] = [
    MaturityV1::Absent,
    MaturityV1::Source,
    MaturityV1::Configured,
    MaturityV1::Constructed,
    MaturityV1::Reached,
    MaturityV1::Effective,
    MaturityV1::OperatorComplete,
    MaturityV1::PackagedProven,
];

impl MaturityV1 {
    /// The ledger's own token for this state.
    pub fn token(self) -> &'static str {
        match self {
            Self::Absent => "ABSENT",
            Self::Source => "SOURCE",
            Self::Configured => "CONFIGURED",
            Self::Constructed => "CONSTRUCTED",
            Self::Reached => "REACHED",
            Self::Effective => "EFFECTIVE",
            Self::OperatorComplete => "OPERATOR_COMPLETE",
            Self::PackagedProven => "PACKAGED_PROVEN",
        }
    }
}

/// The five grades a Success Criterion may carry.
///
/// CLOSED at five. Named `CriterionVerdictV1` and NOT `Verdict`: the crate root
/// already re-exports a `Verdict` from `judge`, and shadowing it would be a
/// silent behavioural landmine for every existing caller.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum CriterionVerdictV1 {
    #[serde(rename = "MET")]
    Met,
    #[serde(rename = "MET_WITH_STATED_EXCEPTIONS")]
    MetWithStatedExceptions,
    #[serde(rename = "PARTIAL")]
    Partial,
    #[serde(rename = "NOT_MET")]
    NotMet,
    #[serde(rename = "UNPROVEN")]
    Unproven,
}

impl CriterionVerdictV1 {
    pub fn token(self) -> &'static str {
        match self {
            Self::Met => "MET",
            Self::MetWithStatedExceptions => "MET_WITH_STATED_EXCEPTIONS",
            Self::Partial => "PARTIAL",
            Self::NotMet => "NOT_MET",
            Self::Unproven => "UNPROVEN",
        }
    }

    /// Whether this grade must PAY for itself with resolving, proven evidence.
    ///
    /// This single predicate is the whole asymmetry. `NOT_MET`, `PARTIAL` and
    /// `UNPROVEN` cost nothing precisely so that an honest downgrade is never
    /// the expensive option.
    pub fn requires_evidence(self) -> bool {
        matches!(self, Self::Met | Self::MetWithStatedExceptions)
    }
}

/// Whether a referenced measurement was actually taken.
///
/// `Unproven` is explicitly representable, mirroring `receipt::Evidence`'s
/// `Unavailable`: an absent measurement must be sayable, or it gets written as a
/// plausible zero.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MeasurementStateV1 {
    Proven,
    Unproven,
}

/// Where an evidence reference points. Resolved by RUNNING something — a stat or
/// a `git cat-file` — never by reading.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum EvidenceLocatorV1 {
    /// A path relative to the repository root.
    Path { path: String },
    /// A git object in this repository.
    GitObject { object: String },
}

/// One evidence reference behind a criterion.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EvidenceRefV1 {
    pub id: String,
    pub locator: EvidenceLocatorV1,
    pub measurement: MeasurementStateV1,
}

impl EvidenceRefV1 {
    /// Resolve this reference against a real repository. `Ok(())` only if the
    /// object actually exists.
    pub fn resolve(&self, repo_root: &Path) -> Result<(), String> {
        match &self.locator {
            EvidenceLocatorV1::Path { path } => {
                let full = repo_root.join(path);
                if full.exists() {
                    Ok(())
                } else {
                    Err(format!("no such path: {}", full.display()))
                }
            }
            EvidenceLocatorV1::GitObject { object } => {
                let out = Command::new("git")
                    .arg("-C")
                    .arg(repo_root)
                    .arg("cat-file")
                    .arg("-t")
                    .arg(object)
                    .output()
                    .map_err(|e| format!("git cat-file failed to run: {e}"))?;
                if out.status.success() {
                    Ok(())
                } else {
                    Err(format!(
                        "git cat-file -t {object} exited {:?}",
                        out.status.code()
                    ))
                }
            }
        }
    }
}

/// A graded Success Criterion.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CriterionV1 {
    pub id: String,
    pub statement: String,
    pub grade: CriterionVerdictV1,
    #[serde(default)]
    pub evidence: Vec<EvidenceRefV1>,
}

impl CriterionV1 {
    /// THE ASYMMETRY. An expensive `MET`, a free `NOT_MET`.
    pub fn verify(&self, repo_root: &Path) -> Result<(), ScorecardError> {
        if !self.grade.requires_evidence() {
            // NOT_MET / PARTIAL / UNPROVEN: no evidence requirement whatsoever.
            return Ok(());
        }
        let grade = self.grade.token().to_string();
        if self.evidence.is_empty() {
            return Err(ScorecardError::MetWithoutEvidence {
                criterion: self.id.clone(),
                grade,
            });
        }
        for ev in &self.evidence {
            if ev.measurement == MeasurementStateV1::Unproven {
                return Err(ScorecardError::EvidenceUnproven {
                    criterion: self.id.clone(),
                    grade,
                    reference: ev.id.clone(),
                });
            }
            if let Err(detail) = ev.resolve(repo_root) {
                return Err(ScorecardError::EvidenceDoesNotResolve {
                    criterion: self.id.clone(),
                    grade,
                    reference: ev.id.clone(),
                    detail,
                });
            }
        }
        Ok(())
    }
}

/// A truth that may not have been measured yet.
///
/// `Unproven` carries what WOULD measure it, so an unmeasured cell states its
/// own discharge condition instead of inviting a plausible guess.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case", deny_unknown_fields)]
pub enum TruthV1 {
    Measured { value: String },
    Unproven { would_be_measured_by: String },
}

/// Maturity as a TRUTH rather than as a bare value.
///
/// Taking the surface inventory exposed the reason this wrapper has to exist:
/// six shipped top-level commands are claimed by no CTRL-01 coverage family, so
/// there is no recorded maturity for them at all. `MaturityV1` has no member
/// meaning "nobody has graded this", and it must not grow one — `ABSENT` would
/// assert the capability does not exist, which is false and worse than silence.
/// So the unmeasured case is lifted OUT of the enum instead of into it: the
/// closed eight-state enum keeps refusing any token the ledger never declared,
/// and "not yet graded" stays sayable without a plausible guess.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case", deny_unknown_fields)]
pub enum MaturityTruthV1 {
    Measured { value: MaturityV1 },
    Unproven { would_be_measured_by: String },
}

/// The seven truths F30-01 and F30-02 require of every surface, all REQUIRED.
///
/// A row missing any one of them is not a partially good row, it is an
/// unreviewable row — so a half-filled row is made unrepresentable rather than
/// merely discouraged. There is no builder that produces one and no `Option`
/// standing in for a truth the requirement demands.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SurfaceRowV1 {
    /// Stable id, e.g. `SURF-07`.
    pub id: String,
    /// The surface's command path as the binary itself spells it.
    pub command_path: String,
    /// Truth 1 — versioned activation state.
    pub versioned_activation: TruthV1,
    /// Truth 2 — operator-completeness state.
    pub operator_completeness: TruthV1,
    /// Truth 3 — maturity. A measured value is drawn from the ledger's own
    /// closed enum; an ungraded surface says so rather than guessing.
    pub maturity: MaturityTruthV1,
    /// Truth 4 — the security authority accountable for this surface.
    pub security_authority_owner: String,
    /// Truth 5 — evidence references.
    pub evidence: Vec<EvidenceRefV1>,
    /// Truth 6 — peer delta, bound to the pinned baseline token.
    pub peer_delta: TruthV1,
    /// Truth 7 — the phase at which this row was last refreshed.
    pub last_refreshed_phase: String,
}

impl SurfaceRowV1 {
    pub fn verify(&self, repo_root: &Path) -> Result<(), ScorecardError> {
        if self.id.trim().is_empty() {
            return Err(ScorecardError::InvalidSurfaceRow {
                surface: self.command_path.clone(),
                detail: "empty id".to_string(),
            });
        }
        if self.command_path.trim().is_empty() {
            return Err(ScorecardError::InvalidSurfaceRow {
                surface: self.id.clone(),
                detail: "empty command_path".to_string(),
            });
        }
        if self.security_authority_owner.trim().is_empty() {
            return Err(ScorecardError::InvalidSurfaceRow {
                surface: self.id.clone(),
                detail: "empty security_authority_owner".to_string(),
            });
        }
        if self.last_refreshed_phase.trim().is_empty() {
            return Err(ScorecardError::InvalidSurfaceRow {
                surface: self.id.clone(),
                detail: "empty last_refreshed_phase".to_string(),
            });
        }
        // A row's own evidence must resolve when it claims a measured truth.
        for ev in &self.evidence {
            if ev.measurement != MeasurementStateV1::Proven {
                continue;
            }
            if let Err(detail) = ev.resolve(repo_root) {
                return Err(ScorecardError::InvalidSurfaceRow {
                    surface: self.id.clone(),
                    detail: format!("evidence `{}` does not resolve: {detail}", ev.id),
                });
            }
        }
        Ok(())
    }
}

/// A whole scorecard document.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ScorecardDocumentV1 {
    pub schema: String,
    pub schema_version: u32,
    /// The commit the figures were taken at. A scorecard that cannot say which
    /// tree it measured cannot be re-run against a different one.
    pub source_sha: String,
    #[serde(default)]
    pub criteria: Vec<CriterionV1>,
    #[serde(default)]
    pub surfaces: Vec<SurfaceRowV1>,
}

impl ScorecardDocumentV1 {
    pub fn verify(&self, repo_root: &Path) -> Result<(), ScorecardError> {
        for c in &self.criteria {
            c.verify(repo_root)?;
        }
        for s in &self.surfaces {
            s.verify(repo_root)?;
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Command-tree walk
// ---------------------------------------------------------------------------

/// One node of the shipped binary's own command tree.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct SurfaceNode {
    pub command_path: String,
    /// Number of direct subcommands. 0 means a leaf.
    pub arity: usize,
    pub synopsis: String,
}

/// Maximum recursion depth for the walk. Bounded EXPLICITLY: an unbounded walk
/// over a binary that accepts any token as a subcommand does not terminate.
pub const MAX_WALK_DEPTH: usize = 3;

/// Walk a real binary's own `--help` tree.
///
/// This is a MEASUREMENT OF THE ARTIFACT, not a reading of a document: it runs
/// the binary. Output is sorted and therefore byte-deterministic, and a command
/// path reached twice is a refusal rather than a loop.
pub fn walk_command_tree(binary: &Path) -> Result<Vec<SurfaceNode>, ScorecardError> {
    let mut out: Vec<SurfaceNode> = Vec::new();
    let mut seen: Vec<String> = Vec::new();
    walk_inner(binary, &[], 0, &mut out, &mut seen)?;
    out.sort();
    Ok(out)
}

fn walk_inner(
    binary: &Path,
    path: &[String],
    depth: usize,
    out: &mut Vec<SurfaceNode>,
    seen: &mut Vec<String>,
) -> Result<(), ScorecardError> {
    if depth > MAX_WALK_DEPTH {
        return Ok(());
    }
    let joined = path.join(" ");
    if !joined.is_empty() {
        if seen.contains(&joined) {
            return Err(ScorecardError::Walk(format!(
                "command path `{joined}` reached twice — refusing rather than looping"
            )));
        }
        seen.push(joined.clone());
    }

    let mut cmd = Command::new(binary);
    for p in path {
        cmd.arg(p);
    }
    cmd.arg("--help");
    let output = cmd
        .output()
        .map_err(|e| ScorecardError::Walk(format!("running {}: {e}", binary.display())))?;
    let help = String::from_utf8_lossy(&output.stdout).to_string();

    let children = parse_subcommands(&help);

    if !joined.is_empty() {
        out.push(SurfaceNode {
            command_path: joined,
            arity: children.len(),
            synopsis: String::new(),
        });
    }

    for (name, synopsis) in &children {
        let mut child_path = path.to_vec();
        child_path.push(name.clone());
        let child_joined = child_path.join(" ");
        walk_inner(binary, &child_path, depth + 1, out, seen)?;
        // Attach the synopsis the PARENT gave, which is the authoritative one.
        if let Some(node) = out.iter_mut().find(|n| n.command_path == child_joined) {
            node.synopsis = synopsis.clone();
        }
    }
    Ok(())
}

/// Parse the `Commands:` block of a clap `--help` rendering.
///
/// Deliberately conservative: only lines inside the commands section, only
/// tokens that look like a subcommand name. `help` is excluded because it is
/// clap's own built-in and is not a product surface.
pub fn parse_subcommands(help: &str) -> Vec<(String, String)> {
    let mut result = Vec::new();
    let mut in_commands = false;
    for line in help.lines() {
        let trimmed = line.trim_end();
        if trimmed.trim_start().starts_with("Commands:") {
            in_commands = true;
            continue;
        }
        if in_commands {
            // A blank line, or a new unindented section header, ends the block.
            if trimmed.trim().is_empty() {
                if !result.is_empty() {
                    break;
                }
                continue;
            }
            if !trimmed.starts_with(' ') {
                break;
            }
            let body = trimmed.trim_start();
            let mut parts = body.splitn(2, char::is_whitespace);
            let name = parts.next().unwrap_or_default().to_string();
            let synopsis = parts.next().unwrap_or_default().trim().to_string();
            if name.is_empty() || name == "help" {
                continue;
            }
            // A subcommand name is a bare identifier; anything else is prose
            // wrapping from the previous entry.
            if !name
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
            {
                continue;
            }
            result.push((name, synopsis));
        }
    }
    result.sort();
    result.dedup_by(|a, b| a.0 == b.0);
    result
}

/// Render the walk as the stable tab-separated table the inventory commits.
pub fn render_surfaces_tsv(nodes: &[SurfaceNode]) -> String {
    let mut s = String::from("command_path\tarity\tsynopsis\n");
    for n in nodes {
        s.push_str(&format!(
            "{}\t{}\t{}\n",
            n.command_path, n.arity, n.synopsis
        ));
    }
    s
}
