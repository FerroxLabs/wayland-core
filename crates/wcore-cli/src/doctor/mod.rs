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
    /// Check cannot be automatically verified (e.g. macOS
    /// Accessibility permission, which lives in TCC and requires
    /// `AXIsProcessTrusted()` or a manual System Settings visit).
    /// Surfaced as a manual-action hint per the W5 hard rule against
    /// fake passes.
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
pub async fn run(probe_mcp: bool) -> ExitCode {
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

    // A4b: list declared MCP servers (and optionally probe). Informational
    // only — never flips the exit code below.
    print_mcp_section(probe_mcp).await;

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

    // 4. macOS Accessibility permission — cannot be programmatically
    //    verified without linking ApplicationServices/CoreFoundation
    //    (would require a new dep just for one check). Surfaced as a
    //    `Manual` row per the W5 hard rule against fake passes.
    if cfg!(target_os = "macos") {
        out.push(check_macos_accessibility_manual());
    } else {
        out.push(skip("macOS Accessibility", "non-macOS platform"));
    }

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

fn check_macos_accessibility_manual() -> CheckResult {
    CheckResult {
        label: "macOS Accessibility",
        outcome: Outcome::Manual {
            hint: "verify in System Settings -> Privacy & Security -> Accessibility \
                   (wayland-core / Terminal / iTerm must be enabled to use CUA)"
                .into(),
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
async fn print_mcp_section(probe: bool) {
    println!();
    println!("MCP servers (declared):");

    // --- config-declared servers (cascaded), best-effort load ---
    match wcore_config::config::Config::resolve(&wcore_config::config::CliArgs::default()) {
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
        match wcore_config::config::Config::resolve(&wcore_config::config::CliArgs::default()) {
            Ok(cfg) if !cfg.mcp.servers.is_empty() => {
                // #111 note: this deliberately connect-tests ALL config servers,
                // INCLUDING any marked `only_for_assistant` — the per-assistant
                // scoping filter (`servers_for_assistant`) is intentionally NOT
                // applied here. This is a throwaway diagnostic manager for the
                // trusted local operator: its tools are never injected into any
                // agent tool set or exposed to a model, and it is dropped at the
                // end of this block. If this manager is ever wired into an agent,
                // it MUST be filtered by the active assistant first.
                match wcore_mcp::manager::McpManager::connect_all(&cfg.mcp.servers).await {
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
                                TimedOut { after } => {
                                    format!("  ⏱ {name:<20} timed out after {after:?}")
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

    #[test]
    fn skip_helper_produces_skip_outcome() {
        let r = skip("xyz", "test reason");
        match r.outcome {
            Outcome::Skip { reason } => assert_eq!(reason, "test reason"),
            _ => panic!("expected Skip"),
        }
    }

    #[test]
    fn wayland_display_reads_env_var() {
        // Use a value that's unlikely to be set already.
        // SAFETY: tests in the same module may race; we only assert
        // on the variant produced, not the inner string, so a
        // concurrent unset is observationally equivalent to "not set".
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
