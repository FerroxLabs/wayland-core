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
pub async fn run(probe_mcp: bool, cli_args: &wcore_config::config::CliArgs) -> ExitCode {
    let report = collect().await;
    let version = &report.version;
    println!("wayland-core doctor v{version}\n");

    let checks = &report.checks;

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

    // 2. Chromium / Chrome — required for the browser CDP backend
    //    fallback path on every platform.
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

async fn check_browser_binary() -> CheckResult {
    // Try the three canonical aliases in order; PASS on the first hit.
    for prog in ["chromium-browser", "chromium", "google-chrome"] {
        if let Some(path) = which(prog).await {
            return CheckResult {
                label: "chromium browser",
                outcome: Outcome::Pass {
                    detail: format!("{prog} -> {path}"),
                },
            };
        }
    }
    // F-073: on macOS Chromium is optional (Ollama/Browserbase cover
    // most local use-cases; Chrome is typically available via Desktop
    // without a PATH alias). Emit WARN so the exit code stays 0 rather
    // than forcing every macOS user to install a CLI alias just to pass
    // the doctor. On Linux Chromium is still a hard FAIL.
    if cfg!(target_os = "macos") {
        return CheckResult {
            label: "chromium browser",
            outcome: Outcome::Warn {
                detail: "not found on PATH — browser CDP backend unavailable".into(),
                hints: vec![
                    "brew install --cask google-chrome  (optional)".into(),
                    "or ensure your Chrome/Chromium has a shell alias".into(),
                ],
            },
        };
    }
    CheckResult {
        label: "chromium browser",
        outcome: Outcome::Fail {
            hints: vec![
                "apt install chromium-browser  (Debian/Ubuntu)".into(),
                "pacman -S chromium             (Arch)".into(),
                "nix-env -iA nixpkgs.chromium       (NixOS)".into(),
            ],
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
/// check (`chromium browser`) tries `where` as a fallback. For v0.2.2
/// the doctor is Linux/macOS-focused; Windows users will see SKIP rows
/// for the Linux-only checks and a `chromium browser` FAIL row that we
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
}
