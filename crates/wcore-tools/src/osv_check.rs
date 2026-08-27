//! T3-3.8 — OSV (Open Source Vulnerabilities) malware check.
//!
//! Ported from the prior Wayland Python engine.
//!
//! Before launching an MCP server via `npx` / `uvx` / `pipx`, this helper
//! queries the OSV API to check whether the packages that invocation will
//! FETCH have any known **malware** advisories (`MAL-*` IDs). Regular CVEs are
//! ignored — only confirmed malware blocks. The check is intentionally narrow
//! because OSV produces a steady stream of low-severity informational CVEs
//! that would otherwise create noise and pressure to override the gate.
//!
//! # What this covers, exactly (core#340)
//!
//! The check reads the CONFIGURED `command` and its argv. It is a lookup
//! against a malware feed, **not an execution boundary**. Specifically:
//!
//! * **Recognised runners:** `npx`, `uvx`, `pipx` (each also matched through a
//!   path and a Windows `.cmd`/`.exe` launcher extension). For `pipx`, only
//!   `pipx run` and `pipx install` are checked — see the NOT-covered list for
//!   the other subcommands.
//! * **Every package the argv fetches**, not just the one it runs: the
//!   positional, whatever `--package` / `--from` / `--spec` names, and every
//!   `--with` / `--with-editable` extra. A `--with` extra is installed and
//!   imported by the run, so checking only the primary clears the argv that
//!   carries the malicious package.
//! * **NOT covered — and this list is load-bearing, because an overstated
//!   guarantee stops the next person looking:**
//!   * Package runners this module does not recognise: `bunx`, `pnpm dlx`,
//!     `yarn dlx`, `npm exec`, `uv tool run`, `deno run npm:…`. These return
//!     [`MalwareCheckOutcome::NotApplicable`] and launch unchecked.
//!   * **Indirect execution of any kind.** `sh -c "npx evil"`, `env npx evil`,
//!     `node -e …`, or a wrapper script in the repo are all opaque here: the
//!     command is not a recognised runner, so the outcome is `NotApplicable`
//!     and the package is fetched and executed. Scanning a shell command line
//!     for a runner token was considered and rejected — it is stepped over in
//!     one character (`n''px`), so it would buy no security while creating
//!     exactly the overstated guarantee this section exists to prevent.
//!   * **`pipx` subcommands other than `run` / `install`.** `pipx inject
//!     <venv> <pkg>`, `pipx upgrade <pkg>`, `pipx reinstall <pkg>` and
//!     `pipx runpip <venv> install <pkg>` all FETCH from PyPI and run the
//!     package's install hooks. They are `NotApplicable` here — deliberately,
//!     because their package operand sits behind a positional this parser
//!     cannot place (`inject`'s first positional is a venv NAME, not a
//!     package), and answering with the venv name would be a WRONG answer,
//!     which this module treats as worse than no answer. Pinned by
//!     `tests/osv_runner_forms.rs::form_pipx_inject_is_a_documented_gap` so
//!     the gap cannot quietly be re-described as coverage.
//!   * Anything the launched program does AFTER it starts. A cleared package
//!     may fetch and execute whatever it likes.
//!   * Transitive dependencies. Only the named packages are queried.
//!   * Malware OSV has not yet published an advisory for.
//!
//! Wayland's engine MUST NOT initiate raw HTTP from inside
//! `wcore-tools` (HTTP belongs to the host / `wcore-providers` /
//! plugin layer), so the actual HTTP query is dispatched through a
//! pluggable [`OsvBackend`] seam. Hosts wire a real backend at
//! construction time; tests inject [`CapturingOsvBackend`] to drive
//! deterministic responses.
//!
//! A query that FAILS returns [`MalwareCheckOutcome::Unavailable`], not
//! `Allowed`. This module does not decide whether an unperformed check is a
//! pass — collapsing the two was how the fail-open became invisible, since the
//! only trace of it was a `tracing::warn!` that never reaches a user with
//! `RUST_LOG` unset. The policy now lives with the caller
//! (`wcore_mcp::malware_gate`), which announces it and offers the operator a
//! strict mode.
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
    match runner_base(command).as_str() {
        "npx" => Some(Ecosystem::Npm),
        "uvx" | "pipx" => Some(Ecosystem::PyPI),
        _ => None,
    }
}

/// Basename of `command`, lowercased, with a Windows launcher extension
/// removed — `/usr/local/bin/npx`, `NPX.CMD` and `npx.exe` are one runner, and
/// enumerating every spelling in every match arm is how one gets missed.
fn runner_base(command: &str) -> String {
    let base = std::path::Path::new(command)
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or(command)
        .to_ascii_lowercase();
    for ext in [".cmd", ".exe", ".bat", ".ps1"] {
        if let Some(stem) = base.strip_suffix(ext) {
            return stem.to_string();
        }
    }
    base
}

/// Leading subcommands that mean "this invocation fetches from a registry".
///
/// An empty table means the runner fetches unconditionally (`npx pkg`,
/// `uvx pkg`). `pipx` is a package MANAGER, not a bare runner, so the
/// subcommand decides: a subcommand-unaware scan reads the literal token `run`
/// as the package name, and `run` is a real PyPI project, so OSV answers CLEAN
/// and the package actually being fetched is never queried. That is a WRONG
/// answer, which is worse than no answer — the same failure the
/// `value_taking_flags` table exists to prevent, one level up.
///
/// Listed here are exactly the subcommands whose package operand this parser
/// can place: `run` and `install` both take it as the first positional.
///
/// Everything else — `list` and `environment`, which fetch nothing, but ALSO
/// `inject`, `upgrade`, `reinstall` and `runpip`, which DO — falls through to
/// `NotApplicable`. That is a real gap, and it is deliberate rather than
/// overlooked: `pipx inject <venv> <pkg>` puts a venv NAME in the position
/// this parser reads, so adding it to the table would query the venv name and
/// report a confident CLEAN on a package nobody looked at. The module docs
/// carry the gap in the NOT-covered list; widening the table is not the fix,
/// per-subcommand operand positions would be.
fn fetching_subcommands(runner: &str) -> &'static [&'static str] {
    match runner {
        "pipx" => &["run", "install"],
        _ => &[],
    }
}

/// The argv remaining after a runner's fetching subcommand, or `None` when
/// this invocation fetches nothing from a registry.
fn fetching_args<'a>(command: &str, args: &'a [String]) -> Option<&'a [String]> {
    let subcommands = fetching_subcommands(&runner_base(command));
    if subcommands.is_empty() {
        return Some(args);
    }
    // Global switches may precede the subcommand (`pipx --quiet run pkg`).
    let mut i = 0;
    while i < args.len() && args[i].starts_with('-') && args[i] != "-" && args[i] != "--" {
        i += 1;
    }
    match args.get(i) {
        Some(token) if subcommands.contains(&token.as_str()) => Some(&args[i + 1..]),
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

/// Flags whose value is an ADDITIONAL package the invocation installs
/// alongside the primary one.
///
/// `uvx --with evil-helper mcp-server` installs BOTH and imports both into the
/// run, so a gate that queries only `mcp-server` clears the argv that carries
/// the malicious package — and `--with` is where that hides best, because the
/// primary is a famous name nobody looks twice at.
///
/// `--with-requirements` is deliberately absent: its value is a file path, not
/// a package name.
fn extra_package_flags(ecosystem: Ecosystem) -> &'static [&'static str] {
    match ecosystem {
        // npm's runners have no analogue: `npx` installs exactly one package.
        Ecosystem::Npm => &[],
        Ecosystem::PyPI => &["--with", "--with-editable"],
    }
}

/// Every ADDITIONAL package the argv installs, in argv order.
pub fn parse_extra_packages(
    args: &[String],
    ecosystem: Ecosystem,
) -> Vec<(String, Option<String>)> {
    let flags = extra_package_flags(ecosystem);
    if flags.is_empty() {
        return Vec::new();
    }
    let mut out = Vec::new();
    let mut i = 0;
    while i < args.len() {
        let arg = args[i].as_str();
        // After `--` the tokens belong to the launched application, not the
        // launcher, so a `--with` there installs nothing.
        if arg == "--" {
            break;
        }
        let value = if let Some((flag, attached)) = arg.split_once('=') {
            flags.contains(&flag).then_some(attached)
        } else if flags.contains(&arg) {
            i += 1;
            args.get(i).map(|s| s.as_str())
        } else {
            None
        };
        if let Some(value) = value.filter(|v| !v.is_empty()) {
            let (package, version) = match ecosystem {
                Ecosystem::Npm => parse_npm_package(value),
                Ecosystem::PyPI => parse_pypi_package(value),
            };
            if let Some(package) = package.filter(|p| !p.is_empty()) {
                out.push((package, version));
            }
        }
        i += 1;
    }
    out
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
    /// The query ran and came back with no malware advisories.
    Allowed,
    /// The query did NOT run — an unreachable feed, an HTTP error, an
    /// unparseable response, or an endpoint that failed the SSRF gate.
    ///
    /// Deliberately NOT folded into [`Self::Allowed`]. "The check did not
    /// happen" and "the check passed" are different facts, and a caller that
    /// cannot tell them apart can neither announce the first nor offer an
    /// operator the choice to refuse on it. `subject` names what went
    /// unchecked; `reason` is the operator-facing cause. Neither carries argv,
    /// so no credential passed in argv can reach a log through this path.
    Unavailable { subject: String, reason: String },
    /// Known malware advisories, with the operator-facing message.
    Blocked(String),
}

/// Check whether the packages `(command, args)` will FETCH have any malware
/// advisories.
///
/// `endpoint` is the OSV endpoint URL (typically [`DEFAULT_OSV_ENDPOINT`]).
/// If the endpoint fails [`is_safe_url`] (SSRF gate) nothing is sent and the
/// result is [`MalwareCheckOutcome::Unavailable`] — there is no legitimate
/// reason for the OSV endpoint to point at an internal address, and the caller
/// decides what an unperformed check costs.
///
/// See the module docs for the exact coverage boundary. In particular this
/// returns [`MalwareCheckOutcome::NotApplicable`] for every form of INDIRECT
/// execution (`sh -c "npx evil"`, a wrapper script, an unrecognised runner) —
/// it reads the configured command, it does not contain what runs.
pub async fn check_package_for_malware(
    command: &str,
    args: &[String],
    endpoint: &str,
    backend: &dyn OsvBackend,
) -> MalwareCheckOutcome {
    let Some(ecosystem) = infer_ecosystem(command) else {
        return MalwareCheckOutcome::NotApplicable;
    };
    // `pipx list` is a package runner invocation that fetches nothing.
    let Some(args) = fetching_args(command, args) else {
        return MalwareCheckOutcome::NotApplicable;
    };
    let PackageRef::Identified { package, version } = parse_package_from_args(args, ecosystem)
    else {
        return MalwareCheckOutcome::Unidentified;
    };

    // Everything this argv installs, primary first. Duplicates are dropped so
    // `uvx --with pkg pkg` costs one query, not two.
    let mut targets = vec![(package, version)];
    for (package, version) in parse_extra_packages(args, ecosystem) {
        if !targets.iter().any(|(known, _)| *known == package) {
            targets.push((package, version));
        }
    }
    let subject = targets
        .iter()
        .map(|(package, _)| package.as_str())
        .collect::<Vec<_>>()
        .join(", ");

    if !is_safe_url(endpoint) {
        return MalwareCheckOutcome::Unavailable {
            subject,
            reason: "the configured OSV endpoint failed the SSRF safety check, \
                     so no query was sent"
                .to_string(),
        };
    }

    for (package, version) in &targets {
        match backend
            .query(endpoint, ecosystem, package, version.as_deref())
            .await
        {
            Ok(advisories) => {
                let malware = filter_malware(advisories);
                if !malware.is_empty() {
                    return MalwareCheckOutcome::Blocked(format_block_message(
                        package, ecosystem, &malware,
                    ));
                }
            }
            // One failed query means the check as a whole did not complete;
            // clearing the remaining packages would report a pass on strictly
            // less evidence than the caller thinks it has.
            Err(exc) => {
                return MalwareCheckOutcome::Unavailable {
                    subject,
                    reason: exc.to_string(),
                };
            }
        }
    }
    MalwareCheckOutcome::Allowed
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
        assert!(
            matches!(outcome, MalwareCheckOutcome::Unavailable { .. }),
            "a failed query is not a pass; the caller owns the fail-open \
             policy, got {outcome:?}"
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
        assert!(
            matches!(outcome, MalwareCheckOutcome::Unavailable { .. }),
            "an endpoint that failed the SSRF gate means the check did NOT \
             run, got {outcome:?}"
        );
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
