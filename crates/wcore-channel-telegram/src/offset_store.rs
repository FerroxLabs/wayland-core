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
        std::env::temp_dir().join(format!(
            "wcore-telegram-offset-{}-{:p}.offset",
            std::process::id(),
            &() as *const ()
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

    #[test]
    fn state_path_is_stable_and_channel_specific() {
        let a = state_path("telegram-main");
        let a2 = state_path("telegram-main");
        let b = state_path("telegram-alt");
        assert_eq!(a, a2, "same channel must map to the same file");
        assert_ne!(a, b, "different channels must not collide");
    }
}
