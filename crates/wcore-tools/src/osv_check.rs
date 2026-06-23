//! T3-3.8 — OSV (Open Source Vulnerabilities) malware check.
//!
//! Ported from the prior Wayland Python engine.
//!
//! Before launching an MCP server via `npx` / `uvx` (or any analogous
//! package-runner shim), this helper queries the OSV API to check
//! whether the requested package has any known **malware** advisories
//! (`MAL-*` IDs). Regular CVEs are ignored — only confirmed malware
//! blocks. The check is intentionally narrow because OSV produces a
//! steady stream of low-severity informational CVEs that would
//! otherwise create noise and pressure to override the gate.
//!
//! Wayland's engine MUST NOT initiate raw HTTP from inside
//! `wcore-tools` (HTTP belongs to the host / `wcore-providers` /
//! plugin layer), so the actual HTTP query is dispatched through a
//! pluggable [`OsvBackend`] seam. Hosts wire a real backend at
//! construction time; tests inject [`CapturingOsvBackend`] to drive
//! deterministic responses. Without a backend bound, the helper
//! **fails open** (returns `None`) — matching the Python original's
//! defensive posture where network errors must never wedge the
//! agent's ability to launch a tool.
//!
//! The configured endpoint URL is validated through
//! [`crate::url_safety::is_safe_url`] for SSRF defense-in-depth, so
//! callers can't be tricked into pointing the check at a private
//! metadata service via environment-variable override.
//!
//! Divergences from the Python original (intentional):
//! * Pluggable backend instead of direct `urllib.request` — keeps
//!   `wcore-tools` free of HTTP client deps and lets the host pick
//!   reqwest / hyper / mock as it sees fit.
//! * `OsvAdvisory` is structured (typed `id` / `summary` strings)
//!   instead of `dict`. The helper still surfaces the same
//!   human-readable BLOCKED message format the Python emits.
//! * SSRF validation on the endpoint URL (Python had none — the
//!   default endpoint is public, but `$OSV_ENDPOINT` could redirect).
//! * `OsvTool` wraps the helper as an agent-facing tool with a
//!   `command` + `args` schema. The helper itself remains usable
//!   from anywhere in `wcore-tools` (e.g. an MCP-launch pre-flight
//!   in a future wave) without going through the tool dispatcher.

use std::sync::Arc;

use async_trait::async_trait;
use serde_json::{Value, json};

use wcore_protocol::events::ToolCategory;
use wcore_types::tool::{JsonSchema, ToolResult};

use crate::Tool;
use crate::url_safety::is_safe_url;

/// Default OSV API endpoint (the public Google-maintained service).
pub const DEFAULT_OSV_ENDPOINT: &str = "https://api.osv.dev/v1/query";

/// One OSV advisory entry returned by the API.
///
/// Only `id` and `summary` are retained — the rest of the OSV record
/// (references, severity vectors, affected ranges) is irrelevant to
/// the malware-only blocking decision and would just inflate test
/// fixtures.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OsvAdvisory {
    pub id: String,
    pub summary: String,
}

/// Inferred package ecosystem.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ecosystem {
    Npm,
    PyPI,
}

impl Ecosystem {
    /// String form expected by the OSV API.
    pub fn as_str(self) -> &'static str {
        match self {
            Ecosystem::Npm => "npm",
            Ecosystem::PyPI => "PyPI",
        }
    }
}

/// Pluggable OSV transport.
///
/// Implementors perform the actual HTTP `POST` to the OSV endpoint
/// (or any equivalent oracle) and return parsed advisories. The
/// helper layer filters down to `MAL-*` entries and formats the
/// final block message — implementors don't need to know about
/// malware-vs-CVE classification.
#[async_trait]
pub trait OsvBackend: Send + Sync {
    /// Query OSV for advisories on `(ecosystem, package, version)`.
    /// `endpoint` is the validated URL the host has wired in.
    /// Returning `Err` triggers the helper's fail-open posture.
    async fn query(
        &self,
        endpoint: &str,
        ecosystem: Ecosystem,
        package: &str,
        version: Option<&str>,
    ) -> Result<Vec<OsvAdvisory>, OsvBackendError>;
}

/// Backend-side error surface. Kept opaque to the helper so any
/// network / parse failure flows through the same fail-open path.
#[derive(Debug, thiserror::Error)]
pub enum OsvBackendError {
    #[error("osv backend network error: {0}")]
    Network(String),
    #[error("osv backend parse error: {0}")]
    Parse(String),
    #[error("osv backend other error: {0}")]
    Other(String),
}

/// Test backend that returns a canned advisory list (or an error)
/// without performing real I/O. Records every call for assertion.
///
/// `Default` is intentionally NOT derived — `Result<_, _>` has no
/// `Default` impl, so the canned `response` field must be set by one
/// of the explicit constructors below.
#[derive(Debug)]
pub struct CapturingOsvBackend {
    pub calls: parking_lot::Mutex<Vec<CapturedOsvCall>>,
    pub response: parking_lot::Mutex<Result<Vec<OsvAdvisory>, OsvBackendError>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CapturedOsvCall {
    pub endpoint: String,
    pub ecosystem: Ecosystem,
    pub package: String,
    pub version: Option<String>,
}

impl CapturingOsvBackend {
    pub fn with_response(advisories: Vec<OsvAdvisory>) -> Self {
        Self {
            calls: parking_lot::Mutex::new(Vec::new()),
            response: parking_lot::Mutex::new(Ok(advisories)),
        }
    }

    pub fn with_error(err: OsvBackendError) -> Self {
        Self {
            calls: parking_lot::Mutex::new(Vec::new()),
            response: parking_lot::Mutex::new(Err(err)),
        }
    }
}

#[async_trait]
impl OsvBackend for CapturingOsvBackend {
    async fn query(
        &self,
        endpoint: &str,
        ecosystem: Ecosystem,
        package: &str,
        version: Option<&str>,
    ) -> Result<Vec<OsvAdvisory>, OsvBackendError> {
        self.calls.lock().push(CapturedOsvCall {
            endpoint: endpoint.to_string(),
            ecosystem,
            package: package.to_string(),
            version: version.map(|s| s.to_string()),
        });
        // Clone the canned result; the response slot stays intact.
        match &*self.response.lock() {
            Ok(adv) => Ok(adv.clone()),
            Err(e) => Err(match e {
                OsvBackendError::Network(s) => OsvBackendError::Network(s.clone()),
                OsvBackendError::Parse(s) => OsvBackendError::Parse(s.clone()),
                OsvBackendError::Other(s) => OsvBackendError::Other(s.clone()),
            }),
        }
    }
}

/// Infer the package ecosystem from the launcher `command` (mirrors
/// the Python `_infer_ecosystem`). Returns `None` for commands that
/// aren't recognized package runners — the caller treats that as
/// "skip the check".
pub fn infer_ecosystem(command: &str) -> Option<Ecosystem> {
    // Take the basename and lowercase — `/usr/local/bin/npx` and
    // `NPX.CMD` both map to `npx`.
    let base = std::path::Path::new(command)
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or(command)
        .to_ascii_lowercase();
    match base.as_str() {
        "npx" | "npx.cmd" => Some(Ecosystem::Npm),
        "uvx" | "uvx.cmd" | "pipx" => Some(Ecosystem::PyPI),
        _ => None,
    }
}

/// Parse `(package, version)` from launcher args for the given
/// `ecosystem`. Returns `(None, None)` when args are empty or every
/// token looks like a flag.
pub fn parse_package_from_args(
    args: &[String],
    ecosystem: Ecosystem,
) -> (Option<String>, Option<String>) {
    // Skip flags to find the first positional token.
    let token = args.iter().find(|a| !a.starts_with('-'));
    let Some(token) = token else {
        return (None, None);
    };
    match ecosystem {
        Ecosystem::Npm => parse_npm_package(token),
        Ecosystem::PyPI => parse_pypi_package(token),
    }
}

/// Parse an npm package token: `@scope/name@version` or
/// `name@version` (version optional; `@latest` drops to `None`).
pub fn parse_npm_package(token: &str) -> (Option<String>, Option<String>) {
    if let Some(rest) = token.strip_prefix('@') {
        // Scoped: @scope/name[@version]
        let (scope_name, version) = match rest.find('/') {
            Some(slash) => {
                let after_slash = &rest[slash + 1..];
                match after_slash.find('@') {
                    Some(at) => {
                        let scope_name = format!("@{}/{}", &rest[..slash], &after_slash[..at]);
                        let version = &after_slash[at + 1..];
                        (
                            scope_name,
                            if version.is_empty() {
                                None
                            } else {
                                Some(version.to_string())
                            },
                        )
                    }
                    None => (format!("@{rest}"), None),
                }
            }
            None => return (Some(token.to_string()), None),
        };
        return (Some(scope_name), version);
    }
    // Unscoped: name[@version]
    if let Some(at) = token.rfind('@') {
        let name = &token[..at];
        let version = &token[at + 1..];
        if version == "latest" || version.is_empty() {
            return (Some(name.to_string()), None);
        }
        return (Some(name.to_string()), Some(version.to_string()));
    }
    (Some(token.to_string()), None)
}

/// Parse a PyPI package token: `name[==version]` with optional
/// `[extra,...]` markers stripped (mirrors PEP 508 lite).
pub fn parse_pypi_package(token: &str) -> (Option<String>, Option<String>) {
    // Find name run: [A-Za-z0-9._-]+
    let name_end = token
        .find(|c: char| !(c.is_ascii_alphanumeric() || c == '.' || c == '_' || c == '-'))
        .unwrap_or(token.len());
    if name_end == 0 {
        return (Some(token.to_string()), None);
    }
    let name = &token[..name_end];
    let mut rest = &token[name_end..];
    // Optional `[extras]`
    if let Some(stripped) = rest.strip_prefix('[')
        && let Some(close) = stripped.find(']')
    {
        rest = &stripped[close + 1..];
    }
    // Optional `==version`
    if let Some(version) = rest.strip_prefix("==") {
        if version.is_empty() {
            return (Some(name.to_string()), None);
        }
        return (Some(name.to_string()), Some(version.to_string()));
    }
    (Some(name.to_string()), None)
}

/// Filter to malware-only advisories (`MAL-*`). Public so callers
/// that want the raw OSV list can still apply the same predicate.
pub fn filter_malware(advisories: Vec<OsvAdvisory>) -> Vec<OsvAdvisory> {
    advisories
        .into_iter()
        .filter(|a| a.id.starts_with("MAL-"))
        .collect()
}

/// Build the human-readable BLOCKED message from a non-empty list
/// of malware advisories. Mirrors the Python `ids` + `summaries`
/// joining (first 3 entries, summary trimmed to 100 chars).
fn format_block_message(package: &str, ecosystem: Ecosystem, malware: &[OsvAdvisory]) -> String {
    let take = malware.iter().take(3).collect::<Vec<_>>();
    let ids = take
        .iter()
        .map(|a| a.id.as_str())
        .collect::<Vec<_>>()
        .join(", ");
    let summaries = take
        .iter()
        .map(|a| {
            let s = if a.summary.is_empty() {
                a.id.as_str()
            } else {
                a.summary.as_str()
            };
            truncate_chars(s, 100).to_string()
        })
        .collect::<Vec<_>>()
        .join("; ");
    format!(
        "BLOCKED: Package '{package}' ({eco}) has known malware advisories: {ids}. Details: {summaries}",
        eco = ecosystem.as_str(),
    )
}

/// Char-aware truncation (matches Python's `[:100]` semantics on a
/// `str` — character count, not bytes).
fn truncate_chars(s: &str, max_chars: usize) -> &str {
    let mut iter = s.char_indices();
    match iter.nth(max_chars) {
        Some((idx, _)) => &s[..idx],
        None => s,
    }
}

/// Top-level helper: check whether a package referenced by
/// `(command, args)` has any malware advisories. Returns
/// `Some(message)` on a hit, `None` for clean / unknown / network
/// errors (fail-open). Mirrors `check_package_for_malware` in the
/// Python original.
///
/// `endpoint` is the OSV endpoint URL (typically [`DEFAULT_OSV_ENDPOINT`]).
/// If the endpoint fails [`is_safe_url`] (SSRF gate), the check
/// short-circuits to `None` — there is no legitimate reason for the
/// OSV endpoint to point at an internal address.
pub async fn check_package_for_malware(
    command: &str,
    args: &[String],
    endpoint: &str,
    backend: &dyn OsvBackend,
) -> Option<String> {
    if !is_safe_url(endpoint) {
        // Refuse to query an unsafe endpoint, but fail open per the
        // overall helper contract. Logged at WARN so operators see SSRF
        // attempts at default log levels — an endpoint that fails the
        // SSRF gate is always operator-visible misconfiguration or an
        // active attack, never normal traffic.
        tracing::warn!(target: "wcore::osv_check", endpoint, "refusing to query unsafe OSV endpoint (SSRF gate)");
        return None;
    }
    let ecosystem = infer_ecosystem(command)?;
    let (package, version) = parse_package_from_args(args, ecosystem);
    let package = package?;
    match backend
        .query(endpoint, ecosystem, &package, version.as_deref())
        .await
    {
        Ok(advisories) => {
            let malware = filter_malware(advisories);
            if malware.is_empty() {
                None
            } else {
                Some(format_block_message(&package, ecosystem, &malware))
            }
        }
        Err(exc) => {
            tracing::debug!(
                target: "wcore::osv_check",
                error = %exc,
                ecosystem = ecosystem.as_str(),
                package = %package,
                "OSV check failed (allowing)",
            );
            None
        }
    }
}

/// Agent-facing tool wrapper. Inputs:
/// * `command` — launcher (e.g. `npx`, `uvx`).
/// * `args` — argv tail.
/// * Optional `endpoint` override (defaults to [`DEFAULT_OSV_ENDPOINT`]).
pub struct OsvTool {
    backend: Arc<dyn OsvBackend>,
    endpoint: String,
}

impl OsvTool {
    pub fn new(backend: Arc<dyn OsvBackend>) -> Self {
        Self {
            backend,
            endpoint: DEFAULT_OSV_ENDPOINT.to_string(),
        }
    }

    pub fn with_endpoint(mut self, endpoint: impl Into<String>) -> Self {
        self.endpoint = endpoint.into();
        self
    }
}

#[async_trait]
impl Tool for OsvTool {
    fn name(&self) -> &str {
        "osv_check"
    }

    fn description(&self) -> &str {
        "Query the OSV (Open Source Vulnerabilities) database for malware advisories against an npm/PyPI package referenced by a (command, args) launcher pair. Returns a BLOCKED message if malware is found; null otherwise. Network / parse failures fail open."
    }

    fn input_schema(&self) -> JsonSchema {
        json!({
            "type": "object",
            "properties": {
                "command": {
                    "type": "string",
                    "description": "Launcher binary (npx, uvx, pipx, ...).",
                },
                "args": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "argv tail passed to the launcher.",
                },
                "endpoint": {
                    "type": "string",
                    "description": "Optional OSV API endpoint override; defaults to https://api.osv.dev/v1/query.",
                }
            },
            "required": ["command", "args"],
            "additionalProperties": false
        })
    }

    fn is_concurrency_safe(&self, _input: &Value) -> bool {
        true
    }

    async fn execute(&self, input: Value) -> ToolResult {
        let command = match input.get("command").and_then(|v| v.as_str()) {
            Some(s) => s.to_string(),
            None => {
                return ToolResult {
                    content: "osv_check: missing required string 'command'".to_string(),
                    is_error: true,
                };
            }
        };
        let args = match input.get("args").and_then(|v| v.as_array()) {
            Some(arr) => arr
                .iter()
                .filter_map(|v| v.as_str().map(|s| s.to_string()))
                .collect::<Vec<_>>(),
            None => {
                return ToolResult {
                    content: "osv_check: missing required array 'args'".to_string(),
                    is_error: true,
                };
            }
        };
        let endpoint = input
            .get("endpoint")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
            .unwrap_or_else(|| self.endpoint.clone());

        let outcome =
            check_package_for_malware(&command, &args, &endpoint, self.backend.as_ref()).await;
        match outcome {
            Some(msg) => ToolResult {
                content: msg,
                is_error: false,
            },
            None => ToolResult {
                content: "ok: no malware advisories".to_string(),
                is_error: false,
            },
        }
    }

    fn category(&self) -> ToolCategory {
        // OSV check is a read-only network query against a public DB;
        // ToolCategory has no Security variant — Info is the closest fit
        // and matches how Wayland classifies other read-only network
        // probes (e.g. vision_analyze, web_fetch).
        ToolCategory::Info
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn infer_ecosystem_recognizes_runners() {
        assert_eq!(infer_ecosystem("npx"), Some(Ecosystem::Npm));
        assert_eq!(infer_ecosystem("/usr/bin/npx"), Some(Ecosystem::Npm));
        assert_eq!(infer_ecosystem("NPX.CMD"), Some(Ecosystem::Npm));
        assert_eq!(infer_ecosystem("uvx"), Some(Ecosystem::PyPI));
        assert_eq!(infer_ecosystem("pipx"), Some(Ecosystem::PyPI));
        assert_eq!(infer_ecosystem("python"), None);
        assert_eq!(infer_ecosystem(""), None);
    }

    #[test]
    fn parse_npm_scoped_and_unscoped() {
        assert_eq!(
            parse_npm_package("@scope/pkg@1.2.3"),
            (Some("@scope/pkg".into()), Some("1.2.3".into()))
        );
        assert_eq!(
            parse_npm_package("@scope/pkg"),
            (Some("@scope/pkg".into()), None)
        );
        assert_eq!(
            parse_npm_package("left-pad@1.0.0"),
            (Some("left-pad".into()), Some("1.0.0".into()))
        );
        assert_eq!(
            parse_npm_package("left-pad@latest"),
            (Some("left-pad".into()), None)
        );
        assert_eq!(
            parse_npm_package("left-pad"),
            (Some("left-pad".into()), None)
        );
    }

    #[test]
    fn parse_pypi_with_extras_and_version() {
        assert_eq!(
            parse_pypi_package("requests==2.31.0"),
            (Some("requests".into()), Some("2.31.0".into()))
        );
        assert_eq!(
            parse_pypi_package("uvicorn[standard]==0.27.0"),
            (Some("uvicorn".into()), Some("0.27.0".into()))
        );
        assert_eq!(parse_pypi_package("httpx"), (Some("httpx".into()), None));
    }

    #[test]
    fn parse_package_skips_flags() {
        let args = s(&["-y", "--quiet", "left-pad@1.0.0"]);
        assert_eq!(
            parse_package_from_args(&args, Ecosystem::Npm),
            (Some("left-pad".into()), Some("1.0.0".into()))
        );
        let only_flags = s(&["-y", "--quiet"]);
        assert_eq!(
            parse_package_from_args(&only_flags, Ecosystem::Npm),
            (None, None)
        );
        let empty: Vec<String> = vec![];
        assert_eq!(
            parse_package_from_args(&empty, Ecosystem::PyPI),
            (None, None)
        );
    }

    #[test]
    fn filter_malware_drops_regular_cves() {
        let advisories = vec![
            OsvAdvisory {
                id: "CVE-2024-1234".into(),
                summary: "some cve".into(),
            },
            OsvAdvisory {
                id: "MAL-2024-5678".into(),
                summary: "malicious".into(),
            },
            OsvAdvisory {
                id: "GHSA-xxxx".into(),
                summary: "advisory".into(),
            },
        ];
        let kept = filter_malware(advisories);
        assert_eq!(kept.len(), 1);
        assert_eq!(kept[0].id, "MAL-2024-5678");
    }

    #[tokio::test]
    async fn check_returns_blocked_message_on_malware_hit() {
        let backend = Arc::new(CapturingOsvBackend::with_response(vec![
            OsvAdvisory {
                id: "MAL-2024-0001".into(),
                summary: "Steals SSH keys on postinstall".into(),
            },
            OsvAdvisory {
                id: "CVE-2024-9999".into(),
                summary: "not malware".into(),
            },
        ]));
        let msg = check_package_for_malware(
            "npx",
            &s(&["-y", "evil-pkg@1.0.0"]),
            DEFAULT_OSV_ENDPOINT,
            backend.as_ref(),
        )
        .await
        .expect("malware should produce a block message");
        assert!(msg.contains("BLOCKED"));
        assert!(msg.contains("evil-pkg"));
        assert!(msg.contains("(npm)"));
        assert!(msg.contains("MAL-2024-0001"));
        assert!(msg.contains("Steals SSH keys"));
        // CVE should NOT bleed into the message.
        assert!(!msg.contains("CVE-2024-9999"));
        let calls = backend.calls.lock();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].ecosystem, Ecosystem::Npm);
        assert_eq!(calls[0].package, "evil-pkg");
        assert_eq!(calls[0].version.as_deref(), Some("1.0.0"));
    }

    #[tokio::test]
    async fn check_returns_none_for_unknown_command() {
        let backend = Arc::new(CapturingOsvBackend::with_response(vec![OsvAdvisory {
            id: "MAL-1".into(),
            summary: "x".into(),
        }]));
        let outcome = check_package_for_malware(
            "python",
            &s(&["evil"]),
            DEFAULT_OSV_ENDPOINT,
            backend.as_ref(),
        )
        .await;
        assert!(outcome.is_none());
        // Backend must NOT be called for unrecognized commands.
        assert!(backend.calls.lock().is_empty());
    }

    #[tokio::test]
    async fn check_fails_open_on_backend_error() {
        let backend = Arc::new(CapturingOsvBackend::with_error(OsvBackendError::Network(
            "connection reset".into(),
        )));
        let outcome = check_package_for_malware(
            "npx",
            &s(&["left-pad@1.0.0"]),
            DEFAULT_OSV_ENDPOINT,
            backend.as_ref(),
        )
        .await;
        assert!(outcome.is_none(), "network errors must fail open");
        assert_eq!(backend.calls.lock().len(), 1);
    }

    #[tokio::test]
    async fn check_refuses_unsafe_endpoint() {
        let backend = Arc::new(CapturingOsvBackend::with_response(vec![OsvAdvisory {
            id: "MAL-1".into(),
            summary: "x".into(),
        }]));
        // Cloud metadata IP — url_safety must block this.
        let outcome = check_package_for_malware(
            "npx",
            &s(&["evil@1.0.0"]),
            "http://169.254.169.254/v1/query",
            backend.as_ref(),
        )
        .await;
        assert!(outcome.is_none());
        // Backend MUST NOT be called when endpoint is unsafe.
        assert!(backend.calls.lock().is_empty());
    }

    #[tokio::test]
    async fn check_handles_clean_package() {
        let backend = Arc::new(CapturingOsvBackend::with_response(vec![]));
        let outcome = check_package_for_malware(
            "uvx",
            &s(&["requests==2.31.0"]),
            DEFAULT_OSV_ENDPOINT,
            backend.as_ref(),
        )
        .await;
        assert!(outcome.is_none());
        let calls = backend.calls.lock();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].ecosystem, Ecosystem::PyPI);
        assert_eq!(calls[0].package, "requests");
        assert_eq!(calls[0].version.as_deref(), Some("2.31.0"));
    }

    #[tokio::test]
    async fn osv_tool_execute_success_path() {
        let backend = Arc::new(CapturingOsvBackend::with_response(vec![OsvAdvisory {
            id: "MAL-2024-XYZ".into(),
            summary: "credential exfiltration on install".into(),
        }]));
        let tool = OsvTool::new(backend);
        let result = tool
            .execute(json!({
                "command": "npx",
                "args": ["-y", "@evil/pkg@9.9.9"],
            }))
            .await;
        assert!(!result.is_error);
        assert!(result.content.contains("BLOCKED"));
        assert!(result.content.contains("@evil/pkg"));
        assert!(result.content.contains("MAL-2024-XYZ"));
    }

    #[tokio::test]
    async fn osv_tool_execute_clean_returns_ok() {
        let backend = Arc::new(CapturingOsvBackend::with_response(vec![]));
        let tool = OsvTool::new(backend);
        let result = tool
            .execute(json!({
                "command": "uvx",
                "args": ["httpx"],
            }))
            .await;
        assert!(!result.is_error);
        assert!(result.content.contains("ok"));
    }

    #[tokio::test]
    async fn osv_tool_execute_missing_command_errors() {
        let backend = Arc::new(CapturingOsvBackend::with_response(vec![]));
        let tool = OsvTool::new(backend);
        let result = tool.execute(json!({ "args": [] })).await;
        assert!(result.is_error);
    }

    #[test]
    fn format_block_message_truncates_long_summaries() {
        let long = "x".repeat(200);
        let adv = vec![OsvAdvisory {
            id: "MAL-1".into(),
            summary: long,
        }];
        let msg = format_block_message("p", Ecosystem::Npm, &adv);
        // 100 x's, not 200.
        assert!(msg.contains(&"x".repeat(100)));
        assert!(!msg.contains(&"x".repeat(101)));
    }
}
