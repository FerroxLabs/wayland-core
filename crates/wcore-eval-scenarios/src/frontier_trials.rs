//! Phase 30 frontier comparative trial harness (F30-03).
//!
//! **This module exists to make three forgeries unrepresentable, not merely discouraged.**
//!
//! 1. *A point estimate with no bounds.* `MeasurementV1::interval` is an `IntervalV1`, not
//!    an `Option<IntervalV1>`. F30-03 says confidence bounds; the way to guarantee bounds
//!    is to make their absence fail at deserialization.
//! 2. *A one-sided comparison.* `ComparativeResultV1::try_new` refuses to build unless every
//!    tool named by the protocol carries a measurement. "We could not run the competitor,
//!    so we win" is not expressible here.
//! 3. *A directional verdict on an indistinguishable result.* `verify` refuses
//!    `WAYLAND_AHEAD` or `PEER_AHEAD` whenever the delta interval contains zero. That single
//!    rule is worth more than any amount of prose about intellectual honesty.
//!
//! **Four verdict states, not three.** The plan specified three. The cross-audit panel
//! (`30-02-decision-evidence/`) overturned that unanimously on one argument: "CI contains
//! zero, therefore tie" silently converts low statistical power into declared equivalence.
//! A wide interval is `INCONCLUSIVE`, not `PRACTICALLY_INDISTINGUISHABLE`. Both added states
//! are non-directional, so rule 3 above is preserved exactly and strengthened — a
//! directional verdict now additionally has to clear the tie band.
//!
//! Every struct crossing the boundary sets `deny_unknown_fields`: a truth that was silently
//! ignored reads exactly like a truth that was supplied.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

/// `thiserror` per AGENTS.md. Every refusal NAMES the thing that caused it — a refusal a
/// reader cannot locate is only marginally better than a silent pass.
#[derive(Debug, thiserror::Error, PartialEq)]
pub enum FrontierTrialError {
    #[error(
        "cannot compute a {kind} over zero trials: a zero-trial proportion is the shape a \
         silently skipped leg takes, so it is refused rather than reported as zero or one"
    )]
    ZeroTrials { kind: String },

    #[error("incoherent proportion: {successes} successes over {trials} trials")]
    IncoherentProportion { successes: u32, trials: u32 },

    #[error(
        "comparative result for `{dimension}` is missing a measurement for tool `{tool}`; a \
         comparison with an unmeasured tool cannot be constructed, so an unrun peer can \
         never become an implicit win"
    )]
    MissingPeerMeasurement { dimension: String, tool: String },

    #[error(
        "comparative result for `{dimension}` reports direction `{direction}` but its delta \
         interval [{lower}, {upper}] CONTAINS ZERO; a directional verdict on an \
         indistinguishable result is the flattering lie this harness exists to refuse"
    )]
    DirectionalVerdictOnIntervalContainingZero {
        dimension: String,
        direction: String,
        lower: f64,
        upper: f64,
    },

    #[error(
        "comparative result for `{dimension}` reports direction `{declared}` but its delta \
         interval [{lower}, {upper}] against tie band {tie_band} entails `{entailed}`"
    )]
    DirectionDisagreesWithInterval {
        dimension: String,
        declared: String,
        entailed: String,
        lower: f64,
        upper: f64,
        tie_band: f64,
    },

    #[error(
        "result set records protocol digest `{recorded}` but the protocol supplied hashes to \
         `{actual}`; the methodology was amended after the measurement, or these results \
         belong to a different protocol"
    )]
    ProtocolDigestMismatch { recorded: String, actual: String },

    #[error("leg accounting is wrong: expected exactly {expected} legs, found {found}")]
    LegCountWrong { expected: usize, found: usize },

    #[error(
        "leg `{tool}`/`{dimension}` appears {count} times; every leg is accounted for exactly once"
    )]
    DuplicateLeg {
        tool: String,
        dimension: String,
        count: usize,
    },

    #[error(
        "leg `{id}` is UNPROVEN but names no blocker; silence about why a leg did not run is \
         exactly how a peer that would not install quietly disappears"
    )]
    UnprovenLegWithoutBlocker { id: String },

    #[error("leg `{id}` names no evidence capture")]
    LegWithoutEvidence { id: String },

    #[error("leg `{id}` is marked RUN but no measurement exists for `{tool}`/`{dimension}`")]
    RunLegWithoutMeasurement {
        id: String,
        tool: String,
        dimension: String,
    },

    #[error(
        "measurement for `{tool}`/`{dimension}` carries scope `{measurement_scope}` but the \
         result set declares `{set_scope}`; a scope that drifts between the measurement and \
         the document is how a harness number becomes a real-world claim"
    )]
    ScopeMismatch {
        tool: String,
        dimension: String,
        measurement_scope: String,
        set_scope: String,
    },

    #[error("interval is malformed: lower {lower} exceeds upper {upper}")]
    MalformedInterval { lower: f64, upper: f64 },

    #[error("bootstrap needs at least one sample and at least one resample")]
    EmptyBootstrap,

    #[error(
        "leg `{tool}`/`{dimension}` mixes {dialect_driven} dialect-compiled trials with \
         {frozen_script} frozen-script trials; a v1 trial spoke one competitor's dialect and a v2 \
         trial spoke the harness's own, so folding them into one proportion re-creates the \
         confound the dialect compiler exists to remove"
    )]
    MixedDialectProvenance {
        tool: String,
        dimension: String,
        dialect_driven: usize,
        frozen_script: usize,
    },

    #[error(
        "leg `{tool}`/`{dimension}` mixes {distinct} DIFFERENT translation digests; every trial in \
         one leg must be driven by the identical compiled dialect or the leg measures the \
         translations rather than the harness"
    )]
    MixedTranslationDigests {
        tool: String,
        dimension: String,
        distinct: usize,
    },
}

// ---------------------------------------------------------------------------
// Closed vocabularies
// ---------------------------------------------------------------------------

/// What a measurement is ALLOWED to be read as. CLOSED — an invented scope fails at
/// deserialization, before any logic runs, because 30-03 refuses a claim whose scope
/// exceeds its evidence's and that check needs this to be unforgeable here.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum ScopeV1 {
    /// The model is held constant by a scripted loopback fixture. Says NOTHING about model
    /// quality, real-world task success, or dollar cost.
    #[serde(rename = "SCRIPTED_HARNESS")]
    ScriptedHarness,
    /// A real provider with real credentials. Not reachable by this lane.
    #[serde(rename = "LIVE_PROVIDER")]
    LiveProvider,
    /// Read off source or documents without executing anything.
    #[serde(rename = "STATIC_SOURCE")]
    StaticSource,
}

impl ScopeV1 {
    pub fn token(self) -> &'static str {
        match self {
            Self::ScriptedHarness => "SCRIPTED_HARNESS",
            Self::LiveProvider => "LIVE_PROVIDER",
            Self::StaticSource => "STATIC_SOURCE",
        }
    }
}

/// The three tools under comparison. CLOSED.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ToolV1 {
    Wayland,
    Hermes,
    Openclaw,
}

impl ToolV1 {
    pub fn token(self) -> &'static str {
        match self {
            Self::Wayland => "wayland",
            Self::Hermes => "hermes",
            Self::Openclaw => "openclaw",
        }
    }
}

/// The five dimensions F30-03 names. CLOSED — `cognitive_tax` is present precisely so the
/// dimension can be carried as UNPROVEN rather than dropped from the accounting.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DimensionV1 {
    Correctness,
    Recovery,
    Security,
    Cost,
    CognitiveTax,
}

impl DimensionV1 {
    pub fn token(self) -> &'static str {
        match self {
            Self::Correctness => "correctness",
            Self::Recovery => "recovery",
            Self::Security => "security",
            Self::Cost => "cost",
            Self::CognitiveTax => "cognitive_tax",
        }
    }
}

/// Every tool, every dimension: the fifteen legs, in accounting order.
pub const ALL_TOOLS: [ToolV1; 3] = [ToolV1::Wayland, ToolV1::Hermes, ToolV1::Openclaw];
pub const ALL_DIMENSIONS: [DimensionV1; 5] = [
    DimensionV1::Correctness,
    DimensionV1::Recovery,
    DimensionV1::Security,
    DimensionV1::Cost,
    DimensionV1::CognitiveTax,
];
/// Five dimensions across three tools. The COMPILER derives it, so it cannot drift.
pub const EXPECTED_LEGS: usize = ALL_TOOLS.len() * ALL_DIMENSIONS.len();

/// How an interval was produced. CLOSED, so a bound cannot claim a method nobody ran.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IntervalMethodV1 {
    #[serde(rename = "wilson_score_95")]
    WilsonScore95,
    /// For a DIFFERENCE of proportions. Not the subtraction of two Wilson endpoints, and
    /// not Wald — codex raised this and neither other member contradicted it.
    #[serde(rename = "newcombe_wilson_95")]
    NewcombeWilson95,
    #[serde(rename = "percentile_bootstrap_95")]
    PercentileBootstrap95,
    /// Every observation identical. Reported as its own state rather than as a zero-width
    /// interval masquerading as precision.
    #[serde(rename = "zero_empirical_variance")]
    ZeroEmpiricalVariance,
}

/// The four reportable verdicts. `INCONCLUSIVE` is the one the plan did not have and the
/// panel insisted on: it is what stops a wide interval being published as equivalence.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum DirectionV1 {
    #[serde(rename = "WAYLAND_AHEAD")]
    WaylandAhead,
    #[serde(rename = "PEER_AHEAD")]
    PeerAhead,
    #[serde(rename = "PRACTICALLY_INDISTINGUISHABLE")]
    PracticallyIndistinguishable,
    #[serde(rename = "INCONCLUSIVE")]
    Inconclusive,
}

impl DirectionV1 {
    /// A verdict that asserts one tool is better than another. Only these two are gated on
    /// the zero rule; the other two are refusals to claim and are always available.
    pub fn is_directional(self) -> bool {
        matches!(self, Self::WaylandAhead | Self::PeerAhead)
    }

    pub fn token(self) -> &'static str {
        match self {
            Self::WaylandAhead => "WAYLAND_AHEAD",
            Self::PeerAhead => "PEER_AHEAD",
            Self::PracticallyIndistinguishable => "PRACTICALLY_INDISTINGUISHABLE",
            Self::Inconclusive => "INCONCLUSIVE",
        }
    }
}

/// Whether a leg produced a measurement or not. There is no third state, and in particular
/// no state meaning "skipped".
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum LegStatusV1 {
    #[serde(rename = "RUN")]
    Run,
    #[serde(rename = "UNPROVEN")]
    Unproven,
}

// ---------------------------------------------------------------------------
// Bounded values
// ---------------------------------------------------------------------------

/// A confidence interval. There is deliberately NO constructor that omits the bounds.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct IntervalV1 {
    pub lower: f64,
    pub upper: f64,
    pub method: IntervalMethodV1,
    pub confidence: f64,
}

impl IntervalV1 {
    /// The load-bearing predicate of this module.
    pub fn contains_zero(&self) -> bool {
        self.lower <= 0.0 && self.upper >= 0.0
    }

    fn validate(&self) -> Result<(), FrontierTrialError> {
        if self.lower > self.upper {
            return Err(FrontierTrialError::MalformedInterval {
                lower: self.lower,
                upper: self.upper,
            });
        }
        Ok(())
    }
}

/// A single tool's measurement on a single dimension.
///
/// `interval` is `IntervalV1` and NOT `Option<IntervalV1>`. That is the whole point: a
/// document that omits it does not deserialize, so an unbounded point estimate cannot
/// reach any downstream reader.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MeasurementV1 {
    pub tool: ToolV1,
    pub dimension: DimensionV1,
    pub scope: ScopeV1,
    pub trials: u32,
    /// Digest over the raw per-trial samples, so a reported estimate is traceable to the
    /// observations that produced it.
    pub samples_sha256: String,
    pub estimate: f64,
    pub interval: IntervalV1,
}

/// The signed difference between Wayland and one peer, with its own required interval.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DeltaV1 {
    pub estimate: f64,
    pub interval: IntervalV1,
}

/// A comparison on one dimension. Constructible ONLY when every required tool measured.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ComparativeResultV1 {
    pub dimension: DimensionV1,
    pub measurements: BTreeMap<ToolV1, MeasurementV1>,
    pub delta: DeltaV1,
    pub tie_band: f64,
    pub direction: DirectionV1,
}

impl ComparativeResultV1 {
    /// The only way to build one. `required` comes from the frozen protocol's tool list.
    ///
    /// The direction is DERIVED here rather than supplied, so a caller cannot assert one;
    /// `verify` then re-derives it independently, which is what catches a hand-edited
    /// document.
    pub fn try_new(
        dimension: DimensionV1,
        measurements: BTreeMap<ToolV1, MeasurementV1>,
        delta: DeltaV1,
        tie_band: f64,
        required: &[ToolV1],
    ) -> Result<Self, FrontierTrialError> {
        for tool in required {
            if !measurements.contains_key(tool) {
                return Err(FrontierTrialError::MissingPeerMeasurement {
                    dimension: dimension.token().to_string(),
                    tool: tool.token().to_string(),
                });
            }
        }
        delta.interval.validate()?;
        let direction = direction_for(&delta.interval, tie_band);
        Ok(Self {
            dimension,
            measurements,
            delta,
            tie_band,
            direction,
        })
    }

    /// Re-check every rule against the values actually present. This runs over documents
    /// read from disk, which `try_new` never saw.
    pub fn verify(&self) -> Result<(), FrontierTrialError> {
        self.delta.interval.validate()?;
        if self.direction.is_directional() && self.delta.interval.contains_zero() {
            return Err(
                FrontierTrialError::DirectionalVerdictOnIntervalContainingZero {
                    dimension: self.dimension.token().to_string(),
                    direction: self.direction.token().to_string(),
                    lower: self.delta.interval.lower,
                    upper: self.delta.interval.upper,
                },
            );
        }
        let entailed = direction_for(&self.delta.interval, self.tie_band);
        if entailed != self.direction {
            return Err(FrontierTrialError::DirectionDisagreesWithInterval {
                dimension: self.dimension.token().to_string(),
                declared: self.direction.token().to_string(),
                entailed: entailed.token().to_string(),
                lower: self.delta.interval.lower,
                upper: self.delta.interval.upper,
                tie_band: self.tie_band,
            });
        }
        Ok(())
    }
}

/// One of the fifteen legs, RUN or UNPROVEN, each naming a capture on disk.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LegV1 {
    pub id: String,
    pub tool: ToolV1,
    pub dimension: DimensionV1,
    pub status: LegStatusV1,
    pub evidence: String,
    /// REQUIRED in practice on every UNPROVEN leg — see `ResultSetV1::verify`.
    pub blocker: Option<String>,
}

/// The whole result document, bound to the protocol it was produced under.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResultSetV1 {
    pub protocol_sha256: String,
    pub scope: ScopeV1,
    pub measurements: Vec<MeasurementV1>,
    pub comparatives: Vec<ComparativeResultV1>,
    pub legs: Vec<LegV1>,
}

impl ResultSetV1 {
    /// `protocol` is the raw bytes of the frozen `protocol.json`.
    pub fn verify(&self, protocol: &[u8]) -> Result<(), FrontierTrialError> {
        let actual = protocol_sha256(protocol);
        if actual != self.protocol_sha256 {
            return Err(FrontierTrialError::ProtocolDigestMismatch {
                recorded: self.protocol_sha256.clone(),
                actual,
            });
        }

        if self.legs.len() != EXPECTED_LEGS {
            return Err(FrontierTrialError::LegCountWrong {
                expected: EXPECTED_LEGS,
                found: self.legs.len(),
            });
        }
        let mut seen: BTreeSet<(ToolV1, DimensionV1)> = BTreeSet::new();
        for leg in &self.legs {
            if !seen.insert((leg.tool, leg.dimension)) {
                return Err(FrontierTrialError::DuplicateLeg {
                    tool: leg.tool.token().to_string(),
                    dimension: leg.dimension.token().to_string(),
                    count: 2,
                });
            }
            if leg.evidence.trim().is_empty() {
                return Err(FrontierTrialError::LegWithoutEvidence { id: leg.id.clone() });
            }
            match leg.status {
                LegStatusV1::Unproven => {
                    if leg
                        .blocker
                        .as_deref()
                        .map(str::trim)
                        .unwrap_or("")
                        .is_empty()
                    {
                        return Err(FrontierTrialError::UnprovenLegWithoutBlocker {
                            id: leg.id.clone(),
                        });
                    }
                }
                LegStatusV1::Run => {
                    let measured = self
                        .measurements
                        .iter()
                        .any(|m| m.tool == leg.tool && m.dimension == leg.dimension);
                    if !measured {
                        return Err(FrontierTrialError::RunLegWithoutMeasurement {
                            id: leg.id.clone(),
                            tool: leg.tool.token().to_string(),
                            dimension: leg.dimension.token().to_string(),
                        });
                    }
                }
            }
        }

        for m in &self.measurements {
            m.interval.validate()?;
            if m.scope != self.scope {
                return Err(FrontierTrialError::ScopeMismatch {
                    tool: m.tool.token().to_string(),
                    dimension: m.dimension.token().to_string(),
                    measurement_scope: m.scope.token().to_string(),
                    set_scope: self.scope.token().to_string(),
                });
            }
        }

        for c in &self.comparatives {
            c.verify()?;
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// The verdict rule
// ---------------------------------------------------------------------------

/// Four states. A directional verdict requires the interval to clear the tie band
/// ENTIRELY, which necessarily excludes zero; an interval inside the band is an
/// equivalence claim; everything else is INCONCLUSIVE, which is where a wide,
/// underpowered interval belongs.
pub fn direction_for(delta: &IntervalV1, tie_band: f64) -> DirectionV1 {
    let band = tie_band.abs();
    if delta.lower > band {
        DirectionV1::WaylandAhead
    } else if delta.upper < -band {
        DirectionV1::PeerAhead
    } else if delta.lower >= -band && delta.upper <= band {
        DirectionV1::PracticallyIndistinguishable
    } else {
        DirectionV1::Inconclusive
    }
}

// ---------------------------------------------------------------------------
// Interval estimation — no new dependency
// ---------------------------------------------------------------------------

const Z_95: f64 = 1.959_963_984_540_054;

/// Wilson score interval for a single proportion at 95%.
///
/// Refuses zero trials with a typed error: a zero-trial proportion is the shape a silently
/// skipped leg takes, and reporting it as 0.0 or 1.0 would let a skipped leg read as a
/// measured one. Clopper–Pearson was rejected by the panel — conservative, wider at n=30,
/// and it does not repair an underpowered design.
pub fn wilson_score_interval(
    successes: u32,
    trials: u32,
) -> Result<IntervalV1, FrontierTrialError> {
    if trials == 0 {
        return Err(FrontierTrialError::ZeroTrials {
            kind: "proportion".to_string(),
        });
    }
    if successes > trials {
        return Err(FrontierTrialError::IncoherentProportion { successes, trials });
    }
    let n = f64::from(trials);
    let p = f64::from(successes) / n;
    let z2 = Z_95 * Z_95;
    let denominator = 1.0 + z2 / n;
    let center = (p + z2 / (2.0 * n)) / denominator;
    let half = (Z_95 / denominator) * (p * (1.0 - p) / n + z2 / (4.0 * n * n)).sqrt();
    Ok(IntervalV1 {
        lower: (center - half).max(0.0),
        upper: (center + half).min(1.0),
        method: IntervalMethodV1::WilsonScore95,
        confidence: 0.95,
    })
}

/// Newcombe's Wilson-based interval for a DIFFERENCE of proportions (Wayland minus peer).
///
/// Deliberately NOT the subtraction of two Wilson endpoints and NOT a Wald interval; the
/// panel flagged both as wrong for this use.
pub fn newcombe_wilson_difference(
    successes_a: u32,
    trials_a: u32,
    successes_b: u32,
    trials_b: u32,
) -> Result<IntervalV1, FrontierTrialError> {
    let a = wilson_score_interval(successes_a, trials_a)?;
    let b = wilson_score_interval(successes_b, trials_b)?;
    let pa = f64::from(successes_a) / f64::from(trials_a);
    let pb = f64::from(successes_b) / f64::from(trials_b);
    // Newcombe method 10: combine each proportion's own Wilson bounds.
    let lower = (pa - pb) - ((pa - a.lower).powi(2) + (b.upper - pb).powi(2)).sqrt();
    let upper = (pa - pb) + ((a.upper - pa).powi(2) + (pb - b.lower).powi(2)).sqrt();
    Ok(IntervalV1 {
        lower,
        upper,
        method: IntervalMethodV1::NewcombeWilson95,
        confidence: 0.95,
    })
}

/// Deterministic SplitMix64.
///
/// **Deliberately NOT `rand::rngs::StdRng`, and this is a strengthening deviation from the
/// plan's letter.** The plan says to seed `rand`. `StdRng`'s output stream is explicitly
/// documented as not guaranteed stable across `rand` releases, so seeding it would NOT
/// deliver the reproducibility the frozen protocol promises — a `rand` bump would silently
/// move every published bound. SplitMix64 is six lines, adds no dependency, and its stream
/// is fixed forever by the constants below.
struct SplitMix64(u64);

impl SplitMix64 {
    fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// Modulo bias here is bounded by `len / 2^64` and is deterministic; for the sample
    /// sizes this protocol uses it is far below the resolution of any reported bound.
    fn next_index(&mut self, len: usize) -> usize {
        (self.next_u64() % len as u64) as usize
    }
}

/// 95% percentile bootstrap over the MEAN, seeded from the frozen protocol so a rerun
/// reproduces the bounds exactly.
///
/// Emits `ZeroEmpiricalVariance` when every observation is identical rather than presenting
/// a zero-width interval as precision.
pub fn percentile_bootstrap(
    samples: &[f64],
    resamples: u32,
    seed: u64,
) -> Result<IntervalV1, FrontierTrialError> {
    if samples.is_empty() || resamples == 0 {
        return Err(FrontierTrialError::EmptyBootstrap);
    }
    let first = samples[0];
    if samples.iter().all(|s| *s == first) {
        return Ok(IntervalV1 {
            lower: first,
            upper: first,
            method: IntervalMethodV1::ZeroEmpiricalVariance,
            confidence: 0.95,
        });
    }
    let mut rng = SplitMix64(seed);
    let mut means = Vec::with_capacity(resamples as usize);
    for _ in 0..resamples {
        let mut total = 0.0;
        for _ in 0..samples.len() {
            total += samples[rng.next_index(samples.len())];
        }
        means.push(total / samples.len() as f64);
    }
    means.sort_by(|a, b| a.partial_cmp(b).expect("bootstrap means are finite"));
    let lo = percentile(&means, 0.025);
    let hi = percentile(&means, 0.975);
    Ok(IntervalV1 {
        lower: lo,
        upper: hi,
        method: IntervalMethodV1::PercentileBootstrap95,
        confidence: 0.95,
    })
}

/// Nearest-rank percentile over an already-sorted slice.
fn percentile(sorted: &[f64], q: f64) -> f64 {
    debug_assert!(!sorted.is_empty());
    let rank = (q * sorted.len() as f64).floor() as usize;
    sorted[rank.min(sorted.len() - 1)]
}

/// 95% percentile bootstrap over the DIFFERENCE of two means (Wayland minus peer),
/// seeded from the frozen protocol. The two samples are resampled independently from one
/// deterministic stream, so a rerun reproduces the bound exactly.
pub fn bootstrap_difference(
    a: &[f64],
    b: &[f64],
    resamples: u32,
    seed: u64,
) -> Result<IntervalV1, FrontierTrialError> {
    if a.is_empty() || b.is_empty() || resamples == 0 {
        return Err(FrontierTrialError::EmptyBootstrap);
    }
    let first_a = a[0];
    let first_b = b[0];
    if a.iter().all(|s| *s == first_a) && b.iter().all(|s| *s == first_b) {
        // Both legs are perfectly deterministic. A zero-width interval here is a real
        // statement about the harness, not spurious precision, but it is labelled so a
        // reader cannot mistake it for a tight estimate over noisy data.
        let d = first_a - first_b;
        return Ok(IntervalV1 {
            lower: d,
            upper: d,
            method: IntervalMethodV1::ZeroEmpiricalVariance,
            confidence: 0.95,
        });
    }
    let mut rng = SplitMix64(seed);
    let mut deltas = Vec::with_capacity(resamples as usize);
    for _ in 0..resamples {
        let mut sum_a = 0.0;
        for _ in 0..a.len() {
            sum_a += a[rng.next_index(a.len())];
        }
        let mut sum_b = 0.0;
        for _ in 0..b.len() {
            sum_b += b[rng.next_index(b.len())];
        }
        deltas.push(sum_a / a.len() as f64 - sum_b / b.len() as f64);
    }
    deltas.sort_by(|x, y| x.partial_cmp(y).expect("bootstrap deltas are finite"));
    Ok(IntervalV1 {
        lower: percentile(&deltas, 0.025),
        upper: percentile(&deltas, 0.975),
        method: IntervalMethodV1::PercentileBootstrap95,
        confidence: 0.95,
    })
}

/// The content address a result set is bound to.
pub fn protocol_sha256(protocol: &[u8]) -> String {
    format!("{:x}", Sha256::digest(protocol))
}

/// Digest over raw per-trial samples, so an estimate is traceable to its observations.
pub fn samples_sha256(samples: &[f64]) -> String {
    let mut hasher = Sha256::new();
    for s in samples {
        hasher.update(s.to_bits().to_be_bytes());
    }
    format!("{:x}", hasher.finalize())
}

// ---------------------------------------------------------------------------
// The tool-neutral adapter
// ---------------------------------------------------------------------------

/// Everything that differs between Wayland, Hermes and OpenClaw.
///
/// Any per-tool special-casing BEYOND this value is a confound, and the plan requires it to
/// be recorded in the results if it proves unavoidable.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ToolInvocationV1 {
    pub tool: ToolV1,
    pub program: String,
    pub args: Vec<String>,
    /// The environment variable through which this tool accepts an OpenAI-compatible base
    /// URL. Measured at the pinned commit, never assumed from HEAD.
    pub base_url_env: String,
    /// Appended to the fixture's loopback root before it is handed to the tool.
    ///
    /// This exists because the three tools genuinely disagree about where `/v1` lives:
    /// Wayland appends `/v1/chat/completions` to whatever base URL it is given, while both
    /// peers follow the OpenAI SDK convention in which the base URL already ends in `/v1`.
    /// Carrying that as a VALUE keeps the adapter tool-neutral; hard-coding it per tool in
    /// the driver would be exactly the per-tool special-casing this type exists to prevent,
    /// and it would be invisible to a reader of the results.
    pub base_url_suffix: String,
    /// Additional non-secret environment entries this tool needs to start at all.
    pub extra_env: BTreeMap<String, String>,
    /// Files written into EVERY fresh trial workspace before the tool is launched, keyed by
    /// path relative to that workspace.
    ///
    /// This exists because the three tools need different first-run setup, and that
    /// difference is itself a finding rather than something to hide in the driver.
    /// Measured: `wayland-core` refuses to start on a headless host with
    /// "Session persistence authority unavailable: no OS keyring was usable", and needs a
    /// `[credentials] backend = "encrypted-file"` config plus a vault passphrase before it
    /// will reach a provider at all. Carrying that as DATA keeps it visible in the results;
    /// putting it in the driver would make Wayland's extra setup invisible to a reader.
    #[serde(default)]
    pub workspace_seed_files: BTreeMap<String, String>,
}

/// How a single trial ended. `HarnessIncompatible` is the panel's amendment: a fixture
/// `unexpected_request` means the tool's request pattern outran the FIFO script, which is
/// an observation about the METER, not a task failure by the tool. Without this state, a
/// peer whose natural request order differs is silently scored as failing and the
/// difference reads as a Wayland win.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TrialOutcomeV1 {
    #[serde(rename = "SUCCESS")]
    Success,
    #[serde(rename = "FAILURE")]
    Failure,
    #[serde(rename = "TIMEOUT")]
    Timeout,
    #[serde(rename = "HARNESS_INCOMPATIBLE")]
    HarnessIncompatible,
    /// The tool never reached the meter at all — it failed the unscored conformance gate.
    #[serde(rename = "NO_CONTACT")]
    NoContact,
}

impl TrialOutcomeV1 {
    /// Only SUCCESS and FAILURE enter a scored proportion. A TIMEOUT is a scored FAILURE
    /// per the protocol — excluding hangs flattens flaky tools into looking reliable.
    pub fn enters_proportion(self) -> bool {
        matches!(self, Self::Success | Self::Failure | Self::Timeout)
    }

    pub fn is_success(self) -> bool {
        matches!(self, Self::Success)
    }

    pub fn token(self) -> &'static str {
        match self {
            Self::Success => "SUCCESS",
            Self::Failure => "FAILURE",
            Self::Timeout => "TIMEOUT",
            Self::HarnessIncompatible => "HARNESS_INCOMPATIBLE",
            Self::NoContact => "NO_CONTACT",
        }
    }
}

/// One trial's record, including the meter's own view of it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TrialRecordV1 {
    pub tool: ToolV1,
    pub dimension: DimensionV1,
    pub index: u32,
    pub outcome: TrialOutcomeV1,
    /// Requests the fixture actually received. Measured on the wire, never self-reported.
    pub fixture_requests: u64,
    /// Synthetic token units metered by the fixture.
    pub token_units: u64,
    pub fixture_violations: Vec<String>,
    pub elapsed_ms: u64,
    pub exit_status: Option<i32>,
    /// Dialect provenance, present ONLY on a trial driven from a compiled translation (protocol
    /// v2). Absent on every v1 record ever written.
    ///
    /// This field is what makes a v1 and a v2 record structurally unpoolable. A v1 run spoke one
    /// competitor's dialect for all three harnesses; a v2 run speaks each harness's own. Averaging
    /// the two would reintroduce the confound this whole exercise exists to remove, and an
    /// `Option` that is `None` on exactly the confounded records lets an assembler refuse the mix
    /// mechanically instead of a reader having to remember.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dialect: Option<crate::dialect_exec::TrialDialectV1>,
}

/// Fold a leg's trials into a bounded proportion measurement.
///
/// `HARNESS_INCOMPATIBLE` and `NO_CONTACT` trials are EXCLUDED from the denominator and
/// reported separately — that is the whole point of those states. If nothing is left to
/// score, this refuses rather than reporting a proportion over zero trials.
pub fn proportion_measurement(
    tool: ToolV1,
    dimension: DimensionV1,
    scope: ScopeV1,
    trials: &[TrialRecordV1],
) -> Result<MeasurementV1, FrontierTrialError> {
    assert_homogeneous_dialect(tool, dimension, trials)?;
    let scored: Vec<&TrialRecordV1> = trials
        .iter()
        .filter(|t| t.outcome.enters_proportion())
        .collect();
    let n = u32::try_from(scored.len()).unwrap_or(u32::MAX);
    let successes =
        u32::try_from(scored.iter().filter(|t| t.outcome.is_success()).count()).unwrap_or(u32::MAX);
    let interval = wilson_score_interval(successes, n)?;
    let samples: Vec<f64> = scored
        .iter()
        .map(|t| if t.outcome.is_success() { 1.0 } else { 0.0 })
        .collect();
    Ok(MeasurementV1 {
        tool,
        dimension,
        scope,
        trials: n,
        samples_sha256: samples_sha256(&samples),
        estimate: f64::from(successes) / f64::from(n),
        interval,
    })
}

/// Refuse a leg whose trials were not all driven by the identical dialect.
///
/// Two distinct mixes are refused, and they fail for different reasons:
///
/// - **v1 mixed with v2** — some trials carry no `dialect` (frozen-script, one competitor's
///   dialect for everybody) and some carry one (each harness's own). Folding them averages a
///   confounded arm with an unconfounded one.
/// - **two different translations** — every trial carries a dialect but not the same one. The
///   proportion would then be measuring which translation each trial happened to get.
///
/// An empty slice is not a mix and is left to [`wilson_score_interval`]'s zero-trial refusal, so
/// this guard cannot mask that one.
fn assert_homogeneous_dialect(
    tool: ToolV1,
    dimension: DimensionV1,
    trials: &[TrialRecordV1],
) -> Result<(), FrontierTrialError> {
    if trials.is_empty() {
        return Ok(());
    }
    let dialect_driven = trials.iter().filter(|t| t.dialect.is_some()).count();
    let frozen_script = trials.len() - dialect_driven;
    if dialect_driven > 0 && frozen_script > 0 {
        return Err(FrontierTrialError::MixedDialectProvenance {
            tool: tool.token().to_string(),
            dimension: dimension.token().to_string(),
            dialect_driven,
            frozen_script,
        });
    }
    let distinct: std::collections::BTreeSet<&str> = trials
        .iter()
        .filter_map(|t| t.dialect.as_ref().map(|d| d.translation_sha256.as_str()))
        .collect();
    if distinct.len() > 1 {
        return Err(FrontierTrialError::MixedTranslationDigests {
            tool: tool.token().to_string(),
            dimension: dimension.token().to_string(),
            distinct: distinct.len(),
        });
    }
    Ok(())
}

/// Fold a leg's trials into a bounded continuous measurement (cost).
pub fn continuous_measurement(
    tool: ToolV1,
    dimension: DimensionV1,
    scope: ScopeV1,
    samples: &[f64],
    resamples: u32,
    seed: u64,
) -> Result<MeasurementV1, FrontierTrialError> {
    let interval = percentile_bootstrap(samples, resamples, seed)?;
    let n = u32::try_from(samples.len()).unwrap_or(u32::MAX);
    Ok(MeasurementV1 {
        tool,
        dimension,
        scope,
        trials: n,
        samples_sha256: samples_sha256(samples),
        estimate: samples.iter().sum::<f64>() / samples.len() as f64,
        interval,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_leg_count_is_derived_by_the_compiler_not_by_a_reviewer_counting() {
        assert_eq!(EXPECTED_LEGS, 15);
    }

    #[test]
    fn a_timeout_is_scored_as_a_failure_and_never_discarded() {
        // Excluding hangs flattens flaky tools into looking reliable.
        assert!(TrialOutcomeV1::Timeout.enters_proportion());
        assert!(!TrialOutcomeV1::Timeout.is_success());
        // A harness incompatibility is NOT scored either way.
        assert!(!TrialOutcomeV1::HarnessIncompatible.enters_proportion());
        assert!(!TrialOutcomeV1::NoContact.enters_proportion());
    }

    #[test]
    fn a_leg_with_nothing_left_to_score_refuses_rather_than_reporting_a_proportion() {
        let trials = vec![TrialRecordV1 {
            tool: ToolV1::Openclaw,
            dimension: DimensionV1::Correctness,
            index: 0,
            outcome: TrialOutcomeV1::HarnessIncompatible,
            fixture_requests: 3,
            token_units: 0,
            fixture_violations: vec!["unexpected_request".to_string()],
            elapsed_ms: 10,
            exit_status: Some(1),
            dialect: None,
        }];
        assert!(matches!(
            proportion_measurement(
                ToolV1::Openclaw,
                DimensionV1::Correctness,
                ScopeV1::ScriptedHarness,
                &trials
            ),
            Err(FrontierTrialError::ZeroTrials { .. })
        ));
    }

    // ---- the anti-pooling guard ---------------------------------------------------------

    fn record(index: u32, success: bool, dialect: Option<&str>) -> TrialRecordV1 {
        TrialRecordV1 {
            tool: ToolV1::Wayland,
            dimension: DimensionV1::Correctness,
            index,
            outcome: if success {
                TrialOutcomeV1::Success
            } else {
                TrialOutcomeV1::Failure
            },
            fixture_requests: 2,
            token_units: 20,
            fixture_violations: vec![],
            elapsed_ms: 10,
            exit_status: Some(0),
            dialect: dialect.map(|sha| crate::dialect_exec::TrialDialectV1 {
                vocabulary_version: crate::dialect::VOCABULARY_VERSION.to_string(),
                corpus_sha256: "c".repeat(64),
                translation_sha256: sha.to_string(),
                bound_tool_label: "wayland".to_string(),
                resolved_tool_names: vec!["Write".to_string()],
            }),
        }
    }

    fn fold(trials: &[TrialRecordV1]) -> Result<MeasurementV1, FrontierTrialError> {
        proportion_measurement(
            ToolV1::Wayland,
            DimensionV1::Correctness,
            ScopeV1::ScriptedHarness,
            trials,
        )
    }

    /// KNOWN-POSITIVE. Without this, every assertion below passes on a guard that refuses
    /// everything — the classic self-passing gate.
    #[test]
    fn a_homogeneous_leg_folds_normally_in_both_provenances() {
        let frozen = vec![record(0, true, None), record(1, false, None)];
        let m = fold(&frozen).expect("an all-v1 leg must still fold");
        assert_eq!(m.trials, 2);
        assert!((m.estimate - 0.5).abs() < f64::EPSILON);

        let sha = "a".repeat(64);
        let compiled = vec![
            record(0, true, Some(&sha)),
            record(1, true, Some(&sha)),
        ];
        let m = fold(&compiled).expect("an all-v2 leg must fold");
        assert_eq!(m.trials, 2);
        assert!((m.estimate - 1.0).abs() < f64::EPSILON);
    }

    /// The confound this whole lane exists to remove, attempted through the back door: average a
    /// v1 arm (one competitor's dialect) with a v2 arm (the harness's own).
    #[test]
    fn a_leg_mixing_frozen_script_and_dialect_compiled_trials_is_refused() {
        let sha = "a".repeat(64);
        let mixed = vec![record(0, false, None), record(1, true, Some(&sha))];
        match fold(&mixed) {
            Err(FrontierTrialError::MixedDialectProvenance {
                dialect_driven,
                frozen_script,
                ..
            }) => {
                assert_eq!((dialect_driven, frozen_script), (1, 1));
            }
            other => panic!("expected MixedDialectProvenance, got {other:?}"),
        }
    }

    /// Every trial compiled, but not from the same translation — the proportion would be
    /// measuring which mapping each trial drew.
    #[test]
    fn a_leg_mixing_two_translation_digests_is_refused() {
        let mixed = vec![
            record(0, true, Some(&"a".repeat(64))),
            record(1, false, Some(&"b".repeat(64))),
        ];
        match fold(&mixed) {
            Err(FrontierTrialError::MixedTranslationDigests { distinct, .. }) => {
                assert_eq!(distinct, 2);
            }
            other => panic!("expected MixedTranslationDigests, got {other:?}"),
        }
    }

    /// The guard must not swallow the zero-trial refusal it sits in front of.
    #[test]
    fn the_dialect_guard_does_not_mask_the_zero_trial_refusal() {
        assert!(matches!(fold(&[]), Err(FrontierTrialError::ZeroTrials { .. })));
    }

    /// A v1 record on disk has no `dialect` key at all. It must still deserialize, or every
    /// 30-02 capture becomes unreadable — and it must round-trip WITHOUT gaining the key, or the
    /// published record digests change.
    #[test]
    fn a_v1_record_without_a_dialect_key_still_deserializes_and_round_trips_clean() {
        let v1 = r#"{"tool":"wayland","dimension":"correctness","index":0,"outcome":"FAILURE",
                     "fixture_requests":2,"token_units":20,"fixture_violations":[],
                     "elapsed_ms":502,"exit_status":0}"#;
        let parsed: TrialRecordV1 = serde_json::from_str(v1).expect("v1 records stay readable");
        assert!(parsed.dialect.is_none());
        let out = serde_json::to_string(&parsed).expect("serializes");
        assert!(
            !out.contains("dialect"),
            "a v1 record must not grow a dialect key on re-serialization: {out}"
        );
    }

    #[test]
    fn the_four_verdict_states_partition_the_interval_space() {
        let band = 0.05;
        let mk = |l: f64, u: f64| IntervalV1 {
            lower: l,
            upper: u,
            method: IntervalMethodV1::NewcombeWilson95,
            confidence: 0.95,
        };
        assert_eq!(
            direction_for(&mk(0.10, 0.30), band),
            DirectionV1::WaylandAhead
        );
        assert_eq!(
            direction_for(&mk(-0.30, -0.10), band),
            DirectionV1::PeerAhead
        );
        assert_eq!(
            direction_for(&mk(-0.02, 0.03), band),
            DirectionV1::PracticallyIndistinguishable
        );
        // Contains zero and far too wide for the band: NOT equivalence.
        assert_eq!(
            direction_for(&mk(-0.40, 0.50), band),
            DirectionV1::Inconclusive
        );
        // Excludes zero but does not clear the band: also not a directional claim.
        assert_eq!(
            direction_for(&mk(0.01, 0.30), band),
            DirectionV1::Inconclusive
        );
    }

    #[test]
    fn newcombe_is_not_the_subtraction_of_two_wilson_endpoints() {
        let n = newcombe_wilson_difference(27, 30, 24, 30).expect("interval");
        let a = wilson_score_interval(27, 30).expect("a");
        let b = wilson_score_interval(24, 30).expect("b");
        let naive_lower = a.lower - b.upper;
        assert!(
            (n.lower - naive_lower).abs() > 1e-6,
            "Newcombe must differ from naive endpoint subtraction: {n:?} vs {naive_lower}"
        );
    }
}
