//! The durable outbound delivery ledger.
//!
//! Phase 24 plan 24-01, Task 2. This is what makes delivery exactly-once
//! across drain, restart, upgrade and rollback — the whole content of the
//! phase's first Success Criterion.
//!
//! # The four persisted states, and why there are four
//!
//! `Accepted` → `Attempted` → `Settled`, plus `Abandoned`. Three would not
//! be enough. The distinction that matters is between an attempt whose
//! outcome is KNOWN (`Settled`) and one whose outcome is UNKNOWN
//! (`Attempted`, with the process gone before it could settle). Only the
//! unknown case may be retried on restart. A ledger that cannot tell them
//! apart must either retry everything — duplicating every delivery that
//! actually landed — or retry nothing, losing every one that did not.
//!
//! # Where the idempotency key lives, and the compatibility cost
//!
//! The key is the caller-supplied delivery id, and it lives HERE, in the
//! ledger, NOT as a new field on the serialized outbound channel message.
//! `wcore-channels`'s outbound struct rejects unknown fields, so adding one
//! would mean an older reader rejects a message a newer writer produced.
//! Keeping the key alongside in the ledger costs nothing on the wire and
//! leaves that struct's compatibility exactly as it was; the price is that
//! a destination which needs the key transmitted must be handed it
//! explicitly by its adapter rather than finding it in the message body.
//! 24-03 consumes this decision and must not build a second store.
//!
//! # Durability shape
//!
//! An append-only JSONL journal, replayed on open. Compaction rewrites it
//! through a same-directory temporary plus a rename — `std::fs::rename`
//! maps to `MoveFileEx` with `MOVEFILE_REPLACE_EXISTING` on Windows, which
//! replaces an existing destination, and no handle is held on the
//! destination while the rename runs. A truncated or unparsable tail is
//! QUARANTINED and reported rather than silently discarded, because a
//! silently dropped tail is a lost delivery (threat T-24-01-03).

use std::collections::BTreeMap;
use std::fs::File;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::pidlock::normalise_path;

const JOURNAL_FILE: &str = "deliveries.jsonl";

/// The persisted state of one delivery.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeliveryState {
    /// Recorded durably; not yet attempted.
    Accepted,
    /// An attempt started. If the process stops here the outcome is
    /// UNKNOWN and this is the only state a restart may retry.
    Attempted,
    /// The attempt's outcome is known. Never retried.
    Settled,
    /// A forced drain gave up on it. Recorded rather than dropped, so a
    /// restart sees an abandonment instead of inferring a loss from an
    /// absent record.
    Abandoned,
}

/// What an `accept` did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Accept {
    /// A new delivery was recorded.
    Accepted,
    /// The id was already known. No new work was created — this is the
    /// outbound idempotency key doing its job.
    Duplicate,
}

#[derive(Debug, thiserror::Error)]
pub enum LedgerError {
    #[error("delivery ledger i/o failed: {0}")]
    Io(#[from] std::io::Error),

    #[error("delivery ledger record could not be encoded: {0}")]
    Encode(#[from] serde_json::Error),

    #[error("unknown delivery id: {0}")]
    Unknown(String),

    /// Acknowledging or re-sending something that was never abandoned is a
    /// caller error, not a no-op. Both verbs exist to dispose of an
    /// abandonment; applied to a live delivery they would write an
    /// acknowledgement nobody can act on, and silently accepting that would
    /// let a typo look like a completed operator action.
    #[error("delivery {id} is not abandoned (state: {state:?})")]
    NotAbandoned {
        id: String,
        state: Option<DeliveryState>,
    },
}

/// Why a delivery was abandoned.
///
/// Two abandon sites exist and they mean materially different things to an
/// operator, so the journal records which one fired. A single undifferentiated
/// "abandoned" would make the surface answer "what did you give up on?" with a
/// list that cannot distinguish a shutdown that ran out of budget from a
/// delivery whose fate is genuinely unknown.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AbandonReason {
    /// A forced drain hit its budget with this delivery still outstanding.
    /// The delivery was never attempted, or its attempt never settled, and
    /// the process had to exit.
    DrainBudgetExpired,
    /// The attempt's outcome is UNKNOWN — the process died between
    /// `begin_attempt` and `settle` — and the destination cannot recognise a
    /// replay, so re-sending would risk a genuine duplicate. F24-C-H1.
    OutcomeUnknownNoDedup,
}

impl AbandonReason {
    /// One line an operator can act on.
    pub fn describe(self) -> &'static str {
        match self {
            Self::DrainBudgetExpired => {
                "shutdown drain ran out of budget before this delivery finished"
            }
            Self::OutcomeUnknownNoDedup => {
                "outcome unknown after a crash mid-attempt, and this destination \
                 cannot recognise a replay, so it was not re-sent"
            }
        }
    }
}

/// One abandoned delivery, as an operator sees it.
///
/// This is the answer to "what did you give up on?": which message, to which
/// destination, when, and why. The message BODY is deliberately absent — see
/// the `destination` field.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Abandonment {
    /// The delivery id, which names the message: `cron:{job}:{millis}`.
    pub id: String,
    /// The channel the message was bound for, captured at `accept` time.
    ///
    /// `None` only for a record written before this field existed, or by a
    /// caller that accepted without naming a destination.
    ///
    /// The message body is NOT stored. It is recoverable from the cron job the
    /// delivery id names, and copying bodies into a second durable append-only
    /// file would create an independent retention and deletion surface for
    /// personal data that the ledger has no business owning.
    pub destination: Option<String>,
    /// When the abandonment was recorded, RFC 3339. Preserved across
    /// compaction — a rewritten timestamp would make the surface lie about
    /// when the product gave up.
    pub at: String,
    /// Which of the two abandon paths fired. `None` only for a record written
    /// before this field existed.
    pub reason: Option<AbandonReason>,
    /// Whether an attempt had already STARTED when the product gave up.
    ///
    /// This is the single fact that decides whether re-sending is safe. A
    /// delivery abandoned before any attempt certainly never reached its
    /// destination, so re-sending it cannot duplicate. One abandoned mid-attempt
    /// may already have landed, and re-sending it to a destination that cannot
    /// recognise a replay produces the second copy Success Criterion 1 forbids.
    ///
    /// `None` for a record written before this field existed, and treated as
    /// "may have landed" — the cautious reading — because guessing the other way
    /// would silently authorise a duplicate.
    pub was_attempted: Option<bool>,
    /// When an operator acknowledged this abandonment, RFC 3339.
    ///
    /// `None` means nobody has looked at it yet, and an UNACKNOWLEDGED
    /// abandonment is never dropped by compaction. See [`DeliveryLedger::compact`].
    pub acknowledged: Option<String>,
    /// When an operator re-sent this delivery, RFC 3339.
    ///
    /// Recorded ALONGSIDE the abandonment rather than replacing it. The record
    /// must keep saying "the product gave up on this at T1" even after a human
    /// repaired it at T2 — collapsing the two would erase the outage from the
    /// only place it was written down.
    pub resent: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Record {
    id: String,
    state: DeliveryState,
    at: String,
    /// Present on a settle: whether the destination actually took it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    delivered: Option<bool>,
    /// The destination this delivery was bound for, captured at `accept`.
    ///
    /// `#[serde(default)]` is load-bearing for compatibility: `open()`
    /// QUARANTINES any line it cannot parse, so a field without a default
    /// would make an upgrade read every pre-existing record as a torn tail —
    /// converting a version bump into a mass phantom loss.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    destination: Option<String>,
    /// Present on an abandon: which path gave up. Defaulted for the same
    /// compatibility reason as `destination`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    reason: Option<AbandonReason>,
    /// Present on an abandon: whether an attempt had started. Defaulted for the
    /// same compatibility reason as `destination`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    was_attempted: Option<bool>,
    /// Present once an operator has acknowledged an abandonment.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    acknowledged: Option<String>,
    /// Present once an operator has re-sent an abandoned delivery.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    resent: Option<String>,
}

/// How many ACKNOWLEDGED abandonments compaction keeps.
///
/// Abandonments are rare by construction — one requires either a shutdown that
/// outran its drain budget or a crash between `begin_attempt` and `settle`
/// against a destination that cannot dedupe. A cap this size is years of
/// operational history at any plausible rate, while still keeping the journal
/// genuinely bounded.
///
/// # Why this bounds only the acknowledged ones
///
/// The cap originally applied to every abandonment, on the reasoning that
/// "retain everything" is not a bound and an unbounded ledger is its own
/// outage. That reasoning was correct *while an abandonment was permanently
/// terminal*: nothing could ever retire one, so unbounded retention had no
/// exit. [`DeliveryLedger::acknowledge`] is that exit. An unacknowledged
/// abandonment is no longer inert history — it is an unresolved work item, the
/// same class as an unsettled delivery, which this ledger already retains
/// without any cap at all.
///
/// So the budgets now split three ways and the unacknowledged set is exempt.
/// The two failure modes are not symmetric: unbounded growth is visible,
/// recoverable, and one JSONL line per record, whereas dropping an
/// unacknowledged abandonment permanently destroys the only record that a
/// specific message was never delivered — which is precisely what this surface
/// exists to prevent. Neglect shows up instead as a loud, growing
/// [`DeliveryLedger::unacknowledged_abandoned_count`].
///
/// Settled records can NEVER evict an abandonment. Anything dropped past this
/// cap is counted and reported through
/// [`DeliveryLedger::dropped_abandonments`] — a silent drop here would erase
/// exactly the record this surface exists to show.
pub const ABANDON_RETENTION: usize = 10_000;

/// The durable outbound delivery ledger for one gateway home.
#[derive(Debug)]
pub struct DeliveryLedger {
    path: PathBuf,
    /// Last known RECORD per id — not merely the last state.
    ///
    /// Keeping the whole record is what lets compaction preserve `at`,
    /// `destination` and `reason`. An earlier shape kept only the state and
    /// rewrote every surviving record with a fresh `now`, so an abandonment
    /// that survived a compaction reported the compaction's time rather than
    /// the moment the product gave up.
    ///
    /// `BTreeMap` so compaction and `pending()` are deterministically ordered
    /// — a nondeterministic retry order makes a duplicate bug irreproducible.
    states: BTreeMap<String, Record>,
    journal: File,
    /// Count of journal lines that could not be parsed on load. Reported,
    /// never silently discarded.
    quarantined: usize,
    /// Abandonments dropped by compaction because the retention cap was hit.
    /// Cumulative for the life of this handle.
    dropped_abandonments: usize,
}

impl DeliveryLedger {
    /// Journal path for `home`.
    pub fn journal_path(home: impl AsRef<Path>) -> PathBuf {
        normalise_path(home).join(JOURNAL_FILE)
    }

    /// Open (or create) the ledger for `home`, replaying the journal.
    pub fn open(home: impl AsRef<Path>) -> Result<Self, LedgerError> {
        let home = normalise_path(home);
        std::fs::create_dir_all(&home)?;
        let path = home.join(JOURNAL_FILE);

        let mut states = BTreeMap::new();
        let mut quarantined = 0usize;
        if path.exists() {
            let reader = BufReader::new(File::open(&path)?);
            for line in reader.lines() {
                let line = line?;
                if line.trim().is_empty() {
                    continue;
                }
                match serde_json::from_str::<Record>(&line) {
                    Ok(r) => {
                        states.insert(r.id.clone(), r);
                    }
                    // A torn tail from a crash mid-write. Counted and
                    // reported; dropping it silently would turn a partial
                    // write into an invisible lost delivery.
                    Err(_) => quarantined += 1,
                }
            }
        }

        let journal = File::options().create(true).append(true).open(&path)?;

        if quarantined > 0 {
            tracing::warn!(
                quarantined,
                journal = %path.display(),
                "delivery ledger quarantined unparsable journal records on load"
            );
        }

        Ok(Self {
            path,
            states,
            journal,
            quarantined,
            dropped_abandonments: 0,
        })
    }

    /// How many journal records were unparsable on load.
    pub fn quarantined(&self) -> usize {
        self.quarantined
    }

    /// How many abandonments compaction has dropped past [`ABANDON_RETENTION`].
    ///
    /// Non-zero means the operator surface is INCOMPLETE — some abandonment is
    /// no longer nameable. Reported rather than silent, because the whole point
    /// of recording an abandonment is that it can be found later.
    pub fn dropped_abandonments(&self) -> usize {
        self.dropped_abandonments
    }

    /// Record a delivery durably before it is attempted.
    ///
    /// `destination` is captured HERE rather than at abandon time because the
    /// forced-drain abandon path only ever sees delivery ids — it iterates
    /// `pending()` and has no target in scope. A destination recorded at
    /// abandon time would therefore be present for one abandon path and
    /// missing for the other.
    pub fn accept(&mut self, id: &str, destination: Option<&str>) -> Result<Accept, LedgerError> {
        if self.states.contains_key(id) {
            return Ok(Accept::Duplicate);
        }
        self.append(id, DeliveryState::Accepted, None, destination, None)?;
        Ok(Accept::Accepted)
    }

    /// Mark an attempt as started. From here the outcome is unknown until
    /// `settle`.
    pub fn begin_attempt(&mut self, id: &str) -> Result<(), LedgerError> {
        let destination = match self.states.get(id) {
            None => return Err(LedgerError::Unknown(id.to_string())),
            Some(r) => r.destination.clone(),
        };
        self.append(
            id,
            DeliveryState::Attempted,
            None,
            destination.as_deref(),
            None,
        )
    }

    /// Record the attempt's known outcome. A settled delivery is never
    /// retried, whether or not the destination took it — `delivered=false`
    /// means the destination refused it (an idempotency-key suppression at
    /// the endpoint counts as refused-because-already-served), which is
    /// still a KNOWN outcome.
    pub fn settle(&mut self, id: &str, delivered: bool) -> Result<(), LedgerError> {
        let destination = match self.states.get(id) {
            None => return Err(LedgerError::Unknown(id.to_string())),
            Some(r) => r.destination.clone(),
        };
        self.append(
            id,
            DeliveryState::Settled,
            Some(delivered),
            destination.as_deref(),
            None,
        )
    }

    /// Record that the product gave up on this delivery, and WHY.
    ///
    /// The reason is not optional. An abandonment an operator cannot explain
    /// is barely better than one they cannot see: the two abandon paths call
    /// for different responses — a drain-budget abandonment is safe to re-run,
    /// while an unknown-outcome abandonment may already have landed and must be
    /// checked at the destination before anything is re-sent.
    pub fn abandon(&mut self, id: &str, reason: AbandonReason) -> Result<(), LedgerError> {
        let (destination, was_attempted) = match self.states.get(id) {
            None => return Err(LedgerError::Unknown(id.to_string())),
            // Captured HERE because the abandon record replaces the previous
            // one in `states`, so after this append nothing remembers whether an
            // attempt had started — and that is the fact a re-send has to
            // consult. Both abandon paths can fire on either state: the drain
            // path abandons everything still `pending()`, which is a mix of
            // never-attempted and outcome-unknown work, so the reason alone
            // does not answer it.
            Some(r) => (
                r.destination.clone(),
                Some(r.state == DeliveryState::Attempted),
            ),
        };
        self.append_record(Record {
            id: id.to_string(),
            state: DeliveryState::Abandoned,
            at: Self::now(),
            delivered: None,
            destination,
            reason: Some(reason),
            was_attempted,
            acknowledged: None,
            resent: None,
        })
    }

    /// Record that an operator has seen this abandonment and disposed of it.
    ///
    /// Acknowledgement is what makes an abandonment eligible for compaction —
    /// see [`ABANDON_RETENTION`]. It is a human signature, so it is deliberately
    /// NOT written by any automatic path: a surface that empties itself is back
    /// to being no surface at all.
    ///
    /// Idempotent, and the FIRST acknowledgement's timestamp wins. Re-running
    /// the verb must not rewrite when the incident was actually reviewed.
    pub fn acknowledge(&mut self, id: &str) -> Result<(), LedgerError> {
        let mut rec = self.abandoned_record(id)?;
        if rec.acknowledged.is_some() {
            return Ok(());
        }
        rec.acknowledged = Some(Self::now());
        self.append_record(rec)
    }

    /// Record that an operator re-sent this abandoned delivery.
    ///
    /// The state stays `Abandoned` on purpose. The product genuinely did give up
    /// on this delivery, and a later human repair does not unmake that; flipping
    /// the record back to `Settled` would erase the outage from the only place
    /// it is written down, and would also put the id back where a reader looking
    /// for lost messages can no longer find it.
    ///
    /// This does NOT acknowledge. The two record different facts — "the payload
    /// went out again" versus "a human reviewed whether the destination now has
    /// two copies" — and only the second is a reason to stop showing the record.
    pub fn mark_resent(&mut self, id: &str) -> Result<(), LedgerError> {
        let mut rec = self.abandoned_record(id)?;
        rec.resent = Some(Self::now());
        self.append_record(rec)
    }

    /// The current record for `id`, refusing anything that is not abandoned.
    fn abandoned_record(&self, id: &str) -> Result<Record, LedgerError> {
        match self.states.get(id) {
            None => Err(LedgerError::Unknown(id.to_string())),
            Some(r) if r.state != DeliveryState::Abandoned => Err(LedgerError::NotAbandoned {
                id: id.to_string(),
                state: Some(r.state),
            }),
            Some(r) => Ok(r.clone()),
        }
    }

    /// The last known state of `id`.
    pub fn state(&self, id: &str) -> Option<DeliveryState> {
        self.states.get(id).map(|r| r.state)
    }

    /// Every abandoned delivery, oldest id first — the answer to "what did you
    /// give up on?".
    ///
    /// This is the read path behind the operator surface. Before it existed,
    /// `Abandoned` was excluded from `pending()`, excluded from
    /// `pending_count()`, compactable as terminal history and referenced
    /// nowhere outside this module, so a delivery the product decided not to
    /// send left no trace an operator could query — only a `tracing::warn!`
    /// that had to be caught in flight.
    pub fn abandoned(&self) -> Vec<Abandonment> {
        self.states
            .values()
            .filter(|r| r.state == DeliveryState::Abandoned)
            .map(|r| Abandonment {
                id: r.id.clone(),
                destination: r.destination.clone(),
                at: r.at.clone(),
                reason: r.reason,
                was_attempted: r.was_attempted,
                acknowledged: r.acknowledged.clone(),
                resent: r.resent.clone(),
            })
            .collect()
    }

    /// How many deliveries were abandoned and are still recorded.
    pub fn abandoned_count(&self) -> usize {
        self.states
            .values()
            .filter(|r| r.state == DeliveryState::Abandoned)
            .count()
    }

    /// How many abandonments nobody has acknowledged yet.
    ///
    /// This is the number that must stay small, and the one an operator is
    /// answerable for. It is also the price of exempting unacknowledged
    /// abandonments from compaction: the journal's growth is now bounded by
    /// review rather than by a cap, so an unreviewed backlog has to be VISIBLE
    /// rather than quietly truncated. See [`ABANDON_RETENTION`].
    pub fn unacknowledged_abandoned_count(&self) -> usize {
        self.states
            .values()
            .filter(|r| r.state == DeliveryState::Abandoned && r.acknowledged.is_none())
            .count()
    }

    /// Deliveries that still need work: accepted-but-unattempted, and
    /// attempted-with-unknown-outcome. Deterministically ordered.
    ///
    /// `Abandoned` is deliberately excluded and stays excluded: an abandonment
    /// is terminal, and putting it back here would re-dispatch the delivery the
    /// product decided not to send. It is surfaced through [`Self::abandoned`]
    /// instead.
    pub fn pending(&self) -> Vec<String> {
        self.states
            .iter()
            .filter(|(_, r)| matches!(r.state, DeliveryState::Accepted | DeliveryState::Attempted))
            .map(|(id, _)| id.clone())
            .collect()
    }

    /// How many deliveries are pending. This is the number drain publishes.
    pub fn pending_count(&self) -> usize {
        self.states
            .values()
            .filter(|r| matches!(r.state, DeliveryState::Accepted | DeliveryState::Attempted))
            .count()
    }

    /// Force everything written so far to a durable point.
    pub fn flush(&mut self) -> Result<(), LedgerError> {
        self.journal.flush()?;
        self.journal.sync_all()?;
        Ok(())
    }

    /// Rewrite the journal keeping EVERY unsettled delivery, EVERY
    /// unacknowledged abandonment, at most `retain_settled` settled ones, and
    /// at most [`ABANDON_RETENTION`] acknowledged abandonments.
    ///
    /// Four budgets, not three, and that is the point. Dropping an unsettled
    /// record to meet a bound would be a lost delivery, so outstanding work is
    /// never bounded at all — and an UNACKNOWLEDGED abandonment is outstanding
    /// work too, so it is exempt for exactly the same reason. Nobody has yet
    /// looked at it, and compacting it away destroys the only evidence that a
    /// particular message was never sent.
    ///
    /// Settled and acknowledged-Abandoned are then bounded SEPARATELY: sharing
    /// one budget let a burst of ordinary settled traffic evict the record of a
    /// message the product had decided not to deliver, which is the silent loss
    /// this surface exists to prevent. Anything dropped past the abandon cap is
    /// counted into [`Self::dropped_abandonments`] and warned about — never
    /// silent.
    ///
    /// Records are rewritten VERBATIM. An earlier version stamped every
    /// surviving record with the compaction's own `now`, so a preserved
    /// abandonment reported the wrong time — the surface would have named the
    /// message and then lied about when it was given up on.
    pub fn compact(&mut self, retain_settled: usize) -> Result<(), LedgerError> {
        let pick = |want: &dyn Fn(&Record) -> bool, states: &BTreeMap<String, Record>| {
            states
                .values()
                .filter(|r| want(r))
                .cloned()
                .collect::<Vec<Record>>()
        };

        let unsettled = pick(
            &|r| matches!(r.state, DeliveryState::Accepted | DeliveryState::Attempted),
            &self.states,
        );
        let settled = pick(&|r| r.state == DeliveryState::Settled, &self.states);
        // Split by acknowledgement, not merely by state. The unreviewed ones are
        // the reason this surface exists and are never dropped.
        let unacknowledged = pick(
            &|r| r.state == DeliveryState::Abandoned && r.acknowledged.is_none(),
            &self.states,
        );
        let acknowledged = pick(
            &|r| r.state == DeliveryState::Abandoned && r.acknowledged.is_some(),
            &self.states,
        );

        let settled_from = settled.len().saturating_sub(retain_settled);
        let abandoned_from = acknowledged.len().saturating_sub(ABANDON_RETENTION);
        if abandoned_from > 0 {
            self.dropped_abandonments += abandoned_from;
            tracing::warn!(
                dropped = abandoned_from,
                cap = ABANDON_RETENTION,
                total_dropped = self.dropped_abandonments,
                "delivery ledger compaction dropped ACKNOWLEDGED abandonment records \
                 past the retention cap; those deliveries can no longer be named"
            );
        }

        let keep: Vec<Record> = unsettled
            .into_iter()
            .chain(settled.into_iter().skip(settled_from))
            .chain(unacknowledged)
            .chain(acknowledged.into_iter().skip(abandoned_from))
            .collect();

        let tmp = self
            .path
            .with_extension(format!("jsonl.{}.tmp", std::process::id()));
        {
            let mut f = File::create(&tmp)?;
            for rec in &keep {
                writeln!(f, "{}", serde_json::to_string(rec)?)?;
            }
            f.sync_all()?;
        }
        // No handle is held on the destination here: `self.journal` is
        // replaced only after the rename returns.
        std::fs::rename(&tmp, &self.path)?;

        self.states = keep.into_iter().map(|r| (r.id.clone(), r)).collect();
        self.journal = File::options().create(true).append(true).open(&self.path)?;
        Ok(())
    }

    fn now() -> String {
        chrono::Utc::now().to_rfc3339()
    }

    fn append(
        &mut self,
        id: &str,
        state: DeliveryState,
        delivered: Option<bool>,
        destination: Option<&str>,
        reason: Option<AbandonReason>,
    ) -> Result<(), LedgerError> {
        self.append_record(Record {
            id: id.to_string(),
            state,
            at: Self::now(),
            delivered,
            destination: destination.map(str::to_string),
            reason,
            was_attempted: None,
            acknowledged: None,
            resent: None,
        })
    }

    /// The one write path. Takes a whole record so the abandon/acknowledge/
    /// re-send verbs can carry `at` forward VERBATIM rather than restamping it —
    /// an acknowledgement that moved the abandonment's timestamp would make the
    /// surface misreport when the product gave up.
    fn append_record(&mut self, rec: Record) -> Result<(), LedgerError> {
        writeln!(self.journal, "{}", serde_json::to_string(&rec)?)?;
        self.states.insert(rec.id.clone(), rec);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_unknown_id_cannot_be_attempted_or_settled() {
        let dir = tempfile::tempdir().unwrap();
        let mut l = DeliveryLedger::open(dir.path()).unwrap();
        assert!(matches!(
            l.begin_attempt("nope"),
            Err(LedgerError::Unknown(_))
        ));
        assert!(matches!(
            l.settle("nope", true),
            Err(LedgerError::Unknown(_))
        ));
        assert!(matches!(
            l.abandon("nope", AbandonReason::DrainBudgetExpired),
            Err(LedgerError::Unknown(_))
        ));
    }

    /// An abandonment must survive a restart carrying its destination, its
    /// reason and the time it actually happened.
    ///
    /// Reopening is the whole test: an in-memory-only surface is not a surface,
    /// because the process that abandoned the delivery is typically the one
    /// that died.
    #[test]
    fn an_abandonment_is_nameable_after_a_restart() {
        let dir = tempfile::tempdir().unwrap();
        let at;
        {
            let mut l = DeliveryLedger::open(dir.path()).unwrap();
            l.accept("cron:nightly:1700000000000", Some("slack-ops"))
                .unwrap();
            l.begin_attempt("cron:nightly:1700000000000").unwrap();
            l.abandon(
                "cron:nightly:1700000000000",
                AbandonReason::OutcomeUnknownNoDedup,
            )
            .unwrap();
            l.flush().unwrap();
            at = l.abandoned()[0].at.clone();
        }

        let l = DeliveryLedger::open(dir.path()).unwrap();
        let found = l.abandoned();
        assert_eq!(found.len(), 1, "the abandonment must survive the restart");
        assert_eq!(found[0].id, "cron:nightly:1700000000000");
        assert_eq!(
            found[0].destination.as_deref(),
            Some("slack-ops"),
            "an operator must be told WHERE the message was going"
        );
        assert_eq!(
            found[0].reason,
            Some(AbandonReason::OutcomeUnknownNoDedup),
            "an operator must be told WHY it was given up on"
        );
        assert_eq!(found[0].at, at, "the recorded time must survive verbatim");
        // And it must NOT come back as work.
        assert!(l.pending().is_empty());
        assert_eq!(l.pending_count(), 0);
    }

    /// Compaction must not rewrite the moment an abandonment happened.
    ///
    /// This is the regression that motivated storing whole records: the old
    /// implementation stamped every retained record with the compaction's own
    /// `now`, so the surface would name the message and then misreport when it
    /// was abandoned.
    #[test]
    fn compaction_preserves_the_abandonment_timestamp_and_reason() {
        let dir = tempfile::tempdir().unwrap();
        let mut l = DeliveryLedger::open(dir.path()).unwrap();
        l.accept("gone", Some("discord-alerts")).unwrap();
        l.abandon("gone", AbandonReason::DrainBudgetExpired)
            .unwrap();
        let before = l.abandoned()[0].clone();

        // Enough settled traffic that a shared budget would have evicted it.
        for i in 0..500 {
            let id = format!("s-{i}");
            l.accept(&id, Some("slack-ops")).unwrap();
            l.settle(&id, true).unwrap();
        }
        std::thread::sleep(std::time::Duration::from_millis(1100));
        l.compact(8).unwrap();

        let after = DeliveryLedger::open(dir.path()).unwrap().abandoned();
        assert_eq!(
            after.len(),
            1,
            "settled traffic must never evict an abandonment"
        );
        assert_eq!(
            after[0], before,
            "the abandonment record must be rewritten verbatim, not restamped"
        );
    }

    /// Records written before `destination`/`reason` existed must still load.
    ///
    /// `open()` quarantines anything it cannot parse, so a non-defaulted new
    /// field would make an upgrade read every pre-existing record as a torn
    /// tail — turning a version bump into a mass phantom loss. This test is
    /// what stops that: it feeds the OLD on-disk shape to the NEW reader.
    #[test]
    fn a_journal_written_before_the_new_fields_still_loads() {
        let dir = tempfile::tempdir().unwrap();
        let path = DeliveryLedger::journal_path(dir.path());
        std::fs::create_dir_all(dir.path()).unwrap();
        std::fs::write(
            &path,
            // Exactly the pre-change serialization: no destination, no reason.
            "{\"id\":\"old-a\",\"state\":\"accepted\",\"at\":\"2026-01-01T00:00:00Z\"}\n\
             {\"id\":\"old-b\",\"state\":\"abandoned\",\"at\":\"2026-01-02T00:00:00Z\"}\n",
        )
        .unwrap();

        let l = DeliveryLedger::open(dir.path()).unwrap();
        assert_eq!(
            l.quarantined(),
            0,
            "an old record must not be mistaken for a torn tail"
        );
        assert_eq!(l.state("old-a"), Some(DeliveryState::Accepted));
        let ab = l.abandoned();
        assert_eq!(ab.len(), 1);
        assert_eq!(ab[0].id, "old-b");
        assert_eq!(ab[0].at, "2026-01-02T00:00:00Z");
        // Unknown rather than invented.
        assert_eq!(ab[0].destination, None);
        assert_eq!(ab[0].reason, None);
        assert_eq!(ab[0].acknowledged, None);
        assert_eq!(ab[0].resent, None);
        assert_eq!(
            ab[0].was_attempted, None,
            "an old record cannot say whether it was attempted, and must not \
             pretend to — the re-send path reads None as 'may have landed'"
        );
        // An upgraded-into record is therefore UNACKNOWLEDGED, which is the safe
        // direction: it is protected from compaction until a human reviews it.
        assert_eq!(l.unacknowledged_abandoned_count(), 1);
    }

    /// Dropping past the abandon cap is REPORTED, never silent.
    ///
    /// The cap now applies only to ACKNOWLEDGED abandonments, so this
    /// acknowledges every record before compacting. Without the acknowledge
    /// calls the cap is not reached at all — which is the property
    /// `an_unacknowledged_abandonment_is_never_compacted_away` asserts.
    #[test]
    fn acknowledged_abandonments_dropped_past_the_cap_are_counted_and_reported() {
        let dir = tempfile::tempdir().unwrap();
        let mut l = DeliveryLedger::open(dir.path()).unwrap();
        for i in 0..(ABANDON_RETENTION + 5) {
            let id = format!("a-{i:06}");
            l.accept(&id, Some("slack-ops")).unwrap();
            l.abandon(&id, AbandonReason::DrainBudgetExpired).unwrap();
            l.acknowledge(&id).unwrap();
        }
        assert_eq!(
            l.dropped_abandonments(),
            0,
            "nothing dropped before compaction"
        );
        assert_eq!(l.unacknowledged_abandoned_count(), 0);
        l.compact(8).unwrap();
        assert_eq!(
            l.dropped_abandonments(),
            5,
            "the overflow must be counted so the surface can admit it is incomplete"
        );
        assert_eq!(l.abandoned_count(), ABANDON_RETENTION);
    }

    /// An abandonment nobody has looked at is NEVER compacted away.
    ///
    /// This is the whole point of the acknowledge verb. An unreviewed
    /// abandonment is outstanding work, not history, and dropping it destroys
    /// the only record that a specific message was never delivered. The
    /// journal's bound becomes review rather than a cap — so the unreviewed
    /// count has to be visible, which is asserted here too.
    #[test]
    fn an_unacknowledged_abandonment_is_never_compacted_away() {
        let dir = tempfile::tempdir().unwrap();
        let mut l = DeliveryLedger::open(dir.path()).unwrap();
        let over = ABANDON_RETENTION + 5;
        for i in 0..over {
            let id = format!("a-{i:06}");
            l.accept(&id, Some("slack-ops")).unwrap();
            l.abandon(&id, AbandonReason::OutcomeUnknownNoDedup)
                .unwrap();
        }
        l.compact(8).unwrap();

        assert_eq!(
            l.dropped_abandonments(),
            0,
            "an unacknowledged abandonment must never be dropped, cap or not"
        );
        assert_eq!(
            l.abandoned_count(),
            over,
            "every unreviewed abandonment must survive compaction"
        );
        assert_eq!(l.unacknowledged_abandoned_count(), over);

        // And it must survive on DISK, not merely in this handle's map — the
        // process that abandoned the delivery is typically the one that died.
        let reopened = DeliveryLedger::open(dir.path()).unwrap();
        assert_eq!(reopened.abandoned_count(), over);
    }

    /// Acknowledging is what retires an abandonment, and it must not rewrite
    /// when the product actually gave up — nor when the review happened, if the
    /// verb is run twice.
    #[test]
    fn acknowledge_preserves_the_abandon_time_and_the_first_review_time() {
        let dir = tempfile::tempdir().unwrap();
        let mut l = DeliveryLedger::open(dir.path()).unwrap();
        l.accept("gone", Some("discord-alerts")).unwrap();
        l.abandon("gone", AbandonReason::DrainBudgetExpired)
            .unwrap();
        let abandoned_at = l.abandoned()[0].at.clone();

        l.acknowledge("gone").unwrap();
        let first_ack = l.abandoned()[0].acknowledged.clone();
        assert!(first_ack.is_some(), "acknowledgement must be recorded");
        assert_eq!(
            l.abandoned()[0].at,
            abandoned_at,
            "acknowledging must not move the moment the product gave up"
        );
        assert_eq!(l.unacknowledged_abandoned_count(), 0);

        std::thread::sleep(std::time::Duration::from_millis(1100));
        l.acknowledge("gone").unwrap();
        assert_eq!(
            l.abandoned()[0].acknowledged,
            first_ack,
            "re-running the verb must not rewrite when the incident was reviewed"
        );
    }

    /// A re-send is recorded ALONGSIDE the abandonment, never in place of it.
    ///
    /// Flipping the record back to a live state would erase the outage from the
    /// only place it is written down, and would hide the id from an operator
    /// looking for messages the product failed to deliver.
    #[test]
    fn a_resend_is_recorded_without_erasing_the_abandonment() {
        let dir = tempfile::tempdir().unwrap();
        let mut l = DeliveryLedger::open(dir.path()).unwrap();
        l.accept("gone", Some("slack-ops")).unwrap();
        l.abandon("gone", AbandonReason::DrainBudgetExpired)
            .unwrap();
        let at = l.abandoned()[0].at.clone();

        l.mark_resent("gone").unwrap();
        l.flush().unwrap();

        let after = DeliveryLedger::open(dir.path()).unwrap();
        let found = after.abandoned();
        assert_eq!(found.len(), 1, "the abandonment must still be listed");
        assert_eq!(after.state("gone"), Some(DeliveryState::Abandoned));
        assert!(found[0].resent.is_some(), "the re-send must be recorded");
        assert_eq!(found[0].at, at, "the abandon time must survive verbatim");
        assert_eq!(
            found[0].acknowledged, None,
            "a re-send must NOT silently acknowledge — a surface that empties \
             itself is no surface"
        );
        assert_eq!(
            after.unacknowledged_abandoned_count(),
            1,
            "it stays outstanding until a human signs it off"
        );
        // And it must not come back as dispatchable work.
        assert!(after.pending().is_empty());
    }

    /// Both verbs refuse anything that is not abandoned, rather than writing an
    /// acknowledgement nobody can act on.
    #[test]
    fn acknowledge_and_resend_refuse_a_delivery_that_is_not_abandoned() {
        let dir = tempfile::tempdir().unwrap();
        let mut l = DeliveryLedger::open(dir.path()).unwrap();
        l.accept("live", Some("slack-ops")).unwrap();
        l.settle("live", true).unwrap();

        assert!(matches!(
            l.acknowledge("live"),
            Err(LedgerError::NotAbandoned {
                state: Some(DeliveryState::Settled),
                ..
            })
        ));
        assert!(matches!(
            l.mark_resent("live"),
            Err(LedgerError::NotAbandoned { .. })
        ));
        assert!(matches!(
            l.acknowledge("nope"),
            Err(LedgerError::Unknown(_))
        ));
        assert!(matches!(
            l.mark_resent("nope"),
            Err(LedgerError::Unknown(_))
        ));
    }

    /// Whether an attempt had STARTED is the fact that decides if re-sending can
    /// duplicate, and it must be captured at abandon time.
    ///
    /// The abandon record replaces the previous one, so after the append nothing
    /// else remembers it. Both abandon reasons can fire on either state — the
    /// drain path abandons everything still pending, which is a mix — so the
    /// reason alone cannot answer the question.
    #[test]
    fn abandon_records_whether_an_attempt_had_started() {
        let dir = tempfile::tempdir().unwrap();
        let mut l = DeliveryLedger::open(dir.path()).unwrap();

        l.accept("never-tried", Some("slack-ops")).unwrap();
        l.abandon("never-tried", AbandonReason::DrainBudgetExpired)
            .unwrap();

        l.accept("mid-flight", Some("slack-ops")).unwrap();
        l.begin_attempt("mid-flight").unwrap();
        l.abandon("mid-flight", AbandonReason::DrainBudgetExpired)
            .unwrap();
        l.flush().unwrap();

        let found = DeliveryLedger::open(dir.path()).unwrap().abandoned();
        let by = |id: &str| found.iter().find(|a| a.id == id).unwrap().was_attempted;
        assert_eq!(
            by("mid-flight"),
            Some(true),
            "an attempt was in flight: it may already have landed"
        );
        assert_eq!(
            by("never-tried"),
            Some(false),
            "never attempted: re-sending this cannot duplicate"
        );
    }

    #[test]
    fn a_torn_tail_is_quarantined_and_reported_not_discarded() {
        let dir = tempfile::tempdir().unwrap();
        {
            let mut l = DeliveryLedger::open(dir.path()).unwrap();
            l.accept("good", None).unwrap();
            l.flush().unwrap();
        }
        // Simulate a crash mid-write: a partial JSON line at the tail.
        let path = DeliveryLedger::journal_path(dir.path());
        let mut f = std::fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .unwrap();
        write!(f, "{{\"id\":\"torn\",\"sta").unwrap();
        drop(f);

        let l = DeliveryLedger::open(dir.path()).unwrap();
        assert_eq!(l.quarantined(), 1, "the torn record must be counted");
        assert_eq!(l.state("good"), Some(DeliveryState::Accepted));
    }
}
