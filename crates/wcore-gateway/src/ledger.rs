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

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Record {
    id: String,
    state: DeliveryState,
    at: String,
    /// Present on a settle: whether the destination actually took it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    delivered: Option<bool>,
}

/// The durable outbound delivery ledger for one gateway home.
#[derive(Debug)]
pub struct DeliveryLedger {
    path: PathBuf,
    /// Last known state per id. `BTreeMap` so compaction and `pending()`
    /// are deterministically ordered — a nondeterministic retry order makes
    /// a duplicate bug irreproducible.
    states: BTreeMap<String, DeliveryState>,
    journal: File,
    /// Count of journal lines that could not be parsed on load. Reported,
    /// never silently discarded.
    quarantined: usize,
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
                        states.insert(r.id, r.state);
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
        })
    }

    /// How many journal records were unparsable on load.
    pub fn quarantined(&self) -> usize {
        self.quarantined
    }

    /// Record a delivery durably before it is attempted.
    pub fn accept(&mut self, id: &str) -> Result<Accept, LedgerError> {
        if self.states.contains_key(id) {
            return Ok(Accept::Duplicate);
        }
        self.append(id, DeliveryState::Accepted, None)?;
        Ok(Accept::Accepted)
    }

    /// Mark an attempt as started. From here the outcome is unknown until
    /// `settle`.
    pub fn begin_attempt(&mut self, id: &str) -> Result<(), LedgerError> {
        if !self.states.contains_key(id) {
            return Err(LedgerError::Unknown(id.to_string()));
        }
        self.append(id, DeliveryState::Attempted, None)
    }

    /// Record the attempt's known outcome. A settled delivery is never
    /// retried, whether or not the destination took it — `delivered=false`
    /// means the destination refused it (an idempotency-key suppression at
    /// the endpoint counts as refused-because-already-served), which is
    /// still a KNOWN outcome.
    pub fn settle(&mut self, id: &str, delivered: bool) -> Result<(), LedgerError> {
        if !self.states.contains_key(id) {
            return Err(LedgerError::Unknown(id.to_string()));
        }
        self.append(id, DeliveryState::Settled, Some(delivered))
    }

    /// Record that a forced drain gave up on this delivery.
    pub fn abandon(&mut self, id: &str) -> Result<(), LedgerError> {
        if !self.states.contains_key(id) {
            return Err(LedgerError::Unknown(id.to_string()));
        }
        self.append(id, DeliveryState::Abandoned, None)
    }

    /// The last known state of `id`.
    pub fn state(&self, id: &str) -> Option<DeliveryState> {
        self.states.get(id).copied()
    }

    /// Deliveries that still need work: accepted-but-unattempted, and
    /// attempted-with-unknown-outcome. Deterministically ordered.
    pub fn pending(&self) -> Vec<String> {
        self.states
            .iter()
            .filter(|(_, s)| matches!(s, DeliveryState::Accepted | DeliveryState::Attempted))
            .map(|(id, _)| id.clone())
            .collect()
    }

    /// How many deliveries are pending. This is the number drain publishes.
    pub fn pending_count(&self) -> usize {
        self.states
            .values()
            .filter(|s| matches!(s, DeliveryState::Accepted | DeliveryState::Attempted))
            .count()
    }

    /// Force everything written so far to a durable point.
    pub fn flush(&mut self) -> Result<(), LedgerError> {
        self.journal.flush()?;
        self.journal.sync_all()?;
        Ok(())
    }

    /// Rewrite the journal keeping EVERY unsettled delivery and at most
    /// `retain_settled` terminal ones.
    ///
    /// The retention applies only to terminal records. Dropping an
    /// unsettled record to meet a bound would be a lost delivery, so the
    /// bound is on history, never on outstanding work.
    pub fn compact(&mut self, retain_settled: usize) -> Result<(), LedgerError> {
        let unsettled: Vec<(String, DeliveryState)> = self
            .states
            .iter()
            .filter(|(_, s)| matches!(s, DeliveryState::Accepted | DeliveryState::Attempted))
            .map(|(k, v)| (k.clone(), *v))
            .collect();
        let terminal: Vec<(String, DeliveryState)> = self
            .states
            .iter()
            .filter(|(_, s)| matches!(s, DeliveryState::Settled | DeliveryState::Abandoned))
            .map(|(k, v)| (k.clone(), *v))
            .collect();
        let keep_from = terminal.len().saturating_sub(retain_settled);

        let now = chrono::Utc::now().to_rfc3339();
        let tmp = self
            .path
            .with_extension(format!("jsonl.{}.tmp", std::process::id()));
        {
            let mut f = File::create(&tmp)?;
            for (id, state) in unsettled.iter().chain(terminal.iter().skip(keep_from)) {
                let rec = Record {
                    id: id.clone(),
                    state: *state,
                    at: now.clone(),
                    delivered: None,
                };
                writeln!(f, "{}", serde_json::to_string(&rec)?)?;
            }
            f.sync_all()?;
        }
        // No handle is held on the destination here: `self.journal` is
        // replaced only after the rename returns.
        std::fs::rename(&tmp, &self.path)?;

        self.states = unsettled
            .into_iter()
            .chain(terminal.into_iter().skip(keep_from))
            .collect();
        self.journal = File::options().create(true).append(true).open(&self.path)?;
        Ok(())
    }

    fn append(
        &mut self,
        id: &str,
        state: DeliveryState,
        delivered: Option<bool>,
    ) -> Result<(), LedgerError> {
        let rec = Record {
            id: id.to_string(),
            state,
            at: chrono::Utc::now().to_rfc3339(),
            delivered,
        };
        writeln!(self.journal, "{}", serde_json::to_string(&rec)?)?;
        self.states.insert(id.to_string(), state);
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
    }

    #[test]
    fn a_torn_tail_is_quarantined_and_reported_not_discarded() {
        let dir = tempfile::tempdir().unwrap();
        {
            let mut l = DeliveryLedger::open(dir.path()).unwrap();
            l.accept("good").unwrap();
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
