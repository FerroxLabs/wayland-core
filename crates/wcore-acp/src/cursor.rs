//! Ordered, gap-aware event cursor — what a disconnected typed client resumes
//! from without loss or duplication.
//!
//! Phase 24 Success Criterion 4, threat T-24-03-05.
//!
//! # The failure this is built to make impossible
//!
//! A client subscribes, receives events, and the connection drops. It
//! reconnects and says "I last saw position 5." The naive server answers with
//! everything after 5. That is correct only if the server's position 5 is the
//! SAME position 5 the client saw — and after a restart it is not, because a
//! fresh in-memory log numbers from 1 again.
//!
//! The client then receives positions 6..n of a DIFFERENT stream, believes it
//! is continuous, and has silently missed events 1..5 of the new one. Nothing
//! errors. Nothing is duplicated. The counts look perfect from inside the
//! server, which is the property that makes it dangerous: it is a delivery
//! system attesting to its own completeness, the exact shape lane 24c measured
//! at the independent sink where a `carried=1 (unknown-outcome 1)` ledger sat
//! next to a destination holding two copies.
//!
//! So a position alone is not a resumable cursor. [`Cursor`] carries the
//! STREAM IDENTITY as well, and [`EventLog::since`] refuses a cursor minted
//! against a different stream by name.
//!
//! # Three refusals, all explicit, none silent
//!
//! | client presents | server answers |
//! |---|---|
//! | a position older than retained history | [`CursorError::TooOld`] naming the oldest servable position |
//! | a position ahead of anything emitted | [`CursorError::Ahead`] naming the next position |
//! | a cursor from another stream | [`CursorError::StreamMismatch`] |
//!
//! `Ahead` matters as much as `TooOld`. Answering an impossible position with
//! an empty list tells a client it is caught up when the server has in fact
//! lost the events it is waiting for, and the client then blocks forever on a
//! stream that will never explain itself.

use std::collections::VecDeque;

use serde::{Deserialize, Serialize};

/// Default number of events retained for resumption.
///
/// Finite by construction: retained history is memory a disconnected client
/// makes the server hold, so an unbounded log is a client-controlled
/// allocation (T-24-03-05).
pub const DEFAULT_RETENTION: usize = 1024;

/// A resumable position in one event stream.
///
/// Both fields are required. A bare `u64` is NOT a cursor — see the module
/// docs on why a position without a stream identity resumes silently into the
/// wrong stream.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Cursor {
    /// Identity of the stream this position was minted against.
    pub stream_id: String,
    /// The last position the holder has seen. Positions start at 1, so 0 is
    /// the well-formed "I have seen nothing" cursor.
    pub position: u64,
}

/// Why a cursor could not be served.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "error", rename_all = "snake_case")]
#[non_exhaustive]
pub enum CursorError {
    /// The requested position has been evicted from retained history. Carries
    /// the oldest position still servable so the client resynchronises
    /// DELIBERATELY instead of silently skipping.
    TooOld {
        requested: u64,
        oldest_available: u64,
    },
    /// The requested position is beyond anything this stream has emitted. The
    /// client's cursor cannot have come from this stream's history.
    Ahead { requested: u64, next: u64 },
    /// The cursor was minted against a different stream — typically the one
    /// this process had before it restarted.
    StreamMismatch { requested: String, actual: String },
}

impl std::fmt::Display for CursorError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CursorError::TooOld {
                requested,
                oldest_available,
            } => write!(
                f,
                "cursor position {requested} is no longer retained; oldest available is \
                 {oldest_available}"
            ),
            CursorError::Ahead { requested, next } => write!(
                f,
                "cursor position {requested} is ahead of this stream, which has emitted \
                 {} events",
                next.saturating_sub(1)
            ),
            CursorError::StreamMismatch { requested, actual } => write!(
                f,
                "cursor belongs to stream {requested}, this is stream {actual}"
            ),
        }
    }
}

impl std::error::Error for CursorError {}

/// One retained event and the position it was assigned.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Positioned<E> {
    pub position: u64,
    pub event: E,
}

/// An ordered, bounded, resumable event log.
#[derive(Debug, Clone)]
pub struct EventLog<E> {
    stream_id: String,
    retained: VecDeque<Positioned<E>>,
    /// Position the NEXT appended event will take. Starts at 1.
    next: u64,
    capacity: usize,
}

impl<E: Clone> EventLog<E> {
    /// Build a log with a fresh stream identity.
    ///
    /// The identity is what makes a cursor from a previous process
    /// recognisably foreign. Callers that persist a log across restarts must
    /// persist the identity WITH it — see [`Self::resume_stream`].
    pub fn new(stream_id: impl Into<String>) -> Self {
        Self::with_capacity(stream_id, DEFAULT_RETENTION)
    }

    pub fn with_capacity(stream_id: impl Into<String>, capacity: usize) -> Self {
        Self {
            stream_id: stream_id.into(),
            retained: VecDeque::new(),
            next: 1,
            capacity: capacity.max(1),
        }
    }

    /// Rebuild a log that continues an existing stream: same identity, and
    /// positions continue from `next_position` rather than restarting at 1.
    ///
    /// Both halves are required together. Restoring the identity while
    /// restarting the numbering is worse than a fresh stream, because it makes
    /// a stale cursor look VALID.
    pub fn resume_stream(
        stream_id: impl Into<String>,
        next_position: u64,
        capacity: usize,
    ) -> Self {
        let mut log = Self::with_capacity(stream_id, capacity);
        log.next = next_position.max(1);
        log
    }

    pub fn stream_id(&self) -> &str {
        &self.stream_id
    }

    /// The position the next appended event will take.
    pub fn next_position(&self) -> u64 {
        self.next
    }

    /// The oldest position still servable, or `next` when nothing is retained.
    pub fn oldest_available(&self) -> u64 {
        self.retained
            .front()
            .map(|p| p.position)
            .unwrap_or(self.next)
    }

    /// A cursor pointing at the newest event — what a live subscriber holds.
    pub fn tip(&self) -> Cursor {
        Cursor {
            stream_id: self.stream_id.clone(),
            position: self.next.saturating_sub(1),
        }
    }

    /// Append `event`, returning the position it was assigned.
    ///
    /// Positions are strictly increasing and are NEVER reused, including
    /// across eviction. A reused position would make two different events
    /// indistinguishable to a resuming client.
    pub fn append(&mut self, event: E) -> u64 {
        let position = self.next;
        self.next += 1;
        self.retained.push_back(Positioned { position, event });
        while self.retained.len() > self.capacity {
            self.retained.pop_front();
        }
        position
    }

    /// Everything strictly after `cursor`, in order, exactly once.
    ///
    /// Refuses rather than guessing in all three of the cases in the module
    /// docs. A caller that receives `Ok` may rely on the result being the
    /// COMPLETE set of events the cursor had not seen.
    pub fn since(&self, cursor: &Cursor) -> Result<Vec<Positioned<E>>, CursorError> {
        if cursor.stream_id != self.stream_id {
            return Err(CursorError::StreamMismatch {
                requested: cursor.stream_id.clone(),
                actual: self.stream_id.clone(),
            });
        }
        // A cursor at or beyond `next` claims to have seen an event this
        // stream has not emitted. Answering "nothing new" would tell it that
        // it is caught up, and it would wait forever.
        if cursor.position >= self.next {
            return Err(CursorError::Ahead {
                requested: cursor.position,
                next: self.next,
            });
        }
        // The client wants everything after `position`, so the first event it
        // needs is `position + 1`. That event must still be retained.
        let wanted_from = cursor.position + 1;
        let oldest = self.oldest_available();
        if wanted_from < oldest {
            return Err(CursorError::TooOld {
                requested: cursor.position,
                oldest_available: oldest,
            });
        }
        Ok(self
            .retained
            .iter()
            .filter(|p| p.position >= wanted_from)
            .cloned()
            .collect())
    }

    /// The genesis cursor for this stream — "I have seen nothing".
    pub fn genesis(&self) -> Cursor {
        Cursor {
            stream_id: self.stream_id.clone(),
            position: 0,
        }
    }

    /// Number of retained events.
    pub fn retained_len(&self) -> usize {
        self.retained.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn log_with(n: u64) -> EventLog<String> {
        let mut log = EventLog::with_capacity("stream-A", 4);
        for i in 1..=n {
            log.append(format!("e{i}"));
        }
        log
    }

    #[test]
    fn positions_start_at_one_and_strictly_increase() {
        let mut log: EventLog<String> = EventLog::new("s");
        assert_eq!(log.append("a".into()), 1);
        assert_eq!(log.append("b".into()), 2);
        assert_eq!(log.append("c".into()), 3);
        assert_eq!(log.next_position(), 4);
    }

    #[test]
    fn a_reconnecting_client_receives_exactly_what_it_missed_in_order_once() {
        let log = log_with(4);
        let cursor = Cursor {
            stream_id: "stream-A".into(),
            position: 2,
        };
        let got = log.since(&cursor).expect("in-range cursor is servable");
        let positions: Vec<u64> = got.iter().map(|p| p.position).collect();
        let bodies: Vec<&str> = got.iter().map(|p| p.event.as_str()).collect();
        assert_eq!(
            positions,
            vec![3, 4],
            "exactly the missed positions, in order"
        );
        assert_eq!(bodies, vec!["e3", "e4"]);
    }

    #[test]
    fn the_genesis_cursor_yields_everything_retained() {
        let log = log_with(3);
        let got = log.since(&log.genesis()).unwrap();
        assert_eq!(got.len(), 3);
        assert_eq!(got[0].position, 1);
    }

    #[test]
    fn a_caught_up_cursor_yields_nothing_without_erroring() {
        // The boundary between "caught up" (fine, empty) and "ahead" (a
        // refusal) is exactly one position, so it is asserted rather than
        // assumed.
        let log = log_with(3);
        let tip = log.tip();
        assert_eq!(tip.position, 3);
        assert!(log.since(&tip).unwrap().is_empty());
    }

    #[test]
    fn a_position_ahead_of_the_stream_is_refused_rather_than_answered_with_nothing() {
        // The dangerous silent case. An empty answer here tells a client it is
        // caught up when the server has lost what it is waiting for, and the
        // client blocks forever on a stream that never explains itself.
        let log = log_with(3);
        let err = log
            .since(&Cursor {
                stream_id: "stream-A".into(),
                position: 9,
            })
            .expect_err("a position the stream never emitted must be refused");
        assert_eq!(
            err,
            CursorError::Ahead {
                requested: 9,
                next: 4
            }
        );
        assert!(err.to_string().contains("ahead"), "{err}");
    }

    #[test]
    fn an_evicted_position_is_refused_with_the_oldest_still_servable() {
        // Capacity 4, six appended: positions 1 and 2 are gone.
        let log = log_with(6);
        assert_eq!(log.retained_len(), 4);
        assert_eq!(log.oldest_available(), 3);
        let err = log
            .since(&Cursor {
                stream_id: "stream-A".into(),
                position: 1,
            })
            .expect_err("position 2 was evicted, so resuming after 1 must refuse");
        assert_eq!(
            err,
            CursorError::TooOld {
                requested: 1,
                oldest_available: 3
            },
            "the refusal must NAME where to resynchronise from, or the client \
             has to guess and will guess by skipping"
        );
    }

    #[test]
    fn the_eviction_boundary_is_exact() {
        // Off-by-one here silently drops one event on every resume near the
        // boundary, which is invisible until someone counts.
        let log = log_with(6); // retained 3,4,5,6
        // Resuming AFTER 2 needs position 3, which is retained: servable.
        let ok = log
            .since(&Cursor {
                stream_id: "stream-A".into(),
                position: 2,
            })
            .expect("resuming after 2 needs 3, which is retained");
        assert_eq!(
            ok.iter().map(|p| p.position).collect::<Vec<_>>(),
            vec![3, 4, 5, 6]
        );
        // Resuming after 1 needs position 2, which is gone: refused.
        assert!(
            log.since(&Cursor {
                stream_id: "stream-A".into(),
                position: 1
            })
            .is_err()
        );
    }

    #[test]
    fn a_cursor_from_another_stream_is_refused_even_when_the_position_is_in_range() {
        // THE case a bare position cannot express. After a restart the log
        // numbers from 1 again; a client holding position 2 of the OLD stream
        // would be handed positions 3.. of the NEW one, would believe itself
        // continuous, and would have silently missed the new stream's 1 and 2.
        // Nothing errors, nothing duplicates, and the server's own counts look
        // perfect — which is precisely why the identity has to be checked.
        let restarted = log_with(4); // stream-A, positions 1..4
        let stale = Cursor {
            stream_id: "stream-A-before-restart".into(),
            position: 2,
        };
        // Positive control: the same POSITION on the right stream is servable,
        // so the refusal below is caused by the identity and nothing else.
        assert!(
            restarted
                .since(&Cursor {
                    stream_id: "stream-A".into(),
                    position: 2
                })
                .is_ok(),
            "positive control: position 2 is in range for this stream"
        );
        let err = restarted
            .since(&stale)
            .expect_err("a cursor from a previous stream must be refused");
        assert!(matches!(err, CursorError::StreamMismatch { .. }), "{err:?}");
    }

    #[test]
    fn resuming_a_stream_continues_the_numbering_rather_than_restarting_it() {
        // Restoring the identity while restarting the numbering would make a
        // stale cursor look VALID — worse than a fresh stream, which at least
        // refuses.
        let resumed: EventLog<String> = EventLog::resume_stream("stream-A", 100, 8);
        assert_eq!(resumed.next_position(), 100);
        let mut resumed = resumed;
        assert_eq!(resumed.append("x".into()), 100);
        // A cursor from before the restart is in the PAST of this log, so it
        // is refused as too old — not silently served from 100.
        let err = resumed
            .since(&Cursor {
                stream_id: "stream-A".into(),
                position: 5,
            })
            .expect_err("a pre-restart position is not retained here");
        assert!(matches!(err, CursorError::TooOld { .. }), "{err:?}");
    }

    #[test]
    fn retention_is_bounded_so_a_disconnected_client_cannot_grow_the_log() {
        // T-24-03-05. Retained history is memory a disconnected client makes
        // the server hold.
        let mut log: EventLog<u64> = EventLog::with_capacity("s", 8);
        for i in 0..10_000 {
            log.append(i);
        }
        assert_eq!(log.retained_len(), 8);
        assert_eq!(log.next_position(), 10_001);
    }

    #[test]
    fn positions_are_never_reused_after_eviction() {
        // A reused position makes two different events indistinguishable to a
        // resuming client.
        let mut log: EventLog<String> = EventLog::with_capacity("s", 2);
        log.append("a".into());
        log.append("b".into());
        log.append("c".into());
        let all = log.since(&log.genesis()).unwrap_err();
        // Genesis is now too old, which is itself the correct answer.
        assert!(matches!(all, CursorError::TooOld { .. }));
        let got = log
            .since(&Cursor {
                stream_id: "s".into(),
                position: 1,
            })
            .unwrap();
        assert_eq!(
            got.iter().map(|p| p.position).collect::<Vec<_>>(),
            vec![2, 3]
        );
    }
}
