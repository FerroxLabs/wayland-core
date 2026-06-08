//! Collaboration partition (blackboard) — multi-agent topic-keyed scratchpad.
//!
//! 4.B.1: gives Mesh/Fleet topologies a place to share artifacts without
//! the parent transcript. Reads and writes are topic-keyed; subscribers
//! filter with a [`BlackboardPredicate`] and receive matches on an mpsc
//! channel.
//!
//! In-memory only by design. The SQLite-backed variant lands in v0.8
//! once we know what the access patterns actually look like.
//!
//! # Invariants
//!
//! * Entries carry a TTL relative to their write timestamp; expired
//!   entries are pruned on every `read_topic` / `read_prefix` call,
//!   and a manual [`Blackboard::prune`] is also exposed for the
//!   periodic sweeper.
//! * Subscriptions are append-only with respect to the writer — a
//!   subscriber added after a write does NOT replay history. Use
//!   `read_prefix` for backfill if you need it.
//! * The audit log is a fixed-size ring buffer (1024 entries) — when
//!   full, the oldest entry is dropped. Production callers should
//!   sample, not exhaustively replay.

use std::collections::VecDeque;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::sync::mpsc;

/// Maximum number of audit-log entries retained before the oldest is dropped.
const AUDIT_LOG_CAP: usize = 1024;

/// Default subscription channel buffer. Slow subscribers may drop notifications
/// once the buffer is full; the audit log preserves the canonical history.
const DEFAULT_SUB_CAP: usize = 64;

/// A single blackboard write.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BlackboardEntry {
    /// Hierarchical topic (e.g. `"plan/step3/proposal"`).
    pub topic: String,
    /// Free-form JSON payload — the partition does not interpret it.
    pub payload: Value,
    /// Identifier of the agent that wrote this entry.
    pub author: String,
    /// Wall-clock write timestamp (SystemTime; serialized as system-millis
    /// in production exports, but the partition itself treats it as
    /// opaque for ordering).
    #[serde(skip, default = "SystemTime::now")]
    pub ts: SystemTime,
    /// Time-to-live for this entry; pruned lazily after `ts + ttl`.
    #[serde(with = "humantime_serde", default = "default_ttl")]
    pub ttl: Duration,
}

fn default_ttl() -> Duration {
    Duration::from_secs(3600)
}

impl BlackboardEntry {
    pub fn new(topic: impl Into<String>, payload: Value, author: impl Into<String>) -> Self {
        Self {
            topic: topic.into(),
            payload,
            author: author.into(),
            ts: SystemTime::now(),
            ttl: default_ttl(),
        }
    }

    pub fn with_ttl(mut self, ttl: Duration) -> Self {
        self.ttl = ttl;
        self
    }

    /// Returns true if `now` is past `ts + ttl`.
    pub fn is_expired(&self, now: SystemTime) -> bool {
        match self.ts.checked_add(self.ttl) {
            Some(expires_at) => now >= expires_at,
            None => false, // saturated TTL = effectively immortal
        }
    }
}

/// Lightweight summary written to the audit log on every blackboard
/// write. Cheap to clone.
#[derive(Debug, Clone)]
pub struct AuditRecord {
    pub author: String,
    pub topic: String,
    pub ts: SystemTime,
}

/// Predicate used by subscribers. The blackboard evaluates this against
/// every freshly-written entry; matches are forwarded on the subscriber's
/// channel.
#[derive(Clone)]
pub enum BlackboardPredicate {
    /// Match all writes.
    All,
    /// Match writes whose topic equals the given string.
    TopicEquals(String),
    /// Match writes whose topic starts with the given prefix.
    TopicStartsWith(String),
    /// Match writes from the given author.
    AuthorEquals(String),
}

impl BlackboardPredicate {
    pub fn matches(&self, entry: &BlackboardEntry) -> bool {
        match self {
            Self::All => true,
            Self::TopicEquals(t) => &entry.topic == t,
            Self::TopicStartsWith(p) => entry.topic.starts_with(p),
            Self::AuthorEquals(a) => &entry.author == a,
        }
    }
}

/// Subscription handle returned by [`Blackboard::subscribe`]. Drop to
/// unsubscribe.
pub struct Subscription {
    pub receiver: mpsc::Receiver<BlackboardEntry>,
    id: u64,
    inner: Arc<Mutex<Inner>>,
}

impl Drop for Subscription {
    fn drop(&mut self) {
        self.inner.lock().subscribers.retain(|s| s.id != self.id);
    }
}

struct SubscriberSlot {
    id: u64,
    predicate: BlackboardPredicate,
    sender: mpsc::Sender<BlackboardEntry>,
}

struct Inner {
    entries: Vec<BlackboardEntry>,
    audit: VecDeque<AuditRecord>,
    subscribers: Vec<SubscriberSlot>,
    next_sub_id: u64,
}

/// Multi-agent blackboard partition. Cheap to clone (Arc-wrapped inner state).
#[derive(Clone)]
pub struct Blackboard {
    inner: Arc<Mutex<Inner>>,
}

impl Default for Blackboard {
    fn default() -> Self {
        Self::new()
    }
}

impl Blackboard {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(Mutex::new(Inner {
                entries: Vec::new(),
                audit: VecDeque::with_capacity(AUDIT_LOG_CAP),
                subscribers: Vec::new(),
                next_sub_id: 0,
            })),
        }
    }

    /// Append a new entry. Notifies every matching subscriber; if a
    /// subscriber's channel is full, the notification is dropped (the
    /// canonical history still lives in the partition).
    ///
    /// Returns the topic of the written entry (useful for chaining).
    pub fn write(&self, entry: BlackboardEntry) -> String {
        let now = SystemTime::now();
        let topic = entry.topic.clone();
        let mut inner = self.inner.lock();

        // Audit (push, then trim — the deque's len is unbounded by
        // default).
        let record = AuditRecord {
            author: entry.author.clone(),
            topic: topic.clone(),
            ts: now,
        };
        inner.audit.push_back(record);
        while inner.audit.len() > AUDIT_LOG_CAP {
            inner.audit.pop_front();
        }

        // Notify subscribers. Use try_send so we never block the writer
        // on a slow consumer — overflow gets surfaced via channel
        // semantics and a tracing warning.
        for s in &inner.subscribers {
            if s.predicate.matches(&entry)
                && let Err(e) = s.sender.try_send(entry.clone())
            {
                tracing::warn!(
                    sub_id = s.id,
                    topic = %entry.topic,
                    err = %e,
                    "blackboard subscriber dropped notification (channel full / closed)"
                );
            }
        }

        inner.entries.push(entry);
        topic
    }

    /// Read every entry whose topic equals `topic`, pruning expired
    /// entries from the partition as a side effect.
    pub fn read_topic(&self, topic: &str) -> Vec<BlackboardEntry> {
        self.read_filter(|e| e.topic == topic)
    }

    /// Read every entry whose topic starts with `prefix`, pruning
    /// expired entries from the partition as a side effect.
    pub fn read_prefix(&self, prefix: &str) -> Vec<BlackboardEntry> {
        self.read_filter(|e| e.topic.starts_with(prefix))
    }

    fn read_filter<F>(&self, f: F) -> Vec<BlackboardEntry>
    where
        F: Fn(&BlackboardEntry) -> bool,
    {
        let now = SystemTime::now();
        let mut inner = self.inner.lock();
        inner.entries.retain(|e| !e.is_expired(now));
        inner.entries.iter().filter(|e| f(e)).cloned().collect()
    }

    /// Sweep expired entries. Cheap; safe to call from a periodic task.
    pub fn prune(&self) -> usize {
        let now = SystemTime::now();
        let mut inner = self.inner.lock();
        let before = inner.entries.len();
        inner.entries.retain(|e| !e.is_expired(now));
        before - inner.entries.len()
    }

    /// Subscribe to all future writes matching `predicate`. Returns a
    /// [`Subscription`] holding the receiver — drop the subscription to
    /// unsubscribe.
    pub fn subscribe(&self, predicate: BlackboardPredicate) -> Subscription {
        self.subscribe_with_buffer(predicate, DEFAULT_SUB_CAP)
    }

    pub fn subscribe_with_buffer(
        &self,
        predicate: BlackboardPredicate,
        buffer: usize,
    ) -> Subscription {
        let (tx, rx) = mpsc::channel::<BlackboardEntry>(buffer.max(1));
        let mut inner = self.inner.lock();
        let id = inner.next_sub_id;
        inner.next_sub_id += 1;
        inner.subscribers.push(SubscriberSlot {
            id,
            predicate,
            sender: tx,
        });
        Subscription {
            receiver: rx,
            id,
            inner: self.inner.clone(),
        }
    }

    /// Snapshot the audit log. Reads do not consume.
    pub fn audit_log(&self) -> Vec<AuditRecord> {
        self.inner.lock().audit.iter().cloned().collect()
    }

    /// Live entry count (post-prune).
    pub fn len(&self) -> usize {
        let now = SystemTime::now();
        let mut inner = self.inner.lock();
        inner.entries.retain(|e| !e.is_expired(now));
        inner.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::thread::sleep;

    #[test]
    fn write_then_read_topic_returns_entry() {
        let bb = Blackboard::new();
        bb.write(BlackboardEntry::new("plan/a", json!({"step": 1}), "alice"));
        let hits = bb.read_topic("plan/a");
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].author, "alice");
    }

    #[test]
    fn read_prefix_matches_hierarchical_topics() {
        let bb = Blackboard::new();
        bb.write(BlackboardEntry::new("plan/a", json!(1), "alice"));
        bb.write(BlackboardEntry::new("plan/b", json!(2), "bob"));
        bb.write(BlackboardEntry::new("other", json!(3), "carol"));
        let hits = bb.read_prefix("plan/");
        assert_eq!(hits.len(), 2);
    }

    #[test]
    fn ttl_eviction_prunes_expired_entries() {
        let bb = Blackboard::new();
        let entry = BlackboardEntry::new("ephemeral", json!(true), "alice")
            .with_ttl(Duration::from_millis(20));
        bb.write(entry);
        assert_eq!(bb.read_topic("ephemeral").len(), 1);
        sleep(Duration::from_millis(60));
        assert_eq!(bb.read_topic("ephemeral").len(), 0, "should be pruned");
    }

    #[test]
    fn explicit_prune_returns_count() {
        let bb = Blackboard::new();
        for i in 0..3 {
            bb.write(
                BlackboardEntry::new(format!("t/{i}"), json!(i), "a")
                    .with_ttl(Duration::from_millis(10)),
            );
        }
        sleep(Duration::from_millis(40));
        let pruned = bb.prune();
        assert_eq!(pruned, 3);
        assert!(bb.is_empty());
    }

    #[tokio::test]
    async fn subscribe_topic_starts_with_delivers_matches() {
        let bb = Blackboard::new();
        let mut sub = bb.subscribe(BlackboardPredicate::TopicStartsWith("plan/".into()));
        bb.write(BlackboardEntry::new("plan/x", json!(1), "a"));
        bb.write(BlackboardEntry::new("other", json!(2), "b"));
        bb.write(BlackboardEntry::new("plan/y", json!(3), "c"));
        let got1 = sub.receiver.recv().await.expect("plan/x");
        let got2 = sub.receiver.recv().await.expect("plan/y");
        assert_eq!(got1.topic, "plan/x");
        assert_eq!(got2.topic, "plan/y");
    }

    #[tokio::test]
    async fn subscribe_author_equals_filters_by_author() {
        let bb = Blackboard::new();
        let mut sub = bb.subscribe(BlackboardPredicate::AuthorEquals("alice".into()));
        bb.write(BlackboardEntry::new("t1", json!(1), "bob"));
        bb.write(BlackboardEntry::new("t2", json!(2), "alice"));
        let got = sub.receiver.recv().await.expect("alice's write");
        assert_eq!(got.author, "alice");
        // Should NOT receive bob's; channel should be empty next.
        assert!(sub.receiver.try_recv().is_err());
    }

    #[tokio::test]
    async fn dropping_subscription_unregisters_subscriber() {
        let bb = Blackboard::new();
        {
            let _sub = bb.subscribe(BlackboardPredicate::All);
            assert_eq!(bb.inner.lock().subscribers.len(), 1);
        } // sub dropped here
        assert_eq!(bb.inner.lock().subscribers.len(), 0);
    }

    #[test]
    fn audit_log_records_every_write_and_caps_at_1024() {
        let bb = Blackboard::new();
        for i in 0..1100u32 {
            bb.write(BlackboardEntry::new(format!("t/{i}"), json!(i), "a"));
        }
        let audit = bb.audit_log();
        assert_eq!(audit.len(), AUDIT_LOG_CAP);
        // The oldest 76 were dropped; remaining starts at "t/76".
        assert_eq!(audit.first().unwrap().topic, "t/76");
    }

    #[test]
    fn predicate_matches_individually() {
        let entry = BlackboardEntry::new("plan/step1", json!(0), "alice");
        assert!(BlackboardPredicate::All.matches(&entry));
        assert!(BlackboardPredicate::TopicEquals("plan/step1".into()).matches(&entry));
        assert!(BlackboardPredicate::TopicStartsWith("plan/".into()).matches(&entry));
        assert!(BlackboardPredicate::AuthorEquals("alice".into()).matches(&entry));
        assert!(!BlackboardPredicate::TopicEquals("other".into()).matches(&entry));
        assert!(!BlackboardPredicate::AuthorEquals("bob".into()).matches(&entry));
    }
}
