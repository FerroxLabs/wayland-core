//! Tirith pre-exec security scanning wrapper.
//!
//! Runs the external `tirith` binary as a subprocess to scan shell commands
//! for content-level threats (homograph URLs, pipe-to-interpreter, terminal
//! injection, etc.).
//!
//! Exit code is the verdict source of truth:
//!   * `0` = allow
//!   * `1` = block
//!   * `2` = warn
//!
//! JSON stdout enriches `findings` / `summary` but never overrides the verdict.
//! Operational failures (spawn error, timeout, unknown exit code) respect the
//! `fail_open` config setting.
//!
//! # Port divergence vs the prior Wayland Python engine
//!
//! The prior Python source additionally contains an
//! **auto-installer** that downloads the `tirith` binary from GitHub releases,
//! verifies it with cosign provenance + SHA-256 checksums, extracts a tarball,
//! and persists a 24h failure-marker on disk. That heavy supply-chain logic
//! (cosign subprocess, GitHub HTTPS download, gzip tar extraction, cross-device
//! move retry) is **deliberately out of scope** for this helper port — it is
//! tirith-specific bootstrap and belongs in a separate installer module if
//! needed. This module only resolves an already-present binary via PATH or
//! `$WAYLAND_HOME/bin/tirith`, and surfaces the [`check_command_security`]
//! main API.

use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::process::Command;
use tokio::time::timeout;
use tracing::warn;

use wcore_config::shell::shell_command_argv;

/// Default failure marker TTL: 24 hours.
const MARKER_TTL_SECS: u64 = 86_400;

/// Maximum findings retained in a result.
pub const MAX_FINDINGS: usize = 50;

/// Maximum length of the summary string.
pub const MAX_SUMMARY_LEN: usize = 500;

// ---------------------------------------------------------------------------
// Config
// ---------------------------------------------------------------------------

/// Security configuration for the tirith scanner.
///
/// Mirrors the prior Python engine's `_load_security_config()` shape. In production callers
/// should populate this from their config layer; the [`SecurityConfig::from_env`]
/// helper honours the `TIRITH_*` environment overrides the Python tool used.
#[derive(Debug, Clone)]
pub struct SecurityConfig {
    /// When false, every command is allowed without invoking tirith.
    pub tirith_enabled: bool,
    /// Path or bare name of the tirith binary. The default is `"tirith"`.
    pub tirith_path: String,
    /// Subprocess timeout in seconds.
    pub tirith_timeout_secs: u64,
    /// When true, spawn/timeout/unknown-exit failures degrade to `allow`.
    pub tirith_fail_open: bool,
}

impl Default for SecurityConfig {
    fn default() -> Self {
        Self {
            tirith_enabled: true,
            tirith_path: "tirith".to_string(),
            tirith_timeout_secs: 5,
            // v0.6.2 cross-audit Round 1: default flipped from `true` to `false`.
            // Previously, missing the optional `tirith` binary silently allowed
            // every command, defeating the guard whenever the binary wasn't
            // installed (the common case). Operators who need the old behavior
            // can set TIRITH_FAIL_OPEN=true.
            tirith_fail_open: false,
        }
    }
}

impl SecurityConfig {
    /// Construct a config with environment variable overrides applied.
    ///
    /// Recognised vars (match the prior Python engine):
    /// * `TIRITH_ENABLED`   (bool: `1`/`true`/`yes` enables)
    /// * `TIRITH_BIN`       (string path)
    /// * `TIRITH_TIMEOUT`   (integer seconds)
    /// * `TIRITH_FAIL_OPEN` (bool)
    pub fn from_env() -> Self {
        let defaults = Self::default();
        Self {
            tirith_enabled: env_bool("TIRITH_ENABLED", defaults.tirith_enabled),
            tirith_path: env::var("TIRITH_BIN").unwrap_or(defaults.tirith_path),
            tirith_timeout_secs: env_u64("TIRITH_TIMEOUT", defaults.tirith_timeout_secs),
            tirith_fail_open: env_bool("TIRITH_FAIL_OPEN", defaults.tirith_fail_open),
        }
    }
}

fn env_bool(key: &str, default: bool) -> bool {
    match env::var(key) {
        Ok(v) => matches!(v.to_lowercase().as_str(), "1" | "true" | "yes"),
        Err(_) => default,
    }
}

fn env_u64(key: &str, default: u64) -> u64 {
    match env::var(key) {
        Ok(v) => v.parse().unwrap_or(default),
        Err(_) => default,
    }
}

// ---------------------------------------------------------------------------
// Verdict / result
// ---------------------------------------------------------------------------

/// Action produced by a tirith scan.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Action {
    Allow,
    Warn,
    Block,
}

impl Action {
    /// Map a tirith exit code to the corresponding action.
    ///
    /// Returns `None` for unknown exit codes (caller decides fail-open vs
    /// fail-closed).
    pub fn from_exit_code(code: i32) -> Option<Self> {
        match code {
            0 => Some(Action::Allow),
            1 => Some(Action::Block),
            2 => Some(Action::Warn),
            _ => None,
        }
    }
}

/// Result of [`check_command_security`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SecurityResult {
    pub action: Action,
    pub findings: Vec<Value>,
    pub summary: String,
}

impl SecurityResult {
    fn allow(summary: impl Into<String>) -> Self {
        Self {
            action: Action::Allow,
            findings: Vec::new(),
            summary: summary.into(),
        }
    }

    fn block(summary: impl Into<String>) -> Self {
        Self {
            action: Action::Block,
            findings: Vec::new(),
            summary: summary.into(),
        }
    }
}

// ---------------------------------------------------------------------------
// Failure marker (disk-persistent)
// ---------------------------------------------------------------------------

/// Return the `$WAYLAND_HOME` directory.
///
/// Honours the `WAYLAND_HOME` env var; falls back to `~/.wayland` (matching
/// the prior Python engine's `wayland_constants.get_wayland_home`).
pub fn wayland_home() -> PathBuf {
    if let Ok(v) = env::var("WAYLAND_HOME") {
        return PathBuf::from(v);
    }
    if let Some(home) = dirs::home_dir() {
        home.join(".wayland")
    } else {
        PathBuf::from(".wayland")
    }
}

/// Path to the install-failure marker file.
pub fn failure_marker_path() -> PathBuf {
    wayland_home().join(".tirith-install-failed")
}

/// Read the failure reason from the marker, or `None` if missing/expired.
pub fn read_failure_reason() -> Option<String> {
    read_failure_reason_at(&failure_marker_path(), MARKER_TTL_SECS)
}

fn read_failure_reason_at(path: &Path, ttl_secs: u64) -> Option<String> {
    let meta = fs::metadata(path).ok()?;
    let mtime = meta.modified().ok()?;
    let age = SystemTime::now().duration_since(mtime).ok()?;
    if age >= Duration::from_secs(ttl_secs) {
        return None;
    }
    fs::read_to_string(path).ok().map(|s| s.trim().to_string())
}

/// Persist an install-failure marker to disk.
pub fn mark_install_failed(reason: &str) {
    let p = failure_marker_path();
    if let Some(parent) = p.parent() {
        let _ = fs::create_dir_all(parent);
    }
    let _ = fs::write(&p, reason);
}

/// Remove the install-failure marker.
pub fn clear_install_failed() {
    let _ = fs::remove_file(failure_marker_path());
}

// ---------------------------------------------------------------------------
// Path resolution
// ---------------------------------------------------------------------------

/// True if the user explicitly configured a non-default tirith path.
pub fn is_explicit_path(configured_path: &str) -> bool {
    configured_path != "tirith"
}

/// Expand a leading `~` to the user's home directory.
fn expand_user(path: &str) -> PathBuf {
    if let Some(rest) = path.strip_prefix("~/") {
        if let Some(home) = dirs::home_dir() {
            return home.join(rest);
        }
    } else if path == "~"
        && let Some(home) = dirs::home_dir()
    {
        return home;
    }
    PathBuf::from(path)
}

/// Search `PATH` for `name`, returning the first match that exists and is
/// executable (best-effort: existence on all platforms; the executable bit on
/// Unix).
fn which(name: &str) -> Option<PathBuf> {
    let path_var = env::var_os("PATH")?;
    for dir in env::split_paths(&path_var) {
        let candidate = dir.join(name);
        if is_executable(&candidate) {
            return Some(candidate);
        }
        #[cfg(windows)]
        {
            if let Ok(pathext) = env::var("PATHEXT") {
                for ext in pathext.split(';') {
                    let with_ext = candidate.with_extension(ext.trim_start_matches('.'));
                    if is_executable(&with_ext) {
                        return Some(with_ext);
                    }
                }
            }
        }
    }
    None
}

fn is_executable(path: &Path) -> bool {
    let Ok(meta) = fs::metadata(path) else {
        return false;
    };
    if !meta.is_file() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        meta.permissions().mode() & 0o111 != 0
    }
    #[cfg(not(unix))]
    {
        true
    }
}

/// Outcome of resolving a tirith binary path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PathResolution {
    /// Binary located at this path.
    Found(PathBuf),
    /// Could not locate a usable binary. The string is a short failure tag
    /// matching the prior Python engine's vocabulary (e.g. `"explicit_path_missing"`,
    /// `"not_on_path"`).
    Missing(String),
}

/// Resolve the tirith binary path using local checks only.
///
/// For an **explicit** path (anything other than the bare default `"tirith"`):
///   * Check the literal path
///   * Fall back to `PATH` lookup if it's a bare name
///
/// For the **default** `"tirith"`:
///   * `PATH` lookup
///   * `$WAYLAND_HOME/bin/tirith`
///
/// This intentionally does **NOT** trigger the network auto-installer that the
/// prior Python engine performs. See the module-level docs.
pub fn resolve_tirith_path(configured_path: &str) -> PathResolution {
    let expanded = expand_user(configured_path);
    let explicit = is_explicit_path(configured_path);

    if explicit {
        if is_executable(&expanded) {
            return PathResolution::Found(expanded);
        }
        // Bare name on PATH?
        if let Some(found) = expanded.to_str().and_then(which) {
            return PathResolution::Found(found);
        }
        return PathResolution::Missing("explicit_path_missing".to_string());
    }

    // Default lookup
    if let Some(found) = which("tirith") {
        return PathResolution::Found(found);
    }

    let wayland_bin = wayland_home().join("bin").join("tirith");
    if is_executable(&wayland_bin) {
        return PathResolution::Found(wayland_bin);
    }

    PathResolution::Missing("not_on_path".to_string())
}

// ---------------------------------------------------------------------------
// Main API
// ---------------------------------------------------------------------------

/// Run a tirith security scan on `command` using the supplied configuration.
///
/// Mirrors the prior Python engine's `check_command_security`. Returns a [`SecurityResult`]
/// whose `action` is determined by tirith's exit code; JSON stdout is parsed
/// for `findings` / `summary` enrichment but never overrides the verdict.
///
/// Operational failures (spawn error, timeout, unknown exit) honour
/// `cfg.tirith_fail_open`.
pub async fn check_command_security(command: &str, cfg: &SecurityConfig) -> SecurityResult {
    if !cfg.tirith_enabled {
        return SecurityResult::allow("");
    }

    let resolution = resolve_tirith_path(&cfg.tirith_path);
    let tirith_path: PathBuf = match resolution {
        PathResolution::Found(p) => p,
        PathResolution::Missing(reason) => {
            let summary = format!("tirith unavailable: {reason}");
            return if cfg.tirith_fail_open {
                warn!(reason = %reason, "tirith binary missing — failing open (TIRITH_FAIL_OPEN=true)");
                SecurityResult::allow(summary)
            } else {
                SecurityResult::block(format!("tirith spawn failed (fail-closed): {reason}"))
            };
        }
    };

    let tirith_str = tirith_path.to_string_lossy().to_string();
    let args = [
        "check",
        "--json",
        "--non-interactive",
        "--shell",
        "posix",
        "--",
        command,
    ];
    let mut cmd: Command = shell_command_argv(&tirith_str, &args);

    let timeout_dur = Duration::from_secs(cfg.tirith_timeout_secs);
    let exec = cmd.output();
    let result = match timeout(timeout_dur, exec).await {
        Ok(Ok(out)) => out,
        Ok(Err(e)) => {
            // Spawn / OS error
            let msg = format!("tirith unavailable: {e}");
            return if cfg.tirith_fail_open {
                warn!(error = %e, "tirith spawn failed — failing open (TIRITH_FAIL_OPEN=true)");
                SecurityResult::allow(msg)
            } else {
                SecurityResult::block(format!("tirith spawn failed (fail-closed): {e}"))
            };
        }
        Err(_elapsed) => {
            // Timeout
            let secs = cfg.tirith_timeout_secs;
            return if cfg.tirith_fail_open {
                warn!(
                    timeout_secs = secs,
                    "tirith timed out — failing open (TIRITH_FAIL_OPEN=true)"
                );
                SecurityResult::allow(format!("tirith timed out ({secs}s)"))
            } else {
                SecurityResult::block("tirith timed out (fail-closed)")
            };
        }
    };

    let exit_code = result.status.code().unwrap_or(-1);
    let action = match Action::from_exit_code(exit_code) {
        Some(a) => a,
        None => {
            return if cfg.tirith_fail_open {
                warn!(
                    exit_code = exit_code,
                    "tirith unknown exit code — failing open (TIRITH_FAIL_OPEN=true)"
                );
                SecurityResult::allow(format!("tirith exit code {exit_code} (fail-open)"))
            } else {
                SecurityResult::block(format!("tirith exit code {exit_code} (fail-closed)"))
            };
        }
    };

    enrich_with_json(action, &result.stdout)
}

/// Parse tirith JSON stdout into a [`SecurityResult`], applying the truncation
/// limits and degradation rules from the prior Python engine. The `action` parameter is the
/// authoritative verdict from the exit code; JSON never overrides it.
pub fn enrich_with_json(action: Action, stdout: &[u8]) -> SecurityResult {
    let text = std::str::from_utf8(stdout).unwrap_or("").trim();
    if text.is_empty() {
        return SecurityResult {
            action,
            findings: Vec::new(),
            summary: degraded_summary(action),
        };
    }

    let parsed: Result<Value, _> = serde_json::from_str(text);
    let data = match parsed {
        Ok(v) => v,
        Err(_) => {
            return SecurityResult {
                action,
                findings: Vec::new(),
                summary: degraded_summary(action),
            };
        }
    };

    let findings: Vec<Value> = data
        .get("findings")
        .and_then(|v| v.as_array())
        .map(|arr| arr.iter().take(MAX_FINDINGS).cloned().collect())
        .unwrap_or_default();

    let summary = data
        .get("summary")
        .and_then(|v| v.as_str())
        .map(|s| {
            if s.len() > MAX_SUMMARY_LEN {
                // Byte-truncate at a char boundary to avoid panicking on
                // multibyte characters at the split point.
                let mut end = MAX_SUMMARY_LEN;
                while !s.is_char_boundary(end) && end > 0 {
                    end -= 1;
                }
                s[..end].to_string()
            } else {
                s.to_string()
            }
        })
        .unwrap_or_default();

    SecurityResult {
        action,
        findings,
        summary,
    }
}

fn degraded_summary(action: Action) -> String {
    match action {
        Action::Block => "security issue detected (details unavailable)".to_string(),
        Action::Warn => "security warning detected (details unavailable)".to_string(),
        Action::Allow => String::new(),
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn action_from_exit_code_maps_known_verdicts() {
        assert_eq!(Action::from_exit_code(0), Some(Action::Allow));
        assert_eq!(Action::from_exit_code(1), Some(Action::Block));
        assert_eq!(Action::from_exit_code(2), Some(Action::Warn));
        assert_eq!(Action::from_exit_code(3), None);
        assert_eq!(Action::from_exit_code(-1), None);
    }

    #[test]
    fn enrich_with_json_truncates_findings_and_summary() {
        let mut findings = Vec::new();
        for i in 0..(MAX_FINDINGS + 25) {
            findings.push(json!({"id": i}));
        }
        let huge_summary = "x".repeat(MAX_SUMMARY_LEN + 100);
        let payload = json!({
            "findings": findings,
            "summary": huge_summary,
        });
        let bytes = serde_json::to_vec(&payload).unwrap();

        let r = enrich_with_json(Action::Block, &bytes);
        assert_eq!(r.action, Action::Block);
        assert_eq!(r.findings.len(), MAX_FINDINGS);
        assert!(r.summary.len() <= MAX_SUMMARY_LEN);
        assert_eq!(r.findings[0], json!({"id": 0}));
    }

    #[test]
    fn enrich_with_json_degrades_on_invalid_json() {
        let r = enrich_with_json(Action::Block, b"not-json-at-all");
        assert_eq!(r.action, Action::Block);
        assert!(r.findings.is_empty());
        assert_eq!(r.summary, "security issue detected (details unavailable)");

        let r_warn = enrich_with_json(Action::Warn, b"{not-json");
        assert_eq!(
            r_warn.summary,
            "security warning detected (details unavailable)"
        );

        let r_allow = enrich_with_json(Action::Allow, b"oops");
        assert!(r_allow.summary.is_empty());
    }

    #[test]
    fn enrich_with_json_handles_empty_stdout() {
        let r = enrich_with_json(Action::Allow, b"");
        assert_eq!(r.action, Action::Allow);
        assert!(r.findings.is_empty());
        assert!(r.summary.is_empty());

        // Whitespace-only is also "empty" per the prior Python engine's .strip() check.
        let r = enrich_with_json(Action::Allow, b"   \n\t  ");
        assert_eq!(r.action, Action::Allow);
        assert!(r.summary.is_empty());
    }

    #[test]
    fn enrich_with_json_never_overrides_verdict() {
        // JSON has no findings; action is still BLOCK because exit code said so.
        let payload = json!({"findings": [], "summary": "all clear"});
        let r = enrich_with_json(Action::Block, &serde_json::to_vec(&payload).unwrap());
        assert_eq!(r.action, Action::Block);
        assert_eq!(r.summary, "all clear");
    }

    #[test]
    fn enrich_with_json_summary_at_char_boundary() {
        // Multibyte UTF-8 char straddling the truncation point.
        // "é" is 2 bytes; build a string where the boundary lands mid-char.
        let mut s = String::new();
        // Fill up to MAX_SUMMARY_LEN - 1 then add a 2-byte char.
        for _ in 0..(MAX_SUMMARY_LEN - 1) {
            s.push('a');
        }
        s.push('é');
        s.push('é');
        let payload = json!({"summary": s});
        let r = enrich_with_json(Action::Allow, &serde_json::to_vec(&payload).unwrap());
        // Must not panic, must be a valid string, must be <= limit.
        assert!(r.summary.len() <= MAX_SUMMARY_LEN);
        assert!(r.summary.is_char_boundary(r.summary.len()));
    }

    #[test]
    fn is_explicit_path_detects_custom_paths() {
        assert!(!is_explicit_path("tirith"));
        assert!(is_explicit_path("/usr/local/bin/tirith"));
        assert!(is_explicit_path("./tirith"));
        assert!(is_explicit_path("tirith-custom"));
    }

    #[test]
    fn security_config_default_fail_closed() {
        // v0.6.2 cross-audit Round 1: default flipped from fail-open (matching
        // the prior Python engine) to fail-closed. Operators who need the old behavior set
        // TIRITH_FAIL_OPEN=true.
        let cfg = SecurityConfig::default();
        assert!(cfg.tirith_enabled);
        assert_eq!(cfg.tirith_path, "tirith");
        assert_eq!(cfg.tirith_timeout_secs, 5);
        assert!(!cfg.tirith_fail_open);
    }

    #[tokio::test]
    async fn check_command_security_disabled_returns_allow() {
        let cfg = SecurityConfig {
            tirith_enabled: false,
            ..SecurityConfig::default()
        };
        let r = check_command_security("rm -rf /", &cfg).await;
        assert_eq!(r.action, Action::Allow);
        assert!(r.findings.is_empty());
        assert!(r.summary.is_empty());
    }

    #[tokio::test]
    async fn check_command_security_missing_binary_fail_open() {
        let cfg = SecurityConfig {
            tirith_enabled: true,
            tirith_path: "/nonexistent/path/to/tirith-binary-xyz".to_string(),
            tirith_timeout_secs: 2,
            tirith_fail_open: true,
        };
        let r = check_command_security("echo hi", &cfg).await;
        assert_eq!(r.action, Action::Allow);
        assert!(r.summary.contains("tirith unavailable"));
    }

    #[tokio::test]
    async fn check_command_security_missing_binary_fail_closed() {
        let cfg = SecurityConfig {
            tirith_enabled: true,
            tirith_path: "/nonexistent/path/to/tirith-binary-xyz".to_string(),
            tirith_timeout_secs: 2,
            tirith_fail_open: false,
        };
        let r = check_command_security("echo hi", &cfg).await;
        assert_eq!(r.action, Action::Block);
        assert!(r.summary.contains("fail-closed"));
    }

    #[test]
    fn failure_marker_read_returns_none_when_missing() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join(".tirith-install-failed");
        assert!(read_failure_reason_at(&p, MARKER_TTL_SECS).is_none());
    }

    #[test]
    fn failure_marker_read_honours_ttl() {
        use std::io::Write;

        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join(".tirith-install-failed");
        let mut f = fs::File::create(&p).unwrap();
        f.write_all(b"cosign_missing").unwrap();
        drop(f);

        // Fresh marker is readable.
        assert_eq!(
            read_failure_reason_at(&p, MARKER_TTL_SECS).as_deref(),
            Some("cosign_missing")
        );

        // Expired marker (zero TTL) returns None.
        assert!(read_failure_reason_at(&p, 0).is_none());
    }

    #[test]
    fn resolve_tirith_path_explicit_missing_returns_tagged_failure() {
        let r = resolve_tirith_path("/definitely/does/not/exist/tirith");
        assert_eq!(
            r,
            PathResolution::Missing("explicit_path_missing".to_string())
        );
    }
}
