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
//! `wcore_mcp::malware_gate` documents the exact coverage boundary — which
//! runner forms are checked and which launch shapes run unchecked. Keep the two
//! in step: this module's `runner_forms` table IS that boundary.
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
    runner_forms(&runner_basename(command)).map(|forms| forms[0].ecosystem)
}

/// Extensions Windows resolves through `PATHEXT`. Node ships `npx` as
/// `npx.cmd` / `npx.ps1` and pnpm/yarn/uv ship `.cmd` shims too, so a table
/// keyed on the bare name is a table the same `config.toml` walks straight
/// past on Windows.
const EXECUTABLE_EXTENSIONS: [&str; 4] = [".exe", ".cmd", ".bat", ".ps1"];

/// Basename of `command`, lowercased, with any Windows executable extension
/// removed: `/usr/local/bin/npx`, `NPX.CMD` and `npx.exe` all yield `npx`.
pub fn runner_basename(command: &str) -> String {
    let base = std::path::Path::new(command)
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or(command)
        .to_ascii_lowercase();
    for ext in EXECUTABLE_EXTENSIONS {
        if let Some(stripped) = base.strip_suffix(ext) {
            return stripped.to_string();
        }
    }
    base
}

/// One shape in which a command fetches a package from a public registry and
/// executes it.
struct RunnerForm {
    ecosystem: Ecosystem,
    /// Sub-command words that must appear, consecutively, before the package
    /// arguments. Empty means the runner takes package arguments directly.
    subcommand: &'static [&'static str],
    /// When set, ONLY a token carrying this prefix names a fetched package.
    /// `deno run ./server.ts` runs a local file and fetches nothing; `deno run
    /// npm:evil` fetches from the npm registry.
    require_prefix: Option<&'static str>,
}

const fn direct(ecosystem: Ecosystem) -> RunnerForm {
    RunnerForm {
        ecosystem,
        subcommand: &[],
        require_prefix: None,
    }
}

const fn sub(ecosystem: Ecosystem, subcommand: &'static [&'static str]) -> RunnerForm {
    RunnerForm {
        ecosystem,
        subcommand,
        require_prefix: None,
    }
}

/// Every command form the gate knows fetches-and-executes from a registry.
///
/// The list is the gate's coverage, verbatim. Anything absent from it is
/// permitted as `NotApplicable`, which is why adding a runner here is the fix
/// for "the gate did not see it" and why the module doc enumerates the list
/// rather than claiming completeness.
fn runner_forms(basename: &str) -> Option<&'static [RunnerForm]> {
    // npm's own runner and the drop-in replacements that speak the same argv.
    const NPX: &[RunnerForm] = &[direct(Ecosystem::Npm)];
    const UVX: &[RunnerForm] = &[direct(Ecosystem::PyPI)];
    const PIPX: &[RunnerForm] = &[
        sub(Ecosystem::PyPI, &["run"]),
        sub(Ecosystem::PyPI, &["install"]),
    ];
    const NPM: &[RunnerForm] = &[sub(Ecosystem::Npm, &["exec"]), sub(Ecosystem::Npm, &["x"])];
    const PNPM: &[RunnerForm] = &[sub(Ecosystem::Npm, &["dlx"])];
    const YARN: &[RunnerForm] = &[sub(Ecosystem::Npm, &["dlx"])];
    const BUN: &[RunnerForm] = &[sub(Ecosystem::Npm, &["x"])];
    const UV: &[RunnerForm] = &[
        sub(Ecosystem::PyPI, &["tool", "run"]),
        sub(Ecosystem::PyPI, &["tool", "install"]),
    ];
    // `deno run npm:pkg` resolves through the npm registry; `deno run ./x.ts`
    // does not, which is what `require_prefix` distinguishes.
    const DENO: &[RunnerForm] = &[
        RunnerForm {
            ecosystem: Ecosystem::Npm,
            subcommand: &["run"],
            require_prefix: Some("npm:"),
        },
        RunnerForm {
            ecosystem: Ecosystem::Npm,
            subcommand: &["install"],
            require_prefix: Some("npm:"),
        },
    ];
    match basename {
        "npx" | "bunx" => Some(NPX),
        "uvx" => Some(UVX),
        "pipx" => Some(PIPX),
        "npm" => Some(NPM),
        "pnpm" => Some(PNPM),
        "yarn" => Some(YARN),
        "bun" => Some(BUN),
        "uv" => Some(UV),
        "deno" => Some(DENO),
        _ => None,
    }
}

/// Position of the first occurrence of `words` as consecutive tokens, or
/// `None`.
///
/// The scan runs over the WHOLE argv rather than requiring the sub-command to
/// be the first token, because a runner-level flag that takes a value
/// (`pnpm -C /srv dlx evil`) would otherwise push `dlx` out of first position
/// and the launch would be waved through unchecked. Over-detecting costs a
/// wasted OSV query; under-detecting costs the check.
fn find_subcommand(args: &[String], words: &[&str]) -> Option<usize> {
    if words.is_empty() {
        return Some(0);
    }
    args.windows(words.len())
        .position(|window| window.iter().zip(words).all(|(a, w)| a == w))
        .map(|start| start + words.len())
}

/// An `npm:` specifier may carry an entry-point sub-path
/// (`npm:@scope/pkg@1.2.3/bin`). Cut it so the version parser sees the
/// package token alone.
fn strip_specifier_subpath(token: &str) -> &str {
    if token.starts_with('@') {
        let mut slashes = token.match_indices('/');
        slashes.next();
        match slashes.next() {
            Some((idx, _)) => &token[..idx],
            None => token,
        }
    } else {
        match token.find('/') {
            Some(idx) => &token[..idx],
            None => token,
        }
    }
}

/// Resolve `(command, args)` to the registry fetch it performs, if any.
///
/// Returns the ecosystem plus the argv slice that names the package, with any
/// runner sub-command already consumed. `None` means this command fetches
/// nothing from a public registry.
fn classify_runner(command: &str, args: &[String]) -> Option<(Ecosystem, Vec<String>)> {
    let forms = runner_forms(&runner_basename(command))?;
    for form in forms {
        let Some(start) = find_subcommand(args, form.subcommand) else {
            continue;
        };
        let rest = &args[start..];
        match form.require_prefix {
            None => return Some((form.ecosystem, rest.to_vec())),
            Some(prefix) => {
                let specifier = rest.iter().find_map(|a| a.strip_prefix(prefix));
                // No registry specifier under this sub-command: a local script.
                let specifier = specifier?;
                return Some((
                    form.ecosystem,
                    vec![strip_specifier_subpath(specifier).to_string()],
                ));
            }
        }
    }
    None
}

/// Shell interpreters whose `-c` argument is a script, not a program name.
const SHELL_INTERPRETERS: [&str; 8] = [
    "sh",
    "bash",
    "zsh",
    "dash",
    "ksh",
    "cmd",
    "pwsh",
    "powershell",
];

/// Command words that prefix another command without being one.
const COMMAND_PREFIXES: [&str; 4] = ["exec", "env", "command", "nohup"];

/// Decompose `sh -c "<script>"` into the invocations the script runs.
///
/// `command = "sh", args = ["-c", "npx evil-pkg"]` is a package-runner launch
/// wearing a shell costume: the classifier sees `sh`, finds no runner, and the
/// launch proceeds unchecked. This splits the script on the separators that
/// start a new command and hands each segment back as `(program, args)`.
///
/// It is deliberately a TOKENISER, not a shell parser: it does not expand
/// variables, resolve quotes beyond stripping them, or follow `$( )`. A script
/// that hides its runner behind any of those is still not covered, and the
/// module doc says so.
fn shell_wrapped_invocations(command: &str, args: &[String]) -> Vec<(String, Vec<String>)> {
    if !SHELL_INTERPRETERS.contains(&runner_basename(command).as_str()) {
        return Vec::new();
    }
    let script_flags = ["-c", "/c", "/C", "-command", "-Command"];
    let Some(idx) = args.iter().position(|a| script_flags.contains(&a.as_str())) else {
        return Vec::new();
    };
    let Some(script) = args.get(idx + 1) else {
        return Vec::new();
    };
    script
        .split([';', '&', '|', '\n', '\r', '(', ')'])
        .filter_map(|segment| {
            let mut tokens = segment
                .split_whitespace()
                .map(|t| t.trim_matches(['"', '\'']).to_string())
                .filter(|t| !t.is_empty());
            let mut head = tokens.next()?;
            // `exec npx …`, `env FOO=1 npx …` — step over the prefix words and
            // any leading VAR=value assignments.
            while COMMAND_PREFIXES.contains(&head.as_str()) || head.contains('=') {
                head = tokens.next()?;
            }
            Some((head, tokens.collect::<Vec<String>>()))
        })
        .collect()
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
    // The literal invocation, plus every command a shell wrapper would run on
    // its behalf (two levels — `sh -c "sh -c '…'"` is the last shape worth the
    // walk; deeper nesting is uncovered and the module doc says so).
    let mut invocations: Vec<(String, Vec<String>)> = vec![(command.to_string(), args.to_vec())];
    let mut frontier = shell_wrapped_invocations(command, args);
    for _ in 0..2 {
        if frontier.is_empty() {
            break;
        }
        let next = frontier
            .iter()
            .flat_map(|(c, a)| shell_wrapped_invocations(c, a))
            .collect::<Vec<_>>();
        invocations.append(&mut frontier);
        frontier = next;
    }

    let mut checked_a_runner = false;
    for (program, argv) in &invocations {
        match check_one_invocation(program, argv, endpoint, backend).await {
            MalwareCheckOutcome::NotApplicable => continue,
            MalwareCheckOutcome::Allowed => checked_a_runner = true,
            // A single blocked or unreadable runner condemns the whole launch:
            // the shell would run it.
            refusal => return refusal,
        }
    }
    if checked_a_runner {
        MalwareCheckOutcome::Allowed
    } else {
        MalwareCheckOutcome::NotApplicable
    }
}

async fn check_one_invocation(
    command: &str,
    args: &[String],
    endpoint: &str,
    backend: &dyn OsvBackend,
) -> MalwareCheckOutcome {
    let Some((ecosystem, package_args)) = classify_runner(command, args) else {
        return MalwareCheckOutcome::NotApplicable;
    };
    if !is_safe_url(endpoint) {
        // Logged at ERROR, not WARN. The CLI caps its stderr writer at
        // `Level::ERROR` (`wcore-cli/src/main.rs`), so with `RUST_LOG` unset a
        // `warn!` reaches the log file and nobody else — the same trap #928
        // documents. An endpoint that fails the SSRF gate is always operator-
        // visible misconfiguration or an active attack, never normal traffic.
        tracing::error!(target: "wcore::osv_check", endpoint, "refusing to query unsafe OSV endpoint (SSRF gate)");
        return MalwareCheckOutcome::Allowed;
    }
    let PackageRef::Identified { package, version } =
        parse_package_from_args(&package_args, ecosystem)
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
            // Fail OPEN, and say so at ERROR.
            //
            // This used to read "warn! is the lowest level that reaches a user
            // with no RUST_LOG set". That was false: `wcore-cli` builds its
            // stderr writer as `stderr.with_max_level(tracing::Level::ERROR)`,
            // so with `RUST_LOG` unset ERROR is the ONLY level that reaches the
            // person launching the server. A `warn!` here meant the fail-open
            // was invisible — which is the whole defect #340 reports, because
            // blocking `api.osv.dev` is then enough to get a known-malicious
            // package executed with nobody told the check did not run.
            tracing::error!(
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

    // ---------------------------------------------------------------------
    // #340 — runner coverage. One test per runner FORM, plus the negative
    // controls that must hold in both the broken and the fixed tree.
    // ---------------------------------------------------------------------

    /// Run the gate against a backend that reports the package clean, and
    /// return the package names the gate actually asked OSV about.
    async fn queried_packages(command: &str, args: &[&str]) -> Vec<String> {
        let backend = CapturingOsvBackend::with_response(vec![]);
        let owned: Vec<String> = args.iter().map(|s| s.to_string()).collect();
        let _ = check_package_for_malware(command, &owned, TEST_OSV_ENDPOINT, &backend).await;
        let calls = backend.calls.lock();
        calls.iter().map(|c| c.package.clone()).collect()
    }

    async fn outcome(
        command: &str,
        args: &[&str],
        advisories: Vec<OsvAdvisory>,
    ) -> MalwareCheckOutcome {
        let backend = CapturingOsvBackend::with_response(advisories);
        let owned: Vec<String> = args.iter().map(|s| s.to_string()).collect();
        check_package_for_malware(command, &owned, TEST_OSV_ENDPOINT, &backend).await
    }

    fn mal() -> Vec<OsvAdvisory> {
        vec![OsvAdvisory {
            id: "MAL-2024-0001".into(),
            summary: "malware".into(),
        }]
    }

    #[tokio::test]
    async fn pipx_run_queries_the_package_not_the_subcommand() {
        assert_eq!(
            queried_packages("pipx", &["run", "evil-pkg"]).await,
            vec!["evil-pkg"]
        );
        assert_eq!(
            queried_packages("pipx", &["install", "evil-pkg"]).await,
            vec!["evil-pkg"]
        );
        assert_eq!(
            queried_packages("pipx", &["run", "--spec", "evil-pkg", "entry"]).await,
            vec!["evil-pkg"]
        );
    }

    #[tokio::test]
    async fn windows_executable_extensions_are_the_same_runner() {
        for command in ["npx.cmd", "npx.exe", "NPX.CMD", "npx.ps1", "npx.bat"] {
            assert_eq!(
                queried_packages(command, &["evil-pkg"]).await,
                vec!["evil-pkg"],
                "{command} must be recognised as npx"
            );
        }
        assert_eq!(
            queried_packages("uvx.exe", &["evil-pkg"]).await,
            vec!["evil-pkg"]
        );
        assert_eq!(
            queried_packages("pipx.exe", &["run", "evil-pkg"]).await,
            vec!["evil-pkg"]
        );
    }

    #[tokio::test]
    async fn every_registry_runner_form_is_checked() {
        let cases: Vec<(&str, Vec<&str>)> = vec![
            ("npx", vec!["-y", "evil-pkg"]),
            ("bunx", vec!["evil-pkg"]),
            ("bun", vec!["x", "evil-pkg"]),
            ("pnpm", vec!["dlx", "evil-pkg"]),
            ("yarn", vec!["dlx", "evil-pkg"]),
            ("npm", vec!["exec", "evil-pkg"]),
            ("npm", vec!["x", "evil-pkg"]),
            ("uvx", vec!["evil-pkg"]),
            ("uv", vec!["tool", "run", "evil-pkg"]),
            ("deno", vec!["run", "-A", "npm:evil-pkg"]),
        ];
        for (command, args) in cases {
            assert_eq!(
                queried_packages(command, &args).await,
                vec!["evil-pkg"],
                "{command} {args:?} must reach OSV"
            );
            assert!(
                matches!(
                    outcome(command, &args, mal()).await,
                    MalwareCheckOutcome::Blocked(_)
                ),
                "{command} {args:?} must be BLOCKED when the package is known malware"
            );
        }
    }

    #[tokio::test]
    async fn a_shell_wrapped_runner_is_not_a_free_pass() {
        for args in [
            vec!["-c", "npx evil-pkg"],
            vec!["-c", "cd /srv && npx -y evil-pkg"],
            vec!["-c", "exec npx evil-pkg"],
            vec!["-c", "env FOO=1 npx evil-pkg"],
            vec!["-c", "pipx run evil-pkg"],
        ] {
            assert!(
                matches!(
                    outcome("sh", &args, mal()).await,
                    MalwareCheckOutcome::Blocked(_)
                ),
                "sh {args:?} must be BLOCKED"
            );
        }
        assert!(matches!(
            outcome("cmd", &["/C", "npx evil-pkg"], mal()).await,
            MalwareCheckOutcome::Blocked(_)
        ));
    }

    // --- NEGATIVE CONTROLS: these must pass in BOTH arms ------------------

    #[tokio::test]
    async fn ordinary_launch_commands_are_still_not_applicable() {
        for (command, args) in [
            ("node", vec!["/opt/server/index.js"]),
            ("python3", vec!["-m", "my_server"]),
            ("/usr/local/bin/my-mcp-server", vec!["--port", "0"]),
            ("deno", vec!["run", "-A", "./server.ts"]),
            ("sh", vec!["-c", "exec /usr/local/bin/my-mcp-server"]),
            ("sh", vec!["-c", "echo hello"]),
        ] {
            assert_eq!(
                outcome(command, &args, mal()).await,
                MalwareCheckOutcome::NotApplicable,
                "{command} {args:?} fetches nothing from a registry"
            );
        }
    }

    #[tokio::test]
    async fn a_clean_runner_package_is_still_allowed() {
        assert_eq!(
            outcome(
                "npx",
                &["-y", "@modelcontextprotocol/server-filesystem"],
                vec![]
            )
            .await,
            MalwareCheckOutcome::Allowed
        );
        assert_eq!(
            outcome("pipx", &["run", "mcp-server-git"], vec![]).await,
            MalwareCheckOutcome::Allowed
        );
    }

    #[tokio::test]
    async fn a_runner_with_no_readable_package_is_still_unidentified() {
        assert_eq!(
            outcome("npx", &["--userconfig", "/tmp/x"], vec![]).await,
            MalwareCheckOutcome::Unidentified
        );
    }

    /// #340 — the fail-open must be VISIBLE. `wcore-cli` builds its stderr
    /// writer as `stderr.with_max_level(tracing::Level::ERROR)`, so ERROR is
    /// the only level a user with no `RUST_LOG` set ever sees. This asserts on
    /// the recorded level rather than the message so a downgrade back to
    /// `warn!` — which still "logs the failure" — reddens it.
    #[tokio::test]
    async fn fail_open_is_visible_at_default_log_levels() {
        use std::sync::{Arc, Mutex};
        use tracing_subscriber::layer::SubscriberExt;

        #[derive(Clone, Default)]
        struct Captured(Arc<Mutex<Vec<tracing::Level>>>);

        impl<S: tracing::Subscriber> tracing_subscriber::Layer<S> for Captured {
            fn on_event(
                &self,
                event: &tracing::Event<'_>,
                _ctx: tracing_subscriber::layer::Context<'_, S>,
            ) {
                self.0.lock().unwrap().push(*event.metadata().level());
            }
        }

        let captured = Captured::default();
        let subscriber = tracing_subscriber::registry::Registry::default().with(captured.clone());
        let guard = tracing::subscriber::set_default(subscriber);

        let backend = CapturingOsvBackend::with_error(OsvBackendError::Network(
            "api.osv.dev is unreachable".into(),
        ));
        let args = vec!["-y".to_string(), "some-pkg".to_string()];
        assert_eq!(
            check_package_for_malware("npx", &args, TEST_OSV_ENDPOINT, &backend).await,
            MalwareCheckOutcome::Allowed,
            "an unreachable OSV endpoint must still fail OPEN"
        );
        drop(guard);

        let levels = captured.0.lock().unwrap().clone();
        assert_eq!(
            levels,
            vec![tracing::Level::ERROR],
            "the launch went ahead unchecked; at RUST_LOG-unset the user only \
             ever sees ERROR, so anything quieter tells them nothing"
        );
    }

    /// The SSRF refusal is the other fail-open, and it is invisible for the
    /// same reason if it is not ERROR.
    #[tokio::test]
    async fn ssrf_refusal_is_visible_at_default_log_levels() {
        use std::sync::{Arc, Mutex};
        use tracing_subscriber::layer::SubscriberExt;

        #[derive(Clone, Default)]
        struct Captured(Arc<Mutex<Vec<tracing::Level>>>);

        impl<S: tracing::Subscriber> tracing_subscriber::Layer<S> for Captured {
            fn on_event(
                &self,
                event: &tracing::Event<'_>,
                _ctx: tracing_subscriber::layer::Context<'_, S>,
            ) {
                self.0.lock().unwrap().push(*event.metadata().level());
            }
        }

        let captured = Captured::default();
        let subscriber = tracing_subscriber::registry::Registry::default().with(captured.clone());
        let guard = tracing::subscriber::set_default(subscriber);

        let backend = CapturingOsvBackend::with_response(vec![]);
        let args = vec!["some-pkg".to_string()];
        assert_eq!(
            check_package_for_malware("npx", &args, "http://169.254.169.254/v1/query", &backend)
                .await,
            MalwareCheckOutcome::Allowed
        );
        drop(guard);

        assert_eq!(
            captured.0.lock().unwrap().clone(),
            vec![tracing::Level::ERROR]
        );
    }
}
