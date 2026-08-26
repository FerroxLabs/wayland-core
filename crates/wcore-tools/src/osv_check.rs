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
//! * No agent-facing tool wrapper. The Python original exposed this
//!   as something the model could choose to call; a malware gate the
//!   model may simply decline to call is not a control. The single
//!   consumer is `wcore_mcp::malware_gate`, which runs the check
//!   automatically on the stdio MCP launch path before the package
//!   runner is executed.

use async_trait::async_trait;

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

/// What a launcher argv names, or that it names nothing readable.
///
/// [`Self::Unidentified`] is deliberately distinct from `infer_ecosystem`
/// returning `None`. "Not a package runner" means there is nothing to check;
/// "package runner with an argv I cannot read" means the check could not be
/// performed, and a caller enforcing a gate must treat those differently.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PackageRef {
    Identified {
        package: String,
        version: Option<String>,
    },
    Unidentified,
}

/// Launcher flags that consume the FOLLOWING argv token as their value.
///
/// Getting this wrong is not cosmetic: `uvx --python 3.12 pkg==1.0` read by a
/// flag-unaware scanner yields `3.12` as the package, so the real package is
/// never queried and the gate reports clean on a name nobody installed.
///
/// Anything NOT listed is assumed to be a bare switch, so the token after it
/// stays a package candidate. That direction is the safe one: an unknown flag
/// costs a wasted query, where over-listing costs a missed check.
fn value_taking_flags(ecosystem: Ecosystem) -> &'static [&'static str] {
    match ecosystem {
        Ecosystem::Npm => &["--package", "-p", "-c", "--call", "--userconfig", "--cache"],
        Ecosystem::PyPI => &[
            "--python",
            "-p",
            "--with",
            "--with-editable",
            "--with-requirements",
            "--from",
            "--spec",
            "--index",
            "--index-url",
            "--extra-index-url",
            "--constraints",
            "-c",
            "--refresh-package",
            "--reinstall-package",
            "--index-strategy",
            "--pip-args",
        ],
    }
}

/// Flags whose value IS the package being installed, rather than a token to
/// step over. `npx --package X -c cmd` installs `X`; `uvx --from X app` installs
/// `X` and runs `app` out of it. In both cases the positional token is the
/// ENTRY POINT, not the package, so these win over any positional.
fn package_naming_flags(ecosystem: Ecosystem) -> &'static [&'static str] {
    match ecosystem {
        Ecosystem::Npm => &["--package", "-p"],
        Ecosystem::PyPI => &["--from", "--spec"],
    }
}

/// Resolve which package a launcher argv refers to.
pub fn parse_package_from_args(args: &[String], ecosystem: Ecosystem) -> PackageRef {
    let value_flags = value_taking_flags(ecosystem);
    let naming_flags = package_naming_flags(ecosystem);

    let mut named: Option<&str> = None;
    let mut positional: Option<&str> = None;
    let mut past_terminator = false;
    let mut i = 0;

    while i < args.len() {
        let arg = args[i].as_str();

        // After `--`, and for any token that is not a flag, this is a
        // positional. A bare `-` is a conventional stdin placeholder, not a flag.
        if past_terminator || !arg.starts_with('-') || arg == "-" {
            if positional.is_none() {
                positional = Some(arg);
            }
            i += 1;
            continue;
        }
        if arg == "--" {
            past_terminator = true;
            i += 1;
            continue;
        }
        // `--flag=value`: the value is attached, so nothing to skip.
        if let Some((flag, value)) = arg.split_once('=') {
            if named.is_none() && !value.is_empty() && naming_flags.contains(&flag) {
                named = Some(&arg[flag.len() + 1..]);
            }
            i += 1;
            continue;
        }
        if value_flags.contains(&arg) {
            if named.is_none() && naming_flags.contains(&arg) {
                named = args.get(i + 1).map(|s| s.as_str());
            }
            // Step over the flag AND the value it consumes.
            i += 2;
            continue;
        }
        i += 1;
    }

    let Some(token) = named.or(positional) else {
        return PackageRef::Unidentified;
    };
    let (package, version) = match ecosystem {
        Ecosystem::Npm => parse_npm_package(token),
        Ecosystem::PyPI => parse_pypi_package(token),
    };
    match package {
        Some(package) if !package.is_empty() => PackageRef::Identified { package, version },
        _ => PackageRef::Unidentified,
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

/// The four answers a caller enforcing a launch gate has to tell apart.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MalwareCheckOutcome {
    /// `command` is not a recognised package runner. Nothing was fetched from
    /// a registry, so there is nothing to check.
    NotApplicable,
    /// `command` IS a package runner, but its argv named no package this
    /// helper could read. Nothing was queried. An argv the gate cannot read is
    /// an argv it cannot clear.
    Unidentified,
    /// Either the query came back with no malware advisories, or the query
    /// itself failed. A malware feed that cannot be reached must not wedge the
    /// user's MCP servers, so a backend error fails OPEN — and ONLY a backend
    /// error does.
    Allowed,
    /// Known malware advisories, with the operator-facing message.
    Blocked(String),
}

/// Check whether the package referenced by `(command, args)` has any malware
/// advisories.
///
/// `endpoint` is the OSV endpoint URL (typically [`DEFAULT_OSV_ENDPOINT`]).
/// If the endpoint fails [`is_safe_url`] (SSRF gate), the check short-circuits
/// to [`MalwareCheckOutcome::Allowed`] — there is no legitimate reason for the
/// OSV endpoint to point at an internal address, and refusing every MCP server
/// over one misconfigured URL is a worse failure than the check not running.
pub async fn check_package_for_malware(
    command: &str,
    args: &[String],
    endpoint: &str,
    backend: &dyn OsvBackend,
) -> MalwareCheckOutcome {
    let Some(ecosystem) = infer_ecosystem(command) else {
        return MalwareCheckOutcome::NotApplicable;
    };
    if !is_safe_url(endpoint) {
        // Logged at WARN so operators see SSRF attempts at default log levels —
        // an endpoint that fails the SSRF gate is always operator-visible
        // misconfiguration or an active attack, never normal traffic.
        tracing::warn!(target: "wcore::osv_check", endpoint, "refusing to query unsafe OSV endpoint (SSRF gate)");
        return MalwareCheckOutcome::Allowed;
    }
    let PackageRef::Identified { package, version } = parse_package_from_args(args, ecosystem)
    else {
        return MalwareCheckOutcome::Unidentified;
    };
    match backend
        .query(endpoint, ecosystem, &package, version.as_deref())
        .await
    {
        Ok(advisories) => {
            let malware = filter_malware(advisories);
            if malware.is_empty() {
                MalwareCheckOutcome::Allowed
            } else {
                MalwareCheckOutcome::Blocked(format_block_message(&package, ecosystem, &malware))
            }
        }
        Err(exc) => {
            // Fail OPEN, and say so at WARN rather than DEBUG: `warn!` is the
            // lowest level that reaches a user with no RUST_LOG set, and "the
            // malware gate did not run" is exactly the thing they must be told.
            tracing::warn!(
                target: "wcore::osv_check",
                error = %exc,
                ecosystem = ecosystem.as_str(),
                package = %package,
                "OSV malware check could not be performed; allowing the launch",
            );
            MalwareCheckOutcome::Allowed
        }
    }
}

#[cfg(test)]
mod tests {
    // The endpoint fixture is a literal public IP, never a hostname.
    //
    // `check_package_for_malware` gates its endpoint through
    // `url_safety::is_safe_url`, which performs a REAL DNS resolution and
    // fails closed on an empty answer. With `DEFAULT_OSV_ENDPOINT` (a
    // hostname) these tests therefore depend on the runner's resolver: four
    // of them fail outright when it hiccups, and two more (the
    // `unknown_command` / `clean_returns_ok` pair) pass VACUOUSLY because the
    // gate fails open before the backend is ever called. A literal IP takes
    // the `parse::<IpAddr>()` fast path and skips resolution entirely.
    // `check_refuses_unsafe_endpoint` below keeps a literal metadata IP, so
    // the SSRF gate itself is still graded.
    const TEST_OSV_ENDPOINT: &str = "https://93.184.216.34/v1/query";
    use std::sync::Arc;

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

    fn identified(package: &str, version: Option<&str>) -> PackageRef {
        PackageRef::Identified {
            package: package.to_string(),
            version: version.map(|v| v.to_string()),
        }
    }

    #[test]
    fn parse_package_skips_flags() {
        let args = s(&["-y", "--quiet", "left-pad@1.0.0"]);
        assert_eq!(
            parse_package_from_args(&args, Ecosystem::Npm),
            identified("left-pad", Some("1.0.0"))
        );
        let only_flags = s(&["-y", "--quiet"]);
        assert_eq!(
            parse_package_from_args(&only_flags, Ecosystem::Npm),
            PackageRef::Unidentified
        );
        let empty: Vec<String> = vec![];
        assert_eq!(
            parse_package_from_args(&empty, Ecosystem::PyPI),
            PackageRef::Unidentified
        );
    }

    /// The defect the flag-unaware scanner had: a launcher flag that takes a
    /// SEPARATE value donated that value as the package name, so the package
    /// actually being installed was never queried.
    #[test]
    fn value_taking_flags_do_not_donate_their_value_as_the_package() {
        assert_eq!(
            parse_package_from_args(&s(&["--python", "3.12", "pkg==1.0"]), Ecosystem::PyPI),
            identified("pkg", Some("1.0")),
            "`--python 3.12` must not make 3.12 the package"
        );
        assert_eq!(
            parse_package_from_args(&s(&["--python=3.12", "pkg==1.0"]), Ecosystem::PyPI),
            identified("pkg", Some("1.0")),
            "the `--flag=value` spelling consumes no following token"
        );
        assert_eq!(
            parse_package_from_args(&s(&["--userconfig", "npmrc", "left-pad"]), Ecosystem::Npm),
            identified("left-pad", None)
        );
    }

    /// `--from` / `--spec` / `--package` NAME the package; the positional after
    /// them is the entry point to run out of it.
    #[test]
    fn package_naming_flags_beat_the_positional() {
        assert_eq!(
            parse_package_from_args(
                &s(&["--with", "pydantic<2.12", "--from", "pkg==1.0", "pkg"]),
                Ecosystem::PyPI
            ),
            identified("pkg", Some("1.0"))
        );
        assert_eq!(
            parse_package_from_args(&s(&["--from", "real-pkg==2.0", "app"]), Ecosystem::PyPI),
            identified("real-pkg", Some("2.0")),
            "the installed package is --from's value, not the entry point"
        );
        assert_eq!(
            parse_package_from_args(&s(&["run", "--spec", "real-pkg", "app"]), Ecosystem::PyPI),
            identified("real-pkg", None),
            "pipx's `run` subcommand must not be mistaken for the package"
        );
        assert_eq!(
            parse_package_from_args(&s(&["--package", "real-pkg", "-c", "cmd"]), Ecosystem::Npm),
            identified("real-pkg", None)
        );
    }

    /// A launcher whose argv names nothing readable must be distinguishable
    /// from a launcher that is not a package runner at all — the caller
    /// refuses one and ignores the other.
    #[test]
    fn an_argv_with_no_package_is_unidentified_not_a_silent_none() {
        assert_eq!(
            parse_package_from_args(&s(&["--python", "3.12"]), Ecosystem::PyPI),
            PackageRef::Unidentified
        );
        assert_eq!(
            parse_package_from_args(&s(&["--from", "pkg"]), Ecosystem::PyPI),
            identified("pkg", None),
            "control: the same shape WITH a naming flag is still identified"
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
        let MalwareCheckOutcome::Blocked(msg) = check_package_for_malware(
            "npx",
            &s(&["-y", "evil-pkg@1.0.0"]),
            TEST_OSV_ENDPOINT,
            backend.as_ref(),
        )
        .await
        else {
            panic!("malware should produce a Blocked outcome");
        };
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
        let outcome =
            check_package_for_malware("python", &s(&["evil"]), TEST_OSV_ENDPOINT, backend.as_ref())
                .await;
        assert_eq!(outcome, MalwareCheckOutcome::NotApplicable);
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
            TEST_OSV_ENDPOINT,
            backend.as_ref(),
        )
        .await;
        assert_eq!(
            outcome,
            MalwareCheckOutcome::Allowed,
            "network errors must fail open"
        );
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
        assert_eq!(outcome, MalwareCheckOutcome::Allowed);
        // Backend MUST NOT be called when endpoint is unsafe.
        assert!(backend.calls.lock().is_empty());
    }

    #[tokio::test]
    async fn check_handles_clean_package() {
        let backend = Arc::new(CapturingOsvBackend::with_response(vec![]));
        let outcome = check_package_for_malware(
            "uvx",
            &s(&["requests==2.31.0"]),
            TEST_OSV_ENDPOINT,
            backend.as_ref(),
        )
        .await;
        assert_eq!(outcome, MalwareCheckOutcome::Allowed);
        let calls = backend.calls.lock();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].ecosystem, Ecosystem::PyPI);
        assert_eq!(calls[0].package, "requests");
        assert_eq!(calls[0].version.as_deref(), Some("2.31.0"));
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
