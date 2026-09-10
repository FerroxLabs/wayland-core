//! Persistent Telegram `getUpdates` offset watermark.
//!
//! The in-memory `offset` alone resets to `0` across a restart. Telegram retains
//! unconfirmed updates for ~24h and re-delivers them on the next `getUpdates`
//! that does not advance the offset — so a restart re-delivers the final
//! unconfirmed batch (up to 100 updates) as duplicate agent turns.
//!
//! This module persists the last-confirmed offset per channel name under the
//! profile home (`$WAYLAND_HOME/channel-state/`) so a restart resumes exactly
//! where it left off. Writes are best-effort: a failure is logged and the
//! in-session in-memory offset still prevents same-process re-delivery.

use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};

/// Deterministic per-channel state-file path. Uses `DefaultHasher` (fixed keys,
/// stable across processes) over the channel name so the same channel always
/// maps to the same file without leaking the name into the filename.
fn state_path(channel_name: &str) -> PathBuf {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    channel_name.hash(&mut h);
    let key = h.finish();
    wcore_config::config::wayland_config_dir()
        .join("channel-state")
        .join(format!("telegram-{key:016x}.offset"))
}

/// Load the persisted offset for this channel, if any.
pub(crate) fn load(channel_name: &str) -> Option<i64> {
    load_from(&state_path(channel_name))
}

fn load_from(path: &Path) -> Option<i64> {
    std::fs::read_to_string(path)
        .ok()?
        .trim()
        .parse::<i64>()
        .ok()
}

/// Persist the offset. Best-effort; a write failure is logged only.
pub(crate) fn save(channel_name: &str, offset: i64) {
    if let Err(e) = save_to(&state_path(channel_name), offset) {
        tracing::warn!(
            target: "wcore_channel_telegram::longpoll",
            error = %e,
            "could not persist telegram update offset; restart may re-deliver",
        );
    }
}

fn save_to(path: &Path, offset: i64) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, offset.to_string())
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
        std::env::temp_dir().join(format!(
            "wcore-telegram-offset-{}-{n}.offset",
            std::process::id()
        ))
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
        save_to(&p, 987654).unwrap();
        assert_eq!(load_from(&p), Some(987654));
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn load_from_garbage_is_none() {
        let p = unique_tmp();
        std::fs::write(&p, "not a number").unwrap();
        assert_eq!(load_from(&p), None);
        let _ = std::fs::remove_file(&p);
    }

    /// GRADED ON THE NAME, NOT THE WHOLE PATH, and that is the point of this
    /// comment rather than a style preference.
    ///
    /// `state_path` roots itself at `wayland_config_dir()`, which reads the
    /// `WAYLAND_HOME` PROCESS GLOBAL. `lib.rs::cfg` in this same crate writes
    /// that global from a `Once`, and its SAFETY note assumed
    /// "process-per-test under nextest; no other thread reads the env". That
    /// assumption does not hold in the shared-process leg, which runs
    /// `cargo test --workspace --lib` — one process for all 79 tests in this
    /// binary, threads in parallel. Comparing whole paths therefore graded
    /// WHERE THE PROFILE HOME HAPPENED TO POINT between two calls, not this
    /// module's hashing.
    ///
    /// It fired: CI run 34438211271, leg `CI (linux-containerized)`, step
    /// "Shared-process lib suite" — FAILED at offset_store.rs:103 on
    /// `same channel must map to the same file`, while the SAME test PASSED in
    /// the nextest leg of the same run (position 5267/18053). That difference
    /// is the whole reason the shared-process leg exists.
    /// `.config/env-global-helper-debt.txt:67` had already named this exact
    /// pair, dated, under gh#1233.
    ///
    /// What the module actually promises, per its own docstring, is that the
    /// FILE NAME is a deterministic function of the channel name. That is what
    /// is asserted here, and it is true under any profile home. The parent is
    /// asserted by its own name (`channel-state`) rather than by a second
    /// reading of the global, so nothing here can be decided by another
    /// thread's env mutation.
    #[test]
    fn state_path_is_stable_and_channel_specific() {
        let a = state_path("telegram-main");
        let a2 = state_path("telegram-main");
        let b = state_path("telegram-alt");
        assert_eq!(
            a.file_name(),
            a2.file_name(),
            "same channel must map to the same file"
        );
        assert_ne!(
            a.file_name(),
            b.file_name(),
            "different channels must not collide"
        );
        assert_eq!(
            a.parent().and_then(|p| p.file_name()),
            Some(std::ffi::OsStr::new("channel-state")),
            "state files must live under the profile's channel-state directory"
        );
        // Non-vacuity: a hashed name is 16 hex digits plus the fixed affixes,
        // so an empty or missing file_name would satisfy the equality above.
        let name = a.file_name().unwrap().to_string_lossy().into_owned();
        assert!(
            name.starts_with("telegram-") && name.ends_with(".offset") && name.len() == 25,
            "expected telegram-<16 hex>.offset, got {name}"
        );
    }
}
