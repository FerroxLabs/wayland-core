//! Persistent Matrix `/sync` cursor (F24-C3-H6).
//!
//! The in-memory `since` token alone has one defect, and it is a silent one:
//! it resets to `None` across a restart, so the first `/sync` after a restart
//! is an *initial* sync, and [`crate::sync`]'s initial-sync replay guard
//! discards that response's timeline. The window the process was down for is
//! exactly what the homeserver accumulated into that timeline, so every
//! message delivered during a deploy, crash or reboot was dropped — with no
//! error, no retry, no log, and a channel that reported healthy.
//!
//! This module persists the cursor per (homeserver × bot user × channel name)
//! under the profile home (`$WAYLAND_HOME/channel-state/`) so a restart
//! resumes exactly where it left off — the same contract, and deliberately the
//! same shape, as `wcore-channel-email`'s `uid_store`: **a restart must
//! neither replay the backlog nor skip what arrived while we were down.**
//!
//! Reads are three-state on purpose. "No cursor yet" (first run) and "a cursor
//! file exists but is unusable" call for different operator messages, and
//! collapsing them into `None` is how a corrupt file becomes a silent restart
//! from now.

use std::hash::{Hash, Hasher};
use std::io::Read;
use std::path::{Path, PathBuf};

/// Longest `since` token we will write or accept back. Real tokens are tens of
/// bytes (`s72595_4483_1934`); 2 KiB is far past any homeserver's and keeps a
/// junk file from being read into memory or pasted into a query string.
const MAX_CURSOR_BYTES: usize = 2048;

/// What the persisted cursor file yielded.
pub(crate) enum Loaded {
    /// A usable cursor. Resume from it: the sync that follows is *incremental*,
    /// so the homeserver serves the window we were down for.
    Cursor(String),
    /// Nothing persisted yet — first run for this account. Seed from an initial
    /// sync (whose timeline is correctly discarded, so the room backlog is not
    /// replayed) and persist immediately.
    Absent,
    /// A file was there but did not hold a usable cursor. Behaves like `Absent`
    /// for control flow, but is a distinct state so the caller can SAY SO
    /// rather than starting from now in silence. Carries the reason.
    Corrupt(&'static str),
}

/// Deterministic per-account state-file path. `DefaultHasher` (fixed keys, so
/// stable across processes) over homeserver + bot user id + channel name, so
/// the same channel always maps to the same file without putting a user id in
/// a filename.
///
/// Keying on the homeserver URL is load-bearing beyond uniqueness: repointing a
/// channel at a different homeserver yields a different key, so a cursor from
/// the old server is never sent to the new one (which would reject it).
pub(crate) fn state_path(api_base: &str, user_id: &str, channel: &str) -> PathBuf {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    api_base.hash(&mut h);
    user_id.hash(&mut h);
    channel.hash(&mut h);
    let key = h.finish();
    wcore_config::config::wayland_config_dir()
        .join("channel-state")
        .join(format!("matrix-{key:016x}.since"))
}

/// Validate a cursor read back from disk. A `since` token is opaque to us, so
/// the checks are structural: it must be non-empty, bounded, and safe to place
/// in a query string. Anything else is corruption, not a cursor.
fn validate(raw: &str) -> Result<String, &'static str> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err("empty");
    }
    if trimmed.len() > MAX_CURSOR_BYTES {
        return Err("oversized");
    }
    if trimmed.chars().any(|c| c.is_control() || c.is_whitespace()) {
        return Err("contains control or whitespace characters");
    }
    Ok(trimmed.to_string())
}

/// Read the persisted cursor for this account, if any.
pub(crate) fn load_from(path: &Path) -> Loaded {
    let file = match std::fs::File::open(path) {
        Ok(f) => f,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Loaded::Absent,
        // Unreadable (permissions, a directory in its place, an I/O error) is
        // NOT "absent": something is there and we could not use it.
        Err(_) => return Loaded::Corrupt("unreadable"),
    };
    // Read through a cap so a junk or truncated-huge file cannot be pulled into
    // memory whole. One byte over the cap is enough to detect oversize.
    let mut buf = Vec::new();
    if file
        .take(MAX_CURSOR_BYTES as u64 + 1)
        .read_to_end(&mut buf)
        .is_err()
    {
        return Loaded::Corrupt("unreadable");
    }
    if buf.len() > MAX_CURSOR_BYTES {
        return Loaded::Corrupt("oversized");
    }
    let text = match String::from_utf8(buf) {
        Ok(t) => t,
        Err(_) => return Loaded::Corrupt("not utf-8"),
    };
    match validate(&text) {
        Ok(c) => Loaded::Cursor(c),
        Err(why) => Loaded::Corrupt(why),
    }
}

/// Persist the cursor. Best-effort: a write failure is logged, and the
/// in-memory cursor still prevents same-process replay.
pub(crate) fn save_to(path: &Path, cursor: &str) {
    if let Err(e) = write_atomic(path, cursor) {
        tracing::warn!(
            target: "wcore_channel_matrix::sync",
            error = %e,
            "could not persist matrix /sync cursor; a restart will re-seed and lose the downtime window",
        );
    }
}

/// Write via temp-file + rename so an interrupted write cannot leave a torn
/// cursor behind. `load_from` degrades safely on a torn file either way, but
/// degrading safely still costs the downtime window — so do not produce one.
fn write_atomic(path: &Path, cursor: &str) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let tmp = path.with_extension("since.tmp");
    std::fs::write(&tmp, cursor)?;
    std::fs::rename(&tmp, path)
}

/// Drop a cursor the homeserver has rejected, so the next start seeds cleanly
/// instead of re-presenting a token that can only fail.
pub(crate) fn clear_at(path: &Path) {
    let _ = std::fs::remove_file(path);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn unique_tmp() -> PathBuf {
        // Per-call unique path; a monotonic counter, not a pointer to a
        // zero-sized temporary (which is one constant address for every call),
        // so parallel tests in this module cannot share a file.
        static SEQ: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let n = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        std::env::temp_dir().join(format!(
            "wcore-matrix-since-{}-{n}.since",
            std::process::id()
        ))
    }

    fn loaded_cursor(l: Loaded) -> Option<String> {
        match l {
            Loaded::Cursor(c) => Some(c),
            _ => None,
        }
    }

    fn corrupt_reason(l: Loaded) -> Option<&'static str> {
        match l {
            Loaded::Corrupt(why) => Some(why),
            _ => None,
        }
    }

    #[test]
    fn save_then_load_round_trips() {
        let p = unique_tmp();
        save_to(&p, "s72595_4483_1934");
        assert_eq!(
            loaded_cursor(load_from(&p)).as_deref(),
            Some("s72595_4483_1934")
        );
        let _ = std::fs::remove_file(&p);
    }

    /// A missing file is `Absent`, NOT `Corrupt` — first run must not warn.
    #[test]
    fn missing_file_is_absent_not_corrupt() {
        let p = unique_tmp();
        let _ = std::fs::remove_file(&p);
        assert!(matches!(load_from(&p), Loaded::Absent));
    }

    /// Every corrupt shape must be classified as corrupt rather than silently
    /// becoming `Absent`. `Absent` restarts from now WITHOUT telling anyone;
    /// that is the failure mode this three-state read exists to prevent.
    #[test]
    fn corrupt_shapes_are_classified_corrupt_with_a_reason() {
        for (bytes, want) in [
            (b"".to_vec(), "empty"),
            (b"   \n\t ".to_vec(), "empty"),
            (
                b"s1 s2".to_vec(),
                "contains control or whitespace characters",
            ),
            (
                b"s1\x00trailing".to_vec(),
                "contains control or whitespace characters",
            ),
            (vec![b'x'; MAX_CURSOR_BYTES + 1], "oversized"),
            (vec![0xff, 0xfe, 0xfd], "not utf-8"),
        ] {
            let p = unique_tmp();
            std::fs::write(&p, &bytes).unwrap();
            assert_eq!(
                corrupt_reason(load_from(&p)),
                Some(want),
                "bytes {bytes:?} must be Corrupt({want})",
            );
            let _ = std::fs::remove_file(&p);
        }
    }

    /// A trailing newline is what an operator's editor adds; it must NOT be
    /// treated as corruption, and must not reach the query string.
    #[test]
    fn trailing_newline_is_trimmed_not_rejected() {
        let p = unique_tmp();
        std::fs::write(&p, "s_abc\n").unwrap();
        assert_eq!(loaded_cursor(load_from(&p)).as_deref(), Some("s_abc"));
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn clear_at_removes_the_cursor_and_is_idempotent() {
        let p = unique_tmp();
        save_to(&p, "s_stale");
        clear_at(&p);
        assert!(matches!(load_from(&p), Loaded::Absent));
        clear_at(&p); // second call on a missing file must not panic
    }

    #[test]
    fn state_path_is_stable_and_account_specific() {
        let a = state_path("https://matrix.org", "@bot:matrix.org", "acme");
        let a2 = state_path("https://matrix.org", "@bot:matrix.org", "acme");
        assert_eq!(a, a2, "same channel must map to the same file");
        assert_ne!(
            a,
            state_path("https://other.org", "@bot:matrix.org", "acme"),
            "a different homeserver must not reuse a cursor",
        );
        assert_ne!(
            a,
            state_path("https://matrix.org", "@other:matrix.org", "acme"),
            "a different bot user must not collide",
        );
        assert_ne!(
            a,
            state_path("https://matrix.org", "@bot:matrix.org", "other"),
            "a different channel must not collide",
        );
    }

    /// The temp file the atomic write goes through must not be left behind,
    /// and must not itself be picked up as a cursor file.
    #[test]
    fn atomic_write_leaves_no_temp_file() {
        let p = unique_tmp();
        save_to(&p, "s_final");
        assert!(!p.with_extension("since.tmp").exists());
        assert_eq!(loaded_cursor(load_from(&p)).as_deref(), Some("s_final"));
        let _ = std::fs::remove_file(&p);
    }
}
