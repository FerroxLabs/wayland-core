//! Phase 24 Success Criterion 5 — the setup-to-recovery journey receipt.
//!
//! A journey receipt is the machine-checkable record of ONE ordered
//! setup-to-recovery sequence driven against the real shipped binary on ONE
//! platform. It exists because a criterion that reads "setup-to-recovery
//! journeys pass on macOS, Linux, and Windows" cannot be satisfied by a test
//! matrix, and because a prose claim that a journey ran is exactly the shape
//! this program has repeatedly found to be unfalsifiable.
//!
//! The receipt is deliberately NOT routed through
//! [`crate::receipt_policy::verify_authoritative_receipt`]: that verifier means
//! "authoritative signed release evidence" and demands a provider, a model, a
//! fixture digest and a set of required evaluation cells. A journey has none of
//! those and this phase holds no signing key, so widening that surface would
//! blur an authority boundary that exists on purpose. It reuses the DISCIPLINE
//! — bind the run to an exact source identity and to the exact binary bytes —
//! and nothing else.
//!
//! # What the verifier refuses, and why each refusal exists
//!
//! * **A step list that is not exactly the canonical ordered list.** A journey
//!   that drops the hard kill is not a shorter journey, it is a different one.
//! * **A step with an empty command or empty captured output.** An assertion
//!   against empty output is the self-passing shape this repository has caught
//!   more than once; a step that captured nothing did not run.
//! * **An `arrival_source` other than [`ARRIVAL_SOURCE_INDEPENDENT_SINK`].** A
//!   runtime counting deliveries out of its own ledger is grading its own
//!   homework. The count has to come from a process the runtime does not
//!   control and cannot restart.
//! * **A recorded binary digest that differs from the digest the verifier
//!   computes itself.** The verifier hashes the file, so the check does not
//!   depend on the driver that wrote the receipt. This also closes the
//!   "artifact newer than its source" trap: a stale binary has a stale digest.
//! * **Counts that do not reconcile.** `duplicates` and `losses` are DERIVED
//!   from submitted/arrived/unique, so a receipt cannot assert zero of either
//!   while carrying numbers that say otherwise.
//! * **A repeat that is not classified.** See [`DeliveryIdentity`]. A repeated
//!   message body is not automatically a duplicate delivery, but a repeat the
//!   receipt declines to classify is refused rather than waved through.

use std::collections::BTreeSet;
use std::path::Path;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// The one ordered step list. Every platform runs exactly these, in exactly
/// this order; a platform difference appears in HOW a step is invoked, never in
/// WHICH steps exist.
pub const CANONICAL_STEPS: [&str; 17] = [
    "preflight-clean",
    "binary-identity",
    "profile-setup",
    "sink-start",
    "gateway-install",
    "gateway-start",
    "status-running",
    "automation-add",
    "deliveries-submit",
    "arrival-before-kill",
    "hard-kill",
    "platform-recover",
    "delivery-reconcile",
    "upgrade-in-place",
    "rollback",
    "redaction-canary",
    "drain-uninstall-clean",
];

/// The only arrival source a receipt may claim. See the module docs.
pub const ARRIVAL_SOURCE_INDEPENDENT_SINK: &str = "independent-sink";

/// The number of outbound channel adapters the product ships. A coverage claim
/// is meaningless without the population it is drawn from: "3 adapters" is a
/// different statement depending on whether the product has four or forty.
///
/// The population, as `wcore_channels_registry::channel_factory_for` dispatches
/// it: `slack`, `telegram`, `email`, `discord`, `sms`, `whatsapp`, `signal`,
/// `matrix`, `msteams`, and `imessage` (macOS-only).
///
/// Kept as a constant rather than counted from the registry at runtime so that
/// a receipt written today and read next year is interpreted against the
/// population it was actually written about, and so this crate does not take a
/// dependency on the registry merely to divide by ten. `ADAPTER_POPULATION`
/// below is the roster the constant is asserted against.
pub const REGISTERED_ADAPTER_TOTAL: u64 = 10;

/// The named roster behind [`REGISTERED_ADAPTER_TOTAL`]. Exists so the constant
/// is checkable against something rather than being a bare number a reader has
/// to trust.
pub const ADAPTER_POPULATION: [&str; 10] = [
    "slack", "telegram", "email", "discord", "sms", "whatsapp", "signal", "matrix", "msteams",
    "imessage",
];

/// The schema tag a receipt must carry, so a future shape change is a loud
/// parse refusal rather than a silently different meaning.
pub const RECEIPT_SCHEMA: &str = "wayland.journey.receipt/1";

/// The shortest secret the redactor will accept. The existing
/// [`crate::redaction::SecretRedactor::from_secret`] SILENTLY DROPS anything
/// shorter, which is a fail-open: the caller believes it redacted and it did
/// not. The journey entry point refuses instead.
pub const MIN_SECRET_BYTES: usize = 8;

/// The shortest canary the scan will accept. A short needle makes "absent from
/// the document" meaningless — a four-byte sentinel is absent from most
/// documents by luck.
pub const MIN_CANARY_BYTES: usize = 16;

/// One journey step as the driver captured it.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JourneyStep {
    /// Must equal the corresponding entry of [`CANONICAL_STEPS`].
    pub name: String,
    /// The exact command the driver ran. Never synthesised after the fact.
    pub command: String,
    /// The verbatim captured output, pre-publication redaction.
    pub output: String,
    /// The driver's own verdict for the step.
    pub ok: bool,
}

/// The five numbers every recovery claim ends in.
///
/// `arrived` and `unique` are counted at the independent sink. `duplicates` and
/// `losses` are DERIVED here rather than trusted from the receipt, which is why
/// a receipt cannot claim `duplicates: 0` while carrying `arrived > unique`.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct DeliveryCounts {
    /// Deliveries the journey submitted through the shipped binary.
    pub submitted: u64,
    /// Total arrival records at the independent sink.
    pub arrived: u64,
    /// Distinct arrival bodies at the independent sink.
    pub unique: u64,
    /// Claimed duplicates. Must equal `arrived - unique`.
    pub duplicates: u64,
    /// Claimed losses. Must equal `submitted - unique`.
    pub losses: u64,
}

/// What KIND of repeat the run's `duplicates` were.
///
/// # Why a repeat is not automatically a duplicate
///
/// `duplicates` is `arrived - unique`: it counts repeats of a message BODY.
/// Exactly-once is not a property of a body. It is scoped to a **delivery
/// identity** — `cron:{job_id}:{scheduled_for_millis}`
/// (`wcore-cron/src/runner.rs:327`), a *(job, scheduled instant)* pair. See
/// [§4 of `docs/delivery-semantics.md`](../../../docs/delivery-semantics.md).
///
/// The journey submits every job with `--trigger every:15`, and `every:15` is
/// rate-floored to **sixty seconds** by
/// `TriggerBound::new((*every_secs).max(60), 1)` (`wcore-cron/src/trigger.rs:238`,
/// applied to the resulting instant at `:366`). Those are therefore **recurring**
/// jobs, and any run alive past one 60 s period legitimately delivers each body
/// again under a NEW identity. The heartbeat measured that floor directly in the
/// Windows run of 2026-07-30: scheduled deltas of 60068 ms and 64940 ms, three
/// occurrences, and nobody ever called those duplicates.
///
/// So the three outcomes are distinguished and never collapsed:
///
/// | bucket | meaning | verdict |
/// |---|---|---|
/// | `replays` | the SAME delivery identity arrived twice | a real exactly-once violation — **fails** |
/// | `recurrences` | the same body under DIFFERENT identities | the trigger fired again — **passes** |
/// | `indeterminate` | a repeat where at least one arrival carries NO identity | unprovable — **fails** |
///
/// # Why `indeterminate` fails rather than passes
///
/// Only 8 of the 24 delivery arrivals in that run carried an `idempotency_key`
/// at all; `twilio.messages` and `whatsapp.messages` emit none. For those a
/// replay is indistinguishable from a recurrence **in principle**, not merely in
/// this harness. Passing them would publish an unmeasurable property as a
/// measured clean one — which is the exact defect (F24-GWP-M1) that the
/// single-snapshot receipt was built to close, one level down. They are counted
/// AGAINST the run.
///
/// # Why this exists at all
///
/// Before it, both gates refused any `duplicates != 0` outright. On Windows the
/// Task Scheduler minimum repetition interval is `PT1M`, which exceeds the 60 s
/// floor, so a Windows kill-and-recover leg **always** crosses a period boundary
/// while launchd and systemd restart inside it. The Windows journey therefore had
/// **no reachable pass state**: a permanently-red gate, which proves as little as
/// a permanently-green one and additionally hides real progress.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct DeliveryIdentity {
    /// Repeats where one delivery identity arrived more than once.
    pub replays: u64,
    /// Repeats where the body recurred under a distinct delivery identity.
    pub recurrences: u64,
    /// Repeats that cannot be classified because an arrival carried no identity.
    pub indeterminate: u64,
    /// Arrivals — repeated or not — that carried no delivery identity.
    pub unidentified: u64,
    /// The sink-observed endpoints those unidentified arrivals came from, so a
    /// reader can see WHICH adapters cannot be graded rather than only how many.
    #[serde(default)]
    pub unidentified_endpoints: Vec<String>,
}

impl DeliveryIdentity {
    /// The three repeat buckets, which must partition `counts.duplicates`.
    #[must_use]
    pub fn classified(&self) -> u64 {
        self.replays
            .saturating_add(self.recurrences)
            .saturating_add(self.indeterminate)
    }

    /// The verdict token for a receipt whose delivery leg is otherwise sound.
    ///
    /// Only the two PASSING tokens can come out of here — the failing states are
    /// returned as refusals by [`verify_counts`], each carrying its own token in
    /// its message. See [`VERDICT_TOKENS`] for why this is a token and not prose.
    #[must_use]
    pub fn verdict(&self) -> &'static str {
        if self.replays > 0 {
            VERDICT_EXACTLY_ONCE_VIOLATED
        } else if self.indeterminate > 0 {
            VERDICT_NOT_PROVEN
        } else if self.recurrences > 0 {
            VERDICT_RECURRENCE
        } else {
            VERDICT_NO_REPEATS
        }
    }
}

/// # The verdict vocabulary, shared with the JavaScript driver
///
/// Both gates — `verify_counts` here and `classifyVerdict` in
/// `scripts/f24-journey.mjs` — emit exactly one `verdict=<TOKEN>` from this list
/// for any receipt, on their success line or inside their refusal.
///
/// It is a single hyphenated token rather than prose so that "the two gates
/// agree" is a **string equality a test can run**, not a claim a human makes by
/// reading two paragraphs. Two gates that contradict each other on one receipt —
/// one passing, one failing — is a worse state than either being wrong alone,
/// because it makes every downstream grade unreadable. The four-quadrant test in
/// `tests/journey_receipt_contract.rs` extracts this token from both sides over
/// the same receipt bytes and fails on any difference.
pub const VERDICT_TOKENS: [&str; 10] = [
    VERDICT_NO_REPEATS,
    VERDICT_RECURRENCE,
    VERDICT_EXACTLY_ONCE_VIOLATED,
    VERDICT_NOT_PROVEN,
    VERDICT_DELIVERY_LOSS,
    VERDICT_UNCLASSIFIED_REPEATS,
    VERDICT_CLASSIFICATION_UNRECONCILED,
    VERDICT_IDENTITY_INCOHERENT,
    VERDICT_UNIDENTIFIED_EXCEEDS_ARRIVED,
    VERDICT_COUNTS_UNRECONCILED,
];

/// PASS. No body arrived twice, so no delivery identity did either.
pub const VERDICT_NO_REPEATS: &str = "NO-REPEATS";
/// PASS. Every repeat carried a distinct delivery identity — the recurring
/// trigger fired again and the product delivered each occurrence once.
pub const VERDICT_RECURRENCE: &str = "RECURRENCE";
/// FAIL. One delivery identity was delivered more than once.
pub const VERDICT_EXACTLY_ONCE_VIOLATED: &str = "EXACTLY-ONCE-VIOLATED";
/// FAIL. A repeat carried no identity, so exactly-once is unprovable for it.
pub const VERDICT_NOT_PROVEN: &str = "NOT-PROVEN";
/// FAIL. A submitted delivery never arrived.
pub const VERDICT_DELIVERY_LOSS: &str = "DELIVERY-LOSS";
/// FAIL. Repeats present and the receipt declined to classify them.
pub const VERDICT_UNCLASSIFIED_REPEATS: &str = "UNCLASSIFIED-REPEATS";
/// FAIL. The classification buckets do not partition the repeats.
pub const VERDICT_CLASSIFICATION_UNRECONCILED: &str = "CLASSIFICATION-UNRECONCILED";
/// FAIL. Indeterminate repeats claimed with nothing unidentified to cause them.
pub const VERDICT_IDENTITY_INCOHERENT: &str = "IDENTITY-INCOHERENT";
/// FAIL. More unidentified arrivals than arrivals.
pub const VERDICT_UNIDENTIFIED_EXCEEDS_ARRIVED: &str = "UNIDENTIFIED-EXCEEDS-ARRIVED";
/// FAIL. The five headline numbers are internally false.
pub const VERDICT_COUNTS_UNRECONCILED: &str = "COUNTS-UNRECONCILED";

/// What one channel adapter carried, at the independent sink.
///
/// `endpoint` is recorded alongside `adapter` deliberately. The adapter name is
/// what the driver *configured*; the endpoint is what the sink *observed being
/// called*. A driver that names three adapters while all three land on one
/// endpoint has configured three and exercised one, and only the endpoint can
/// tell you that. Recording just the name would reproduce the very blindness
/// this type exists to remove, one level up.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AdapterDelivery {
    /// The platform tag from the channel config — `slack`, `whatsapp`, `sms`.
    pub adapter: String,
    /// The endpoint the independent sink recorded this adapter calling, e.g.
    /// `chat.postMessage`. Observed, never configured.
    pub endpoint: String,
    /// Deliveries submitted through this adapter.
    pub submitted: u64,
    /// Arrival records at the sink attributed to this adapter's endpoint.
    pub arrived: u64,
    /// Distinct arrival bodies at the sink for this adapter's endpoint.
    pub unique: u64,
}

/// Which slice of the adapter population a journey's delivery leg actually
/// exercised.
///
/// # Why this field exists
///
/// Phase 24 Criterion 1 was graded `12 of 12 clean` off a run in which **one**
/// adapter of ten carried every delivery — Slack, the sole adapter implementing
/// the property under test. The journey receipt then inherited that run and
/// added no field that could reveal it: a receipt reading
/// `submitted=12 arrived=12 unique=12 duplicates=0 losses=0` is **byte-identical**
/// whether one adapter ran or ten. Three platform receipts were published in
/// that shape and every one of them was Slack-only.
///
/// A receipt that cannot distinguish 1-of-10 from 10-of-10 is not neutral about
/// coverage — it silently reads as the stronger claim, because a reader has
/// nothing to read it against. So the field is **mandatory**: a journey may
/// honestly exercise one adapter, but it may not decline to say so.
///
/// This deliberately does NOT impose a minimum. A one-adapter journey is a
/// legitimate journey and refusing it would be inventing a stricter criterion
/// than the one written down. Enforcing a *matrix* is a separate, explicit
/// decision the caller makes with `verify --min-adapters N`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AdapterCoverage {
    /// The adapter population this coverage is a fraction OF.
    pub registered_total: u64,
    /// One entry per adapter actually exercised. Never empty.
    pub exercised: Vec<AdapterDelivery>,
}

impl AdapterCoverage {
    /// Distinct adapters exercised.
    #[must_use]
    pub fn adapter_count(&self) -> u64 {
        self.exercised
            .iter()
            .map(|entry| entry.adapter.as_str())
            .collect::<BTreeSet<_>>()
            .len() as u64
    }

    /// `slack,sms,whatsapp` — sorted, so two receipts are comparable by string.
    #[must_use]
    pub fn adapter_list(&self) -> String {
        self.exercised
            .iter()
            .map(|entry| entry.adapter.clone())
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect::<Vec<_>>()
            .join(",")
    }
}

/// The journey receipt.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JourneyReceipt {
    /// [`RECEIPT_SCHEMA`].
    pub schema: String,
    /// `linux` | `macos` | `windows`.
    pub platform: String,
    /// The platform's own service mechanism that performed the recovery.
    pub service_family: String,
    /// The source commit the DRIVEN BINARY was built from, read out of the
    /// binary itself (`wayland-core --build-info`) rather than from whatever
    /// the host's checkout happens to be sitting at. Those two diverge exactly
    /// when a stale binary is being driven, which is the case worth catching.
    pub candidate_commit: String,
    /// The driven binary's own reported version string.
    pub binary_version: String,
    /// sha256 of the driven binary, as the driver saw it.
    pub binary_sha256: String,
    /// The commit of the instrumentation (this driver + this tool). Recorded
    /// separately because the instrumentation and the product candidate are
    /// different things and conflating them is how a journey ends up proving
    /// the harness.
    pub driver_commit: String,
    /// RFC3339 UTC.
    pub started_at: String,
    /// RFC3339 UTC.
    pub finished_at: String,
    /// Must be [`ARRIVAL_SOURCE_INDEPENDENT_SINK`].
    pub arrival_source: String,
    /// The five numbers.
    pub counts: DeliveryCounts,
    /// What kind of repeat `counts.duplicates` were — see [`DeliveryIdentity`].
    ///
    /// Optional in the SHAPE, mandatory in the CASE that matters: a receipt with
    /// `duplicates == 0` has nothing to classify and may omit it, and one with
    /// `duplicates > 0` and no block is refused as
    /// [`JourneyError::UnclassifiedRepeats`]. Absence is therefore never a pass —
    /// it is only permitted where it is vacuous.
    ///
    /// `duplicates == 0` implies `replays == 0` without needing this field at
    /// all: a replayed delivery identity is by construction a second arrival of
    /// the same job text, so it always shows up as a repeated body first.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub delivery_identity: Option<DeliveryIdentity>,
    /// Which adapters carried those numbers. Mandatory — see
    /// [`AdapterCoverage`] for why a receipt is not allowed to be silent here.
    pub adapter_coverage: AdapterCoverage,
    /// The ordered steps.
    pub steps: Vec<JourneyStep>,
}

/// Every way a receipt can be refused. One variant per refusal so a test can
/// name the one it expects rather than matching on a string.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum JourneyError {
    #[error("receipt is empty")]
    EmptyReceipt,
    #[error("receipt is not parsable: {0}")]
    Unparsable(String),
    #[error("receipt schema is {found:?}, expected {expected:?}")]
    WrongSchema { found: String, expected: String },
    #[error("receipt records platform {found:?}, expected {expected:?}")]
    WrongPlatform { found: String, expected: String },
    #[error("receipt records candidate commit {found:?}, expected {expected:?}")]
    WrongCommit { found: String, expected: String },
    #[error("step list mismatch at position {index}: found {found:?}, expected {expected:?}")]
    StepMismatch {
        index: usize,
        found: String,
        expected: String,
    },
    #[error("step list has {found} steps, expected {expected}")]
    StepCount { found: usize, expected: usize },
    #[error("step {name:?} carries an empty command")]
    EmptyCommand { name: String },
    #[error("step {name:?} carries empty captured output")]
    EmptyOutput { name: String },
    #[error("step {name:?} is recorded as failed")]
    StepFailed { name: String },
    #[error("arrival_source is {found:?}; the only accepted value is {expected:?}")]
    WrongArrivalSource { found: String, expected: String },
    #[error(
        "verdict=COUNTS-UNRECONCILED counts do not reconcile: submitted={submitted} \
         arrived={arrived} unique={unique} duplicates={duplicates} \
         (derived {derived_duplicates}) losses={losses} (derived {derived_losses})"
    )]
    CountsUnreconciled {
        submitted: u64,
        arrived: u64,
        unique: u64,
        duplicates: u64,
        derived_duplicates: u64,
        losses: u64,
        derived_losses: u64,
    },
    #[error("journey submitted zero deliveries; a reconciliation over nothing proves nothing")]
    NoDeliveries,
    #[error(
        "verdict=DELIVERY-LOSS {losses} of {submitted} submitted deliveries never arrived at the \
         independent sink ({unique} distinct bodies arrived)"
    )]
    DeliveryLoss {
        losses: u64,
        submitted: u64,
        unique: u64,
    },
    #[error(
        "verdict=UNCLASSIFIED-REPEATS receipt carries {duplicates} repeated deliver{plural} and \
         no delivery_identity block; a repeat the receipt declines to classify is not a clean \
         one. A repeated BODY may be a replay (exactly-once violated) or a recurrence (the \
         trigger fired again under a new delivery identity) and the receipt has to say which"
    )]
    UnclassifiedRepeats {
        duplicates: u64,
        plural: &'static str,
    },
    #[error(
        "verdict=CLASSIFICATION-UNRECONCILED repeat classification does not partition the \
         repeats: duplicates={duplicates} but replays={replays} + recurrences={recurrences} + \
         indeterminate={indeterminate} = {classified}; a classifier whose buckets do not add up \
         cannot support any verdict"
    )]
    RepeatClassificationUnreconciled {
        duplicates: u64,
        replays: u64,
        recurrences: u64,
        indeterminate: u64,
        classified: u64,
    },
    #[error(
        "verdict=IDENTITY-INCOHERENT receipt reports {indeterminate} indeterminate repeat(s) but \
         {unidentified} unidentified arrival(s); a repeat is only indeterminate because an \
         arrival carried no delivery identity, so these cannot both be right"
    )]
    IdentityIncoherent {
        indeterminate: u64,
        unidentified: u64,
    },
    #[error(
        "verdict=UNIDENTIFIED-EXCEEDS-ARRIVED receipt reports {unidentified} unidentified \
         arrival(s) out of {arrived} total; a receipt cannot have seen more unidentified \
         arrivals than arrivals"
    )]
    UnidentifiedExceedsArrived { unidentified: u64, arrived: u64 },
    #[error(
        "verdict=EXACTLY-ONCE-VIOLATED {replays} deliver{plural} arrived under a delivery \
         identity that had already been delivered. This is the real duplicate — the destination \
         received one (job, scheduled instant) pair more than once"
    )]
    ExactlyOnceViolated { replays: u64, plural: &'static str },
    #[error(
        "verdict=NOT-PROVEN {indeterminate} repeat(s) carry no delivery identity to judge by \
         ({unidentified} unidentified arrival(s) at endpoint(s) {endpoints}), so exactly-once can \
         be neither established nor refuted for them. An unmeasurable property is not a clean \
         one; this is an outbound-idempotency gap in those adapters, NOT evidence of a duplicate"
    )]
    UnprovenRepeats {
        indeterminate: u64,
        unidentified: u64,
        endpoints: String,
    },
    #[error(
        "receipt names zero exercised adapters; a delivery tally that cannot say which adapter \
         carried it reads as the whole population"
    )]
    EmptyAdapterCoverage,
    #[error(
        "adapter_coverage.registered_total is {found}, expected {expected}; a coverage fraction \
         against the wrong population overstates or understates itself"
    )]
    WrongAdapterPopulation { found: u64, expected: u64 },
    #[error(
        "adapter {adapter:?} claims {submitted} submitted; an adapter that carried nothing is not \
         coverage and listing it inflates the fraction"
    )]
    IdleAdapterClaimed { adapter: String, submitted: u64 },
    #[error(
        "adapter {adapter:?} is listed twice; one entry per adapter, or the sums are ambiguous"
    )]
    DuplicateAdapter { adapter: String },
    #[error(
        "adapter {adapter:?} carries an empty observed endpoint; the endpoint is what distinguishes \
         a configured adapter from an exercised one"
    )]
    EmptyAdapterEndpoint { adapter: String },
    #[error(
        "{count} adapters share the observed endpoint {endpoint:?}; adapters that all land on one \
         endpoint were configured separately and exercised as one"
    )]
    SharedAdapterEndpoint { endpoint: String, count: usize },
    #[error(
        "per-adapter {field} sums to {sum} but the receipt's top-level count is {total}; the \
         breakdown and the headline are describing different runs"
    )]
    AdapterCountsUnreconciled {
        field: &'static str,
        sum: u64,
        total: u64,
    },
    #[error(
        "receipt exercises {found} adapter(s) of {population}; caller required at least {required}"
    )]
    InsufficientAdapterCoverage {
        found: u64,
        required: u64,
        population: u64,
    },
    #[error("binary digest mismatch: receipt records {recorded}, verifier computed {computed}")]
    DigestMismatch { recorded: String, computed: String },
    // NOT named `source`: thiserror treats a field of that name as the error
    // source and demands it implement `std::error::Error`, which `String` does
    // not. The io error is carried as text so this enum can stay `PartialEq`
    // and a test can name the variant it expects.
    #[error("binary is missing or unreadable at {path}: {reason}")]
    BinaryUnreadable { path: String, reason: String },
}

/// sha256 a file and return the lowercase hex digest.
pub fn sha256_file(path: &Path) -> Result<String, JourneyError> {
    let bytes = std::fs::read(path).map_err(|error| JourneyError::BinaryUnreadable {
        path: path.display().to_string(),
        reason: error.to_string(),
    })?;
    let mut hasher = Sha256::new();
    hasher.update(&bytes);
    Ok(hex(&hasher.finalize()))
}

fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    bytes.iter().fold(String::new(), |mut acc, byte| {
        let _ = write!(acc, "{byte:02x}");
        acc
    })
}

/// Parse a receipt, refusing an empty or unparsable one by name.
pub fn parse_receipt(raw: &str) -> Result<JourneyReceipt, JourneyError> {
    if raw.trim().is_empty() {
        return Err(JourneyError::EmptyReceipt);
    }
    let receipt: JourneyReceipt =
        serde_json::from_str(raw).map_err(|error| JourneyError::Unparsable(error.to_string()))?;
    if receipt.schema != RECEIPT_SCHEMA {
        return Err(JourneyError::WrongSchema {
            found: receipt.schema,
            expected: RECEIPT_SCHEMA.to_string(),
        });
    }
    Ok(receipt)
}

/// The whole receipt check, minus the binary digest (which needs the file).
///
/// Split out so the crate tests can exercise every structural refusal without
/// staging a binary on disk for each one.
pub fn verify_structure(
    receipt: &JourneyReceipt,
    expect_platform: &str,
    expect_commit: &str,
) -> Result<(), JourneyError> {
    if receipt.platform != expect_platform {
        return Err(JourneyError::WrongPlatform {
            found: receipt.platform.clone(),
            expected: expect_platform.to_string(),
        });
    }
    if !receipt.candidate_commit.eq_ignore_ascii_case(expect_commit) {
        return Err(JourneyError::WrongCommit {
            found: receipt.candidate_commit.clone(),
            expected: expect_commit.to_string(),
        });
    }
    if receipt.steps.len() != CANONICAL_STEPS.len() {
        return Err(JourneyError::StepCount {
            found: receipt.steps.len(),
            expected: CANONICAL_STEPS.len(),
        });
    }
    for (index, (step, expected)) in receipt.steps.iter().zip(CANONICAL_STEPS).enumerate() {
        if step.name != expected {
            return Err(JourneyError::StepMismatch {
                index,
                found: step.name.clone(),
                expected: expected.to_string(),
            });
        }
        if step.command.trim().is_empty() {
            return Err(JourneyError::EmptyCommand {
                name: step.name.clone(),
            });
        }
        if step.output.trim().is_empty() {
            return Err(JourneyError::EmptyOutput {
                name: step.name.clone(),
            });
        }
        if !step.ok {
            return Err(JourneyError::StepFailed {
                name: step.name.clone(),
            });
        }
    }
    if receipt.arrival_source != ARRIVAL_SOURCE_INDEPENDENT_SINK {
        return Err(JourneyError::WrongArrivalSource {
            found: receipt.arrival_source.clone(),
            expected: ARRIVAL_SOURCE_INDEPENDENT_SINK.to_string(),
        });
    }
    verify_counts(&receipt.counts, receipt.delivery_identity.as_ref())?;
    verify_adapter_coverage(&receipt.adapter_coverage, &receipt.counts)
}

/// The adapter-coverage checks, separable so each refusal is directly testable.
///
/// This is the half of the receipt that Phase 24 was missing. Everything here
/// exists to stop a single-adapter run from reading as a matrix — see
/// [`AdapterCoverage`].
pub fn verify_adapter_coverage(
    coverage: &AdapterCoverage,
    counts: &DeliveryCounts,
) -> Result<(), JourneyError> {
    if coverage.exercised.is_empty() {
        return Err(JourneyError::EmptyAdapterCoverage);
    }
    if coverage.registered_total != REGISTERED_ADAPTER_TOTAL {
        return Err(JourneyError::WrongAdapterPopulation {
            found: coverage.registered_total,
            expected: REGISTERED_ADAPTER_TOTAL,
        });
    }

    let mut seen_adapters = BTreeSet::new();
    // Endpoint -> how many distinct adapters claimed it. Three adapters that all
    // land on `chat.postMessage` are one adapter wearing three names, which is
    // the exact failure this whole type exists to make visible.
    let mut endpoint_users: std::collections::BTreeMap<&str, usize> =
        std::collections::BTreeMap::new();

    for entry in &coverage.exercised {
        if !seen_adapters.insert(entry.adapter.as_str()) {
            return Err(JourneyError::DuplicateAdapter {
                adapter: entry.adapter.clone(),
            });
        }
        if entry.endpoint.trim().is_empty() {
            return Err(JourneyError::EmptyAdapterEndpoint {
                adapter: entry.adapter.clone(),
            });
        }
        if entry.submitted == 0 {
            return Err(JourneyError::IdleAdapterClaimed {
                adapter: entry.adapter.clone(),
                submitted: entry.submitted,
            });
        }
        *endpoint_users.entry(entry.endpoint.as_str()).or_insert(0) += 1;
    }

    if let Some((endpoint, count)) = endpoint_users.iter().find(|(_, count)| **count > 1) {
        return Err(JourneyError::SharedAdapterEndpoint {
            endpoint: (*endpoint).to_string(),
            count: *count,
        });
    }

    // The breakdown must add up to the headline. Without this, a receipt could
    // carry an honest-looking three-adapter list beside a twelve-delivery
    // headline that only one of them produced.
    for (field, sum, total) in [
        (
            "submitted",
            coverage.exercised.iter().map(|e| e.submitted).sum::<u64>(),
            counts.submitted,
        ),
        (
            "arrived",
            coverage.exercised.iter().map(|e| e.arrived).sum::<u64>(),
            counts.arrived,
        ),
        (
            "unique",
            coverage.exercised.iter().map(|e| e.unique).sum::<u64>(),
            counts.unique,
        ),
    ] {
        if sum != total {
            return Err(JourneyError::AdapterCountsUnreconciled { field, sum, total });
        }
    }
    Ok(())
}

/// The count reconciliation, on its own so its refusals are directly testable.
///
/// # The predicate, stated once
///
/// A journey's delivery leg is **clean** iff all of:
///
/// 1. the arithmetic reconciles (`duplicates == arrived - unique`,
///    `losses == submitted - unique`, `unique` within both bounds);
/// 2. `losses == 0`;
/// 3. every repeat is classified — `duplicates > 0` requires an identity block,
///    and its three buckets must sum to exactly `duplicates`;
/// 4. `replays == 0`;
/// 5. `indeterminate == 0`.
///
/// `recurrences` is unconstrained. A run in which the trigger fired again under
/// a new delivery identity is a run in which the product behaved correctly, and
/// it is a STRONGER exactly-once measurement than a short run, not a weaker one:
/// 24 arrivals under 24 distinct identities, each delivered once, is 24
/// observations of the property where a 12-arrival run is 12.
///
/// This function is the Rust half of a pair. `classifyVerdict` in
/// `scripts/f24-journey.mjs` is the JavaScript half and implements the same five
/// clauses; `tests/journey_receipt_contract.rs` drives both over the same
/// receipt bytes in all four quadrants and fails if they ever disagree. Changing
/// one without the other produces two gates that contradict each other on one
/// receipt, which is worse than either being wrong alone.
pub fn verify_counts(
    counts: &DeliveryCounts,
    identity: Option<&DeliveryIdentity>,
) -> Result<(), JourneyError> {
    if counts.submitted == 0 {
        return Err(JourneyError::NoDeliveries);
    }
    let derived_duplicates = counts.arrived.saturating_sub(counts.unique);
    let derived_losses = counts.submitted.saturating_sub(counts.unique);
    // `unique` can never exceed either bound; treating that as "reconciled"
    // would let a receipt claim more distinct arrivals than it ever submitted.
    let coherent = counts.unique <= counts.submitted
        && counts.unique <= counts.arrived
        && counts.duplicates == derived_duplicates
        && counts.losses == derived_losses;
    if !coherent {
        return Err(JourneyError::CountsUnreconciled {
            submitted: counts.submitted,
            arrived: counts.arrived,
            unique: counts.unique,
            duplicates: counts.duplicates,
            derived_duplicates,
            losses: counts.losses,
            derived_losses,
        });
    }
    // Clause 2. A loss is unconditional: nothing about identity can make a
    // delivery that never arrived acceptable.
    if counts.losses != 0 {
        return Err(JourneyError::DeliveryLoss {
            losses: counts.losses,
            submitted: counts.submitted,
            unique: counts.unique,
        });
    }
    // Clause 3a. Absence of the block is only permitted where it is vacuous.
    let Some(identity) = identity else {
        if counts.duplicates != 0 {
            return Err(JourneyError::UnclassifiedRepeats {
                duplicates: counts.duplicates,
                plural: plural(counts.duplicates, "y", "ies"),
            });
        }
        return Ok(());
    };
    // Clause 3b. Checked even when `duplicates == 0`, so a receipt cannot carry
    // a block asserting replays beside a headline that admits no repeats.
    let classified = identity.classified();
    if classified != counts.duplicates {
        return Err(JourneyError::RepeatClassificationUnreconciled {
            duplicates: counts.duplicates,
            replays: identity.replays,
            recurrences: identity.recurrences,
            indeterminate: identity.indeterminate,
            classified,
        });
    }
    if identity.unidentified > counts.arrived {
        return Err(JourneyError::UnidentifiedExceedsArrived {
            unidentified: identity.unidentified,
            arrived: counts.arrived,
        });
    }
    if identity.indeterminate > 0 && identity.unidentified == 0 {
        return Err(JourneyError::IdentityIncoherent {
            indeterminate: identity.indeterminate,
            unidentified: identity.unidentified,
        });
    }
    // Clause 4, then clause 5. Ordered so a run that is BOTH violated and
    // unproven reports the violation, which is the more serious of the two.
    if identity.replays != 0 {
        return Err(JourneyError::ExactlyOnceViolated {
            replays: identity.replays,
            plural: plural(identity.replays, "y", "ies"),
        });
    }
    if identity.indeterminate != 0 {
        return Err(JourneyError::UnprovenRepeats {
            indeterminate: identity.indeterminate,
            unidentified: identity.unidentified,
            endpoints: if identity.unidentified_endpoints.is_empty() {
                "(none recorded)".to_string()
            } else {
                identity.unidentified_endpoints.join(",")
            },
        });
    }
    Ok(())
}

/// Singular/plural suffix. Exists so a refusal message reads as English rather
/// than `1 deliverys`, which invites a reader to distrust the number beside it.
fn plural(count: u64, one: &'static str, many: &'static str) -> &'static str {
    if count == 1 { one } else { many }
}

/// The full verification, including hashing the binary the verifier was given.
///
/// The verifier hashes the file ITSELF. Trusting a digest handed to it by the
/// same driver that wrote the receipt would make the check a restatement.
/// `min_adapters` is the caller's explicit matrix demand. `None` verifies a
/// journey on its own terms — a one-adapter journey is legitimate and passes,
/// but the success line then SAYS `adapters=1/10`, so nobody can read it as ten.
///
/// The coverage is printed, not merely validated. A field that a receipt carries
/// and no consumer surfaces is an advertised-but-dead surface, and the whole
/// reason this defect survived three platform receipts is that nothing ever
/// printed which adapter carried them.
pub fn verify_receipt(
    raw: &str,
    binary: &Path,
    expect_platform: &str,
    expect_commit: &str,
    min_adapters: Option<u64>,
) -> Result<String, JourneyError> {
    let receipt = parse_receipt(raw)?;
    verify_structure(&receipt, expect_platform, expect_commit)?;
    let exercised = receipt.adapter_coverage.adapter_count();
    if let Some(required) = min_adapters
        && exercised < required
    {
        return Err(JourneyError::InsufficientAdapterCoverage {
            found: exercised,
            required,
            population: receipt.adapter_coverage.registered_total,
        });
    }
    let computed = sha256_file(binary)?;
    if !computed.eq_ignore_ascii_case(&receipt.binary_sha256) {
        return Err(JourneyError::DigestMismatch {
            recorded: receipt.binary_sha256.clone(),
            computed,
        });
    }
    // The repeat classification is PRINTED, not merely validated. A receipt can
    // now pass while carrying `duplicates=12`, so a success line that reported
    // only the headline would read as "twelve duplicates were fine" to anyone who
    // did not know the rule. Same reasoning as the adapter list beside it: a
    // field a verifier accepts and no consumer surfaces is how the last defect
    // survived three published receipts.
    let repeats = match receipt.delivery_identity.as_ref() {
        Some(identity) => format!(
            " repeats={} (replays={} recurrences={} indeterminate={} unidentified={})",
            identity.classified(),
            identity.replays,
            identity.recurrences,
            identity.indeterminate,
            identity.unidentified,
        ),
        None => " repeats=0 (no delivery_identity block; permitted only at duplicates=0)".into(),
    };
    Ok(format!(
        "JOURNEY VERIFIED platform={} commit={} steps={} submitted={} arrived={} unique={} \
         duplicates={} losses={} adapters={}/{} exercised={} verdict={}{}",
        receipt.platform,
        receipt.candidate_commit,
        receipt.steps.len(),
        receipt.counts.submitted,
        receipt.counts.arrived,
        receipt.counts.unique,
        receipt.counts.duplicates,
        receipt.counts.losses,
        exercised,
        receipt.adapter_coverage.registered_total,
        receipt.adapter_coverage.adapter_list(),
        receipt
            .delivery_identity
            .as_ref()
            .map_or(VERDICT_NO_REPEATS, DeliveryIdentity::verdict),
        repeats,
    ))
}

/// Every way a set of receipts fails to describe ONE candidate.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum BindError {
    #[error("bind needs at least two receipts, got {0}")]
    TooFew(usize),
    #[error("receipts disagree on the candidate commit: {0:?}")]
    CommitDisagreement(Vec<String>),
    #[error("receipts disagree on the driver commit: {0:?}")]
    DriverDisagreement(Vec<String>),
    #[error("two receipts claim the same platform {0:?}")]
    DuplicatePlatform(String),
    #[error(
        "receipts disagree on which adapters were exercised: {0:?}; a bound set is one candidate \
         measured over one delivery surface, not three platforms each measuring a different one"
    )]
    AdapterDisagreement(Vec<String>),
}

/// The executable form of the three-platform binding discipline: the platforms
/// must be proving ONE candidate, not three different builds that each happened
/// to pass.
pub fn bind_receipts(receipts: &[JourneyReceipt]) -> Result<String, BindError> {
    if receipts.len() < 2 {
        return Err(BindError::TooFew(receipts.len()));
    }
    let commits: BTreeSet<String> = receipts
        .iter()
        .map(|r| r.candidate_commit.to_ascii_lowercase())
        .collect();
    if commits.len() != 1 {
        return Err(BindError::CommitDisagreement(
            commits.into_iter().collect::<Vec<_>>(),
        ));
    }
    let drivers: BTreeSet<String> = receipts
        .iter()
        .map(|r| r.driver_commit.to_ascii_lowercase())
        .collect();
    if drivers.len() != 1 {
        return Err(BindError::DriverDisagreement(
            drivers.into_iter().collect::<Vec<_>>(),
        ));
    }
    // Three receipts that all say `linux` are one platform measured three
    // times, and would satisfy a naive same-commit check.
    let mut seen = BTreeSet::new();
    for receipt in receipts {
        if !seen.insert(receipt.platform.clone()) {
            return Err(BindError::DuplicatePlatform(receipt.platform.clone()));
        }
    }
    // Three platforms that each drove a DIFFERENT adapter are three
    // one-adapter journeys, and binding them would let the union read as a
    // matrix that no single platform ever ran.
    let adapter_sets: BTreeSet<String> = receipts
        .iter()
        .map(|r| r.adapter_coverage.adapter_list())
        .collect();
    if adapter_sets.len() != 1 {
        return Err(BindError::AdapterDisagreement(
            adapter_sets.into_iter().collect::<Vec<_>>(),
        ));
    }

    let commit = commits.into_iter().next().unwrap_or_default();
    let driver = drivers.into_iter().next().unwrap_or_default();
    let adapters = adapter_sets.into_iter().next().unwrap_or_default();
    let population = receipts
        .first()
        .map(|r| r.adapter_coverage.registered_total)
        .unwrap_or_default();
    let exercised = receipts
        .first()
        .map(|r| r.adapter_coverage.adapter_count())
        .unwrap_or_default();
    Ok(format!(
        "BOUND commit={} driver={} receipts={} platforms={} adapters={}/{} exercised={}",
        commit,
        driver,
        receipts.len(),
        seen.into_iter().collect::<Vec<_>>().join(","),
        exercised,
        population,
        adapters,
    ))
}

/// Every way the canary scan is refused.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum ScanError {
    #[error("canary file is empty")]
    EmptyCanaryFile,
    #[error(
        "canary {0:?} is shorter than {MIN_CANARY_BYTES} bytes; a short needle makes absence meaningless"
    )]
    CanaryTooShort(String),
    #[error("no raw captures were supplied")]
    NoRawCaptures,
    #[error("raw capture {0} is empty")]
    EmptyRawCapture(String),
    #[error("document is empty")]
    EmptyDocument,
    #[error(
        "canary {0:?} is absent from EVERY raw capture — it never travelled a capture path, so its absence from the document proves nothing"
    )]
    ControlAbsent(String),
    #[error("canary {0:?} is PRESENT in the published document")]
    LeakedToDocument(String),
}

/// One canary's verdict.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CanaryVerdict {
    pub canary: String,
    pub control_present: bool,
    pub published_absent: bool,
}

/// The positive-controlled redaction scan.
///
/// A canary that was never planted is trivially absent, so the absence check
/// alone certifies nothing. Both halves must hold: PRESENT in a real
/// pre-redaction capture, ABSENT from what will be committed.
pub fn scan_canaries(
    document: &str,
    canaries: &[String],
    raw_captures: &[(String, String)],
) -> Result<Vec<CanaryVerdict>, ScanError> {
    if canaries.is_empty() {
        return Err(ScanError::EmptyCanaryFile);
    }
    for canary in canaries {
        if canary.len() < MIN_CANARY_BYTES {
            return Err(ScanError::CanaryTooShort(canary.clone()));
        }
    }
    if raw_captures.is_empty() {
        return Err(ScanError::NoRawCaptures);
    }
    for (name, contents) in raw_captures {
        if contents.trim().is_empty() {
            return Err(ScanError::EmptyRawCapture(name.clone()));
        }
    }
    if document.trim().is_empty() {
        return Err(ScanError::EmptyDocument);
    }
    let mut verdicts = Vec::new();
    for canary in canaries {
        let control_present = raw_captures
            .iter()
            .any(|(_, contents)| contents.contains(canary.as_str()));
        if !control_present {
            return Err(ScanError::ControlAbsent(canary.clone()));
        }
        if document.contains(canary.as_str()) {
            return Err(ScanError::LeakedToDocument(canary.clone()));
        }
        verdicts.push(CanaryVerdict {
            canary: canary.clone(),
            control_present,
            published_absent: true,
        });
    }
    Ok(verdicts)
}

/// Parse a newline-delimited canary file, dropping blank lines only.
pub fn parse_canary_file(raw: &str) -> Vec<String> {
    raw.lines()
        .map(|line| line.trim_end_matches(['\r', '\n']).trim().to_string())
        .filter(|line| !line.is_empty())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn steps() -> Vec<JourneyStep> {
        CANONICAL_STEPS
            .iter()
            .map(|name| JourneyStep {
                name: (*name).to_string(),
                command: format!("run {name}"),
                output: format!("output of {name}"),
                ok: true,
            })
            .collect()
    }

    fn receipt() -> JourneyReceipt {
        JourneyReceipt {
            schema: RECEIPT_SCHEMA.to_string(),
            platform: "linux".into(),
            service_family: "systemd".into(),
            candidate_commit: "a".repeat(40),
            binary_version: "wayland-core 0.12.25".into(),
            binary_sha256: "b".repeat(64),
            driver_commit: "c".repeat(40),
            started_at: "2026-07-28T00:00:00Z".into(),
            finished_at: "2026-07-28T00:05:00Z".into(),
            arrival_source: ARRIVAL_SOURCE_INDEPENDENT_SINK.into(),
            counts: DeliveryCounts {
                submitted: 12,
                arrived: 12,
                unique: 12,
                duplicates: 0,
                losses: 0,
            },
            // A clean run has nothing to classify, and the receipt is allowed to
            // be silent EXACTLY there. `duplicates == 0` implies `replays == 0`
            // by construction, so the silence carries no claim.
            delivery_identity: None,
            // Deliberately the historically-published shape: all 12 deliveries
            // carried by Slack alone, at Slack's endpoint. This fixture is the
            // one the old receipt could not distinguish from a ten-adapter run,
            // and keeping it that way means the refusal tests below are graded
            // against the real defect rather than a tidied-up version of it.
            adapter_coverage: AdapterCoverage {
                registered_total: REGISTERED_ADAPTER_TOTAL,
                exercised: vec![AdapterDelivery {
                    adapter: "slack".into(),
                    endpoint: "chat.postMessage".into(),
                    submitted: 12,
                    arrived: 12,
                    unique: 12,
                }],
            },
            steps: steps(),
        }
    }

    #[test]
    fn a_well_formed_receipt_passes_the_structural_check() {
        // The positive control. Without it every refusal test below could be
        // passing because the fixture is broken rather than because the
        // refusal fires.
        assert_eq!(
            verify_structure(&receipt(), "linux", &"a".repeat(40)),
            Ok(())
        );
    }

    #[test]
    fn a_missing_step_is_refused() {
        let mut r = receipt();
        r.steps.remove(10);
        assert!(matches!(
            verify_structure(&r, "linux", &"a".repeat(40)),
            Err(JourneyError::StepCount { .. })
        ));
    }

    #[test]
    fn a_reordered_step_list_is_refused() {
        let mut r = receipt();
        r.steps.swap(10, 11);
        assert!(matches!(
            verify_structure(&r, "linux", &"a".repeat(40)),
            Err(JourneyError::StepMismatch { index: 10, .. })
        ));
    }

    #[test]
    fn an_empty_captured_output_is_refused() {
        let mut r = receipt();
        r.steps[12].output = "   \n".into();
        assert!(matches!(
            verify_structure(&r, "linux", &"a".repeat(40)),
            Err(JourneyError::EmptyOutput { .. })
        ));
    }

    #[test]
    fn a_runtime_sourced_arrival_count_is_refused() {
        let mut r = receipt();
        r.arrival_source = "gateway-ledger".into();
        assert!(matches!(
            verify_structure(&r, "linux", &"a".repeat(40)),
            Err(JourneyError::WrongArrivalSource { .. })
        ));
    }

    #[test]
    fn unreconciled_counts_are_refused_even_when_the_receipt_claims_zero() {
        let mut r = receipt();
        r.counts.arrived = 14; // two duplicates, still claiming zero
        assert!(matches!(
            verify_structure(&r, "linux", &"a".repeat(40)),
            Err(JourneyError::CountsUnreconciled { .. })
        ));
    }

    #[test]
    fn a_journey_that_submitted_nothing_is_refused() {
        let mut r = receipt();
        r.counts = DeliveryCounts {
            submitted: 0,
            arrived: 0,
            unique: 0,
            duplicates: 0,
            losses: 0,
        };
        assert_eq!(
            verify_structure(&r, "linux", &"a".repeat(40)),
            Err(JourneyError::NoDeliveries)
        );
    }

    // ── the repeat classification ───────────────────────────────────────────
    //
    // Before these, `verify_counts` refused any `duplicates != 0`. On Windows
    // the Task Scheduler `PT1M` minimum repetition exceeds the 60 s rate floor
    // that `every:15` is clamped to, so a Windows kill-and-recover leg ALWAYS
    // crosses a period boundary and the gate had no reachable pass state.
    //
    // Each of the four quadrants below is one of the two directions
    // LANE-BRIEF §3b-iii demands, and the pair of `..._is_refused` /
    // `..._passes` tests is what proves the gate can do both.

    /// A run that recurred: same bodies, all under fresh delivery identities.
    fn recurred() -> JourneyReceipt {
        let mut r = receipt();
        r.counts = DeliveryCounts {
            submitted: 12,
            arrived: 24,
            unique: 12,
            duplicates: 12,
            losses: 0,
        };
        r.delivery_identity = Some(DeliveryIdentity {
            replays: 0,
            recurrences: 12,
            indeterminate: 0,
            unidentified: 0,
            unidentified_endpoints: vec![],
        });
        r.adapter_coverage.exercised[0].arrived = 24;
        r
    }

    #[test]
    fn quadrant_1_a_run_of_proven_recurrences_passes() {
        // THE POINT OF THE WHOLE CHANGE. Twelve repeated bodies, every one under
        // a distinct delivery identity, zero replays, zero unprovable repeats.
        // The trigger fired again because it is a recurring trigger; the product
        // delivered each occurrence exactly once.
        assert_eq!(
            verify_structure(&recurred(), "linux", &"a".repeat(40)),
            Ok(())
        );
    }

    #[test]
    fn quadrant_2_a_planted_replay_is_refused() {
        // One of the twelve is moved from the recurrence bucket to the replay
        // bucket and NOTHING else changes — same headline, same partition sum.
        // So this test cannot pass for any reason other than the replay.
        let mut r = recurred();
        let id = r.delivery_identity.as_mut().expect("fixture has identity");
        id.replays = 1;
        id.recurrences = 11;
        assert_eq!(
            verify_structure(&r, "linux", &"a".repeat(40)),
            Err(JourneyError::ExactlyOnceViolated {
                replays: 1,
                plural: "y"
            })
        );
    }

    #[test]
    fn quadrant_3_an_indeterminate_repeat_is_refused() {
        // A repeat that cannot be judged is not a clean one. Same discipline as
        // quadrant 2: one bucket moved, the sum untouched.
        let mut r = recurred();
        let id = r.delivery_identity.as_mut().expect("fixture has identity");
        id.recurrences = 4;
        id.indeterminate = 8;
        id.unidentified = 16;
        id.unidentified_endpoints = vec!["twilio.messages".into(), "whatsapp.messages".into()];
        assert_eq!(
            verify_structure(&r, "linux", &"a".repeat(40)),
            Err(JourneyError::UnprovenRepeats {
                indeterminate: 8,
                unidentified: 16,
                endpoints: "twilio.messages,whatsapp.messages".into(),
            })
        );
    }

    #[test]
    fn quadrant_4_a_clean_run_with_no_repeats_still_passes() {
        // The gate must not have become a gate that only knows how to pass runs
        // WITH repeats. The unmodified fixture is `duplicates: 0` and no
        // identity block, i.e. every receipt published before this change.
        assert_eq!(
            verify_structure(&receipt(), "linux", &"a".repeat(40)),
            Ok(())
        );
    }

    #[test]
    fn a_replay_outranks_an_indeterminate_in_the_refusal() {
        // A run that is both violated and unproven must report the violation:
        // "we found a duplicate" is strictly more serious than "we could not
        // tell", and a reader who sees only NOT PROVEN would under-read it.
        let mut r = recurred();
        let id = r.delivery_identity.as_mut().expect("fixture has identity");
        id.replays = 2;
        id.recurrences = 4;
        id.indeterminate = 6;
        id.unidentified = 6;
        assert!(matches!(
            verify_structure(&r, "linux", &"a".repeat(40)),
            Err(JourneyError::ExactlyOnceViolated { replays: 2, .. })
        ));
    }

    #[test]
    fn repeats_with_no_classification_block_are_refused_not_waved_through() {
        // The rule that stops the new pass state from being a hole: a receipt
        // may only be silent about repeats when it has none. Silence beside
        // `duplicates: 12` is the M1 defect wearing a new coat.
        let mut r = recurred();
        r.delivery_identity = None;
        assert_eq!(
            verify_structure(&r, "linux", &"a".repeat(40)),
            Err(JourneyError::UnclassifiedRepeats {
                duplicates: 12,
                plural: "ies"
            })
        );
    }

    #[test]
    fn a_classification_that_does_not_partition_the_repeats_is_refused() {
        // Without this, a driver could pass any run by writing
        // `recurrences: <duplicates>` — or, as here, by simply under-reporting.
        let mut r = recurred();
        r.delivery_identity
            .as_mut()
            .expect("fixture has identity")
            .recurrences = 11;
        assert_eq!(
            verify_structure(&r, "linux", &"a".repeat(40)),
            Err(JourneyError::RepeatClassificationUnreconciled {
                duplicates: 12,
                replays: 0,
                recurrences: 11,
                indeterminate: 0,
                classified: 11,
            })
        );
    }

    #[test]
    fn a_block_claiming_replays_beside_a_clean_headline_is_refused() {
        // The partition check runs even at `duplicates == 0`, so the two halves
        // of the receipt cannot describe different runs in either direction.
        let mut r = receipt();
        r.delivery_identity = Some(DeliveryIdentity {
            replays: 3,
            ..DeliveryIdentity::default()
        });
        assert!(matches!(
            verify_structure(&r, "linux", &"a".repeat(40)),
            Err(JourneyError::RepeatClassificationUnreconciled { classified: 3, .. })
        ));
    }

    #[test]
    fn an_indeterminate_repeat_with_nothing_unidentified_is_incoherent() {
        // A repeat is only indeterminate BECAUSE an arrival carried no identity.
        // A receipt asserting the first while denying the second is describing
        // an impossible run, and the likeliest cause is a hand-edited receipt.
        let mut r = recurred();
        let id = r.delivery_identity.as_mut().expect("fixture has identity");
        id.recurrences = 11;
        id.indeterminate = 1;
        id.unidentified = 0;
        assert_eq!(
            verify_structure(&r, "linux", &"a".repeat(40)),
            Err(JourneyError::IdentityIncoherent {
                indeterminate: 1,
                unidentified: 0
            })
        );
    }

    #[test]
    fn a_loss_is_refused_whatever_the_classification_says() {
        // Losses were folded into the old blanket refusal. Splitting the repeat
        // half out must not have taken the loss half with it.
        let mut r = recurred();
        r.counts.unique = 11;
        r.counts.losses = 1;
        r.counts.duplicates = 13;
        r.delivery_identity
            .as_mut()
            .expect("fixture has identity")
            .recurrences = 13;
        r.adapter_coverage.exercised[0].unique = 11;
        assert_eq!(
            verify_structure(&r, "linux", &"a".repeat(40)),
            Err(JourneyError::DeliveryLoss {
                losses: 1,
                submitted: 12,
                unique: 11
            })
        );
    }

    #[test]
    fn the_arithmetic_check_still_runs_ahead_of_the_classification() {
        // The classification must not have become a way to bypass the derived
        // counts: a receipt whose headline is internally false is refused before
        // anyone looks at what kind of repeat it claims to have had.
        let mut r = recurred();
        r.counts.duplicates = 11; // arrived - unique is 12
        assert!(matches!(
            verify_structure(&r, "linux", &"a".repeat(40)),
            Err(JourneyError::CountsUnreconciled { .. })
        ));
    }

    #[test]
    fn the_verdict_names_all_four_states() {
        let id = |replays, recurrences, indeterminate| DeliveryIdentity {
            replays,
            recurrences,
            indeterminate,
            unidentified: 0,
            unidentified_endpoints: vec![],
        };
        assert_eq!(id(0, 0, 0).verdict(), VERDICT_NO_REPEATS);
        assert_eq!(id(0, 3, 0).verdict(), VERDICT_RECURRENCE);
        assert_eq!(id(0, 0, 3).verdict(), VERDICT_NOT_PROVEN);
        assert_eq!(id(1, 0, 3).verdict(), VERDICT_EXACTLY_ONCE_VIOLATED);
    }

    #[test]
    fn a_passing_receipt_with_repeats_says_so_on_the_success_line() {
        // A pass at `duplicates=12` that printed only the headline would read as
        // "twelve duplicates were acceptable". The verdict and the buckets are
        // part of the success line for the same reason the adapter list is.
        let r = recurred();
        let line = format!(
            "duplicates={} verdict={} repeats={}",
            r.counts.duplicates,
            r.delivery_identity
                .as_ref()
                .map_or(VERDICT_NO_REPEATS, DeliveryIdentity::verdict),
            r.delivery_identity
                .as_ref()
                .map_or(0, DeliveryIdentity::classified),
        );
        assert_eq!(line, "duplicates=12 verdict=RECURRENCE repeats=12");
    }

    #[test]
    fn every_delivery_leg_refusal_carries_exactly_one_known_verdict_token() {
        // The cross-gate agreement test compares a `verdict=<TOKEN>` extracted
        // from each side. That comparison is only meaningful if every refusal
        // emits exactly one token from the shared vocabulary — a refusal with no
        // token would make the extraction silently fall through to whatever
        // matched next, which is the self-passing shape in a regex's clothing.
        let mut clean = recurred();
        clean.counts.duplicates = 11; // COUNTS-UNRECONCILED

        let mut loss = recurred();
        loss.counts.unique = 11;
        loss.counts.duplicates = 13;
        loss.counts.losses = 1;

        let mut unclassified = recurred();
        unclassified.delivery_identity = None;

        let mut unpartitioned = recurred();
        unpartitioned
            .delivery_identity
            .as_mut()
            .expect("identity")
            .recurrences = 11;

        let mut incoherent = recurred();
        {
            let id = incoherent.delivery_identity.as_mut().expect("identity");
            id.recurrences = 11;
            id.indeterminate = 1;
        }

        let mut over = recurred();
        over.delivery_identity.as_mut().expect("identity").unidentified = 999;

        let mut replayed = recurred();
        {
            let id = replayed.delivery_identity.as_mut().expect("identity");
            id.replays = 1;
            id.recurrences = 11;
        }

        let mut unproven = recurred();
        {
            let id = unproven.delivery_identity.as_mut().expect("identity");
            id.recurrences = 4;
            id.indeterminate = 8;
            id.unidentified = 16;
        }

        for (label, receipt, expected) in [
            ("arithmetic", clean, VERDICT_COUNTS_UNRECONCILED),
            ("loss", loss, VERDICT_DELIVERY_LOSS),
            ("unclassified", unclassified, VERDICT_UNCLASSIFIED_REPEATS),
            (
                "partition",
                unpartitioned,
                VERDICT_CLASSIFICATION_UNRECONCILED,
            ),
            ("incoherent", incoherent, VERDICT_IDENTITY_INCOHERENT),
            ("over", over, VERDICT_UNIDENTIFIED_EXCEEDS_ARRIVED),
            ("replay", replayed, VERDICT_EXACTLY_ONCE_VIOLATED),
            ("unproven", unproven, VERDICT_NOT_PROVEN),
        ] {
            let Err(error) = verify_counts(&receipt.counts, receipt.delivery_identity.as_ref())
            else {
                panic!("{label} must be refused, and was not");
            };
            let text = error.to_string();
            let found: Vec<&str> = VERDICT_TOKENS
                .iter()
                .copied()
                .filter(|token| text.contains(&format!("verdict={token}")))
                .collect();
            // Exactly one, not at-least-one: `NOT-PROVEN` is not a substring of
            // any other token today, but a future token that contained another
            // would make the extraction ambiguous without this.
            assert_eq!(found, vec![expected], "{label}: {text}");
        }
    }

    #[test]
    fn a_wrong_platform_is_refused() {
        assert!(matches!(
            verify_structure(&receipt(), "macos", &"a".repeat(40)),
            Err(JourneyError::WrongPlatform { .. })
        ));
    }

    #[test]
    fn a_wrong_commit_is_refused() {
        assert!(matches!(
            verify_structure(&receipt(), "linux", &"d".repeat(40)),
            Err(JourneyError::WrongCommit { .. })
        ));
    }

    #[test]
    fn bind_refuses_receipts_from_different_candidates() {
        let mut second = receipt();
        second.platform = "macos".into();
        second.candidate_commit = "e".repeat(40);
        assert!(matches!(
            bind_receipts(&[receipt(), second]),
            Err(BindError::CommitDisagreement(_))
        ));
    }

    #[test]
    fn bind_refuses_the_same_platform_twice() {
        assert!(matches!(
            bind_receipts(&[receipt(), receipt()]),
            Err(BindError::DuplicatePlatform(_))
        ));
    }

    #[test]
    fn bind_accepts_three_platforms_at_one_candidate() {
        let mut mac = receipt();
        mac.platform = "macos".into();
        let mut win = receipt();
        win.platform = "windows".into();
        let bound = bind_receipts(&[receipt(), mac, win]).expect("one candidate");
        assert!(bound.starts_with("BOUND commit="), "{bound}");
        assert!(bound.contains("receipts=3"), "{bound}");
    }

    #[test]
    fn a_canary_absent_from_every_capture_fails_the_positive_control() {
        let canary = "WLJ-CANARY-0123456789abcdef".to_string();
        let err = scan_canaries(
            "a published document",
            std::slice::from_ref(&canary),
            &[("raw".into(), "nothing relevant here".into())],
        )
        .unwrap_err();
        assert_eq!(err, ScanError::ControlAbsent(canary));
    }

    #[test]
    fn a_canary_present_in_the_document_fails() {
        let canary = "WLJ-CANARY-0123456789abcdef".to_string();
        let err = scan_canaries(
            &format!("leaked {canary} here"),
            std::slice::from_ref(&canary),
            &[("raw".into(), format!("planted {canary}"))],
        )
        .unwrap_err();
        assert_eq!(err, ScanError::LeakedToDocument(canary));
    }

    #[test]
    fn a_short_canary_is_refused() {
        let canary = "fifteen-bytes!!".to_string();
        assert_eq!(canary.len(), 15);
        assert_eq!(
            scan_canaries(
                "doc",
                std::slice::from_ref(&canary),
                &[("raw".into(), canary.clone())]
            ),
            Err(ScanError::CanaryTooShort(canary))
        );
    }

    #[test]
    fn a_clean_scan_reports_both_halves() {
        let canary = "WLJ-CANARY-0123456789abcdef".to_string();
        let verdicts = scan_canaries(
            "a published document with nothing sensitive",
            std::slice::from_ref(&canary),
            &[("raw".into(), format!("planted {canary}"))],
        )
        .expect("clean");
        assert_eq!(
            verdicts,
            vec![CanaryVerdict {
                canary,
                control_present: true,
                published_absent: true,
            }]
        );
    }

    #[test]
    fn an_empty_receipt_is_refused_by_name() {
        assert_eq!(
            parse_receipt("   ").unwrap_err(),
            JourneyError::EmptyReceipt
        );
    }

    #[test]
    fn a_receipt_with_the_wrong_schema_tag_is_refused() {
        let mut value = serde_json::to_value(receipt()).expect("serialise");
        value["schema"] = serde_json::Value::String("something.else/9".into());
        let raw = serde_json::to_string(&value).expect("serialise");
        assert!(matches!(
            parse_receipt(&raw),
            Err(JourneyError::WrongSchema { .. })
        ));
    }
}
