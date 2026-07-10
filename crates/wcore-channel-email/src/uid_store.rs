//! Persistent IMAP UID watermark.
//!
//! The in-memory `last_seen_uid` alone has two defects: on first connect it is
//! `0`, so a `UID 1:*` search returns the **entire mailbox** and replays every
//! existing message as new inbound; and across a restart it resets to `0`,
//! either replaying again or (with first-connect seeding) skipping mail that
//! arrived while the process was down.
//!
//! This module persists the watermark per account+mailbox under the profile
//! home (`$WAYLAND_HOME/channel-state/`) so a restart resumes exactly where it
//! left off. Writes are best-effort: a failure is logged and the in-session
//! in-memory watermark still prevents same-process re-delivery.

use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};

/// Deterministic per-account state-file path. Uses `DefaultHasher` (fixed keys,
/// stable across processes) over host+user+mailbox so the same account always
/// maps to the same file without leaking the address into the filename.
fn state_path(host: &str, user: &str, mailbox: &str) -> PathBuf {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    host.hash(&mut h);
    user.hash(&mut h);
    mailbox.hash(&mut h);
    let key = h.finish();
    wcore_config::config::wayland_config_dir()
        .join("channel-state")
        .join(format!("imap-{key:016x}.uid"))
}

/// Load the persisted high-water UID for this account+mailbox, if any.
pub(crate) fn load(host: &str, user: &str, mailbox: &str) -> Option<u32> {
    load_from(&state_path(host, user, mailbox))
}

fn load_from(path: &Path) -> Option<u32> {
    std::fs::read_to_string(path)
        .ok()?
        .trim()
        .parse::<u32>()
        .ok()
}

/// Persist the high-water UID. Best-effort; a write failure is logged only.
pub(crate) fn save(host: &str, user: &str, mailbox: &str, uid: u32) {
    if let Err(e) = save_to(&state_path(host, user, mailbox), uid) {
        tracing::warn!(
            target: "wcore_channel_email::imap",
            error = %e,
            "could not persist imap uid watermark; restart may re-seed",
        );
    }
}

fn save_to(path: &Path, uid: u32) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, uid.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn unique_tmp() -> PathBuf {
        // Per-call unique path. A monotonic counter (not a pointer to a
        // zero-sized temporary, which is the same constant address for every
        // call) keeps parallel tests in this module from sharing a file.
        static SEQ: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let n = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        std::env::temp_dir().join(format!("wcore-email-uid-{}-{n}.uid", std::process::id()))
    }

    #[test]
    fn load_from_missing_file_is_none() {
        let p = unique_tmp();
        let _ = std::fs::remove_file(&p);
        assert_eq!(load_from(&p), None);
    }

    #[test]
    fn save_then_load_round_trips() {
        let p = unique_tmp();
        save_to(&p, 4242).unwrap();
        assert_eq!(load_from(&p), Some(4242));
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn load_from_garbage_is_none() {
        let p = unique_tmp();
        std::fs::write(&p, "not a number").unwrap();
        assert_eq!(load_from(&p), None);
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn state_path_is_stable_and_account_specific() {
        let a = state_path("imap.example.com", "alice@example.com", "INBOX");
        let a2 = state_path("imap.example.com", "alice@example.com", "INBOX");
        let b = state_path("imap.example.com", "bob@example.com", "INBOX");
        assert_eq!(a, a2, "same account must map to the same file");
        assert_ne!(a, b, "different accounts must not collide");
    }
}
