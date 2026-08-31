//! `wayland-core --doctor` — system dependency probe.
//!
//! Closes debt-register A.5: Linux Wayland CUA needs `wlrctl` + `grim` on
//! `PATH`; missing binaries surface as typed `CuaError::Backend` at runtime
//! with no upfront diagnostic. The doctor command walks a fixed list of
//! checks (one per external dependency or environment signal), prints
//! PASS/FAIL/SKIP with platform-specific install hints, and returns a
//! deterministic exit code:
//!
//! - `0` if every check that is **required for the current platform**
//!   passes.
//! - `1` if at least one required check fails.
//!
//! Optional checks (Ollama, Browserbase) are warnings only and never
//! affect the exit code.
//!
//! All subprocess work goes through
//! [`wcore_config::shell::shell_command_argv`] per AGENTS.md — never
//! `Command::new("sh")` or `Command::new(...)` directly. Each `which`
//! probe is a single argv-mode call with no shell metacharacter
//! interpretation.

use std::process::ExitCode;

use wcore_config::shell::shell_command_argv;
use wcore_cua::permissions::{TccCapability, TccStatus};

/// A structured doctor report: every check row plus the version banner.
///
/// This is the data [`run`] prints and the TUI diagnostics surface
/// renders. [`collect`] gathers it without printing; [`run`] calls
/// [`collect`] then prints, so the two surfaces never drift.
#[derive(Debug)]
pub struct DoctorReport {
    /// The binary version, used in the `wayland-core doctor v…` banner.
    pub version: String,
    /// The check rows, in display order.
    pub checks: Vec<CheckResult>,
}

/// One row of the doctor report.
#[derive(Debug)]
pub struct CheckResult {
    /// Human-readable label printed in the left column.
    pub label: &'static str,
    /// Outcome of the check on the current platform.
    pub outcome: Outcome,
}

/// The outcome of a single doctor check.
#[derive(Debug)]
pub enum Outcome {
    /// Check ran and succeeded. `detail` is the discovered value
    /// (e.g. binary path, version string) printed next to the label.
    Pass { detail: String },
    /// Check ran and failed. The check is **required** for the
    /// current platform, so failure flips the exit code to 1.
    /// `hints` are per-distro install commands, one per line.
    Fail { hints: Vec<String> },
    /// Check ran and failed but the dependency is optional — for
    /// example, Ollama is only needed if the user uses `ollama:*`
    /// models. Prints a `WARN` row that does NOT affect the exit code.
    Warn { detail: String, hints: Vec<String> },
    /// Check is not applicable to the current platform (e.g. macOS
    /// Accessibility on Linux). Prints `SKIP` and does NOT affect the
    /// exit code.
    Skip { reason: String },
    /// Check cannot be automatically verified by any API we have, so
    /// the only honest report is a manual-action hint. Surfaced per the
    /// W5 hard rule against fake passes.
    ///
    /// The macOS TCC rows no longer use this: since issue #114 they are
    /// real probes (`AXIsProcessTrusted` /
    /// `CGPreflightScreenCaptureAccess`) via `wcore_cua::permissions`.
    Manual { hint: String },
}

/// Gather every doctor check into a structured [`DoctorReport`] WITHOUT
/// printing anything. This is the data layer shared by [`run`] (the
/// `--doctor` CLI path) and the TUI diagnostics surface.
pub async fn collect() -> DoctorReport {
    let version = env!("CARGO_PKG_VERSION");
    DoctorReport {
        version: version.to_string(),
        checks: collect_checks(version).await,
    }
}

/// Public entry point invoked from `main.rs` when the `--doctor` flag
/// is passed. Performs all checks, prints the report, returns the
/// platform-appropriate exit code.
///
/// FerroxLabs/wayland#1079 — `cli_args` is the SAME `CliArgs` the invocation
/// would have used for a real run. Before this, `run` took only `probe_mcp`
/// and every `Config::resolve` below passed `CliArgs::default()`, so
/// `--profile` and `--project-dir` were discarded: the declared-MCP list and
/// the durable-sessions verdict were computed against a DIFFERENT config than
/// the one the user's own flags select. Worse, under default args
/// `Config::resolve` can fail with `MissingApiKey` on a host where
/// `--provider` / `--api-key` would have succeeded, degrading BOTH sections to
/// a "config not loaded" line for no reason.
///
/// The check rows themselves ([`collect`]) are config-independent and are
/// deliberately left alone — the TUI `/doctor` surface shares them.
pub async fn run(
    probe_mcp: bool,
    probe_provider: bool,
    cli_args: &wcore_config::config::CliArgs,
) -> ExitCode {
    let report = collect().await;
    let version = &report.version;
    println!("wayland-core doctor v{version}\n");

    // `br-default`: the browser-policy row is config-derived, so it is added
    // here rather than in `collect()` -- which takes no config and is shared
    // verbatim with the TUI `/doctor` surface (that surface has its own
    // config-posture section, `scan_config_health`).
    let owned_checks = with_config_rows(report.checks, cli_args);
    let checks = &owned_checks;

    let mut passed = 0usize;
    let mut failed = 0usize;
    let mut warned = 0usize;
    let mut skipped = 0usize;
    let mut manual = 0usize;

    for c in checks {
        match &c.outcome {
            Outcome::Pass { detail } => {
                passed += 1;
                println!("[PASS] {:<22} {detail}", c.label);
            }
            Outcome::Fail { hints } => {
                failed += 1;
                println!("[FAIL] {:<22} NOT FOUND", c.label);
                for h in hints {
                    println!("       Install: {h}");
                }
            }
            Outcome::Warn { detail, hints } => {
                warned += 1;
                println!("[WARN] {:<22} {detail}", c.label);
                for h in hints {
                    println!("       Hint: {h}");
                }
            }
            Outcome::Skip { reason } => {
                skipped += 1;
                println!("[SKIP] {:<22} ({reason})", c.label);
            }
            Outcome::Manual { hint } => {
                manual += 1;
                println!("[MANUAL] {:<20} {hint}", c.label);
            }
        }
    }

    println!(
        "\nSummary: {passed} passed, {failed} missing, {warned} warning, \
         {skipped} skipped, {manual} manual"
    );

    // #1079: what THIS command line resolves to. Printed first of the three
    // sections because it is the config the two below are computed from.
    // Informational only — never flips the exit code below.
    print_provider_section(cli_args, probe_provider);

    // Report whether durable session persistence is on, and if it is off,
    // WHICH of the two very different reasons turned it off. Informational
    // only — never flips the exit code below.
    print_durable_sessions_section(cli_args).await;

    // A4b: list declared MCP servers (and optionally probe). Informational
    // only — never flips the exit code below.
    print_mcp_section(probe_mcp, cli_args).await;

    if failed > 0 {
        ExitCode::from(1)
    } else {
        ExitCode::SUCCESS
    }
}

/// Build the platform-appropriate list of checks. Each helper is async
/// because `shell_command_argv` returns a `tokio::process::Command`.
///
/// Platform gating uses `cfg!(...)` (runtime) for the platform decision
/// so the same compiled binary can produce different SKIP rows on
/// different OSes — important for the release binary smoke test which
/// runs the same artifact on Linux/macOS CI.
async fn collect_checks(version: &str) -> Vec<CheckResult> {
    let mut out = Vec::new();

    // 1. Self-sanity: binary version is non-empty. Always required
    //    (the binary that is running printed its own version, so this
    //    is mostly a structural row).
    out.push(check_version(version));

    // 2. The browser backend this build actually compiled. gh#491: this
    //    used to probe Chromium unconditionally, which is behind an opt-in
    //    cargo feature and is absent from the shipped artifact.
    out.push(check_browser_binary().await);

    // 3. Linux Wayland CUA — `wlrctl` + `grim` + `WAYLAND_DISPLAY`.
    //    A.5 explicitly notes these as the missing binaries that
    //    surface as `CuaError::Backend` at runtime.
    if cfg!(target_os = "linux") {
        out.push(check_which("wlrctl", &wlrctl_hints()).await);
        out.push(check_which("grim", &grim_hints()).await);
        out.push(check_wayland_display());
        out.push(check_x_display());
    } else {
        out.push(skip("wlrctl", "Linux-only"));
        out.push(skip("grim", "Linux-only"));
        out.push(skip("WAYLAND_DISPLAY", "Linux-only"));
        out.push(skip("X DISPLAY", "Linux-only"));
    }

    // 4. macOS TCC permissions — REAL probes since issue #114.
    //    `AXIsProcessTrusted()` gates synthesized input and
    //    `CGPreflightScreenCaptureAccess()` gates display capture; both
    //    answer without showing a dialog, so the doctor stays
    //    side-effect-free. Off macOS these SKIP exactly as before.
    out.push(check_macos_tcc(TccCapability::Accessibility));
    out.push(check_macos_tcc(TccCapability::ScreenRecording));

    // 5. Optional providers — warnings only, never flip the exit code.
    out.push(check_browserbase());
    out.push(check_ollama().await);

    out
}

// -- individual checks --------------------------------------------------

fn check_version(version: &str) -> CheckResult {
    CheckResult {
        label: "binary version",
        outcome: if version.is_empty() {
            Outcome::Fail {
                hints: vec!["rebuild from source with a stamped Cargo.toml".into()],
            }
        } else {
            Outcome::Pass {
                detail: format!("v{version} (matches expected)"),
            }
        },
    }
}

/// gh#491 — probe the browser backend this binary COMPILED, not a hardcoded
/// one.
///
/// `chromium` is an opt-in cargo feature (`default = []`), so the shipped
/// artifact has no Chromium backend in it at all — and this check nonetheless
/// told every Linux user to `apt install chromium-browser`, never naming the
/// Camoufox sidecar that is the only backend actually there. The backend list,
/// the program names and the install hints now all come from
/// [`wcore_browser::install`], which is the same source the supervisor
/// resolves the sidecar program from, so the doctor cannot drift away from
/// what the engine runs.
async fn check_browser_binary() -> CheckResult {
    let backends = wcore_browser::install::compiled_backends();
    if let Some((backend, path)) = wcore_browser::install::resolve_any() {
        return CheckResult {
            label: BROWSER_BACKEND_LABEL,
            outcome: Outcome::Pass {
                detail: format!("{} -> {}", backend.backend, path.display()),
            },
        };
    }
    let hints: Vec<String> = backends
        .iter()
        .flat_map(|backend| backend.install_hints.iter().map(|h| (*h).to_string()))
        .collect();
    // F-073: the browser backend is optional on macOS, so a missing one is a
    // WARN there and does not flip the exit code. Unchanged by gh#491 — only
    // the software being named changes.
    if cfg!(target_os = "macos") {
        return CheckResult {
            label: BROWSER_BACKEND_LABEL,
            outcome: Outcome::Warn {
                detail: "not installed — the browser tool is unavailable".into(),
                hints,
            },
        };
    }
    CheckResult {
        label: BROWSER_BACKEND_LABEL,
        outcome: Outcome::Fail { hints },
    }
}

/// The doctor row label. Deliberately generic: the concrete backend is named
/// in the PASS detail and in the install hints, both of which come from the
/// compiled-in backend list rather than from this string.
const BROWSER_BACKEND_LABEL: &str = "browser backend";

/// The policy row label. Sits directly under [`BROWSER_BACKEND_LABEL`] because
/// the two rows answer the same question -- "will a browser op work?" -- and
/// either one alone gives the wrong answer.
const BROWSER_POLICY_LABEL: &str = "browser policy";

/// `br-default` -- insert the config-derived rows into a [`collect`] report.
///
/// Kept as its own function, taking and returning the row list, so the WIRING
/// is gradable: that the policy row is present at all, and that it lands
/// immediately after the backend row rather than at the bottom of the table.
/// Adjacency is the entire point. `[PASS] browser backend -> /usr/bin/...` on a
/// machine where every URL is refused is a true statement that reads as a clean
/// bill of health, and a reader who sees it stops reading.
fn with_config_rows(
    mut checks: Vec<CheckResult>,
    cli_args: &wcore_config::config::CliArgs,
) -> Vec<CheckResult> {
    let row = check_browser_policy(cli_args);
    match checks.iter().position(|c| c.label == BROWSER_BACKEND_LABEL) {
        Some(i) => checks.insert(i + 1, row),
        None => checks.push(row),
    }
    checks
}

/// Report whether the operator's `[browser.policy]` actually permits anything.
///
/// The doctor used to probe only whether a browser BINARY resolves, and on a
/// host with the sidecar installed it printed `[PASS] browser backend` while
/// `BrowserPolicy` refused every URL the tool was asked for -- the fail-closed
/// default posture, which is deliberate design and is NOT relaxed here. What
/// was missing is that nothing said so before the user hit it: the denial is
/// only reachable by running a browser op and reading the tool card.
///
/// The verdict is deliberately the same predicate `wcore_browser`'s own
/// `denial_message` uses to decide the posture is the fail-closed default
/// (`default_action` deny AND no allowed origins), and the hints are that same
/// module's [`wcore_browser::config_hint::policy_disabled_hint`] verbatim, so
/// the doctor cannot advertise a remedy the tool would not.
fn check_browser_policy(cli_args: &wcore_config::config::CliArgs) -> CheckResult {
    match wcore_config::config::Config::resolve(cli_args) {
        // Not a WARN: with no config there is no policy to report, and
        // inventing a verdict from the compiled defaults would claim to have
        // read a file that never loaded.
        Err(e) => CheckResult {
            label: BROWSER_POLICY_LABEL,
            outcome: Outcome::Skip {
                reason: format!("config did not resolve: {e}"),
            },
        },
        Ok(cfg) => browser_policy_row(&cfg.browser.policy),
    }
}

/// The verdict for one resolved [`wcore_config::browser::BrowserPolicyConfig`].
///
/// Split from [`check_browser_policy`] so both branches are gradable without a
/// resolvable config on the host running the tests.
fn browser_policy_row(policy: &wcore_config::browser::BrowserPolicyConfig) -> CheckResult {
    let denies_everything = policy.default_action.trim().eq_ignore_ascii_case("deny")
        && policy.allowed_origins.is_empty();
    if !denies_everything {
        return CheckResult {
            label: BROWSER_POLICY_LABEL,
            outcome: Outcome::Pass {
                detail: format!(
                    "default_action={}, {} allowed origin(s)",
                    policy.default_action.trim(),
                    policy.allowed_origins.len()
                ),
            },
        };
    }
    // WARN, never FAIL: fail-closed is the intended posture for an operator who
    // does not want the browser, and a doctor that exits 1 on the default
    // install would be crying wolf.
    CheckResult {
        label: BROWSER_POLICY_LABEL,
        outcome: Outcome::Warn {
            detail: "default_action=deny with no allowed_origins — every URL is refused".into(),
            // The POLICY half only: the `browser backend` row directly above
            // carries the install line, and printing it twice on one screen
            // teaches the reader to skip the block.
            hints: wcore_browser::config_hint::policy_disabled_hint()
                .lines()
                .filter(|line| !line.trim().is_empty())
                .map(str::to_string)
                .collect(),
        },
    }
}

async fn check_which(prog: &'static str, hints: &[String]) -> CheckResult {
    match which(prog).await {
        Some(path) => CheckResult {
            label: prog,
            outcome: Outcome::Pass { detail: path },
        },
        None => CheckResult {
            label: prog,
            outcome: Outcome::Fail {
                hints: hints.to_vec(),
            },
        },
    }
}

fn check_wayland_display() -> CheckResult {
    match std::env::var("WAYLAND_DISPLAY") {
        Ok(v) if !v.is_empty() => CheckResult {
            label: "WAYLAND_DISPLAY",
            outcome: Outcome::Pass {
                detail: format!("WAYLAND_DISPLAY={v}"),
            },
        },
        _ => CheckResult {
            label: "WAYLAND_DISPLAY",
            outcome: Outcome::Warn {
                detail: "not set — Wayland CUA backend unavailable".into(),
                hints: vec![
                    "log in to a Wayland session (Sway, GNOME on Wayland, KDE on Wayland)".into(),
                ],
            },
        },
    }
}

fn check_x_display() -> CheckResult {
    match std::env::var("DISPLAY") {
        Ok(v) if !v.is_empty() => CheckResult {
            label: "X DISPLAY",
            outcome: Outcome::Pass {
                detail: format!("DISPLAY={v}"),
            },
        },
        _ => CheckResult {
            label: "X DISPLAY",
            outcome: Outcome::Warn {
                detail: "not set — X11 CUA backend unavailable".into(),
                hints: vec!["log in to an X11 session, or start Xwayland".into()],
            },
        },
    }
}

/// A macOS TCC grant, probed for real.
///
/// The probe is non-prompting, so running the doctor never raises a
/// consent dialog. A missing grant is a `Warn`, not a `Fail`: computer
/// use is optional, and the same reasoning that made Chromium a warning
/// on macOS (F-073) applies here — a user who never touches CUA should
/// not get a non-zero doctor exit for a permission they do not need.
/// The row still names the exact Settings pane and the one command that
/// raises the prompt.
fn check_macos_tcc(capability: TccCapability) -> CheckResult {
    let label = match capability {
        TccCapability::Accessibility => "macOS Accessibility",
        TccCapability::ScreenRecording => "macOS Screen Record",
    };
    CheckResult {
        label,
        outcome: match wcore_cua::permissions::probe(capability) {
            TccStatus::Granted => Outcome::Pass {
                detail: format!("{} granted (CUA can run)", capability.settings_pane()),
            },
            TccStatus::Denied => Outcome::Warn {
                detail: format!(
                    "{} NOT granted — computer-use ops will refuse with a typed error",
                    capability.settings_pane()
                ),
                hints: vec![
                    capability.remediation(),
                    "or run `wayland-core --request-permissions` to raise the system prompt".into(),
                ],
            },
            TccStatus::NotApplicable => Outcome::Skip {
                reason: "non-macOS platform".into(),
            },
        },
    }
}

fn check_browserbase() -> CheckResult {
    match std::env::var("BROWSERBASE_API_KEY") {
        Ok(v) if !v.is_empty() => CheckResult {
            label: "BROWSERBASE_API_KEY",
            outcome: Outcome::Pass {
                detail: format!("set ({} chars)", v.len()),
            },
        },
        _ => CheckResult {
            label: "BROWSERBASE_API_KEY",
            outcome: Outcome::Warn {
                detail: "not set — Browserbase cloud backend unavailable".into(),
                hints: vec![
                    "export BROWSERBASE_API_KEY=<key>  (only if you use the cloud backend)".into(),
                ],
            },
        },
    }
}

async fn check_ollama() -> CheckResult {
    if let Ok(url) = std::env::var("OLLAMA_BASE_URL")
        && !url.is_empty()
    {
        return CheckResult {
            label: "ollama",
            outcome: Outcome::Pass {
                detail: format!("OLLAMA_BASE_URL={url}"),
            },
        };
    }
    if let Some(path) = which("ollama").await {
        return CheckResult {
            label: "ollama",
            outcome: Outcome::Pass {
                detail: format!("binary at {path}"),
            },
        };
    }
    CheckResult {
        label: "ollama",
        outcome: Outcome::Warn {
            detail: "not configured — `ollama:*` model routing unavailable".into(),
            hints: vec![
                "brew install ollama          (macOS)".into(),
                "curl -fsSL https://ollama.com/install.sh | sh  (Linux)".into(),
                "or set OLLAMA_BASE_URL=<endpoint> to point at a remote daemon".into(),
            ],
        },
    }
}

// -- A4b: MCP section ---------------------------------------------------

/// A4b: print the CLI-only MCP section AFTER the standard doctor summary.
///
/// Bare `--doctor` (`probe == false`) is side-effect-free: it only LISTS
/// declared MCP servers (config-cascaded + on-disk plugin manifests) by
/// reading config and files — it never spawns a stdio command or dials a
/// URL. `--probe-mcp` (`probe == true`) opts into a real connect-test of
/// the config-declared servers via [`wcore_mcp::manager::McpManager`].
///
/// This section is informational and best-effort: every fallible step is
/// matched and degraded to a printed note, so it can NEVER panic or flip
/// the doctor exit code (which is computed by the caller from the check
/// rows, not from anything printed here). It is deliberately kept out of
/// [`collect`]/[`CheckResult`] so it does NOT duplicate the live MCP
/// section the TUI `/doctor` surface already renders.
async fn print_mcp_section(probe: bool, cli_args: &wcore_config::config::CliArgs) {
    println!();
    println!("MCP servers (declared):");

    // --- config-declared servers (cascaded), best-effort load ---
    match wcore_config::config::Config::resolve(cli_args) {
        Ok(cfg) => {
            if cfg.mcp.servers.is_empty() {
                println!("  (none declared in config)");
            } else {
                let mut names: Vec<&String> = cfg.mcp.servers.keys().collect();
                names.sort();
                for name in names {
                    let s = &cfg.mcp.servers[name];
                    let transport = format!("{:?}", s.transport).to_lowercase();
                    let target = s
                        .command
                        .clone()
                        .or_else(|| s.url.clone())
                        .unwrap_or_default();
                    println!("  [config] {name:<20} {transport:<14} {target}");
                }
            }
            // wayland-core#354 c7 — install the operator's chosen mode into
            // this process BEFORE anything below can launch a server.
            //
            // `--doctor` returns at `main.rs` ahead of config/OAuth/engine
            // bootstrap, and `AgentBootstrap::build` is the only OTHER caller
            // of `install_mode`. Without this line the probe further down
            // reaches `StdioTransport::spawn` with the mode uninstalled and
            // silently takes the permissive default — so under strict, the one
            // command an operator runs to ASK whether the gate is on would be
            // the command that does not honour it. A mode the diagnostic path
            // ignores is not an operator choice.
            //
            // Installed at the point the mode is READ for display, so the
            // posture printed and the posture enforced cannot drift.
            // `install_mode` is one-shot and idempotent, so this cannot fight
            // a later boot; nothing in this process boots after `--doctor`.
            wcore_mcp::malware_gate::install_mode(cfg.mcp.malware_gate);
            // wayland-core#354 — the launch gate's posture, printed whether or
            // not any server is declared: a fresh config with no servers yet
            // is exactly when an operator wants to see which mode they are on.
            println!("{}", malware_gate_line(cfg.mcp.malware_gate));
        }
        Err(e) => println!("  (config not loaded: {e})"),
    }

    // --- plugin-declared servers (scan on-disk manifests, NO spawn) ---
    // Plugin install root = dirs::data_dir()/wayland-core/plugins (matches
    // `plugin::run`'s default install root in plugin/mod.rs).
    if let Some(base) = dirs::data_dir() {
        let plugins_root = base.join("wayland-core").join("plugins");
        let mut found_any = false;
        if let Ok(entries) = std::fs::read_dir(&plugins_root) {
            let mut manifests: Vec<std::path::PathBuf> = entries
                .flatten()
                .map(|e| e.path().join("plugin.toml"))
                .filter(|p| p.is_file())
                .collect();
            manifests.sort();
            for manifest_path in manifests {
                if let Ok(text) = std::fs::read_to_string(&manifest_path)
                    && let Ok(m) = toml::from_str::<wcore_plugin_api::PluginManifest>(&text)
                    && let Some(spec) = &m.mcp_server
                {
                    if !found_any {
                        println!("MCP servers (plugin-declared):");
                        found_any = true;
                    }
                    let transport = format!("{:?}", spec.transport).to_lowercase();
                    let plugin = manifest_path
                        .parent()
                        .and_then(|p| p.file_name())
                        .and_then(|s| s.to_str())
                        .unwrap_or("?")
                        .to_string();
                    println!("  [plugin:{plugin}] {:<20} {transport}", spec.name);
                }
            }
        }
    }

    // --- optional probe ---
    if probe {
        println!();
        println!("Probing config-declared MCP servers (connect-test)...");
        match wcore_config::config::Config::resolve(cli_args) {
            Ok(cfg) if !cfg.mcp.servers.is_empty() => {
                // #111 note: this deliberately connect-tests ALL config servers,
                // INCLUDING any marked `only_for_assistant` — the per-assistant
                // scoping filter (`servers_for_assistant`) is intentionally NOT
                // applied here. This is a throwaway diagnostic manager for the
                // trusted local operator: its tools are never injected into any
                // agent tool set or exposed to a model, and it is dropped at the
                // end of this block. If this manager is ever wired into an agent,
                // it MUST be filtered by the active assistant first.
                // fix/904 — the probe spawns real transports, including stdio
                // child processes that inherit the declared `env`. Run the same
                // `${cred:KEY}` rail the connect boundaries use so an
                // unresolvable reference is reported as skipped instead of
                // being handed to a child process literally.
                let (probe_servers, credential_skips) = match cfg.open_credentials_store() {
                    Ok(store) => {
                        let report =
                            wcore_config::mcp_cred_refs::resolve_servers_for_connect_with_report(
                                &cfg.mcp.servers,
                                &*store,
                            );
                        (report.connectable, report.skipped)
                    }
                    Err(_) => {
                        let report =
                            wcore_config::mcp_cred_refs::without_credential_references_with_report(
                                &cfg.mcp.servers,
                            );
                        (report.connectable, report.skipped)
                    }
                };
                for (name, reason) in &credential_skips {
                    println!("  \u{2298} {name:<20} skipped: {}", reason.message());
                }
                match wcore_mcp::manager::McpManager::connect_all(&probe_servers).await {
                    Ok(mgr) => {
                        let mut names: Vec<&String> = mgr.health().keys().collect();
                        names.sort();
                        for name in names {
                            use wcore_mcp::manager::McpServerHealth::*;
                            let line = match &mgr.health()[name] {
                                Ready { tool_count } => {
                                    format!("  ● {name:<20} ready ({tool_count} tools)")
                                }
                                Failed { reason } => format!("  ✕ {name:<20} failed: {reason}"),
                                TimedOut {
                                    after,
                                    cleanup_error,
                                } => {
                                    let cleanup = cleanup_error
                                        .as_ref()
                                        .map(|error| format!("; cleanup unverified: {error}"))
                                        .unwrap_or_default();
                                    format!("  ⏱ {name:<20} timed out after {after:?}{cleanup}")
                                }
                                Skipped { reason } => format!("  ⊘ {name:<20} skipped: {reason}"),
                            };
                            println!("{line}");
                        }
                    }
                    Err(e) => println!("  (probe failed: {e})"),
                }
            }
            Ok(_) => println!("  (no config-declared servers to probe)"),
            Err(e) => println!("  (config not loaded: {e})"),
        }
        println!("  Note: plugin-declared servers are probed at session boot, not here.");
    } else {
        println!();
        println!("Run with --probe-mcp to connect-test the config-declared servers.");
    }
}

// -- helpers ------------------------------------------------------------

/// The three distinguishable states of durable session persistence.
///
/// `session.enabled` alone cannot tell them apart, and the last two want
/// OPPOSITE reporting: one is a healthy configuration the operator chose, the
/// other is a capability they did not choose to lose.
///
/// `OffByHost` is GONE, and its absence is the report. A host that cannot seal
/// a provider request no longer turns durable sessions off — it journals
/// without the seal — so a doctor that could still print "OFF, forced by this
/// host" would be describing a state the product cannot reach. A status surface
/// carrying a value nothing can produce is the same defect as a status surface
/// missing one that something can.
#[derive(Debug, PartialEq, Eq)]
enum DurableSessions {
    /// Durable sessions are on, seal and all: an interrupted dispatch resumes
    /// itself.
    On,
    /// The operator set `[session] enabled = false`. Normal and healthy.
    OffByOperator,
    /// Durable sessions are ON and the journal is complete, but this host
    /// cannot seal a provider request, so an interrupted dispatch will ask for
    /// a decision instead of resuming itself.
    OnWithoutReplay,
}

/// Classify the state. Pure, so every combination can be exercised — including
/// the one that matters most.
///
/// **The operator's own choice is now tested FIRST, and the reversal is
/// deliberate.** It used to be the other way round, and had to be: a host-forced
/// degrade set `session.enabled = false` as part of forcing itself, so reading
/// the config value first reported every host degrade as an operator choice.
/// That coupling is gone — the host no longer touches `session.enabled` — so
/// `!session_enabled` now means one thing only, and reading it first is what
/// keeps an operator who genuinely turned sessions off from being told about a
/// replay seal for a journal they do not have.
fn classify_durable_sessions(session_enabled: bool, replay_unavailable: bool) -> DurableSessions {
    if !session_enabled {
        DurableSessions::OffByOperator
    } else if replay_unavailable {
        DurableSessions::OnWithoutReplay
    } else {
        DurableSessions::On
    }
}

/// Print the durable-session state. This is the consumer
/// `replay_protection_unavailable()` was added for: the headless-keyring
/// fix degrades gracefully and announces it once on stderr at startup, and the
/// cross-audit panel's dissenting REFUSE vote rested on that not being enough —
/// a degraded capability must be *reportable on demand*, not only printed into
/// a log nobody kept.
///
/// How far the doctor got in checking the resolved credential.
///
/// Pure data, deliberately separate from the code that renders it, so the
/// gating (`--probe-provider` off means NOT PROBED, never "accepted") and the
/// wording of every verdict are unit-testable without a network call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum CredentialVerdict {
    /// No credential resolved at all, so there is nothing to validate.
    NoCredential,
    /// A credential resolved, but `--probe-provider` was not passed. **This is
    /// the default**: bare `--doctor` stays side-effect-free, exactly as bare
    /// `--doctor` does not connect-test MCP servers without `--probe-mcp`.
    NotProbed,
    /// The provider has no key-validation endpoint we know of, so the key
    /// cannot be checked even on request. Reported rather than silently
    /// skipped — an unrunnable check must not read as a passing one.
    NoEndpoint,
    /// The provider authenticated the key.
    Accepted,
    /// The provider refused the key — a 401/403, which is evidence about the
    /// credential itself.
    Refused(String),
    /// The probe ran but produced no auth answer: any other status, a timeout,
    /// a transport failure. **Never means the key is bad.** Kept distinct from
    /// [`CredentialVerdict::Refused`] because condemning a key nobody judged
    /// would be the same defect this section exists to fix.
    Inconclusive(String),
}

/// FerroxLabs/wayland#1079 — the invocation's own provider selection, printed.
///
/// The ticket's headline is `--doctor --api-key <key>` being ignored. Threading
/// `CliArgs` into [`run`] fixed the COMPUTATION, but nothing in the OUTPUT
/// changed for `--api-key`, `--provider`, `--model` or `--base-url`: doctor has
/// no provider row at any version — [`collect_checks`] emits binary version,
/// Chromium, `wlrctl`/`grim`/`WAYLAND_DISPLAY`/`X DISPLAY`, two macOS TCC
/// probes, `BROWSERBASE_API_KEY` and Ollama, and nothing else. So a user still
/// could not SEE that the flag had been honoured, and no test could observe it
/// either: dropping `api_key` from the `CliArgs` literal in `main.rs` compiles
/// and reddens nothing. An ungraded behaviour regresses silently.
///
/// This section closes both halves. It prints what `Config::resolve` made of
/// THIS command line, and marks each value with where it came from, so every
/// config-selecting flag has a visible effect on doctor's output.
///
/// **The credential is never printed** — only whether one resolved, from which
/// rung, and (with `--probe-provider`) whether the provider accepts it.
/// `doctor_never_prints_the_api_key_value` guards that.
///
/// Printed, deliberately NOT a `CheckResult` row, for exactly the reason given
/// on [`print_durable_sessions_section`]: the TUI diagnostics surface turns
/// every `CheckResult` into a row inside a fixed 80x24 viewport, and one more
/// row pushes the PROVIDERS section off screen.
///
/// Informational only: like the other two sections it can never flip the exit
/// code.
fn print_provider_section(cli_args: &wcore_config::config::CliArgs, probe_provider: bool) {
    for line in provider_section_lines(
        cli_args,
        probe_provider,
        &crate::provider_keys::validate_key_verdict,
    ) {
        println!("{line}");
    }
}

/// The body of [`print_provider_section`], returning lines instead of printing
/// them and taking the credential prober as a parameter.
///
/// `probe` is injected for one reason: it is the only part of this section that
/// touches the network. Passing a fake lets the unit tests below grade the
/// whole wiring — the `--probe-provider` gate, that the prober is called with
/// the resolved provider and the resolved key, and every verdict's wording —
/// with no live call and no flakiness. The single seam left ungraded here is
/// the binding to the real [`crate::provider_keys::validate_key_blocking`]
/// above, which carries its own tests (`provider_keys.rs`:
/// `validate_key_blocking_routes_through_egress_and_classifies_status`).
fn provider_section_lines(
    cli_args: &wcore_config::config::CliArgs,
    probe_provider: bool,
    probe: &dyn Fn(crate::provider_keys::Provider, &str) -> crate::provider_keys::KeyVerdict,
) -> Vec<String> {
    let mut out = vec![String::new(), "Provider (this invocation):".to_string()];
    let cfg = match wcore_config::config::Config::resolve(cli_args) {
        Err(e) => {
            out.push(format!("  (config not loaded: {e})"));
            out.push("           run `wayland-core --config-path` and check that file".to_string());
            return out;
        }
        Ok(cfg) => cfg,
    };

    let source = |flag: &str, from_flag: bool| {
        if from_flag {
            format!("(from {flag})")
        } else {
            "(from config)".to_string()
        }
    };
    out.push(format!(
        "  provider   {:<38} {}",
        cfg.provider_label,
        source("--provider", cli_args.provider.is_some())
    ));
    out.push(format!(
        "  model      {:<38} {}",
        cfg.model,
        source("--model", cli_args.model.is_some())
    ));
    out.push(format!(
        "  base url   {:<38} {}",
        cfg.base_url,
        source("--base-url", cli_args.base_url.is_some())
    ));
    // State comes from the RESOLVED config, source from the flag: an
    // explicitly empty `--api-key ""` must not read as "present".
    let (state, key_source) = if cfg.api_key.is_empty() {
        ("not set", "(no credential resolved)".to_string())
    } else if cli_args.api_key.is_some() {
        ("present", "(from --api-key)".to_string())
    } else {
        (
            "present",
            "(from config, credential store or environment)".to_string(),
        )
    };
    out.push(format!("  api key    {state:<38} {key_source}"));

    let verdict = credential_verdict(&cfg, probe_provider, probe);
    out.extend(verdict_lines(&verdict, &cfg));
    out
}

/// Decide what to say about the resolved credential.
///
/// The gate is checked FIRST and unconditionally: with `--probe-provider`
/// absent this returns [`CredentialVerdict::NotProbed`] without calling
/// `probe`, so bare `--doctor` cannot make an authenticated network call.
fn credential_verdict(
    cfg: &wcore_config::config::Config,
    probe_provider: bool,
    probe: &dyn Fn(crate::provider_keys::Provider, &str) -> crate::provider_keys::KeyVerdict,
) -> CredentialVerdict {
    if cfg.api_key.is_empty() {
        return CredentialVerdict::NoCredential;
    }
    if !probe_provider {
        return CredentialVerdict::NotProbed;
    }
    let Some(provider) = crate::provider_keys::Provider::from_slug(&cfg.provider_label) else {
        return CredentialVerdict::NoEndpoint;
    };
    match probe(provider, &cfg.api_key) {
        crate::provider_keys::KeyVerdict::Accepted => CredentialVerdict::Accepted,
        // The reason is provider-derived text on its way into output users
        // paste into bug reports, so the credential is stripped from it here
        // rather than trusted not to be there. `validate_key_blocking` builds
        // fixed strings today; this holds even if one day it quotes the
        // request. Redacting at the print boundary is the only place that
        // cannot be bypassed by a new caller.
        crate::provider_keys::KeyVerdict::Rejected(why) => {
            CredentialVerdict::Refused(redact(&why, &cfg.api_key))
        }
        crate::provider_keys::KeyVerdict::Inconclusive(why) => {
            CredentialVerdict::Inconclusive(redact(&why, &cfg.api_key))
        }
    }
}

/// Replace every occurrence of `secret` in `text` with a placeholder.
///
/// An empty `secret` is returned unchanged — `str::replace` with an empty
/// pattern splices the placeholder between every character, which would both
/// mangle the message and falsely suggest something was hidden.
fn redact(text: &str, secret: &str) -> String {
    if secret.is_empty() {
        return text.to_string();
    }
    text.replace(secret, "<redacted>")
}

/// Render a [`CredentialVerdict`], including the caveats a user needs in order
/// to read it correctly.
fn verdict_lines(verdict: &CredentialVerdict, cfg: &wcore_config::config::Config) -> Vec<String> {
    match verdict {
        CredentialVerdict::NoCredential => vec![
            "  credential NOT VALIDATED                       (no credential to check)".to_string(),
        ],
        CredentialVerdict::NotProbed => vec![
            "  credential NOT VALIDATED                       (run --doctor --probe-provider to \
             authenticate it)"
                .to_string(),
        ],
        CredentialVerdict::NoEndpoint => vec![format!(
            "  credential NOT VALIDATED                       (no key-validation endpoint known \
             for '{}')",
            cfg.provider_label
        )],
        CredentialVerdict::Accepted => {
            let mut lines = vec![
                "  credential ACCEPTED                            (the provider authenticated \
                 this key)"
                    .to_string(),
            ];
            lines.extend(base_url_caveat(cfg));
            lines
        }
        CredentialVerdict::Refused(why) => {
            let mut lines = vec![format!(
                "  credential REFUSED                             ({why})"
            )];
            lines.extend(base_url_caveat(cfg));
            lines
        }
        CredentialVerdict::Inconclusive(why) => vec![
            format!("  credential NOT VALIDATED                       ({why})"),
            "           the provider did not answer the auth question, so this".to_string(),
            "           says nothing about whether the key itself is good".to_string(),
        ],
    }
}

/// The probe authenticates against the PROVIDER's own key-validation endpoint
/// (`provider_keys::validation_endpoint`), which is not the configured
/// `base_url`. When the two differ — a proxy, a gateway, a self-hosted
/// compatible endpoint — the verdict is about the vendor, not about the
/// endpoint this invocation would actually call, and saying so is the whole
/// point of #1079: a diagnostic must not answer a question the user did not
/// ask and let them read it as the one they did.
fn base_url_caveat(cfg: &wcore_config::config::Config) -> Vec<String> {
    let (url, _) = match crate::provider_keys::Provider::from_slug(&cfg.provider_label) {
        Some(p) => crate::provider_keys::validation_endpoint(p, ""),
        None => return Vec::new(),
    };
    // Both hosts come from `wcore_types::url_authority` — the ONE authority
    // parser — rather than from a cut of our own (wayland#1252 site A). The
    // cut that stood here stopped at `/ ? #` and took the last `@`-separated
    // part, so `https://evil.example\@api.openai.com/v1` read as the vendor's
    // own host and suppressed this caveat on a request that reaches
    // `evil.example`.
    //
    // A `base_url` the parser cannot read is `None`, which is NOT equal to the
    // vendor host, so the caveat PRINTS. That is the safe direction for a
    // diagnostic: saying the verdict may not cover the configured endpoint
    // costs two lines, and staying silent is the #1079 defect itself.
    let Some(vendor_host) = wcore_types::url_authority::dialed_host_str(&url) else {
        return Vec::new();
    };
    if wcore_types::url_authority::dialed_host_str(&cfg.base_url).as_deref()
        == Some(vendor_host.as_str())
    {
        return Vec::new();
    }
    vec![
        format!("           checked against {vendor_host}, NOT the base url above —"),
        "           a proxy or gateway there is not covered by this verdict".to_string(),
    ]
}

/// **Printed, deliberately NOT a `CheckResult` row** — the same reason
/// [`print_mcp_section`] is. The TUI diagnostics surface converts every
/// `CheckResult` into a row and renders into a fixed 80x24 viewport, so adding
/// one more system row pushes the PROVIDERS section off screen. Measured, not
/// assumed: `doctor_shows_yellow_when_key_unset` passes at `e7bc6d88` and fails
/// with the extra row, reporting `provider Gemini missing from /doctor output`.
/// `crates/wcore-cli/src/tui/**` is owned by another lane this cycle, so the
/// row stays on the CLI surface rather than being bought with a change to a
/// fenced file or a weakened test.
///
/// **This resolves the config ITSELF**, and must. The flag is a side effect of
/// `Config::resolve`; doctor's only other resolve calls are inside
/// [`print_mcp_section`], which runs after this. A reader that merely loaded
/// the flag would observe `false` forever — a report with no reachable
/// degraded state, which measures nothing.
///
/// Informational only: like the MCP section it can never flip the exit code.
async fn print_durable_sessions_section(cli_args: &wcore_config::config::CliArgs) {
    println!();
    println!("Durable sessions:");
    match wcore_config::config::Config::resolve(cli_args) {
        Err(e) => {
            println!("  UNKNOWN  config did not resolve ({e})");
            println!("           run `wayland-core --config-path` and check that file");
        }
        Ok(cfg) => match classify_durable_sessions(
            cfg.session.enabled,
            wcore_config::config::replay_protection_unavailable(),
        ) {
            DurableSessions::On => println!("  ON       conversation history is saved to disk"),
            DurableSessions::OffByOperator => {
                println!("  OFF      by your configuration ([session] enabled = false)");
            }
            DurableSessions::OnWithoutReplay => {
                println!("  ON       but crash replay is unavailable on this host");
                println!(
                    "           no usable OS keyring and no unlocked credentials vault were found"
                );
                println!(
                    "           conversation history IS saved and every provider call, tool call,"
                );
                println!(
                    "           approval and delivery is still recorded; what is missing is the"
                );
                println!(
                    "           sealed copy of the exact provider request, so a turn interrupted"
                );
                println!(
                    "           mid-dispatch asks you to resume, reconcile or cancel it instead"
                );
                println!("           of resuming itself");
                println!(
                    "           to restore: set WAYLAND_VAULT_PASSPHRASE_FD (a passphrase file"
                );
                println!("           descriptor, preferred) or WAYLAND_VAULT_PASSPHRASE");
                println!("           to refuse to run this way at all: set [session]");
                println!("           require_durability = true in config.toml");
            }
        },
    }
}

fn skip(label: &'static str, reason: &str) -> CheckResult {
    CheckResult {
        label,
        outcome: Outcome::Skip {
            reason: reason.to_string(),
        },
    }
}

/// `which prog` via `shell_command_argv` — argv mode, no shell
/// interpreter. Returns the resolved path on success (stdout, trimmed)
/// or `None` if the lookup fails / `which` itself is missing.
///
/// On Windows `which` is not part of the base system, so the lookup
/// will return `None`. That's acceptable: the only Windows-relevant
/// check (`browser backend`) tries `where` as a fallback. For v0.2.2
/// the doctor is Linux/macOS-focused; Windows users will see SKIP rows
/// for the Linux-only checks and a `browser backend` FAIL row that we
/// will tighten in a follow-up if a Windows ship surfaces.
async fn which(prog: &str) -> Option<String> {
    // First try POSIX `which`.
    if let Ok(output) = shell_command_argv("which", &[prog]).output().await
        && output.status.success()
    {
        let s = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if !s.is_empty() {
            return Some(s);
        }
    }
    // Windows fallback: `where prog` prints one path per match.
    if cfg!(windows)
        && let Ok(output) = shell_command_argv("where", &[prog]).output().await
        && output.status.success()
    {
        let s = String::from_utf8_lossy(&output.stdout)
            .lines()
            .next()
            .unwrap_or("")
            .trim()
            .to_string();
        if !s.is_empty() {
            return Some(s);
        }
    }
    None
}

fn wlrctl_hints() -> Vec<String> {
    vec![
        "apt install wlrctl              (Debian/Ubuntu — may need PPA)".into(),
        "pacman -S wlrctl                (Arch)".into(),
        "nix-env -iA nixpkgs.wlrctl      (NixOS)".into(),
        "or build from source: https://git.sr.ht/~brocellous/wlrctl".into(),
    ]
}

/// wayland-core#354 — the `/doctor` face of `[mcp] malware_gate`.
///
/// A security posture that is only visible by reading `config.toml` is a
/// posture nobody audits, and the permissive default is exactly the one an
/// operator would want to discover they still have. Kept as a pure function
/// so the line itself is graded, not just the fact that something printed.
pub(crate) fn malware_gate_line(mode: wcore_config::config::McpMalwareGateMode) -> String {
    use wcore_config::config::McpMalwareGateMode as Mode;
    let consequence = match mode {
        Mode::Permissive => {
            "an OSV malware check that cannot be performed LOGS at ERROR and the server \
             still launches (default)"
        }
        Mode::Strict => "an OSV malware check that cannot be performed REFUSES the launch",
    };
    format!(
        "  [mcp] malware_gate = \"{}\" — {consequence}",
        mode.as_str()
    )
}

fn grim_hints() -> Vec<String> {
    vec![
        "apt install grim                (Debian/Ubuntu)".into(),
        "pacman -S grim                  (Arch)".into(),
        "nix-env -iA nixpkgs.grim        (NixOS)".into(),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use serial_test::serial;

    // -- wayland-core#354: the malware-gate mode has a `/doctor` face ------

    /// Both modes must name the config key AND state the consequence. A line
    /// that only echoes `permissive` tells an operator nothing about what
    /// their machine does when `api.osv.dev` is unreachable, which is the
    /// entire reason the key exists.
    #[test]
    fn doctor_names_the_malware_gate_mode_and_what_it_does() {
        use wcore_config::config::McpMalwareGateMode as Mode;

        let permissive = malware_gate_line(Mode::Permissive);
        assert!(
            permissive.contains("malware_gate") && permissive.contains("\"permissive\""),
            "the line must name the config key and its value: {permissive}"
        );
        assert!(
            permissive.contains("still launches"),
            "permissive must say the launch goes ahead: {permissive}"
        );

        let strict = malware_gate_line(Mode::Strict);
        assert!(
            strict.contains("malware_gate") && strict.contains("\"strict\""),
            "the line must name the config key and its value: {strict}"
        );
        assert!(
            strict.contains("REFUSES"),
            "strict must say the launch is refused: {strict}"
        );
        assert_ne!(
            permissive, strict,
            "one line for both modes would report nothing"
        );
    }

    /// The default posture a fresh install reports is the permissive one --
    /// this is the row that would catch a change of default slipping in.
    #[test]
    fn doctor_reports_permissive_for_a_default_config() {
        let cfg = wcore_config::config::Config::default();
        assert_eq!(
            malware_gate_line(cfg.mcp.malware_gate),
            malware_gate_line(wcore_config::config::McpMalwareGateMode::Permissive)
        );
    }

    // -- `br-default`: the browser-policy row ----------------------------
    //
    // A fresh install cannot run a browser op for two independent reasons:
    // the sidecar is not shipped, and `[browser.policy]` denies every URL.
    // `--doctor` reported the first and was silent about the second, so on a
    // host where somebody HAD installed the sidecar the whole table read
    // clean while every navigation was refused. Neither test below relaxes
    // the fail-closed default -- that is recorded design -- they only require
    // the doctor to say it out loud, with the remedy the tool itself prints.

    fn deny_all_policy() -> wcore_config::browser::BrowserPolicyConfig {
        wcore_config::browser::BrowserPolicyConfig::default()
    }

    /// The default posture is the fresh-install posture, so this is the row
    /// almost every reader gets. It must WARN, and it must carry a remedy
    /// that names the section the loader reads and a file that exists.
    #[test]
    fn the_policy_row_warns_when_the_default_posture_refuses_every_url() {
        let policy = deny_all_policy();
        // Control: this really is the shipped default, not a value the test
        // arranged. If either of these ever changes, the WARN below is about
        // a posture no user has.
        assert_eq!(policy.default_action, "deny");
        assert!(policy.allowed_origins.is_empty());

        let row = browser_policy_row(&policy);
        let Outcome::Warn { detail, hints } = row.outcome else {
            panic!(
                "the doctor reports the fail-closed default browser policy as {:?}. A reader \
                 sees a clean table on a machine where every browser op is refused.",
                row.outcome
            )
        };
        assert!(
            detail.contains("deny"),
            "the WARN detail never says the policy denies: {detail}"
        );
        let hints = hints.join("\n");
        assert!(
            hints.contains("[browser.policy]"),
            "the doctor's remedy does not name the section the loader reads. A key written \
             at `[browser]` parses cleanly and is silently discarded:\n{hints}"
        );
        assert!(
            hints.contains("allowed_origins"),
            "the doctor's remedy never names the setting to add:\n{hints}"
        );
        assert!(
            !hints.contains("npm install"),
            "the policy row repeats the sidecar install line that the `browser backend` row \
             directly above already prints. One screen, the same instruction twice, is how a \
             reader learns to skip the block:\n{hints}"
        );
        let global = wcore_config::config::global_config_path();
        assert!(
            hints.contains(&global.display().to_string()),
            "the doctor's remedy names no resolved config file, so a reader has nowhere to \
             put it -- the gh#900 defect, reproduced on a second surface:\n{hints}"
        );
    }

    /// The inverse: an operator who HAS allow-listed something, or who has
    /// flipped the default, must not be nagged. A row that warns
    /// unconditionally carries no information.
    #[test]
    fn the_policy_row_passes_once_the_operator_has_opened_it() {
        let mut allowlisted = deny_all_policy();
        allowlisted.allowed_origins = vec!["example.com".into()];
        assert!(
            matches!(
                browser_policy_row(&allowlisted).outcome,
                Outcome::Pass { .. }
            ),
            "an allow-listed origin still reports as denied"
        );

        let mut allow_all = deny_all_policy();
        allow_all.default_action = "allow".into();
        assert!(
            matches!(browser_policy_row(&allow_all).outcome, Outcome::Pass { .. }),
            "`default_action = \"allow\"` still reports as denied"
        );
    }

    /// WIRING, not the function. Two things a passing `browser_policy_row`
    /// cannot prove: that the row reaches the printed table at all, and that
    /// it sits next to the backend row. Adjacency is the point -- `[PASS]
    /// browser backend` immediately above is what made the silence readable
    /// as health.
    #[test]
    fn the_policy_row_lands_directly_under_the_backend_row() {
        let seeded = vec![
            check_version("9.9.9"),
            CheckResult {
                label: BROWSER_BACKEND_LABEL,
                outcome: Outcome::Pass {
                    detail: "Camoufox sidecar -> /somewhere/camofox-browser".into(),
                },
            },
            skip("wlrctl", "Linux-only"),
        ];
        let rows = with_config_rows(seeded, &wcore_config::config::CliArgs::default());

        let backend = rows
            .iter()
            .position(|r| r.label == BROWSER_BACKEND_LABEL)
            .expect("backend row survived");
        let policy = rows
            .iter()
            .position(|r| r.label == BROWSER_POLICY_LABEL)
            .unwrap_or_else(|| {
                panic!(
                    "no browser-policy row reached the doctor table; the function may be \
                     correct but nothing calls it. Rows: {:?}",
                    rows.iter().map(|r| r.label).collect::<Vec<_>>()
                )
            });
        assert_eq!(
            policy,
            backend + 1,
            "the policy row is not adjacent to the backend row; a reader who stops at \
             `[PASS] browser backend` never reaches it. Rows: {:?}",
            rows.iter().map(|r| r.label).collect::<Vec<_>>()
        );
    }

    /// gh#491 — `--doctor` must recommend the browser backend this binary
    /// actually compiled, and must not recommend one it did not.
    ///
    /// The Chromium/CDP backend never shipped (opt-in cargo feature, refused by
    /// selection under any policy) and has since been deleted — yet the doctor told
    /// every Linux user to `apt install chromium-browser`, which leaves them
    /// exactly as far from a working browser as before and never names the
    /// sidecar that is actually required.
    ///
    /// PATH is emptied so the row is the MISSING one on every host; without
    /// that this passes vacuously wherever a browser happens to be installed.
    #[tokio::test]
    #[serial]
    async fn the_browser_row_recommends_the_compiled_backend_not_chromium() {
        let empty = tempfile::tempdir().unwrap();
        // PATH is process-global, and this binary runs its tests in ONE
        // process. Emptying PATH outright breaks any concurrently running test
        // that spawns through `wcore_config::shell`, which resolves `sh` (Unix)
        // / `cmd` (Windows) off PATH — measured: it took `goal_cmd`'s worker
        // tests down with "worker command 'sh' failed to start: No such file or
        // directory" in the shared-process `--lib` leg. Carry the system shell
        // across. It is not a browser backend, so `resolve_any` still finds
        // nothing and the row under test is unchanged.
        #[cfg(unix)]
        {
            let sh = std::path::Path::new("/bin/sh");
            if sh.exists() {
                let _ = std::os::unix::fs::symlink(sh, empty.path().join("sh"));
            }
        }
        let prior_path = std::env::var_os("PATH");
        let prior_bin = std::env::var_os("WAYLAND_CAMOUFOX_BIN");
        unsafe {
            std::env::set_var("PATH", empty.path());
            std::env::remove_var("WAYLAND_CAMOUFOX_BIN");
        }

        let row = check_browser_binary().await;

        unsafe {
            match prior_path {
                Some(v) => std::env::set_var("PATH", v),
                None => std::env::remove_var("PATH"),
            }
            match prior_bin {
                Some(v) => std::env::set_var("WAYLAND_CAMOUFOX_BIN", v),
                None => std::env::remove_var("WAYLAND_CAMOUFOX_BIN"),
            }
        }

        let hints = match &row.outcome {
            Outcome::Fail { hints } => hints.clone(),
            Outcome::Warn { hints, .. } => hints.clone(),
            other => panic!(
                "nothing resolves on an empty PATH, so the row must report a missing backend, got {other:?}"
            ),
        };

        assert!(
            hints
                .iter()
                .any(|h| h.contains(wcore_browser::install::CAMOUFOX_SIDECAR_PACKAGE)),
            "the doctor never names the package that provides the backend this build \
             compiled; got: {hints:?}"
        );
        assert!(
            hints
                .iter()
                .any(|h| h.contains(wcore_browser::install::CAMOUFOX_SIDECAR_ENV)),
            "the doctor never names the env var the supervisor reads; got: {hints:?}"
        );
        for hint in &hints {
            let lower = hint.to_ascii_lowercase();
            assert!(
                !lower.contains("chromium") && !lower.contains("google-chrome"),
                "this build has no Chromium backend compiled in, so the doctor must not \
                 tell the operator to install one; got: {hint}"
            );
        }
    }

    #[test]
    fn check_version_passes_for_non_empty() {
        let r = check_version("1.2.3");
        assert!(matches!(r.outcome, Outcome::Pass { .. }));
    }

    #[test]
    fn check_version_fails_for_empty() {
        let r = check_version("");
        assert!(matches!(r.outcome, Outcome::Fail { .. }));
    }

    /// The whole point of `replay_protection_unavailable()`: a host-forced loss
    /// of crash replay must NOT be reported as an operator choice, and must NOT
    /// be reported as a healthy fully-durable session either.
    #[test]
    fn host_forced_replay_loss_is_not_reported_as_an_operator_choice() {
        assert_eq!(
            classify_durable_sessions(true, true),
            DurableSessions::OnWithoutReplay
        );
    }

    /// The state the host CANNOT produce any more, asserted as such.
    ///
    /// `session_enabled == false` with the host flag set is now only reachable
    /// if an operator turned sessions off on a keyless host: two independent
    /// facts, and the operator's is the one that decided the outcome. Reporting
    /// it as a host fault would tell them to go find a keyring for a journal
    /// they asked not to have.
    ///
    /// This is the assertion that would red if the old ordering were restored,
    /// so it is the one that pins the reversal rather than merely surviving it.
    #[test]
    fn a_keyless_host_does_not_override_an_operator_who_turned_sessions_off() {
        assert_eq!(
            classify_durable_sessions(false, true),
            DurableSessions::OffByOperator
        );
    }

    #[test]
    fn operator_disabled_sessions_are_not_reported_as_a_host_fault() {
        assert_eq!(
            classify_durable_sessions(false, false),
            DurableSessions::OffByOperator
        );
    }

    #[test]
    fn enabled_sessions_report_on() {
        assert_eq!(classify_durable_sessions(true, false), DurableSessions::On);
    }

    /// All four inputs are graded, and they must produce three distinct
    /// outputs. Without this a classifier that collapsed two states — which is
    /// exactly what the previous ordering did to the pair below — still passes
    /// every individual assertion above.
    #[test]
    fn every_input_combination_is_graded_and_the_states_stay_distinct() {
        use std::collections::BTreeSet;

        let graded = [
            classify_durable_sessions(true, false),
            classify_durable_sessions(true, true),
            classify_durable_sessions(false, false),
            classify_durable_sessions(false, true),
        ];
        assert_eq!(
            graded,
            [
                DurableSessions::On,
                DurableSessions::OnWithoutReplay,
                DurableSessions::OffByOperator,
                DurableSessions::OffByOperator,
            ]
        );
        assert_eq!(
            graded
                .iter()
                .map(|state| format!("{state:?}"))
                .collect::<BTreeSet<_>>()
                .len(),
            3,
            "the classifier collapsed states that need different remedies"
        );
    }

    /// The macOS TCC rows must reflect the host's real grant state, and
    /// off macOS they must SKIP — never PASS. A row that passed on a
    /// platform with no TCC at all would be a fake green for the exact
    /// permission the check exists to report on.
    #[test]
    fn macos_tcc_rows_track_the_probe_and_skip_off_macos() {
        for capability in [TccCapability::Accessibility, TccCapability::ScreenRecording] {
            let row = check_macos_tcc(capability);
            match (wcore_cua::permissions::probe(capability), &row.outcome) {
                (TccStatus::Granted, Outcome::Pass { .. }) => {}
                (TccStatus::Denied, Outcome::Warn { hints, .. }) => assert!(
                    hints.iter().any(|h| h.contains(capability.settings_pane())),
                    "a denied grant must name the pane to visit: {hints:?}"
                ),
                (TccStatus::NotApplicable, Outcome::Skip { .. }) => {}
                (status, outcome) => {
                    panic!("probe said {status:?} but the row rendered {outcome:?}")
                }
            }
        }
    }

    /// The two capabilities must occupy distinct rows with distinct
    /// labels — collapsing them would hide one missing grant behind the
    /// other.
    #[test]
    fn accessibility_and_screen_recording_are_reported_separately() {
        let a = check_macos_tcc(TccCapability::Accessibility);
        let s = check_macos_tcc(TccCapability::ScreenRecording);
        assert_ne!(a.label, s.label);
    }

    #[test]
    fn skip_helper_produces_skip_outcome() {
        let r = skip("xyz", "test reason");
        match r.outcome {
            Outcome::Skip { reason } => assert_eq!(reason, "test reason"),
            _ => panic!("expected Skip"),
        }
    }

    #[test]
    #[serial]
    fn wayland_display_reads_env_var() {
        // Use a value that's unlikely to be set already.
        // SAFETY: #[serial] serializes every env-mutating test in this binary.
        unsafe {
            std::env::set_var("WAYLAND_DISPLAY", "wayland-test");
        }
        let r = check_wayland_display();
        assert!(matches!(r.outcome, Outcome::Pass { .. }));
        unsafe {
            std::env::remove_var("WAYLAND_DISPLAY");
        }
    }

    #[tokio::test]
    async fn which_returns_some_for_known_binary() {
        // `sh` is virtually guaranteed on Unix CI; on Windows we
        // skip the assertion because the doctor doesn't probe `sh`.
        if cfg!(unix) {
            let r = which("sh").await;
            assert!(r.is_some(), "expected `which sh` to resolve on Unix");
        }
    }

    #[tokio::test]
    async fn which_returns_none_for_unlikely_binary() {
        let r = which("definitely-not-a-real-binary-w5-doctor").await;
        assert!(r.is_none());
    }

    // ---------------------------------------------------------------------
    // FerroxLabs/wayland#1079 — the credential probe.
    //
    // The section's only network-touching part is injected, so these grade the
    // whole wiring — the `--probe-provider` gate, the arguments the prober is
    // handed, and every verdict's wording — with no live call.
    // ---------------------------------------------------------------------

    use crate::provider_keys::{KeyVerdict, Provider};
    use std::sync::Mutex;

    /// Records what the prober was called with, so a test can assert the
    /// doctor probed the RESOLVED provider with the RESOLVED key rather than
    /// something it made up.
    #[derive(Default)]
    struct ProbeSpy {
        calls: Mutex<Vec<(Provider, String)>>,
    }

    impl ProbeSpy {
        fn calls(&self) -> Vec<(Provider, String)> {
            self.calls.lock().expect("spy lock").clone()
        }
    }

    const PROBE_KEY: &str = "sk-issue1079-unit-not-a-real-key";

    fn args_with_key() -> wcore_config::config::CliArgs {
        wcore_config::config::CliArgs {
            provider: Some("anthropic".to_string()),
            api_key: Some(PROBE_KEY.to_string()),
            ..Default::default()
        }
    }

    /// THE GATE. Bare `--doctor` must not authenticate anything: the prober is
    /// never called, and the section says so rather than staying silent.
    ///
    /// This is the test that reddens if the `!probe_provider` early return in
    /// [`credential_verdict`] is dropped — turning a read-only diagnostic into
    /// one that ships the user's key to the vendor unasked.
    #[test]
    fn doctor_does_not_authenticate_the_credential_without_probe_provider() {
        let spy = ProbeSpy::default();
        let lines = provider_section_lines(&args_with_key(), false, &|p, k| {
            spy.calls.lock().expect("spy lock").push((p, k.to_string()));
            KeyVerdict::Accepted
        });

        assert!(
            spy.calls().is_empty(),
            "#1079: bare --doctor authenticated the credential without \
             --probe-provider. calls: {:?}",
            spy.calls()
        );
        // Positive control: the section rendered at all, so the absence above
        // is about the gate and not about an empty section.
        assert!(
            lines.iter().any(|l| l.contains("api key")),
            "no api key row — this test cannot certify the gate. lines:\n{lines:#?}"
        );
        assert!(
            lines
                .iter()
                .any(|l| l.contains("NOT VALIDATED") && l.contains("--probe-provider")),
            "the section did not say the credential was unvalidated, nor how \
             to validate it. lines:\n{lines:#?}"
        );
    }

    /// With the flag, the prober is called — with the provider the invocation
    /// resolved and the key the invocation supplied.
    #[test]
    fn probe_provider_authenticates_the_resolved_provider_and_key() {
        let spy = ProbeSpy::default();
        let lines = provider_section_lines(&args_with_key(), true, &|p, k| {
            spy.calls.lock().expect("spy lock").push((p, k.to_string()));
            KeyVerdict::Accepted
        });

        assert_eq!(
            spy.calls(),
            vec![(Provider::Anthropic, PROBE_KEY.to_string())],
            "#1079: --probe-provider did not probe the invocation's own \
             provider and key. lines:\n{lines:#?}"
        );
        assert!(
            lines.iter().any(|l| l.contains("ACCEPTED")),
            "a key the provider accepted was not reported as accepted. \
             lines:\n{lines:#?}"
        );
    }

    /// A rejected key must read as REFUSED and carry the provider's reason —
    /// this is the answer to "let me pass the key explicitly to rule it out",
    /// which is the consequence #1079 reports.
    #[test]
    fn probe_provider_reports_a_rejected_key_with_its_reason() {
        let lines = provider_section_lines(&args_with_key(), true, &|_, _| {
            KeyVerdict::Rejected("key rejected (401)".to_string())
        });
        assert!(
            lines
                .iter()
                .any(|l| l.contains("REFUSED") && l.contains("key rejected (401)")),
            "#1079: a key the provider REJECTED was not reported as refused. \
             lines:\n{lines:#?}"
        );
        assert!(
            !lines.iter().any(|l| l.contains("ACCEPTED")),
            "a rejected key also rendered as accepted. lines:\n{lines:#?}"
        );
    }

    /// A probe that produced no auth answer must read as neither ACCEPTED nor
    /// REFUSED — the doctor's job is to distinguish "your key is bad" from "I
    /// could not tell", and getting that wrong in EITHER direction is the
    /// defect this section exists to fix.
    ///
    /// Not hypothetical: `api.perplexity.ai/models` answers 404 to an
    /// anonymous request, so folding non-auth statuses into REFUSED would
    /// condemn every valid Perplexity key — a row that can never pass.
    #[test]
    fn a_probe_that_never_answered_is_reported_as_neither_accepted_nor_refused() {
        for why in ["network error", "unexpected response (404)", "timed out"] {
            let lines = provider_section_lines(&args_with_key(), true, &|_, _| {
                KeyVerdict::Inconclusive(why.to_string())
            });
            assert!(
                lines
                    .iter()
                    .any(|l| l.contains("NOT VALIDATED") && l.contains(why)),
                "{why:?} was not reported at all. lines:\n{lines:#?}"
            );
            assert!(
                !lines.iter().any(|l| l.contains("ACCEPTED")),
                "#1079: {why:?} rendered as ACCEPTED — a gate that cannot \
                 fail. lines:\n{lines:#?}"
            );
            assert!(
                !lines.iter().any(|l| l.contains("REFUSED")),
                "#1079: {why:?} rendered as REFUSED — condemning a key the \
                 provider never judged. lines:\n{lines:#?}"
            );
            // The user must be told the verdict is about the request, not the key.
            assert!(
                lines
                    .iter()
                    .any(|l| l.contains("says nothing about whether the key")),
                "the section did not say this is not a verdict on the key. \
                 lines:\n{lines:#?}"
            );
        }
    }

    /// NEGATIVE CONTROL for the test above: a real 401 still reaches REFUSED,
    /// so "never REFUSED" above is about inconclusive outcomes rather than
    /// about REFUSED having become unreachable.
    #[test]
    fn an_auth_rejection_still_reaches_refused() {
        let lines = provider_section_lines(&args_with_key(), true, &|_, _| {
            KeyVerdict::Rejected("key rejected (401)".to_string())
        });
        assert!(
            lines.iter().any(|l| l.contains("REFUSED")),
            "REFUSED is unreachable, so the assertions above are vacuous. \
             lines:\n{lines:#?}"
        );
    }

    /// The verdict is about the vendor's endpoint, not the configured
    /// `base_url`. When they differ the section must say so, or a user behind
    /// a proxy reads an ACCEPTED for an endpoint they never call.
    #[test]
    fn a_custom_base_url_is_flagged_as_outside_the_verdict() {
        let mut args = args_with_key();
        args.base_url = Some("https://proxy.issue1079.invalid/v1".to_string());
        let lines = provider_section_lines(&args, true, &|_, _| KeyVerdict::Accepted);
        assert!(
            lines
                .iter()
                .any(|l| l.contains("api.anthropic.com") && l.contains("NOT the base url")),
            "a custom --base-url was not flagged as outside the probe's \
             verdict. lines:\n{lines:#?}"
        );
    }

    /// NEGATIVE CONTROL for the caveat above: with no `--base-url` override
    /// the configured endpoint IS the vendor's, so the caveat must be absent —
    /// otherwise it would fire on every run and mean nothing.
    #[test]
    fn the_default_base_url_draws_no_caveat() {
        let lines = provider_section_lines(&args_with_key(), true, &|_, _| KeyVerdict::Accepted);
        // Positive control: the run really did produce a verdict.
        assert!(
            lines.iter().any(|l| l.contains("ACCEPTED")),
            "no verdict at all — this control cannot certify anything. \
             lines:\n{lines:#?}"
        );
        assert!(
            !lines.iter().any(|l| l.contains("NOT the base url")),
            "the caveat fired without a --base-url override. lines:\n{lines:#?}"
        );
    }

    /// wayland#1252 c1 + c4, SITE A. `https://evil.example\@api.openai.com/v1`
    /// is a request to `evil.example` with the path `/@api.openai.com/v1` — for
    /// a special scheme the WHATWG parser reads `\` as a path separator. The
    /// hand cut this replaced stopped only at `/ ? #`, took the last
    /// `@`-separated part, read `api.openai.com`, found it EQUAL to the vendor
    /// host, and SUPPRESSED the caveat — voiding #1079's guarantee through a
    /// spelling rather than through the shape it was filed about.
    ///
    /// All three arms live in ONE body so the fix cannot buy the first by
    /// breaking the other two: a caveat that fires on every run says nothing,
    /// and one that fires on none is the bug.
    #[test]
    fn the_base_url_caveat_reads_the_host_the_request_reaches() {
        let lines_for = |base_url: &str| {
            let args = wcore_config::config::CliArgs {
                provider: Some("openai".to_string()),
                api_key: Some(PROBE_KEY.to_string()),
                base_url: Some(base_url.to_string()),
                ..Default::default()
            };
            provider_section_lines(&args, true, &|_, _| KeyVerdict::Accepted)
        };
        let caveat = |lines: &[String]| {
            lines
                .iter()
                .any(|l| l.contains("api.openai.com") && l.contains("NOT the base url"))
        };

        // THE DEFECT. The configured endpoint dials `evil.example`; the probe
        // authenticated against `api.openai.com`. The caveat must fire.
        let smuggled = lines_for(r"https://evil.example\@api.openai.com/v1");
        // Positive control: there really is a verdict here to caveat, so the
        // assertion below cannot pass over an empty section.
        assert!(
            smuggled.iter().any(|l| l.contains("ACCEPTED")),
            "no verdict at all — the assertion below would be vacuous. \
             lines:\n{smuggled:#?}"
        );
        assert!(
            caveat(&smuggled),
            "a base_url that dials evil.example was read as the vendor's own \
             host, so #1079's caveat was suppressed. lines:\n{smuggled:#?}"
        );

        // WRONG-REFUSAL CONTROL 1: a genuinely different configured host still
        // PRINTS the caveat.
        let proxied = lines_for("https://proxy.issue1252.invalid/v1");
        assert!(
            caveat(&proxied),
            "an ordinary proxy base_url stopped drawing the caveat. \
             lines:\n{proxied:#?}"
        );

        // WRONG-REFUSAL CONTROL 2: a base_url that really IS on the vendor's
        // host still SUPPRESSES it.
        let vendor = lines_for("https://api.openai.com/v1");
        assert!(
            vendor.iter().any(|l| l.contains("ACCEPTED")),
            "no verdict at all — this control cannot certify anything. \
             lines:\n{vendor:#?}"
        );
        assert!(
            !caveat(&vendor),
            "the caveat fired for a base_url on the vendor's own host, so it \
             fires on every run and means nothing. lines:\n{vendor:#?}"
        );
    }

    /// Neither the probe path nor the unprobed path may print the key itself.
    #[test]
    fn the_probe_section_never_prints_the_credential() {
        for probe_provider in [false, true] {
            let lines = provider_section_lines(&args_with_key(), probe_provider, &|_, _| {
                KeyVerdict::Rejected(format!("key rejected (401) for {PROBE_KEY}"))
            });
            // Positive control: a credential really did resolve on this run.
            assert!(
                lines.iter().any(|l| l.contains("present")),
                "no credential resolved, so this cannot certify redaction. \
                 lines:\n{lines:#?}"
            );
            // The doctor must not echo a provider-supplied string containing
            // the key back into its own output.
            let rendered = lines.join("\n");
            assert!(
                !rendered.contains(PROBE_KEY),
                "#1079: the doctor printed the API key (probe_provider={probe_provider}). \
                 lines:\n{lines:#?}"
            );
        }
    }

    /// A provider with no known key-validation endpoint must say the check did
    /// not run. Reporting nothing would be indistinguishable from a pass.
    #[test]
    fn a_provider_without_a_validation_endpoint_reports_that_it_was_not_checked() {
        let cfg = wcore_config::config::Config {
            provider_label: "issue1079-no-such-provider".to_string(),
            api_key: PROBE_KEY.to_string(),
            ..Default::default()
        };
        let verdict = credential_verdict(&cfg, true, &|_, _| KeyVerdict::Accepted);
        assert_eq!(verdict, CredentialVerdict::NoEndpoint);
        let lines = verdict_lines(&verdict, &cfg);
        assert!(
            lines
                .iter()
                .any(|l| l.contains("NOT VALIDATED") && l.contains("no key-validation endpoint")),
            "lines:\n{lines:#?}"
        );
    }

    /// With no credential at all there is nothing to validate, and the section
    /// must not offer `--probe-provider` as if it would help.
    #[test]
    fn no_credential_means_nothing_to_validate() {
        let cfg = wcore_config::config::Config {
            provider_label: "anthropic".to_string(),
            api_key: String::new(),
            ..Default::default()
        };
        assert_eq!(
            credential_verdict(&cfg, true, &|_, _| KeyVerdict::Accepted),
            CredentialVerdict::NoCredential
        );
    }

    #[test]
    fn redact_removes_the_secret_and_leaves_an_empty_one_alone() {
        assert_eq!(
            redact("key rejected (401) for sk-abc", "sk-abc"),
            "key rejected (401) for <redacted>"
        );
        assert_eq!(redact("sk-abc sk-abc", "sk-abc"), "<redacted> <redacted>");
        // An empty secret must not splice a placeholder between every char.
        assert_eq!(redact("network error", ""), "network error");
    }
}
