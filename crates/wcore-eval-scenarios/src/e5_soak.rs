//! The F28-02 1,000-session soak — canonical definitions, VOID rules and verdicts.
//!
//! # Why the definitions live here and the execution does not
//!
//! Identical to [`crate::e5_cases`], and forced by the same constraint: the
//! certification Mac may run the CI-produced `wayland-core` binary but may **not** run
//! cargo beyond `cargo fmt --all -- --check`. A soak implemented as a cargo-built test
//! harness therefore cannot run there at all, and an observable whose only
//! implementation is such a harness silently loses a whole OS family.
//!
//! So this module is the **canonical definition** — the session target, the block and
//! window geometry, the six canary channels, the per-platform census backends, the four
//! observables and, most importantly, the conditions under which a result is **VOID
//! rather than green** — and `scripts/f28-native-soak.mjs` is the **executor**, which
//! needs only a Node runtime and the shipped binary. `tests/e5_soak_contract.rs` asserts
//! the two agree, so the executor cannot drift from the definition it implements.
//!
//! # The rule this module exists to enforce
//!
//! A canary scan reporting zero detections and a canary scan that never ran produce
//! **identical output**. So do a clean orphan census and an orphan census that never
//! enumerated. Absence of a detection and absence of a detector are indistinguishable
//! from the outside, and a certification that cannot tell them apart signs "no secret
//! leaked" over the output of nothing.
//!
//! Every observable therefore carries a **positive control** — a deliberately leaked
//! canary that MUST be detected, a deliberately orphaned process that MUST be found, a
//! deliberately growing resource lane whose slope MUST be flagged — and a run whose
//! positive control was missed is [`Verdict::Void`], never [`Verdict::Green`]. The miss
//! is itself a finding.
//!
//! # Bands are never defaulted
//!
//! The quality/performance delta has no threshold anywhere in the program; plan 28-03
//! task 1 decided one by cross-audit and committed it to `evidence/28-03/bands.json`
//! before any soak session ran. [`drift_verdict`] takes `Option<&Bands>` and returns
//! [`Verdict::Void`] when it is `None`. A defaulted band is a band nobody decided, and a
//! harness that silently supplies one makes the criterion unfalsifiable.
//!
//! # The census caveat is carried forward, not dropped
//!
//! [`crate::process_tree`] owns per-platform process-tree ownership: a private cgroup v2
//! on Linux entered from `pre_exec` before candidate code can fork, a kill-on-close Job
//! Object assigned on Windows before the primary thread resumes, and an **explicitly
//! non-authoritative** observed-process-group fallback elsewhere, because a hostile
//! descendant can leave its process group. [`CensusBackend`] mirrors that table exactly
//! and `tests/e5_soak_contract.rs` asserts the mirror against the real
//! `ProcessTree::prepare()` on whichever platform the test runs on. A macOS census that
//! reports zero orphans reports it **non-authoritatively**, and says so.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::Platform;

// ---------------------------------------------------------------------------------------
// Geometry — the requirement, not a preference
// ---------------------------------------------------------------------------------------

/// F28-02 says a 1,000-session soak. This plan has no authority to reduce it: a family
/// that cannot reach it records how many it completed and marks the criterion NOT MET.
pub const SESSION_TARGET: u64 = 1_000;

/// Sessions per block. Blocks, not individual sessions, are the unit of drift comparison
/// so that a single load spike cannot move a window on its own.
pub const BLOCK_SIZE: u64 = 100;

/// The number of blocks a complete soak produces.
pub const BLOCK_COUNT: u64 = SESSION_TARGET / BLOCK_SIZE;

/// A zero-concurrency soak is not this soak. A defect that needs two siblings to
/// manifest is invisible to a serial run, and this program has already measured one — a
/// parallel-sibling budget-authority fault that reproduced 13 of 24 times on Linux and
/// 23 of 24 on Windows. Concurrency is therefore a property of the workload and a
/// configuration that removes it fails the contract rather than running quietly.
pub const MIN_CONCURRENCY: u32 = 2;

// ---------------------------------------------------------------------------------------
// Canary channels — mirrored from the receipt model, all six or it is not a scan
// ---------------------------------------------------------------------------------------

/// The six channels [`crate::receipt::CanaryScanEvidenceV1`] already models.
///
/// A scan that omits one is rejected: a dropped channel is exactly how a leak survives a
/// clean-looking scan, and per-channel counts (never a boolean) are what make the
/// omission visible.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CanaryChannel {
    Protocol,
    Stdout,
    Stderr,
    Files,
    Logs,
    Telemetry,
}

impl CanaryChannel {
    pub const ALL: [Self; 6] = [
        Self::Protocol,
        Self::Stdout,
        Self::Stderr,
        Self::Files,
        Self::Logs,
        Self::Telemetry,
    ];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Protocol => "protocol",
            Self::Stdout => "stdout",
            Self::Stderr => "stderr",
            Self::Files => "files",
            Self::Logs => "logs",
            Self::Telemetry => "telemetry",
        }
    }
}

// ---------------------------------------------------------------------------------------
// Census backends — mirrored from process_tree, caveat included
// ---------------------------------------------------------------------------------------

/// Per-platform process-tree ownership, mirroring [`crate::process_tree`].
///
/// `is_authoritative()` is the honesty seam. On macOS the fallback observes a process
/// group, and a hostile descendant can leave one, so a zero census there is a zero
/// **observation** rather than a guarantee. The soak result records that distinction
/// instead of implying a containment property it does not have.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum CensusBackend {
    CgroupV2,
    WindowsJobObject,
    ProcessGroupObservedNonauthoritative,
}

impl CensusBackend {
    /// The same strings [`crate::process_tree::ProcessTree::backend_name`] returns.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::CgroupV2 => "cgroup-v2",
            Self::WindowsJobObject => "windows-job-object",
            Self::ProcessGroupObservedNonauthoritative => "process-group-observed-nonauthoritative",
        }
    }

    pub const fn is_authoritative(self) -> bool {
        match self {
            Self::CgroupV2 | Self::WindowsJobObject => true,
            Self::ProcessGroupObservedNonauthoritative => false,
        }
    }

    pub const fn for_platform(os: Platform) -> Self {
        match os {
            Platform::Linux => Self::CgroupV2,
            Platform::Windows => Self::WindowsJobObject,
            Platform::Macos => Self::ProcessGroupObservedNonauthoritative,
        }
    }
}

// ---------------------------------------------------------------------------------------
// Observables and verdicts
// ---------------------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Observable {
    CanaryIntegrity,
    OrphanCensus,
    ResourceSeries,
    QualityPerformanceDrift,
}

impl Observable {
    pub const ALL: [Self; 4] = [
        Self::CanaryIntegrity,
        Self::OrphanCensus,
        Self::ResourceSeries,
        Self::QualityPerformanceDrift,
    ];
}

/// There is no `Green` that can be reached without a caught positive control.
///
/// `Void` is deliberately not a flavour of `Red`: a red is a measurement of the product,
/// a void is the absence of a measurement. Collapsing them would let a broken detector
/// be reported as a product defect, or — far worse in the other direction — let a
/// missing detector's silence be read as a clean result.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "verdict", rename_all = "lowercase")]
pub enum Verdict {
    Green,
    Red { code: String, detail: String },
    Void { code: String, detail: String },
}

impl Verdict {
    pub fn red(code: &str, detail: impl Into<String>) -> Self {
        Self::Red {
            code: code.to_string(),
            detail: detail.into(),
        }
    }

    pub fn void(code: &str, detail: impl Into<String>) -> Self {
        Self::Void {
            code: code.to_string(),
            detail: detail.into(),
        }
    }

    pub const fn is_green(&self) -> bool {
        matches!(self, Self::Green)
    }

    pub const fn is_void(&self) -> bool {
        matches!(self, Self::Void { .. })
    }
}

// ---------------------------------------------------------------------------------------
// The measured record — what the executor emits and what every verdict is computed from
// ---------------------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CanaryScan {
    /// Per-channel detection counts of the REAL canaries. Never a boolean: a boolean
    /// cannot distinguish a clean scan from a scan that did not run.
    pub channels: BTreeMap<String, u64>,
    /// Channels the scan actually covered. A channel present in `channels` but absent
    /// here would be a count nobody measured.
    pub channels_scanned: Vec<String>,
    /// The deliberately leaked control canary. If this was not detected there is no
    /// working detector and the clean result means nothing.
    pub control_detected: bool,
    pub control_channel: String,
}

impl CanaryScan {
    pub fn real_detections(&self) -> u64 {
        self.channels.values().copied().fold(0, u64::saturating_add)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OrphanCensus {
    pub backend: CensusBackend,
    pub authoritative: bool,
    /// Product processes still alive after the run that the harness did not own.
    pub orphans_found: u64,
    /// The deliberately orphaned control process. Unfound means the census did not
    /// enumerate, which is not the same thing as there being nothing to find.
    pub control_orphan_found: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResourceSample {
    pub session_index: u64,
    pub metrics: BTreeMap<String, f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResourceSeries {
    pub samples: Vec<ResourceSample>,
    /// A deliberately growing lane. Its slope MUST be flagged by the same evaluator, or
    /// the evaluator does not detect growth and a flat verdict on the real metrics is
    /// an absence of a detector rather than an absence of growth.
    pub control_growth_flagged: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DriftMeasurement {
    pub metric: String,
    pub early: f64,
    pub late: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DriftBand {
    pub metric: String,
    /// `late <= early * max_ratio`. Exactly one of `max_ratio` / `max_absolute_drop` is
    /// set; the schema check in the validator enforces that.
    pub max_ratio: Option<f64>,
    /// `late >= early - max_absolute_drop`, for rates where a ratio is meaningless.
    pub max_absolute_drop: Option<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FloorBand {
    pub metric: String,
    /// `">="` or `"<="`.
    pub op: String,
    pub value: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SlopeBand {
    pub metric: String,
    /// Maximum permitted increase per 1,000 sessions, expressed as a ratio of the first
    /// sample when `ratio` is true and in absolute units otherwise.
    pub max_growth: f64,
    pub ratio: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Bands {
    pub session_target: u64,
    pub block_size: u64,
    pub min_concurrency: u32,
    /// Minimum retained resource samples. An endpoint-only run cannot distinguish a leak
    /// from a high-water mark, so it is rejected rather than evaluated.
    pub resource_min_samples: usize,
    pub early_blocks: Vec<u64>,
    pub late_blocks: Vec<u64>,
    pub drift: Vec<DriftBand>,
    pub floors: Vec<FloorBand>,
    pub slopes: Vec<SlopeBand>,
}

// ---------------------------------------------------------------------------------------
// The verdict functions — the whole point of the module
// ---------------------------------------------------------------------------------------

/// VOID when the control canary went undetected, or when a channel was not scanned.
/// RED when a real canary was detected anywhere. GREEN only with a caught control and
/// full coverage.
pub fn canary_verdict(scan: &CanaryScan) -> Verdict {
    let missing: Vec<&str> = CanaryChannel::ALL
        .iter()
        .map(|c| c.as_str())
        .filter(|c| !scan.channels_scanned.iter().any(|s| s == c))
        .collect();
    if !missing.is_empty() {
        return Verdict::void(
            "F28S-010",
            format!(
                "canary scan did not cover channel(s): {}",
                missing.join(",")
            ),
        );
    }
    if !scan.control_detected {
        return Verdict::void(
            "F28S-011",
            format!(
                "control canary planted in `{}` was NOT detected; a clean scan from a \
                 detector that cannot detect is not a clean result",
                scan.control_channel
            ),
        );
    }
    let detections = scan.real_detections();
    if detections != 0 {
        return Verdict::red(
            "F28S-012",
            format!("{detections} real canary detection(s) across the scanned channels"),
        );
    }
    Verdict::Green
}

/// VOID when the control orphan went unfound. RED when any orphan was found.
pub fn orphan_verdict(census: &OrphanCensus) -> Verdict {
    if census.authoritative != census.backend.is_authoritative() {
        return Verdict::void(
            "F28S-022",
            format!(
                "census claims authoritative={} for backend `{}`, whose authority is {}",
                census.authoritative,
                census.backend.as_str(),
                census.backend.is_authoritative()
            ),
        );
    }
    if !census.control_orphan_found {
        return Verdict::void(
            "F28S-020",
            "the deliberately orphaned control process was NOT found; the census did not \
             enumerate, which is not the same as there being nothing to enumerate",
        );
    }
    if census.orphans_found != 0 {
        return Verdict::red(
            "F28S-021",
            format!(
                "{} orphaned product process(es) survived the run",
                census.orphans_found
            ),
        );
    }
    Verdict::Green
}

/// VOID when the series is endpoint-only or the growth control went unflagged.
/// RED when a slope exceeds its band.
pub fn resource_verdict(series: &ResourceSeries, bands: &Bands) -> Verdict {
    if series.samples.len() < bands.resource_min_samples {
        return Verdict::void(
            "F28S-030",
            format!(
                "resource series retained {} sample(s), below the decided minimum of {}; \
                 an endpoint reading cannot distinguish a leak from a high-water mark",
                series.samples.len(),
                bands.resource_min_samples
            ),
        );
    }
    if !series.control_growth_flagged {
        return Verdict::void(
            "F28S-031",
            "the deliberately growing control lane was NOT flagged by the slope evaluator; \
             a flat verdict from an evaluator that cannot see growth is not a flat result",
        );
    }
    let mut breaches = Vec::new();
    for band in &bands.slopes {
        let Some(growth) = series_growth(series, &band.metric) else {
            return Verdict::void(
                "F28S-032",
                format!("no retained samples carry metric `{}`", band.metric),
            );
        };
        let observed = if band.ratio {
            growth.ratio
        } else {
            growth.absolute
        };
        if observed > band.max_growth {
            breaches.push(format!(
                "{}: growth {:.4} exceeds decided {:.4}",
                band.metric, observed, band.max_growth
            ));
        }
    }
    if breaches.is_empty() {
        Verdict::Green
    } else {
        Verdict::red("F28S-033", breaches.join("; "))
    }
}

pub struct Growth {
    pub absolute: f64,
    pub ratio: f64,
}

/// First-to-last growth normalised to a 1,000-session run, computed from the retained
/// series. The endpoint reading survives only as one term of this, never as the verdict.
pub fn series_growth(series: &ResourceSeries, metric: &str) -> Option<Growth> {
    let points: Vec<(f64, f64)> = series
        .samples
        .iter()
        .filter_map(|s| s.metrics.get(metric).map(|v| (s.session_index as f64, *v)))
        .collect();
    let (first_idx, first) = *points.first()?;
    let (last_idx, last) = *points.last()?;
    let span = (last_idx - first_idx).max(1.0);
    let scale = SESSION_TARGET as f64 / span;
    let absolute = (last - first) * scale;
    let ratio = if first.abs() < f64::EPSILON {
        if absolute.abs() < f64::EPSILON {
            0.0
        } else {
            f64::INFINITY
        }
    } else {
        1.0 + absolute / first
    };
    Some(Growth { absolute, ratio })
}

/// VOID when no bands were supplied. The harness FAILS rather than defaulting, because
/// a default band is a band nobody decided.
pub fn drift_verdict(measurements: &[DriftMeasurement], bands: Option<&Bands>) -> Verdict {
    let Some(bands) = bands else {
        return Verdict::void(
            "F28S-040",
            "no bands file was available; a defaulted delta band is a band nobody decided \
             and would make the criterion unfalsifiable",
        );
    };
    let mut breaches = Vec::new();
    for band in &bands.drift {
        let Some(m) = measurements.iter().find(|m| m.metric == band.metric) else {
            return Verdict::void(
                "F28S-041",
                format!(
                    "decided band `{}` has no corresponding measurement",
                    band.metric
                ),
            );
        };
        if let Some(max_ratio) = band.max_ratio {
            let limit = m.early * max_ratio;
            if m.late > limit {
                breaches.push(format!(
                    "{}: late {:.3} exceeds early {:.3} x {:.3} = {:.3}",
                    band.metric, m.late, m.early, max_ratio, limit
                ));
            }
        }
        if let Some(max_drop) = band.max_absolute_drop {
            let limit = m.early - max_drop;
            if m.late < limit {
                breaches.push(format!(
                    "{}: late {:.4} falls below early {:.4} - {:.4} = {:.4}",
                    band.metric, m.late, m.early, max_drop, limit
                ));
            }
        }
    }
    if breaches.is_empty() {
        Verdict::Green
    } else {
        Verdict::red("F28S-042", breaches.join("; "))
    }
}

/// A shortfall is recorded as a shortfall. The soak has no authority to reduce its own
/// target, so `completed < target` is never a pass — it is the criterion NOT MET for
/// that family, with the number named.
pub fn session_count_verdict(completed: u64, target: u64) -> Verdict {
    if completed >= target {
        Verdict::Green
    } else {
        Verdict::red(
            "F28S-050",
            format!(
                "{completed} of {target} sessions completed; the shortfall is {} and the \
                 criterion is NOT MET for this family",
                target - completed
            ),
        )
    }
}

/// A soak configured without concurrent children is not this soak.
pub fn concurrency_verdict(concurrency: u32) -> Verdict {
    if concurrency >= MIN_CONCURRENCY {
        Verdict::Green
    } else {
        Verdict::red(
            "F28S-060",
            format!(
                "concurrency {concurrency} is below the required minimum {MIN_CONCURRENCY}; a \
                 sibling-dependent defect is invisible to a serial run"
            ),
        )
    }
}

// ---------------------------------------------------------------------------------------
// Family record
// ---------------------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FamilyRecord {
    pub family: String,
    pub host: String,
    pub target: String,
    pub binary_sha256: String,
    pub ledger_sha256: String,
    pub sessions_completed: u64,
    pub session_target: u64,
    pub concurrency: u32,
    pub canary: CanaryScan,
    pub census: OrphanCensus,
    pub resources: ResourceSeries,
    pub drift: Vec<DriftMeasurement>,
}

impl FamilyRecord {
    /// A family running a different build is not certifying the candidate, so the digest
    /// assertion is a precondition of every other verdict rather than one verdict among
    /// them.
    pub fn digest_bound(&self) -> bool {
        !self.ledger_sha256.is_empty() && self.binary_sha256 == self.ledger_sha256
    }

    pub fn verdicts(&self, bands: Option<&Bands>) -> Vec<(Observable, Verdict)> {
        if !self.digest_bound() {
            let v = Verdict::void(
                "F28S-001",
                format!(
                    "binary sha256 {} does not match the candidate ledger's {} for target {}",
                    self.binary_sha256, self.ledger_sha256, self.target
                ),
            );
            return Observable::ALL.iter().map(|o| (*o, v.clone())).collect();
        }
        vec![
            (Observable::CanaryIntegrity, canary_verdict(&self.canary)),
            (Observable::OrphanCensus, orphan_verdict(&self.census)),
            (
                Observable::ResourceSeries,
                match bands {
                    Some(b) => resource_verdict(&self.resources, b),
                    None => Verdict::void(
                        "F28S-040",
                        "no bands file was available for the resource slope evaluation",
                    ),
                },
            ),
            (
                Observable::QualityPerformanceDrift,
                drift_verdict(&self.drift, bands),
            ),
        ]
    }

    /// Criterion 2 is MET for this family only when every observable is green AND the
    /// full session target was reached AND concurrency was present. Anything else is
    /// stated plainly rather than qualified.
    pub fn criterion2_met(&self, bands: Option<&Bands>) -> bool {
        self.verdicts(bands).iter().all(|(_, v)| v.is_green())
            && session_count_verdict(self.sessions_completed, self.session_target).is_green()
            && concurrency_verdict(self.concurrency).is_green()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scan() -> CanaryScan {
        CanaryScan {
            channels: CanaryChannel::ALL
                .iter()
                .map(|c| (c.as_str().to_string(), 0))
                .collect(),
            channels_scanned: CanaryChannel::ALL
                .iter()
                .map(|c| c.as_str().to_string())
                .collect(),
            control_detected: true,
            control_channel: "files".to_string(),
        }
    }

    #[test]
    fn a_clean_scan_with_an_undetected_control_is_void_not_green() {
        let mut s = scan();
        s.control_detected = false;
        assert!(canary_verdict(&s).is_void());
        assert!(canary_verdict(&scan()).is_green());
    }

    #[test]
    fn a_dropped_channel_voids_the_scan() {
        let mut s = scan();
        s.channels_scanned.retain(|c| c != "telemetry");
        assert!(canary_verdict(&s).is_void());
    }

    #[test]
    fn census_backend_authority_mirrors_process_tree_semantics() {
        assert!(CensusBackend::for_platform(Platform::Linux).is_authoritative());
        assert!(CensusBackend::for_platform(Platform::Windows).is_authoritative());
        assert!(!CensusBackend::for_platform(Platform::Macos).is_authoritative());
    }

    /// The mirror is asserted against the REAL `process_tree`, not against a copy of its
    /// table. If that module renames a backend or changes which backends are
    /// authoritative, the soak's census would start describing an ownership model the
    /// crate no longer implements, and the macOS non-authoritative caveat is exactly the
    /// honesty this catches losing.
    ///
    /// Deliberately tolerant of the Linux cgroup fallback: `Cgroup::create()` can fail on
    /// a host without delegated cgroup v2, and process_tree then falls back to the
    /// observed process group. That is a property of the host, not a drift between the
    /// two tables, so the assertion is on the NAME-TO-AUTHORITY mapping rather than on
    /// which backend today's host happened to get.
    #[test]
    fn the_census_backend_table_agrees_with_the_real_process_tree() {
        let tree = crate::process_tree::ProcessTree::prepare()
            .expect("process_tree must be preparable on a supported host");
        let name = tree.backend_name();
        let mirrored = [
            CensusBackend::CgroupV2,
            CensusBackend::WindowsJobObject,
            CensusBackend::ProcessGroupObservedNonauthoritative,
        ]
        .into_iter()
        .find(|b| b.as_str() == name)
        .unwrap_or_else(|| {
            panic!(
                "process_tree reports backend `{name}`, which e5_soak::CensusBackend does not model"
            )
        });
        assert_eq!(
            mirrored.is_authoritative(),
            tree.is_authoritative(),
            "e5_soak and process_tree disagree about whether `{name}` is authoritative"
        );
    }

    #[test]
    fn a_shortfall_is_never_a_pass() {
        assert!(session_count_verdict(999, 1000).is_green() == false);
        assert!(session_count_verdict(1000, 1000).is_green());
    }
}
