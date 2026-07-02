//! Bounded index of RFC `Message-ID`s this channel has sent.
//!
//! Written by the SMTP send path (before the wire send, so the entry is
//! guaranteed to exist by the time the mail can possibly be delivered) and
//! read by the IMAP poll task to recognize the channel's own outbound mail
//! when it echoes back into the monitored mailbox (self-send, forwarding
//! rules, shared inbox). An echoed message is marked `is_self` so the
//! dispatch kernel's loop guard drops it instead of triggering a new agent
//! turn — the wayland#547 unbounded self-reply loop.

use std::collections::{HashSet, VecDeque};
use std::sync::{Arc, Mutex as StdMutex};

/// Shared handle to the sent-id index. `std::Mutex` because the IMAP poll
/// task is synchronous.
pub(crate) type SentIdIndex = Arc<StdMutex<SentMessageIds>>;

/// Upper bound on remembered outbound ids. FIFO eviction: old entries only
/// matter for as long as an echo of that mail could still arrive, so
/// forgetting the oldest under sustained send volume is safe.
pub(crate) const SENT_INDEX_CAP: usize = 512;

/// FIFO set of bare (no angle brackets) outbound `Message-ID`s.
#[derive(Debug, Default)]
pub(crate) struct SentMessageIds {
    set: HashSet<String>,
    order: VecDeque<String>,
}

impl SentMessageIds {
    /// Record an outbound id, evicting the oldest entry past
    /// [`SENT_INDEX_CAP`]. Empty ids and duplicates are no-ops.
    pub(crate) fn record(&mut self, id: String) {
        if id.is_empty() || self.set.contains(&id) {
            return;
        }
        if self.order.len() >= SENT_INDEX_CAP
            && let Some(oldest) = self.order.pop_front()
        {
            self.set.remove(&oldest);
        }
        self.set.insert(id.clone());
        self.order.push_back(id);
    }

    /// Whether `id` is a Message-ID this channel sent (and still remembers).
    pub(crate) fn contains(&self, id: &str) -> bool {
        self.set.contains(id)
    }
}

/// Fresh shared index.
pub(crate) fn new_index() -> SentIdIndex {
    Arc::new(StdMutex::new(SentMessageIds::default()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn record_and_contains_roundtrip() {
        let mut s = SentMessageIds::default();
        s.record("a@x".into());
        assert!(s.contains("a@x"));
        assert!(!s.contains("b@x"));
    }

    #[test]
    fn empty_id_is_ignored() {
        let mut s = SentMessageIds::default();
        s.record(String::new());
        assert!(!s.contains(""));
        assert!(s.order.is_empty());
    }

    #[test]
    fn duplicate_record_does_not_grow_order() {
        let mut s = SentMessageIds::default();
        s.record("a@x".into());
        s.record("a@x".into());
        assert_eq!(s.order.len(), 1);
    }

    #[test]
    fn eviction_is_fifo_and_bounded() {
        let mut s = SentMessageIds::default();
        for i in 0..(SENT_INDEX_CAP + 3) {
            s.record(format!("id-{i}@x"));
        }
        assert_eq!(s.order.len(), SENT_INDEX_CAP);
        assert_eq!(s.set.len(), SENT_INDEX_CAP);
        // The three oldest fell out; the newest are retained.
        assert!(!s.contains("id-0@x"));
        assert!(!s.contains("id-2@x"));
        assert!(s.contains("id-3@x"));
        assert!(s.contains(&format!("id-{}@x", SENT_INDEX_CAP + 2)));
    }
}
