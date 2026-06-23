//! T3-3.1.8 — Per-tool opt-in debug session (ported from the prior
//! Wayland Python engine).
//!
//! This is intentionally narrow: a developer-debug shim that lets a
//! specific tool (e.g. a future web_tools, vision_tools, or MOA port)
//! dump its in-process call sequence to a JSON file when an env var is
//! set. It is **not** a replacement for the `tracing` crate or
//! `wcore-observability` — those record structured spans/traces for the
//! whole agent loop. `DebugSession` exists for tool authors who want a
//! cheap, opt-in, file-per-session dump while debugging tool-specific
//! issues outside the normal trace pipeline (which strips/sanitizes data
//! that may matter when reproducing a flaky tool path).
//!
//! Disabled-by-default discipline: every method is a no-op unless the
//! configured env var is set to `"true"` (case-insensitive), matching
//! the predecessor's semantics. Construction is cheap (one `env::var` lookup,
//! no allocation when disabled past the empty `Vec`).
//!
//! Log files land in `wayland_config_dir()/logs/` (e.g.
//! `~/Library/Application Support/wayland-core/logs/` on macOS, or
//! `$WAYLAND_HOME/logs/` in sandboxed/hermetic runs). Resolved via
//! `wcore_config::config::wayland_config_dir()` so `WAYLAND_HOME` traps
//! the log path alongside the rest of the engine's state (F-010, #270).
//! We never write to `/tmp` or CWD as a fallback.

use std::path::PathBuf;
use std::sync::Mutex;

use chrono::Utc;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use uuid::Uuid;

/// Resolve the directory where debug session logs are written.
///
/// Routes through `wcore_config::config::wayland_config_dir()` so
/// `WAYLAND_HOME` hermetically sandboxes debug logs alongside the rest of
/// the engine's on-disk state (F-010, #270). Returns `Some` in all cases
/// since the canonical helper has a `PathBuf::from("wayland-core")`
/// fallback; we keep the `Option` signature so callers can opt out
/// uniformly if a future variant of the helper goes back to `Option`.
fn default_log_dir() -> Option<PathBuf> {
    Some(wcore_config::config::wayland_config_dir().join("logs"))
}

/// One recorded tool-call entry. Mirrors the predecessor's JSON shape:
/// `{ "timestamp": ..., "tool_name": ..., ...call_data }`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DebugCall {
    /// RFC3339 timestamp captured at `log_call()` time.
    pub timestamp: String,
    /// Logical name of the call (e.g. `"web_search"`).
    pub tool_name: String,
    /// Arbitrary structured payload supplied by the caller.
    #[serde(flatten)]
    pub data: Value,
}

/// Summary returned by `DebugSession::session_info()`. Matches the
/// shape the predecessor's `get_debug_session_info()` helper exposed so
/// the downstream protocol surface stays compatible if/when those tools
/// land in Rust.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DebugSessionInfo {
    pub enabled: bool,
    pub session_id: Option<String>,
    pub log_path: Option<String>,
    pub total_calls: usize,
}

/// Per-tool debug session. Cheap when disabled.
///
/// ```ignore
/// use wcore_tools::debug_helpers::DebugSession;
/// use serde_json::json;
///
/// let dbg = DebugSession::new("web_tools", "WEB_TOOLS_DEBUG");
/// dbg.log_call("web_search", json!({"query": "rust", "hits": 10}));
/// dbg.save(); // no-op unless WEB_TOOLS_DEBUG=true
/// ```
pub struct DebugSession {
    tool_name: String,
    enabled: bool,
    session_id: String,
    log_dir: Option<PathBuf>,
    start_time: String,
    calls: Mutex<Vec<DebugCall>>,
}

impl DebugSession {
    /// Create a session that activates when `env_var` is set to
    /// `"true"` (case-insensitive). Disabled sessions never touch the
    /// filesystem and never allocate a UUID — they only carry the
    /// empty call buffer.
    pub fn new(tool_name: impl Into<String>, env_var: &str) -> Self {
        let tool_name = tool_name.into();
        let enabled = std::env::var(env_var)
            .map(|v| v.eq_ignore_ascii_case("true"))
            .unwrap_or(false);

        let (session_id, start_time, log_dir) = if enabled {
            let dir = default_log_dir();
            if let Some(d) = &dir {
                if let Err(e) = std::fs::create_dir_all(d) {
                    tracing::warn!(
                        target: "wcore_tools::debug_helpers",
                        tool = %tool_name,
                        error = %e,
                        "failed to create debug log dir; session will be active but save() will fail",
                    );
                }
            } else {
                tracing::warn!(
                    target: "wcore_tools::debug_helpers",
                    tool = %tool_name,
                    "no platform config dir; debug session will not be able to save logs",
                );
            }
            let id = Uuid::new_v4().to_string();
            tracing::debug!(
                target: "wcore_tools::debug_helpers",
                tool = %tool_name,
                session_id = %id,
                "debug session enabled",
            );
            (id, Utc::now().to_rfc3339(), dir)
        } else {
            (String::new(), String::new(), None)
        };

        Self {
            tool_name,
            enabled,
            session_id,
            log_dir,
            start_time,
            calls: Mutex::new(Vec::new()),
        }
    }

    /// Whether this session is recording.
    pub fn is_active(&self) -> bool {
        self.enabled
    }

    /// Record one call. No-op when disabled.
    pub fn log_call(&self, call_name: impl Into<String>, call_data: Value) {
        if !self.enabled {
            return;
        }
        let call = DebugCall {
            timestamp: Utc::now().to_rfc3339(),
            tool_name: call_name.into(),
            data: call_data,
        };
        // Lock poisoning is non-fatal — we drop the entry rather than
        // panic during a debug-only path.
        if let Ok(mut buf) = self.calls.lock() {
            buf.push(call);
        }
    }

    /// Path the debug log will be written to (if/when `save()` runs).
    pub fn log_path(&self) -> Option<PathBuf> {
        if !self.enabled {
            return None;
        }
        self.log_dir
            .as_ref()
            .map(|d| d.join(format!("{}_debug_{}.json", self.tool_name, self.session_id)))
    }

    /// Flush the recorded calls to disk. No-op when disabled. Errors
    /// during write are logged at `WARN` and swallowed — debug
    /// instrumentation must never crash a tool.
    pub fn save(&self) {
        if !self.enabled {
            return;
        }
        let Some(path) = self.log_path() else {
            tracing::warn!(
                target: "wcore_tools::debug_helpers",
                tool = %self.tool_name,
                "save(): no log path resolved; skipping",
            );
            return;
        };

        let calls_snapshot: Vec<DebugCall> = match self.calls.lock() {
            Ok(g) => g.clone(),
            Err(e) => {
                tracing::warn!(
                    target: "wcore_tools::debug_helpers",
                    tool = %self.tool_name,
                    error = %e,
                    "save(): call buffer mutex poisoned; skipping",
                );
                return;
            }
        };

        let payload = json!({
            "session_id": self.session_id,
            "start_time": self.start_time,
            "end_time": Utc::now().to_rfc3339(),
            "debug_enabled": true,
            "total_calls": calls_snapshot.len(),
            "tool_calls": calls_snapshot,
        });

        let serialized = match serde_json::to_vec_pretty(&payload) {
            Ok(b) => b,
            Err(e) => {
                tracing::warn!(
                    target: "wcore_tools::debug_helpers",
                    tool = %self.tool_name,
                    error = %e,
                    "save(): failed to serialize debug payload",
                );
                return;
            }
        };

        if let Err(e) = std::fs::write(&path, &serialized) {
            tracing::warn!(
                target: "wcore_tools::debug_helpers",
                tool = %self.tool_name,
                path = %path.display(),
                error = %e,
                "save(): failed to write debug log",
            );
        } else {
            tracing::debug!(
                target: "wcore_tools::debug_helpers",
                tool = %self.tool_name,
                path = %path.display(),
                "debug log saved",
            );
        }
    }

    /// Summary suitable for protocol surfacing or test introspection.
    pub fn session_info(&self) -> DebugSessionInfo {
        if !self.enabled {
            return DebugSessionInfo {
                enabled: false,
                session_id: None,
                log_path: None,
                total_calls: 0,
            };
        }
        let total_calls = self.calls.lock().map(|g| g.len()).unwrap_or(0);
        DebugSessionInfo {
            enabled: true,
            session_id: Some(self.session_id.clone()),
            log_path: self.log_path().map(|p| p.display().to_string()),
            total_calls,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serial_test::serial;

    /// Module is reachable + disabled-by-default behavior is a true
    /// no-op (no UUID minted, no log path, no calls recorded even when
    /// `log_call` is invoked).
    #[test]
    fn disabled_session_is_inert() {
        // Use a uniquely-named env var so concurrent tests can't flip it.
        let var = "WCORE_DEBUG_HELPERS_TEST_DISABLED";
        // Belt-and-suspenders: ensure unset before constructing.
        // SAFETY: tests in this module use unique env-var names so
        // concurrent test threads do not race on the same key.
        unsafe {
            std::env::remove_var(var);
        }

        let s = DebugSession::new("unit_test_disabled", var);
        assert!(!s.is_active());
        s.log_call("noop_call", json!({"k": "v"}));
        s.save(); // must not panic / must not create files

        let info = s.session_info();
        assert!(!info.enabled);
        assert_eq!(info.total_calls, 0);
        assert!(info.session_id.is_none());
        assert!(info.log_path.is_none());
    }

    /// Happy path: enabled session records calls and counts them.
    #[test]
    #[serial(env)]
    fn enabled_session_records_calls() {
        let var = "WCORE_DEBUG_HELPERS_TEST_ENABLED";
        // SAFETY: see disabled_session_is_inert — unique env var.
        unsafe {
            std::env::set_var(var, "TRUE"); // case-insensitive
        }

        let s = DebugSession::new("unit_test_enabled", var);
        assert!(s.is_active());

        s.log_call("op_a", json!({"x": 1}));
        s.log_call("op_b", json!({"y": "two"}));

        let info = s.session_info();
        assert!(info.enabled);
        assert_eq!(info.total_calls, 2);
        let sid = info.session_id.expect("session id present when enabled");
        // UUID v4 string is 36 chars (8-4-4-4-12).
        assert_eq!(sid.len(), 36);

        // SAFETY: unique env-var name; no other tests read it.
        unsafe {
            std::env::remove_var(var);
        }
    }

    /// Edge: explicit log-dir injection bypasses the platform-config
    /// dir so we can verify save() actually writes a well-formed JSON
    /// payload without polluting the user's real config dir. We poke
    /// the internal `log_dir` after construction — that's the same
    /// fixture pattern other wcore-tools tests use.
    #[test]
    #[serial(env)]
    fn save_writes_well_formed_json_with_custom_dir() {
        let var = "WCORE_DEBUG_HELPERS_TEST_SAVE";
        // SAFETY: see disabled_session_is_inert — unique env var.
        unsafe {
            std::env::set_var(var, "true");
        }

        let tmp = tempfile::tempdir().expect("tempdir");
        let mut s = DebugSession::new("unit_test_save", var);
        // Re-point log_dir at the tempdir for hermetic test.
        s.log_dir = Some(tmp.path().to_path_buf());

        s.log_call("op_x", json!({"hits": 7}));
        s.save();

        let path = s.log_path().expect("enabled session has log path");
        assert!(
            path.exists(),
            "save() should have created the log file at {path:?}"
        );

        let raw = std::fs::read_to_string(&path).expect("read back log");
        let parsed: Value = serde_json::from_str(&raw).expect("log is valid JSON");

        assert_eq!(parsed["debug_enabled"], Value::Bool(true));
        assert_eq!(parsed["total_calls"], Value::from(1));
        assert!(parsed["session_id"].as_str().is_some());
        assert!(parsed["start_time"].as_str().is_some());
        assert!(parsed["end_time"].as_str().is_some());
        let calls = parsed["tool_calls"].as_array().expect("tool_calls array");
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0]["tool_name"], Value::from("op_x"));
        assert_eq!(calls[0]["hits"], Value::from(7));

        // SAFETY: unique env-var name.
        unsafe {
            std::env::remove_var(var);
        }
    }
}
