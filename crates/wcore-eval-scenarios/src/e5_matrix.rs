//! The Phase 28 E5 certification matrix, as DATA.
//!
//! The matrix is GENERATED, not written. Nine dimensions come from requirement F28-01
//! verbatim and are FIXED. Three OS families are fixed. The surfaces are resolved from the
//! candidate binary by `.planning/scripts/f28-resolve-candidate.py`. Cells are the cross
//! product, filtered by declared applicability.
//!
//! Writing cells by hand is how a dimension silently loses a platform. Here a dimension
//! absent from an OS family without an explicit applicability record is a CONSTRUCTION
//! ERROR, so it fails the build instead of passing unnoticed.
//!
//! # What this module refuses, and why refusal beats warning
//!
//! Construction is fail-closed. A matrix that reports its own defects at the end has
//! already spent the hardware, so every one of these is an `Err` from `generate`:
//!
//! * a dimension absent from an OS family with no applicability record;
//! * a skip carrying no class — unrepresentable, see [`SkipEvidence`];
//! * a CRITICAL cell carrying any skip at all: **a critical cell has NO legal skip**, and
//!   a critical cell that cannot be run is a RED;
//! * an `observation-blocked` skip whose evidence is not a control measured in the
//!   certification environment at run time;
//! * a missing mandatory cell, or a mandatory cell downgraded off `critical`.
//!
//! # The sandbox activeness rule
//!
//! A GREEN on any sandbox-dimension cell requires POSITIVE evidence that the sandbox was
//! ACTIVE for that cell. Absence of an observed violation is NOT evidence of a sandbox:
//! Windows can run with the sandbox silently disabled on an AppContainer ACL lease
//! SID/profile mismatch, and a cell that merely failed to observe a violation would then
//! report green over no sandbox at all.
//!
//! This is enforced in the TYPE, not by convention. [`ActivenessEvidence`] has exactly two
//! variants — `Observed` and `NotMeasured` — and **there is no variant expressing "no
//! violation observed"**. `NotMeasured` carries a reason and no verdict and no count, per
//! the standing rule that a measurement which cannot be taken must never render as `0`.
//! [`SandboxPass`] is constructible only from a non-empty `Observed`.
//!
//! Full specification: `.planning/phases/28-native-cross-platform-certification/`
//! `28-01-CERTIFICATION-CONTRACT.md`.

use std::collections::BTreeSet;
use std::fmt;

use thiserror::Error;

use crate::Platform;

// ---------------------------------------------------------------------------------------
// Dimensions — FIXED, verbatim from F28-01
// ---------------------------------------------------------------------------------------

/// The nine F28-01 dimensions.
///
/// > **F28-01**: Native macOS, Linux, and Windows E5 matrices cover sandbox probes,
/// > Unicode, long paths, UNC/reparse/symlink cases, process cleanup, suspend/resume,
/// > offline, disk-full/read-only, and hostile inputs.
///
/// Nine. Do not add one because it seems useful, do not merge two because they seem
/// similar, and do not rename one to something tidier — the requirement text is the
/// authority and a renamed dimension is an unprovable one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Dimension {
    SandboxProbes,
    Unicode,
    LongPaths,
    UncReparseSymlink,
    ProcessCleanup,
    SuspendResume,
    Offline,
    DiskFullReadOnly,
    HostileInputs,
}

impl Dimension {
    /// All nine, in requirement order. Fixed.
    pub const ALL: [Self; 9] = [
        Self::SandboxProbes,
        Self::Unicode,
        Self::LongPaths,
        Self::UncReparseSymlink,
        Self::ProcessCleanup,
        Self::SuspendResume,
        Self::Offline,
        Self::DiskFullReadOnly,
        Self::HostileInputs,
    ];

    /// The stable wire id. Consumed by `f28-ledger.py --check-matrix`, which pins the
    /// same nine strings, so a rename here fails that gate too.
    pub const fn id(self) -> &'static str {
        match self {
            Self::SandboxProbes => "sandbox-probes",
            Self::Unicode => "unicode",
            Self::LongPaths => "long-paths",
            Self::UncReparseSymlink => "unc-reparse-symlink",
            Self::ProcessCleanup => "process-cleanup",
            Self::SuspendResume => "suspend-resume",
            Self::Offline => "offline",
            Self::DiskFullReadOnly => "disk-full-read-only",
            Self::HostileInputs => "hostile-inputs",
        }
    }

    /// The requirement's own words for this dimension.
    pub const fn requirement_text(self) -> &'static str {
        match self {
            Self::SandboxProbes => "sandbox probes",
            Self::Unicode => "Unicode",
            Self::LongPaths => "long paths",
            Self::UncReparseSymlink => "UNC/reparse/symlink cases",
            Self::ProcessCleanup => "process cleanup",
            Self::SuspendResume => "suspend/resume",
            Self::Offline => "offline",
            Self::DiskFullReadOnly => "disk-full/read-only",
            Self::HostileInputs => "hostile inputs",
        }
    }
}

impl fmt::Display for Dimension {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.id())
    }
}

// ---------------------------------------------------------------------------------------
// Criticality — a property of (dimension, OS family), declared in a table
// ---------------------------------------------------------------------------------------

/// Criticality is declared per (dimension, OS family) in [`criticality`], NEVER decided
/// per cell at run time. Deciding it per cell is how a critical case becomes non-critical
/// the moment it is inconvenient.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Criticality {
    Critical,
    Standard,
}

impl Criticality {
    pub const fn id(self) -> &'static str {
        match self {
            Self::Critical => "critical",
            Self::Standard => "standard",
        }
    }
    pub const fn is_critical(self) -> bool {
        matches!(self, Self::Critical)
    }
}

/// The criticality table.
///
/// Two dimensions are CRITICAL on every family, because each is the literal subject matter
/// of a Phase 28 Success Criterion:
///
/// * `sandbox-probes` — Criterion 1 is "the required hostile platform matrix", which the
///   requirement spells out as including sandbox probes.
/// * `process-cleanup` — Criterion 2 requires the soak complete with "no orphan process".
///
/// Everything else is standard. Standard does NOT mean optional; it means a legal skip
/// exists for it when one of the four contract classes applies with its evidence.
pub const fn criticality(dimension: Dimension, _os: Platform) -> Criticality {
    match dimension {
        Dimension::SandboxProbes | Dimension::ProcessCleanup => Criticality::Critical,
        _ => Criticality::Standard,
    }
}

// ---------------------------------------------------------------------------------------
// Skips — exactly four classes, and no fifth
// ---------------------------------------------------------------------------------------

/// The four legal skip classes from the certification contract. **No fifth class may be
/// added mid-run**; adding one is a finding against the run, not a fix for it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum SkipClass {
    PlatformInapplicability,
    ObservationBlocked,
    ArchitecturalImpossibility,
    UnresolvedSurface,
}

impl SkipClass {
    pub const fn id(self) -> &'static str {
        match self {
            Self::PlatformInapplicability => "platform-inapplicability",
            Self::ObservationBlocked => "observation-blocked",
            Self::ArchitecturalImpossibility => "architectural-impossibility",
            Self::UnresolvedSurface => "unresolved-surface",
        }
    }
}

/// A skip's evidence, with the class DERIVED from the variant.
///
/// This is why an unclassified skip is not merely rejected but **unrepresentable**: there
/// is no way to express a skip without also expressing which class it is and supplying
/// that class's required evidence.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SkipEvidence {
    /// The case is *meaningless* on that family — not hard, not unsupported, meaningless.
    PlatformInapplicability { fact: String, observable: String },
    /// The observation channel is broken INDEPENDENTLY OF THE PRODUCT.
    ///
    /// `control_ref` must name a control MEASURED IN THE CERTIFICATION ENVIRONMENT AT RUN
    /// TIME, constructed so that it fails when the channel is sound:
    /// `control:<id>@<host>:<run-id>`.
    ///
    /// A citation of a handoff, a prior phase, or any inherited belief is NOT acceptable
    /// evidence — **including a document that reports in the product's favour**. A
    /// laundering channel does not become sound by pointing it at good news. Absent a
    /// passing control the cell is a RED, not a skip.
    ObservationBlocked { control_ref: String },
    /// The behaviour CANNOT EXIST on that platform by construction. Requires an executable
    /// check, not an argument that it would be hard. Difficulty is not impossibility.
    ArchitecturalImpossibility { impossibility_check: String },
    /// The surface a phase claimed was not landed. Requires the phase AND that phase's own
    /// recorded requirement disposition.
    UnresolvedSurface {
        phase: String,
        req_disposition: String,
    },
}

impl SkipEvidence {
    pub const fn class(&self) -> SkipClass {
        match self {
            Self::PlatformInapplicability { .. } => SkipClass::PlatformInapplicability,
            Self::ObservationBlocked { .. } => SkipClass::ObservationBlocked,
            Self::ArchitecturalImpossibility { .. } => SkipClass::ArchitecturalImpossibility,
            Self::UnresolvedSurface { .. } => SkipClass::UnresolvedSurface,
        }
    }

    /// Flattened for the TSV, so `f28-ledger.py` validates exactly what Rust constructed.
    pub fn render(&self) -> String {
        match self {
            Self::PlatformInapplicability { fact, observable } => {
                format!("fact={fact}; observable={observable}")
            }
            Self::ObservationBlocked { control_ref } => control_ref.clone(),
            Self::ArchitecturalImpossibility {
                impossibility_check,
            } => format!("impossibility_check={impossibility_check}"),
            Self::UnresolvedSurface {
                phase,
                req_disposition,
            } => format!("phase={phase} req_disposition={req_disposition}"),
        }
    }
}

/// Documentary-citation markers. An `observation-blocked` control reference matching any
/// of these is rejected. Mirrors `LORE_PATTERNS` in `f28-ledger.py`.
const LORE_MARKERS: [&str; 9] = [
    ".md",
    "handoff",
    "intel/",
    "-plan",
    "-summary",
    "requirements",
    "roadmap",
    "lore",
    "as established",
];

/// `control:<id>@<host>:<run-id>` — a control bound to where and when it was measured.
fn is_runtime_control_ref(s: &str) -> bool {
    let Some(rest) = s.strip_prefix("control:") else {
        return false;
    };
    let Some((id, tail)) = rest.split_once('@') else {
        return false;
    };
    let Some((host, run)) = tail.split_once(':') else {
        return false;
    };
    let ok = |v: &str, dash: bool| {
        !v.is_empty()
            && v.chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || (dash && (c == '.' || c == '_')))
    };
    ok(id, false) && ok(host, true) && ok(run, true)
}

// ---------------------------------------------------------------------------------------
// Sandbox activeness
// ---------------------------------------------------------------------------------------

/// Evidence that the sandbox was active for a cell.
///
/// **There is deliberately no variant expressing "no violation observed."** Absence of a
/// failure is not evidence of a sandbox, and making that inexpressible is the structural
/// answer to the silent-disable defect.
///
/// `NotMeasured` carries a reason and no verdict and no count: a measurement that cannot
/// be taken must never render as `0`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ActivenessEvidence {
    /// POSITIVE: the sandbox was observed active for this cell.
    Observed { probe: String, detail: String },
    /// The measurement could not be taken. This is a RED, never a green and never a skip.
    NotMeasured { reason: String },
}

/// A PASSED outcome on a sandbox-dimension cell.
///
/// Constructible only from a non-empty [`ActivenessEvidence::Observed`], so
/// "the sandbox cell passed because nothing went wrong" cannot be expressed at all.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SandboxPass {
    probe: String,
    detail: String,
}

impl SandboxPass {
    pub fn new(cell_id: &str, evidence: ActivenessEvidence) -> Result<Self, MatrixError> {
        match evidence {
            ActivenessEvidence::Observed { probe, detail }
                if !probe.trim().is_empty() && !detail.trim().is_empty() =>
            {
                Ok(Self { probe, detail })
            }
            ActivenessEvidence::Observed { .. } => Err(MatrixError::SandboxActivenessEmpty {
                cell_id: cell_id.to_string(),
            }),
            ActivenessEvidence::NotMeasured { reason } => {
                Err(MatrixError::SandboxPassWithoutActiveness {
                    cell_id: cell_id.to_string(),
                    reason,
                })
            }
        }
    }

    pub fn probe(&self) -> &str {
        &self.probe
    }
    pub fn detail(&self) -> &str {
        &self.detail
    }
}

// ---------------------------------------------------------------------------------------
// Cells
// ---------------------------------------------------------------------------------------

/// A surface resolved from the candidate binary's own command tree.
///
/// Constructed from `evidence/28-01/candidate.json`, never typed. This is the only input
/// to the generator that varies with what phases 24-27 actually landed.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct Surface {
    pub id: String,
    pub entrypoint: String,
    pub depth: u8,
}

impl Surface {
    pub fn new(id: impl Into<String>, entrypoint: impl Into<String>, depth: u8) -> Self {
        Self {
            id: id.into(),
            entrypoint: entrypoint.into(),
            depth,
        }
    }

    /// `cmd:gateway/status` -> `gateway-status`
    fn slug(&self) -> String {
        self.id
            .strip_prefix("cmd:")
            .unwrap_or(&self.id)
            .replace('/', "-")
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Applicability {
    Applicable,
    Skipped(SkipEvidence),
}

/// Whether a sandbox-activeness assertion is required for this cell.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActivenessRequirement {
    /// Sandbox-dimension cell: a green requires positive activeness evidence.
    Required,
    NotApplicable,
}

impl ActivenessRequirement {
    pub const fn id(self) -> &'static str {
        match self {
            Self::Required => "required",
            Self::NotApplicable => "n/a",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Cell {
    pub id: String,
    pub dimension: Dimension,
    pub os: Platform,
    pub surface: String,
    pub criticality: Criticality,
    pub applicability: Applicability,
    pub activeness: ActivenessRequirement,
}

impl Cell {
    /// Record a PASSED outcome.
    ///
    /// On a sandbox-dimension cell this requires positive activeness evidence and returns
    /// `Err` otherwise, so a silently-disabled sandbox cannot report green.
    pub fn record_pass(
        &self,
        activeness: Option<ActivenessEvidence>,
    ) -> Result<Option<SandboxPass>, MatrixError> {
        if self.activeness == ActivenessRequirement::Required {
            let Some(evidence) = activeness else {
                return Err(MatrixError::SandboxPassWithoutActiveness {
                    cell_id: self.id.clone(),
                    reason: "no activeness evidence supplied; absence of an observed \
                             violation is not evidence of a sandbox"
                        .to_string(),
                });
            };
            return SandboxPass::new(&self.id, evidence).map(Some);
        }
        Ok(None)
    }
}

/// A declared (dimension, OS family) applicability record.
///
/// A dimension missing from an OS family WITHOUT one of these is a construction error.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApplicabilityRecord {
    pub dimension: Dimension,
    pub os: Platform,
    pub evidence: SkipEvidence,
}

// ---------------------------------------------------------------------------------------
// Mandatory cells
// ---------------------------------------------------------------------------------------

/// Emitted regardless of what surface resolution produces. All three are CRITICAL and
/// therefore unskippable.
#[derive(Debug, Clone, Copy)]
pub struct MandatoryCell {
    pub id: &'static str,
    pub dimension: Dimension,
    pub os: Platform,
    pub rationale: &'static str,
}

pub const MANDATORY_CELLS: [MandatoryCell; 3] = [
    MandatoryCell {
        id: "w-sandbox-silent-disable",
        dimension: Dimension::SandboxProbes,
        os: Platform::Windows,
        rationale: "The AppContainer ACL lease SID/profile mismatch under which the sandbox \
                    is silently inactive while the product keeps executing. Criterion 1 \
                    exists to force this into the open; a generator that could omit it \
                    would defeat the criterion.",
    },
    MandatoryCell {
        id: "w-process-cleanup-descendant-tree",
        dimension: Dimension::ProcessCleanup,
        os: Platform::Windows,
        rationale: "Descendant process-tree reaping. Criterion 2 names \"no orphan process\" \
                    as its own subject.",
    },
    MandatoryCell {
        id: "w-sandbox-observability-control",
        dimension: Dimension::SandboxProbes,
        os: Platform::Windows,
        rationale: "Whether the Windows sandbox is observable IN THE CERTIFICATION \
                    ENVIRONMENT is a question this phase MEASURES, in either direction. It \
                    occupies a cell so its answer is recorded as evidence rather than as an \
                    executor's recollection.",
    },
];

/// The surface recorded for a mandatory cell. These are product-wide, not surface-specific.
pub const PRODUCT_WIDE_SURFACE: &str = "product-wide";

// ---------------------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum MatrixError {
    #[error(
        "F28M-005 dimension `{dimension}` is absent from OS family `{os}` with no \
         applicability record carrying a reason"
    )]
    DimensionMissingOnOs { dimension: Dimension, os: Platform },

    #[error(
        "F28M-002 cell `{cell_id}` is CRITICAL and carries a `{class}` skip; a critical \
         cell has NO legal skip, and a critical cell that cannot be run is a RED"
    )]
    CriticalCellSkipped {
        cell_id: String,
        class: &'static str,
    },

    #[error(
        "F28M-004 cell `{cell_id}`: observation-blocked evidence `{evidence}` is not a \
         control measured in the certification environment at run time \
         (control:<id>@<host>:<run-id>); absent such a control the cell is a RED, not a skip"
    )]
    ObservationBlockedWithoutControl { cell_id: String, evidence: String },

    #[error(
        "F28M-004 cell `{cell_id}`: observation-blocked evidence `{evidence}` cites a \
         document (matched `{marker}`); a citation of a handoff, a prior phase or any \
         inherited belief is NOT acceptable evidence, including one that reports in the \
         product's favour"
    )]
    ObservationBlockedCitesDocument {
        cell_id: String,
        evidence: String,
        marker: &'static str,
    },

    #[error("F28M-012 cell `{cell_id}`: `{class}` skip carries empty evidence")]
    SkipEvidenceEmpty {
        cell_id: String,
        class: &'static str,
    },

    #[error("F28M-007 mandatory cell `{cell_id}` is absent from the matrix")]
    MandatoryCellAbsent { cell_id: &'static str },

    #[error("F28M-007 mandatory cell `{cell_id}` is not marked critical")]
    MandatoryCellNotCritical { cell_id: &'static str },

    #[error("F28M-007 mandatory cell `{cell_id}` carries a skip; it is critical and unskippable")]
    MandatoryCellSkipped { cell_id: &'static str },

    #[error(
        "F28M-006 cell `{cell_id}` cannot be recorded PASSED: {reason}. Absence of an \
         observed violation is not evidence of a sandbox; this cell is a RED."
    )]
    SandboxPassWithoutActiveness { cell_id: String, reason: String },

    #[error("F28M-006 cell `{cell_id}`: activeness evidence is present but empty")]
    SandboxActivenessEmpty { cell_id: String },

    #[error(
        "F28M-015 no surfaces were resolved; the matrix would certify nothing. Resolve the \
         candidate first with f28-resolve-candidate.py"
    )]
    NoSurfaces,

    #[error("F28M-016 duplicate cell id `{cell_id}`; cell ids must be stable AND unique")]
    DuplicateCellId { cell_id: String },
}

// ---------------------------------------------------------------------------------------
// The matrix
// ---------------------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Matrix {
    pub cells: Vec<Cell>,
    pub max_surface_depth: u8,
}

impl Matrix {
    /// Generate the matrix: nine fixed dimensions crossed with three OS families and the
    /// resolved surfaces, filtered by declared applicability.
    ///
    /// Deterministic: cells are ordered by (dimension, OS family, surface id) and the
    /// mandatory cells are appended in their declared order, so two runs over the same
    /// `candidate.json` produce identical ids in identical order and the matrix is stable
    /// across the three hosts that will execute it.
    pub fn generate(
        surfaces: &[Surface],
        applicability: &[ApplicabilityRecord],
        max_surface_depth: u8,
    ) -> Result<Self, MatrixError> {
        let mut selected: Vec<&Surface> = surfaces
            .iter()
            .filter(|s| s.depth <= max_surface_depth)
            .collect();
        selected.sort();
        if selected.is_empty() {
            return Err(MatrixError::NoSurfaces);
        }

        let mut cells = Vec::new();

        for dimension in Dimension::ALL {
            for os in Platform::ALL {
                let record = applicability
                    .iter()
                    .find(|r| r.dimension == dimension && r.os == os);
                let crit = criticality(dimension, os);

                // A declared (dimension, OS) skip on a CRITICAL dimension is rejected here
                // rather than silently producing skipped critical cells.
                if let Some(record) = record {
                    if crit.is_critical() {
                        return Err(MatrixError::CriticalCellSkipped {
                            cell_id: format!("{dimension}-{os}-*"),
                            class: record.evidence.class().id(),
                        });
                    }
                }

                for surface in &selected {
                    let id = format!("{}-{}-{}", dimension.id(), os, surface.slug());
                    let applicability = match record {
                        Some(r) => Applicability::Skipped(r.evidence.clone()),
                        None => Applicability::Applicable,
                    };
                    cells.push(Cell {
                        id,
                        dimension,
                        os,
                        surface: surface.id.clone(),
                        criticality: crit,
                        applicability,
                        activeness: activeness_for(dimension),
                    });
                }
            }
        }

        // NOTE: no coverage check here. Counting what this loop just inserted would be a
        // check that cannot fail — the exact self-passing shape this program has paid for
        // repeatedly. Coverage is asserted by `validate()` below, against the ASSEMBLED
        // cells, which is a state a hand-built `Matrix` can genuinely violate.

        for m in MANDATORY_CELLS {
            cells.push(Cell {
                id: m.id.to_string(),
                dimension: m.dimension,
                os: m.os,
                surface: PRODUCT_WIDE_SURFACE.to_string(),
                criticality: Criticality::Critical,
                applicability: Applicability::Applicable,
                activeness: activeness_for(m.dimension),
            });
        }

        let matrix = Self {
            cells,
            max_surface_depth,
        };
        matrix.validate()?;
        Ok(matrix)
    }

    /// Every construction-time rejection, applied to an assembled matrix.
    ///
    /// Called by `generate`, and public so a caller that assembles cells by another route
    /// cannot bypass the rules.
    pub fn validate(&self) -> Result<(), MatrixError> {
        let mut seen: BTreeSet<&str> = BTreeSet::new();
        for cell in &self.cells {
            if !seen.insert(cell.id.as_str()) {
                return Err(MatrixError::DuplicateCellId {
                    cell_id: cell.id.clone(),
                });
            }

            if cell.dimension == Dimension::SandboxProbes
                && cell.activeness != ActivenessRequirement::Required
            {
                return Err(MatrixError::SandboxPassWithoutActiveness {
                    cell_id: cell.id.clone(),
                    reason: "sandbox-dimension cell does not require activeness evidence"
                        .to_string(),
                });
            }

            let Applicability::Skipped(evidence) = &cell.applicability else {
                continue;
            };

            if cell.criticality.is_critical() {
                return Err(MatrixError::CriticalCellSkipped {
                    cell_id: cell.id.clone(),
                    class: evidence.class().id(),
                });
            }

            check_skip_evidence(&cell.id, evidence)?;
        }

        for m in MANDATORY_CELLS {
            let Some(cell) = self.cells.iter().find(|c| c.id == m.id) else {
                return Err(MatrixError::MandatoryCellAbsent { cell_id: m.id });
            };
            if !cell.criticality.is_critical() {
                return Err(MatrixError::MandatoryCellNotCritical { cell_id: m.id });
            }
            if matches!(cell.applicability, Applicability::Skipped(_)) {
                return Err(MatrixError::MandatoryCellSkipped { cell_id: m.id });
            }
        }

        for dimension in Dimension::ALL {
            for os in Platform::ALL {
                if !self
                    .cells
                    .iter()
                    .any(|c| c.dimension == dimension && c.os == os)
                {
                    return Err(MatrixError::DimensionMissingOnOs { dimension, os });
                }
            }
        }
        Ok(())
    }

    /// The machine artifact plans 02 and 04 consume. Validated by
    /// `f28-ledger.py --check-matrix` and `--check-no-uncontrolled-skips`.
    pub fn to_tsv(&self) -> String {
        let mut out = String::new();
        out.push_str(
            "# Phase 28 E5 certification matrix. GENERATED by \
             wcore_eval_scenarios::e5_matrix — do not edit.\n\
             # Nine F28-01 dimensions x three OS families x the surfaces resolved from the \
             candidate.\n\
             # Validated by: f28-ledger.py --check-matrix and --check-no-uncontrolled-skips\n",
        );
        out.push_str(&format!(
            "# max_surface_depth={} cells={}\n",
            self.max_surface_depth,
            self.cells.len()
        ));
        out.push_str(
            "#cell_id\tdimension\tos\tsurface\tcriticality\tapplicability\tskip_class\t\
             skip_evidence\tactiveness\n",
        );
        for c in &self.cells {
            let (applicability, class, evidence) = match &c.applicability {
                Applicability::Applicable => ("applicable", "-".to_string(), "-".to_string()),
                Applicability::Skipped(e) => ("skipped", e.class().id().to_string(), e.render()),
            };
            out.push_str(&format!(
                "{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\n",
                c.id,
                c.dimension.id(),
                c.os,
                c.surface,
                c.criticality.id(),
                applicability,
                class,
                evidence,
                c.activeness.id(),
            ));
        }
        out
    }

    /// Surfaces read off the candidate ledger emitted by `f28-resolve-candidate.py`.
    ///
    /// The surface list comes from the BINARY, via that resolver. No feature name is read
    /// out of a planning document.
    pub fn surfaces_from_candidate_json(json: &str) -> Result<Vec<Surface>, String> {
        let doc: serde_json::Value =
            serde_json::from_str(json).map_err(|e| format!("candidate.json is not JSON: {e}"))?;
        let list = doc
            .get("surfaces")
            .and_then(|v| v.as_array())
            .ok_or_else(|| "candidate.json has no `surfaces` array".to_string())?;
        let mut out = Vec::with_capacity(list.len());
        for entry in list {
            let id = entry
                .get("id")
                .and_then(|v| v.as_str())
                .ok_or_else(|| "a surface entry has no `id`".to_string())?;
            let entrypoint = entry
                .get("entrypoint")
                .and_then(|v| v.as_str())
                .ok_or_else(|| format!("surface `{id}` has no `entrypoint`"))?;
            let depth = entry
                .get("depth")
                .and_then(serde_json::Value::as_u64)
                .ok_or_else(|| format!("surface `{id}` has no `depth`"))?;
            out.push(Surface::new(id, entrypoint, depth as u8));
        }
        Ok(out)
    }
}

const fn activeness_for(dimension: Dimension) -> ActivenessRequirement {
    match dimension {
        Dimension::SandboxProbes => ActivenessRequirement::Required,
        _ => ActivenessRequirement::NotApplicable,
    }
}

fn check_skip_evidence(cell_id: &str, evidence: &SkipEvidence) -> Result<(), MatrixError> {
    let empty = |v: &str| v.trim().is_empty();
    match evidence {
        SkipEvidence::PlatformInapplicability { fact, observable } => {
            if empty(fact) || empty(observable) {
                return Err(MatrixError::SkipEvidenceEmpty {
                    cell_id: cell_id.to_string(),
                    class: evidence.class().id(),
                });
            }
        }
        SkipEvidence::ArchitecturalImpossibility {
            impossibility_check,
        } => {
            if empty(impossibility_check) {
                return Err(MatrixError::SkipEvidenceEmpty {
                    cell_id: cell_id.to_string(),
                    class: evidence.class().id(),
                });
            }
        }
        SkipEvidence::UnresolvedSurface {
            phase,
            req_disposition,
        } => {
            if empty(phase) || empty(req_disposition) {
                return Err(MatrixError::SkipEvidenceEmpty {
                    cell_id: cell_id.to_string(),
                    class: evidence.class().id(),
                });
            }
        }
        SkipEvidence::ObservationBlocked { control_ref } => {
            let lowered = control_ref.to_ascii_lowercase();
            if let Some(marker) = LORE_MARKERS.iter().copied().find(|m| lowered.contains(*m)) {
                return Err(MatrixError::ObservationBlockedCitesDocument {
                    cell_id: cell_id.to_string(),
                    evidence: control_ref.clone(),
                    marker,
                });
            }
            if !is_runtime_control_ref(control_ref) {
                return Err(MatrixError::ObservationBlockedWithoutControl {
                    cell_id: cell_id.to_string(),
                    evidence: control_ref.clone(),
                });
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn surfaces() -> Vec<Surface> {
        vec![
            Surface::new("cmd:alpha", "wayland-core alpha", 1),
            Surface::new("cmd:beta", "wayland-core beta", 1),
        ]
    }

    #[test]
    fn nine_dimensions_exactly_and_ids_are_unique() {
        assert_eq!(Dimension::ALL.len(), 9);
        let ids: BTreeSet<&str> = Dimension::ALL.iter().map(|d| d.id()).collect();
        assert_eq!(ids.len(), 9, "dimension ids must be unique");
    }

    #[test]
    fn generation_covers_every_dimension_on_every_os_family() {
        let m = Matrix::generate(&surfaces(), &[], 1).expect("generates");
        for dimension in Dimension::ALL {
            for os in Platform::ALL {
                assert!(
                    m.cells
                        .iter()
                        .any(|c| c.dimension == dimension && c.os == os),
                    "{dimension} missing on {os}"
                );
            }
        }
        // 9 dimensions x 3 families x 2 surfaces + 3 mandatory
        assert_eq!(m.cells.len(), 9 * 3 * 2 + 3);
    }

    #[test]
    fn a_control_reference_is_required_and_a_document_is_not_one() {
        assert!(is_runtime_control_ref(
            "control:appc-observe@seandesktop:30184651330"
        ));
        assert!(!is_runtime_control_ref(
            ".planning/intel/APPCONTAINER-SSH-LORE-READJUDICATION.md"
        ));
        assert!(!is_runtime_control_ref("the channel was clearly broken"));
        assert!(!is_runtime_control_ref("control:@host:run"));
    }

    #[test]
    fn a_sandbox_pass_needs_positive_evidence_and_not_measured_is_a_red() {
        let ok = SandboxPass::new(
            "c",
            ActivenessEvidence::Observed {
                probe: "appcontainer-sid".into(),
                detail: "profile WCore-x active".into(),
            },
        );
        assert!(ok.is_ok());

        let red = SandboxPass::new(
            "c",
            ActivenessEvidence::NotMeasured {
                reason: "probe unavailable".into(),
            },
        );
        assert!(matches!(
            red,
            Err(MatrixError::SandboxPassWithoutActiveness { .. })
        ));
    }
}
