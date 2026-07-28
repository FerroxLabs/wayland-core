//! The producer for [`crate::Trigger::Event`]: a durable, cross-process queue
//! of published topics that the tick drains.
//!
//! 24-C2. Before this, `--trigger event:build.finished` validated, persisted
//! and listed, and then nothing in the runtime could ever make it fire — the
//! customer got no error, saw the job in `cron list`, and it silently never
//! ran. A trigger vocabulary with no producer is a promise, not a feature.
//!
//! # Why a directory of files and not a channel
//!
//! The publisher and the consumer are DIFFERENT PROCESSES. The gateway (or the
//! `cron daemon`) owns the schedule; the thing that knows a build finished is a
//! CI step, a hook, or an operator at a shell. An in-process broadcast could
//! only ever serve a publisher living inside the daemon, which is the one case
//! that does not need a bus. So the queue is on disk, beside `jobs.json`, in
//! the same directory the schedule lease already governs.
//!
//! One file per event, never one appended file:
//!
//! - a publish is a `write-temp` + `rename`, which is atomic on both POSIX and
//!   Windows-with-replace, so a reader never observes half an event and two
//!   concurrent publishers cannot interleave into each other's bytes;
//! - a drain deletes individual files, so a crash mid-drain loses nothing.
//!
//! Filenames are opaque (a v4 UUID). The topic is INSIDE the JSON and never in
//! the path: a topic is operator-supplied text, and deriving a path from it
//! would let `--trigger event:../../../etc/whatever` address the filesystem.
//!
//! # Delivery semantics, stated rather than implied
//!
//! **At least once.** An event file is deleted AFTER its matching jobs have
//! been dispatched, so a process killed between the dispatch and the delete
//! re-fires on the next tick. The other order — delete first — would lose the
//! event outright, and a lost automation trigger is worse than a repeated one
//! for every target this runtime has. Callers must tolerate a duplicate fire.
//!
//! **Fan-out.** One event fires EVERY enabled job subscribed to its topic, not
//! the first. Consuming the event on the first match would make a second
//! subscriber silently dead — the same defect class this module exists to fix.
//!
//! **Exact topic match.** No prefix, no glob. A matching rule is a compat
//! constraint the moment it ships, and an operator who wants a hierarchy can
//! publish two topics.
//!
//! # Bounds
//!
//! The queue is capped ([`MAX_PENDING`]) and a publish that would exceed the
//! cap is REFUSED with a non-zero exit rather than silently dropping the
//! oldest entry. A runaway publisher that gets `ok` from every call while the
//! queue quietly discards is exactly the silent failure this work exists to
//! remove. The drain is capped per tick ([`MAX_DRAIN_PER_TICK`]) so a backlog
//! accumulated over an outage cannot fire as one burst.

use std::fs;
use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::CronError;

/// Subdirectory of the cron directory holding pending events.
pub const EVENTS_SUBDIR: &str = "events";

/// Most events that may be pending at once. A publish beyond this is refused.
pub const MAX_PENDING: usize = 1024;

/// Most events one tick will drain. A backlog drains over several ticks
/// instead of firing as a single burst.
pub const MAX_DRAIN_PER_TICK: usize = 64;

/// One published occurrence of a topic.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PublishedEvent {
    /// Opaque identity. Also the filename stem, so a re-publish of the same
    /// topic is a distinct event rather than an overwrite.
    pub id: String,
    /// The topic an `Event` trigger matches on, exactly.
    pub topic: String,
    /// When the publisher recorded it. Used to order the drain, and to decide
    /// that a job created after an event was published does not consume it.
    pub published_at: DateTime<Utc>,
}

/// The events directory for a cron directory.
pub fn events_dir(cron_dir: impl AsRef<Path>) -> PathBuf {
    cron_dir.as_ref().join(EVENTS_SUBDIR)
}

/// Publish `topic` into `cron_dir`'s queue.
///
/// Returns the event as recorded. Refuses — rather than dropping anything —
/// when the queue is already at [`MAX_PENDING`].
pub fn publish(
    cron_dir: impl AsRef<Path>,
    topic: &str,
    now: DateTime<Utc>,
) -> crate::Result<PublishedEvent> {
    let topic = topic.trim();
    if topic.is_empty() {
        return Err(CronError::InvalidExpression(
            "cannot publish an empty topic".into(),
        ));
    }
    let dir = events_dir(&cron_dir);
    fs::create_dir_all(&dir).map_err(CronError::Io)?;
    restrict_permissions(&dir);

    // Counted before the write, so the cap is enforced rather than reported.
    let pending = pending_count(&dir);
    if pending >= MAX_PENDING {
        return Err(CronError::Dispatch(format!(
            "event queue is full ({pending} pending, cap {MAX_PENDING}); refusing to publish \
             {topic:?}. Nothing is draining it — is the gateway or `cron daemon` running?"
        )));
    }

    let event = PublishedEvent {
        id: uuid::Uuid::new_v4().to_string(),
        topic: topic.to_string(),
        published_at: now,
    };
    let body = serde_json::to_vec(&event).map_err(CronError::Serde)?;

    // Temp-then-rename: a drain running concurrently sees either no file or a
    // complete one, never a truncated read of a partial write.
    let tmp = dir.join(format!("{}.tmp", event.id));
    let final_path = dir.join(format!("{}.json", event.id));
    fs::write(&tmp, &body).map_err(CronError::Io)?;
    fs::rename(&tmp, &final_path).map_err(|e| {
        let _ = fs::remove_file(&tmp);
        CronError::Io(e)
    })?;
    Ok(event)
}

/// Every pending event, oldest first, capped at [`MAX_DRAIN_PER_TICK`].
///
/// Unparsable entries are removed and counted rather than left to jam the
/// queue forever: a file that cannot be read is a file that will never fire,
/// and leaving it in place would consume a cap slot on every subsequent tick.
pub fn pending(cron_dir: impl AsRef<Path>) -> Vec<(PathBuf, PublishedEvent)> {
    let dir = events_dir(&cron_dir);
    let Ok(entries) = fs::read_dir(&dir) else {
        return Vec::new();
    };
    let mut out: Vec<(PathBuf, PublishedEvent)> = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }
        match fs::read(&path)
            .ok()
            .and_then(|b| serde_json::from_slice::<PublishedEvent>(&b).ok())
        {
            Some(ev) => out.push((path, ev)),
            None => {
                tracing::warn!(
                    target: "wcore_cron::events",
                    path = %path.display(),
                    "discarding an unreadable event record"
                );
                let _ = fs::remove_file(&path);
            }
        }
    }
    out.sort_by_key(|(_, e)| e.published_at);
    out.truncate(MAX_DRAIN_PER_TICK);
    out
}

/// Remove one drained event. Non-fatal: a delete that fails re-fires the event
/// on the next tick, which is the at-least-once side of the contract and is
/// preferable to aborting the tick.
pub fn consume(path: &Path) {
    if let Err(e) = fs::remove_file(path) {
        tracing::warn!(
            target: "wcore_cron::events",
            path = %path.display(),
            error = %e,
            "failed to remove a drained event; it will fire again"
        );
    }
}

fn pending_count(dir: &Path) -> usize {
    fs::read_dir(dir)
        .map(|rd| {
            rd.flatten()
                .filter(|e| e.path().extension().and_then(|x| x.to_str()) == Some("json"))
                .count()
        })
        .unwrap_or(0)
}

/// Owner-only on unix. Anyone who can write this directory can fire any
/// event-triggered job, which is the same authority as editing `jobs.json`,
/// so it gets the same posture rather than the umask's.
#[cfg(unix)]
fn restrict_permissions(dir: &Path) {
    use std::os::unix::fs::PermissionsExt;
    let _ = fs::set_permissions(dir, fs::Permissions::from_mode(0o700));
}

#[cfg(not(unix))]
fn restrict_permissions(_dir: &Path) {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_published_topic_is_readable_back() {
        let dir = tempfile::tempdir().unwrap();
        let ev = publish(dir.path(), "build.finished", Utc::now()).unwrap();
        let got = pending(dir.path());
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].1.topic, "build.finished");
        assert_eq!(got[0].1.id, ev.id);
    }

    #[test]
    fn two_publishes_of_one_topic_are_two_events() {
        // Not a set. Two builds finishing is two occurrences, and collapsing
        // them would silently drop work.
        let dir = tempfile::tempdir().unwrap();
        publish(dir.path(), "t", Utc::now()).unwrap();
        publish(dir.path(), "t", Utc::now()).unwrap();
        assert_eq!(pending(dir.path()).len(), 2);
    }

    #[test]
    fn a_topic_cannot_address_the_filesystem() {
        // The filename is opaque; the topic never reaches the path. Without
        // this the topic is a path-traversal primitive reachable from the CLI.
        let dir = tempfile::tempdir().unwrap();
        let ev = publish(dir.path(), "../../escape", Utc::now()).unwrap();
        let listed = pending(dir.path());
        assert_eq!(listed.len(), 1, "the event must land inside the queue dir");
        assert!(
            listed[0].0.starts_with(events_dir(dir.path())),
            "event file escaped the queue directory: {:?}",
            listed[0].0
        );
        assert_eq!(listed[0].1.topic, "../../escape");
        assert!(!ev.id.contains("escape"));
    }

    #[test]
    fn the_queue_is_capped_and_a_publish_past_the_cap_is_refused_not_dropped() {
        let dir = tempfile::tempdir().unwrap();
        for _ in 0..MAX_PENDING {
            publish(dir.path(), "t", Utc::now()).unwrap();
        }
        let err = publish(dir.path(), "t", Utc::now())
            .expect_err("a publish past the cap must be refused, not silently dropped");
        assert!(
            format!("{err}").contains("full"),
            "the refusal must say why, got {err}"
        );
        assert_eq!(pending_count(&events_dir(dir.path())), MAX_PENDING);
    }

    #[test]
    fn a_drain_is_capped_per_tick() {
        let dir = tempfile::tempdir().unwrap();
        for _ in 0..(MAX_DRAIN_PER_TICK + 10) {
            publish(dir.path(), "t", Utc::now()).unwrap();
        }
        assert_eq!(
            pending(dir.path()).len(),
            MAX_DRAIN_PER_TICK,
            "a backlog must drain over several ticks, not fire as one burst"
        );
    }

    #[test]
    fn an_empty_topic_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        assert!(publish(dir.path(), "   ", Utc::now()).is_err());
    }

    #[test]
    fn an_unreadable_record_is_discarded_rather_than_jamming_the_queue() {
        let dir = tempfile::tempdir().unwrap();
        publish(dir.path(), "good", Utc::now()).unwrap();
        fs::write(events_dir(dir.path()).join("garbage.json"), b"{not json").unwrap();
        let got = pending(dir.path());
        assert_eq!(got.len(), 1, "the good event still drains");
        assert!(
            !events_dir(dir.path()).join("garbage.json").exists(),
            "the unreadable record must not keep consuming a cap slot"
        );
    }
}
