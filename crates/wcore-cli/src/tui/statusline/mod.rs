//! User-defined statusLine shell command (SPEC §6, Q11) + the shared
//! last-good cache the status bar reads.
//!
//! ## Architecture: OFF the render thread (audit BLOCKER)
//!
//! `Surface::render` is SYNCHRONOUS and runs while holding the `App` mutex.
//! Forking/awaiting a user command there would freeze every frame AND
//! starve the engine-bridge task. So the command is NEVER run on the render
//! path. Instead — mirroring `widgets/header.rs`'s `SystemSampler` — a
//! background async task ([`exec::spawn_statusline_sampler`]) owns the
//! command, runs it at most once per debounce window (≥1 s) with a hard
//! 500 ms timeout, and publishes a sanitized one-line result into a shared
//! [`StatusLineCache`]. The status bar only READS that cached string as
//! plain data ([`cached_line`]); it never spawns a process.
//!
//! The cache is a process-global ([`CACHE`]): there is exactly one status
//! bar and one sampler per process, and `widgets::status_bar` is a free
//! function with a frozen signature (no `App` field, no extra parameter
//! plumbed through `surfaces::render`). A global keeps the entire read/
//! publish path inside this module.
//!
//! ## SECURITY / trust boundary (SPEC §6)
//!
//! `statusLine.command` is SETTINGS-FILE-ONLY. The model CANNOT set it:
//! there is no protocol command, no slash command, and no tool that writes
//! it. The trust boundary is "the user trusts their own status command" —
//! it runs with the user's full environment and shell. The executor
//! defends the chrome regardless: hard timeout, stdout cap, one-line
//! truncation, and ANSI/OSC sanitization (see [`exec`] +
//! [`sanitize_status_output`]) so a command can never inject escape
//! sequences, move the cursor, or hang the UI.

mod exec;

pub use exec::spawn_statusline_sampler;

use std::sync::{Arc, Mutex, OnceLock};
use std::time::Instant;

/// Settings-file-only config (Q3+Q11 coupled bucket). NEVER writable by the
/// model — see the module-level security note. Lives here (not in the
/// shared engine `Config`) so the TUI owns its own statusLine plumbing.
#[derive(Debug, Clone, Default)]
pub struct StatusLineConfig {
    /// The user's shell command, or `None` for the curated default bar.
    pub command: Option<String>,
}

/// Shared last-good cache the sampler publishes into and the status bar
/// reads. `line` is already one-line + ANSI/OSC-sanitized by the sampler.
#[derive(Debug, Default)]
pub struct StatusLineCache {
    /// The most recent sanitized one-line output, or `None` before the
    /// first successful run.
    pub line: Option<String>,
    /// When `line` was last refreshed. `None` until the first publish.
    pub updated_at: Option<Instant>,
}

/// The process-global cache. Initialized lazily by both the sampler (on
/// spawn) and [`cached_line`] (on read) so neither ordering matters.
static CACHE: OnceLock<Arc<Mutex<StatusLineCache>>> = OnceLock::new();

/// Handle to the shared cache, creating it on first access.
pub fn cache() -> Arc<Mutex<StatusLineCache>> {
    CACHE
        .get_or_init(|| Arc::new(Mutex::new(StatusLineCache::default())))
        .clone()
}

/// Read the cached statusLine string for the status bar. `None` when no
/// command has produced output yet (the curated default renders instead).
/// A poisoned lock degrades to `None` rather than panicking the renderer.
pub fn cached_line() -> Option<String> {
    let cache = cache();
    let guard = cache.lock().ok()?;
    guard.line.clone()
}

/// Initialize the statusLine subsystem from config: when a command is set,
/// spawn the off-thread background sampler. A no-op when `command` is
/// `None` (the curated default renders). Call once from the TUI run-loop
/// setup, inside the tokio runtime.
pub fn init(config: &StatusLineConfig) {
    if let Some(command) = config.command.clone() {
        spawn_statusline_sampler(command, cache());
    }
}

/// Build the JSON contract handed to the user command on stdin
/// (reimplements CC's `buildStatusLineCommandInput`). Includes a
/// `contract_version` so a command can branch on schema changes (carried
/// audit-LOW).
#[allow(clippy::too_many_arguments)]
pub fn build_contract_json(
    session_name: &str,
    model_id: &str,
    model_display: &str,
    current_dir: &str,
    project_dir: &str,
    version: &str,
    cost_usd: f64,
    duration_ms: u64,
    used_pct: f64,
    window_size: u64,
) -> String {
    serde_json::json!({
        "contract_version": 1,
        "session_name": session_name,
        "model": { "id": model_id, "display_name": model_display },
        "workspace": { "current_dir": current_dir, "project_dir": project_dir },
        "version": version,
        "cost": { "total_cost_usd": cost_usd, "total_duration_ms": duration_ms },
        "context_window": {
            "used_pct": used_pct,
            "remaining_pct": 100.0 - used_pct,
            "context_window_size": window_size
        },
        "exceeds_200k_tokens": window_size > 200_000,
        "rate_limits": {
            "five_hour": serde_json::Value::Null,
            "seven_day": serde_json::Value::Null
        }
    })
    .to_string()
}

/// Strip ANSI/OSC/control sequences and clamp to one line so a user
/// command cannot inject escapes or move the cursor. Keeps only the first
/// line and drops every control char (incl. ESC `\x1b`, CSI `\x9b`, BEL
/// `\x07`, and any other C0/C1 control).
pub fn sanitize_status_output(raw: &str) -> String {
    let first_line = raw.lines().next().unwrap_or("");
    first_line
        .chars()
        .filter(|c| *c != '\x1b' && *c != '\u{9b}' && *c != '\x07' && !c.is_control())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn contract_json_includes_contract_version_and_no_rate_limits() {
        let s = build_contract_json("s", "m", "M", "/d", "/p", "0.9.2", 0.02, 123, 47.0, 200_000);
        assert!(
            s.contains("\"contract_version\":1"),
            "contract_version missing: {s}"
        );
        assert!(s.contains("\"used_pct\":47"), "used_pct missing: {s}");
        // 200_000 is NOT > 200_000 → exceeds flag is false.
        assert!(
            s.contains("\"exceeds_200k_tokens\":false"),
            "exceeds flag wrong: {s}"
        );
        assert!(s.contains("\"five_hour\":null"), "rate_limits missing: {s}");
    }

    #[test]
    fn contract_json_sets_exceeds_flag_above_200k() {
        let s = build_contract_json("s", "m", "M", "/d", "/p", "0.9.2", 0.0, 0, 10.0, 500_000);
        assert!(
            s.contains("\"exceeds_200k_tokens\":true"),
            "exceeds flag should be true: {s}"
        );
    }

    #[test]
    fn sanitize_strips_escapes_and_keeps_one_line() {
        // The ESC (\x1b) and BEL (\x07) control bytes are removed and only
        // the first line survives; the printable `[31m` SGR args that
        // followed the ESC remain (we strip control bytes, not full CSI
        // grammar — the goal is to neutralize cursor/escape control).
        let dirty = "ok\x1b[31mred\x07\nsecond line";
        assert_eq!(sanitize_status_output(dirty), "ok[31mred");
    }

    #[test]
    fn sanitize_drops_csi_and_bel_and_tabs() {
        // CSI (\u{9b}), BEL (\x07), and tab (a control char) are stripped;
        // the printable bytes that followed the CSI survive (we strip the
        // control byte, not a full escape grammar — sanitization is about
        // neutralizing cursor/escape control, not parsing CSI args).
        let dirty = "a\u{9b}b\x07\tc";
        assert_eq!(sanitize_status_output(dirty), "abc");
    }

    #[test]
    fn sanitize_handles_empty_and_blank() {
        assert_eq!(sanitize_status_output(""), "");
        assert_eq!(sanitize_status_output("\n\nlater"), "");
    }

    // NOTE: the process-global `CACHE` is deliberately NOT mutated in unit
    // tests — `cargo test` runs them in parallel threads, so a shared
    // global would race with `widgets::statusbar`'s curated-default test
    // (which asserts `cached_line() == None`). The cache-publish + timeout
    // + last-good behavior is instead tested in `exec.rs` against a LOCAL
    // `Arc<Mutex<StatusLineCache>>`, and the real timeout is verified in
    // live-smoke. See `exec::tests`.
}
