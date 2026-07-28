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
}

/// How many abandoned deliveries compaction keeps.
///
/// Abandonments are rare by construction — one requires either a shutdown that
/// outran its drain budget or a crash between `begin_attempt` and `settle`
/// against a destination that cannot dedupe. A cap this size is years of
/// operational history at any plausible rate, while still keeping the journal
/// genuinely bounded: "retain everything" is not a bound, and a ledger that
/// grows without limit is its own outage.
///
/// Settled records can NEVER evict an abandonment; the two have separate
/// budgets. Anything dropped past this cap is counted and reported through
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
        let destination = match self.states.get(id) {
            None => return Err(LedgerError::Unknown(id.to_string())),
            Some(r) => r.destination.clone(),
        };
        self.append(
            id,
            DeliveryState::Abandoned,
            None,
            destination.as_deref(),
            Some(reason),
        )
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

    /// Rewrite the journal keeping EVERY unsettled delivery, at most
    /// `retain_settled` settled ones, and at most [`ABANDON_RETENTION`]
    /// abandoned ones.
    ///
    /// Three budgets, not two, and that is the point. Dropping an unsettled
    /// record to meet a bound would be a lost delivery, so outstanding work is
    /// never bounded at all. Settled and Abandoned are then bounded
    /// SEPARATELY: sharing one budget let a burst of ordinary settled traffic
    /// evict the record of a message the product had decided not to deliver,
    /// which is the silent loss this surface exists to prevent. Anything
    /// dropped past the abandon cap is counted into
    /// [`Self::dropped_abandonments`] and warned about — never silent.
    ///
    /// Records are rewritten VERBATIM. An earlier version stamped every
    /// surviving record with the compaction's own `now`, so a preserved
    /// abandonment reported the wrong time — the surface would have named the
    /// message and then lied about when it was given up on.
    pub fn compact(&mut self, retain_settled: usize) -> Result<(), LedgerError> {
        let pick = |want: &dyn Fn(DeliveryState) -> bool, states: &BTreeMap<String, Record>| {
            states
                .values()
                .filter(|r| want(r.state))
                .cloned()
                .collect::<Vec<Record>>()
        };

        let unsettled = pick(
            &|s| matches!(s, DeliveryState::Accepted | DeliveryState::Attempted),
            &self.states,
        );
        let settled = pick(&|s| s == DeliveryState::Settled, &self.states);
        let abandoned = pick(&|s| s == DeliveryState::Abandoned, &self.states);

        let settled_from = settled.len().saturating_sub(retain_settled);
        let abandoned_from = abandoned.len().saturating_sub(ABANDON_RETENTION);
        if abandoned_from > 0 {
            self.dropped_abandonments += abandoned_from;
            tracing::warn!(
                dropped = abandoned_from,
                cap = ABANDON_RETENTION,
                total_dropped = self.dropped_abandonments,
                "delivery ledger compaction dropped abandonment records past the \
                 retention cap; those deliveries can no longer be named"
            );
        }

        let keep: Vec<Record> = unsettled
            .into_iter()
            .chain(settled.into_iter().skip(settled_from))
            .chain(abandoned.into_iter().skip(abandoned_from))
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

    fn append(
        &mut self,
        id: &str,
        state: DeliveryState,
        delivered: Option<bool>,
        destination: Option<&str>,
        reason: Option<AbandonReason>,
    ) -> Result<(), LedgerError> {
        let rec = Record {
            id: id.to_string(),
            state,
            at: chrono::Utc::now().to_rfc3339(),
            delivered,
            destination: destination.map(str::to_string),
            reason,
        };
        writeln!(self.journal, "{}", serde_json::to_string(&rec)?)?;
        self.states.insert(id.to_string(), rec);
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
    }

    /// Dropping past the abandon cap is REPORTED, never silent.
    #[test]
    fn abandonments_dropped_past_the_cap_are_counted_and_reported() {
        let dir = tempfile::tempdir().unwrap();
        let mut l = DeliveryLedger::open(dir.path()).unwrap();
        for i in 0..(ABANDON_RETENTION + 5) {
            let id = format!("a-{i:06}");
            l.accept(&id, Some("slack-ops")).unwrap();
            l.abandon(&id, AbandonReason::DrainBudgetExpired).unwrap();
        }
        assert_eq!(
            l.dropped_abandonments(),
            0,
            "nothing dropped before compaction"
        );
        l.compact(8).unwrap();
        assert_eq!(
            l.dropped_abandonments(),
            5,
            "the overflow must be counted so the surface can admit it is incomplete"
        );
        assert_eq!(l.abandoned_count(), ABANDON_RETENTION);
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
