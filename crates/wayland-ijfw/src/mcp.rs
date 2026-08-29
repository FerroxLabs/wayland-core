//! G.7 — register the IJFW MCP server.
//!
//! Spawns the `ijfw-memory` bin from `@ijfw/memory-server` via stdio.
//! The MCP server itself
//! exposes the canonical memory tools, whose names carry the `ijfw_`
//! prefix at runtime (e.g. `ijfw_memory_store`, `ijfw_memory_search`,
//! `ijfw_memory_recall`, `ijfw_memory_prelude`, `ijfw_run`,
//! `ijfw_update_apply`) — wcore-mcp's tool proxy ingests the server's
//! tool list at first use and surfaces it through the normal MCP tool
//! path. The hook→context dispatch contract matches a registered hook
//! NAME against the advertised tool NAME, so the hook names in
//! `hooks::HOOKS` (e.g. `ijfw_memory_prelude`) MUST equal these prefixed
//! tool names.
//!
//! Plugin-side we only register the `McpServerSpec`. Actual MCP
//! connection is owned by `wcore-mcp` in the host adapter.

use std::collections::HashMap;

use wcore_plugin_api::mcp_server_spec::{McpServerSpec, McpTransport};
use wcore_plugin_api::{PluginContext, PluginResult};

/// Canonical name for the IJFW MCP server. The wcore-mcp tool proxy
/// scopes every tool the server advertises with this name.
pub const SERVER_NAME: &str = "ijfw-memory";

/// npm package that ships the IJFW memory MCP server.
pub const MEMORY_SERVER_PACKAGE: &str = "@ijfw/memory-server";

/// Executable inside [`MEMORY_SERVER_PACKAGE`] that speaks MCP over stdio.
///
/// **#928.** `npx <pkg>` only resolves an executable when the package ships a
/// bin whose name equals the package's *unscoped* name. `@ijfw/memory-server`
/// ships `ijfw-memory`, `ijfw` and `ijfw-dispatch-plan` — there is no
/// `memory-server` bin — so the bare `npx -y @ijfw/memory-server` this used to
/// spawn exits 1 with `npm error could not determine executable to run` on
/// every platform (measured against 1.6.5 on Linux and on Windows 11 26200).
/// The bin has to be named explicitly and the package passed with
/// `--package=`; that form completes an MCP `initialize` handshake.
pub const MEMORY_SERVER_BIN: &str = "ijfw-memory";

/// Build the default IJFW MCP server spec. Operators override the
/// transport (npx vs locally-installed binary) via plugin config.
pub fn default_server_spec() -> McpServerSpec {
    McpServerSpec {
        name: SERVER_NAME.to_string(),
        transport: McpTransport::Stdio {
            command: "npx".to_string(),
            args: vec![
                "-y".to_string(),
                format!("--package={MEMORY_SERVER_PACKAGE}"),
                MEMORY_SERVER_BIN.to_string(),
            ],
        },
        env: HashMap::new(),
    }
}

/// Register the IJFW MCP server through `ctx.mcp_servers`. Manifest
/// declares `register_mcp_server = true`, so the registry must be
/// present.
///
/// Build a [`std::process::Command`] that runs `program` with Windows
/// PATHEXT shim resolution.
///
/// **Issue #6:** on Windows, Node ships `npx` as `npx.cmd` / `npx.ps1`
/// (there is no `npx.exe`), and a bare `Command::new("npx")` does NOT
/// resolve `.cmd`/`.bat`/`.ps1` shims — Rust's std only appends `.exe`. So
/// the presence/reachability probes below were failing on Windows even when
/// npx was installed and on PATH, which silently skipped MCP registration.
/// Routing the probe through `cmd /C` makes the Windows shell apply PATHEXT,
/// mirroring how the wcore-mcp stdio transport spawns the server itself
/// (`shell_command_builder` → `cmd /C …`). On Unix `npx` is a real binary /
/// symlink, so we spawn it directly.
///
/// (This plugin can't reuse `wcore_config::shell` — audit F2 forbids any
/// `wcore-*` core dep — so the cmd-wrapping is inlined here.)
#[cfg(windows)]
fn shim_aware_command(program: &str) -> std::process::Command {
    let mut c = std::process::Command::new("cmd");
    c.arg("/C").arg(program);
    c
}

#[cfg(not(windows))]
fn shim_aware_command(program: &str) -> std::process::Command {
    std::process::Command::new(program)
}

/// `true` if `program <version_arg>` starts and exits 0 — a fast PATH (+
/// PATHEXT on Windows) presence check. Used to gate MCP registration on npx
/// being installed.
fn command_available(program: &str, version_arg: &str) -> bool {
    shim_aware_command(program)
        .arg(version_arg)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// F-060 / B4: gate on `npx` being present on PATH AND the server being
/// reachable. Two-stage probe:
///
/// 1. `npx --version` presence check (fast; PATHEXT-aware on Windows so it
///    recognises `npx.cmd` — issue #6).
/// 2. If the transport is `Stdio { command: "node", args: [path, …] }`,
///    check the script path exists with `std::fs::metadata`.  Otherwise,
///    run the command with a 2-second timeout (`--help` or similar) to
///    verify it starts rather than exiting immediately.
///
/// On any probe failure we log INFO and return `Ok(())` — the MCP server
/// is optional infrastructure and must NOT block the engine.
///
/// RANK-57 hardening: the reachability probe (`command_available` +
/// `mcp_server_is_reachable`) spawns child processes and busy-polls with
/// blocking `std::thread::sleep` for up to ~2s. Running that directly on a
/// tokio worker starves the reactor (guaranteed cold-start latency, and on a
/// single-threaded runtime it blocks every other task). We therefore make
/// `register` async and run the blocking probe inside
/// `tokio::task::spawn_blocking`, awaiting the result so no async worker is
/// ever blocked. The probe's semantics (same reachability decision, same 2s
/// timeout budget) are unchanged — only the thread it runs on changes.
pub async fn register(ctx: &mut PluginContext<'_>) -> PluginResult<()> {
    // Wave RB STABILITY MINOR #13: typed HostMisconfiguration error.
    let registry = ctx.mcp_servers.as_mut().ok_or_else(|| {
        wcore_plugin_api::PluginError::HostMisconfiguration {
            plugin: "wayland-ijfw".into(),
            surface: "mcp_servers".into(),
        }
    })?;

    let spec = default_server_spec();

    // Offload both blocking stages onto a blocking thread so the async
    // worker stays free during the up-to-2s probe.
    let probe_spec = spec.clone();
    let reachable = tokio::task::spawn_blocking(move || probe_reachability(&probe_spec))
        .await
        // A panic inside the blocking probe must not block registration; treat
        // a join failure as "not reachable" and skip (the server is optional).
        .unwrap_or(false);

    if !reachable {
        return Ok(());
    }

    registry.register_mcp_server(spec)?;
    Ok(())
}

/// `true` if `command` fetches a package from a public registry and executes
/// it.
///
/// wayland-core#340. Kept in step with
/// `wcore_tools::osv_check::runner_forms`, which is the authoritative table —
/// this plugin cannot depend on a `wcore-*` core crate (audit F2), so the
/// names are mirrored. Over-inclusion is the safe direction here: it only
/// costs a skipped smoke test, where under-inclusion costs an ungated fetch.
fn is_package_runner(command: &str) -> bool {
    let base = std::path::Path::new(command)
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or(command)
        .to_ascii_lowercase();
    let base = [".exe", ".cmd", ".bat", ".ps1"]
        .iter()
        .find_map(|ext| base.strip_suffix(ext))
        .unwrap_or(base.as_str());
    matches!(
        base,
        "npx" | "bunx" | "uvx" | "pipx" | "npm" | "pnpm" | "yarn" | "bun" | "uv" | "deno"
    )
}

/// Synchronous two-stage reachability probe. Safe to run on a blocking
/// thread only (it spawns child processes and sleeps). Returns `true` iff
/// `npx` is present AND the server smoke-test passes.
fn probe_reachability(spec: &wcore_plugin_api::mcp_server_spec::McpServerSpec) -> bool {
    // Stage 1: npx presence (fast, no startup cost). PATHEXT-aware so the
    // Windows `npx.cmd` shim is found (issue #6).
    if !command_available("npx", "--version") {
        tracing::info!(
            "ijfw-memory: npx not found on PATH — skipping MCP registration \
             (install Node.js to enable)"
        );
        return false;
    }

    // Stage 2: verify the server is actually reachable.
    //
    // #928: this is logged at ERROR, not INFO. `npx` being absent (stage 1) is
    // an ordinary state for a user with no Node install, but *`npx` present and
    // the memory server still not starting* means a feature the user can see in
    // the UI has been silently switched off. The CLI subscriber caps the stderr
    // writer at `Level::ERROR` (wcore-cli/src/main.rs), so with `RUST_LOG`
    // unset an `info!` here reached the log file and nothing else — the user
    // clicked Memory, got an error, and core had already decided not to say
    // why.
    if !mcp_server_is_reachable(spec) {
        notify_server_unreachable();
        return false;
    }

    true
}

/// Emit the user-visible notice that the memory server was skipped.
///
/// Split out of [`probe_reachability`] so the LEVEL is directly testable:
/// `wcore-cli` builds its stderr writer as
/// `stderr.with_max_level(tracing::Level::ERROR)`, so anything below ERROR
/// reaches the log file and never the person who just clicked Memory. A
/// downgrade back to `info!`/`warn!` would re-open #928 while still "logging
/// the failure", which is why `notice_for_unreachable_server_is_user_visible`
/// asserts on the recorded level rather than on the message text.
fn notify_server_unreachable() {
    tracing::error!(
        "ijfw-memory: MCP server did not start cleanly — memory tools are \
         disabled for this session. Run `npx -y --package={} {} --help` \
         manually to diagnose.",
        MEMORY_SERVER_PACKAGE,
        MEMORY_SERVER_BIN
    );
}

/// Returns `true` if the MCP server is reachable / will start.
///
/// For `Stdio { command: "node", args: [script, …] }`: checks the script
/// file exists on disk (fast, no process spawn).
///
/// For all other stdio commands (e.g. `npx -y --package=@ijfw/memory-server
/// ijfw-memory`): spawns the server with a `--help` flag and waits up to 2
/// seconds. It is reachable if the child exits *cleanly* (`--help` handled) or
/// is still running at the deadline (a real server that ignored `--help`). A
/// non-zero exit means the command line did not resolve to a working
/// executable and the server is NOT reachable — see #928.
fn mcp_server_is_reachable(spec: &wcore_plugin_api::mcp_server_spec::McpServerSpec) -> bool {
    use wcore_plugin_api::mcp_server_spec::McpTransport;
    match &spec.transport {
        McpTransport::Stdio { command, args } => {
            // Fast path: if the command is `node` (or `python`/`deno`)
            // and the first arg is an absolute path, check the file exists.
            if (command == "node"
                || command == "python3"
                || command == "python"
                || command == "deno")
                && args
                    .first()
                    .map(|a| std::path::Path::new(a).is_absolute())
                    .unwrap_or(false)
            {
                let script = std::path::Path::new(&args[0]);
                if !script.exists() {
                    tracing::info!(
                        "ijfw-memory: script not found at {} — skipping registration",
                        script.display()
                    );
                    return false;
                }
                return true;
            }

            // wayland-core#340: a package runner is NOT smoke-tested here.
            //
            // `npx -y --package=@ijfw/memory-server ijfw-memory --help`
            // DOWNLOADS AND EXECUTES the package. This probe runs at plugin
            // registration, long before `StdioTransport::spawn_…` reaches
            // `wcore_mcp::malware_gate`, so it was a production pre-exec
            // bypass: the registry fetch, and any install-time code in it,
            // ran before OSV was ever queried. A reachability probe is not
            // worth executing ungated code for.
            //
            // The #928 failure this probe was added to catch — a spec whose
            // argv `npx` cannot resolve to an executable — is a property of a
            // CONSTANT (`default_server_spec`), not of the machine, so it is
            // graded by `default_spec_names_the_bin_explicitly` at build time
            // instead of by spawning the package at every startup. Stage 1
            // (`npx --version`) still runs: it names no package, so it fetches
            // nothing.
            if is_package_runner(command) {
                return true;
            }

            // Smoke-test path: spawn the command with `--help` and give
            // it 2 seconds to respond. We consider it reachable if the
            // process starts at all (even if `--help` returns non-zero).
            let mut probe_args: Vec<&str> = args.iter().map(String::as_str).collect();
            probe_args.push("--help");

            // PATHEXT-aware (issue #6): on Windows this becomes
            // `cmd /C npx -y --package=@ijfw/memory-server ijfw-memory
            // --help` so the `npx.cmd`
            // shim resolves; on Unix it spawns `npx …` directly.
            let mut cmd = shim_aware_command(command);
            cmd.args(&probe_args)
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null());

            // Spawn and wait with a timeout implemented via `wait_timeout`
            // from the standard library's thread::sleep approach. We avoid
            // pulling in the `wait-timeout` crate to keep deps minimal.
            match cmd.spawn() {
                Err(_) => false,
                Ok(mut child) => {
                    // Poll for up to 2 seconds in 50ms increments.
                    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
                    loop {
                        match child.try_wait() {
                            Ok(Some(status)) => {
                                // #928: a successful *spawn* proves nothing
                                // here. On Windows the child is `cmd /C`, and
                                // through `npx` the child is the npm shim, so
                                // spawning succeeds for a command that cannot
                                // possibly work — this arm used to return
                                // `true` for any exit code and so could never
                                // reject anything. `npx -y @ijfw/memory-server
                                // --help` exits 1 with "could not determine
                                // executable to run"; the probe called that
                                // reachable, the server was registered, and
                                // every call then failed ("Retry isn't
                                // responding"). A clean exit is the only
                                // evidence that the command line resolved to
                                // a real executable.
                                return status.success();
                            }
                            Ok(None) if std::time::Instant::now() < deadline => {
                                std::thread::sleep(std::time::Duration::from_millis(50));
                            }
                            Ok(None) => {
                                // Still running after 2 s — it's a real
                                // server, treat as reachable.
                                let _ = child.kill();
                                return true;
                            }
                            Err(_) => {
                                return false;
                            }
                        }
                    }
                }
            }
        }
        // SSE / HTTP transports: we can't do a cheap local probe, so
        // trust the registration and let wcore-mcp surface errors at
        // connection time.
        _ => true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_spec_round_trips_serde() {
        let spec = default_server_spec();
        let s = serde_json::to_string(&spec).unwrap();
        let parsed: McpServerSpec = serde_json::from_str(&s).unwrap();
        assert_eq!(parsed.name, SERVER_NAME);
        match parsed.transport {
            McpTransport::Stdio { command, args } => {
                assert_eq!(command, "npx");
                assert!(args.iter().any(|a| a.contains(MEMORY_SERVER_PACKAGE)));
            }
            _ => panic!("expected stdio transport for default IJFW MCP server"),
        }
    }

    /// #928 — the invocation must NAME the executable.
    ///
    /// `npx <positional>` treats the trailing positional as a *bin* name and
    /// only falls back to "install this package and guess its bin" when the
    /// package's unscoped name happens to equal one of its bins.
    /// `@ijfw/memory-server` ships `ijfw-memory` / `ijfw` /
    /// `ijfw-dispatch-plan`, so the old `npx -y @ijfw/memory-server` exited 1
    /// with `could not determine executable to run` on every platform and the
    /// memory server could never start.
    ///
    /// This asserts the *mechanism*, not the literal string: the package must
    /// travel as a `--package=` option (never as the executable positional),
    /// and the executable positional must be a bare bin name — no `@`, no `/`.
    /// Reverting to `npx -y @ijfw/memory-server` fails both halves.
    #[test]
    fn default_spec_names_an_executable_rather_than_the_package() {
        let McpTransport::Stdio { command, args } = default_server_spec().transport else {
            panic!("expected stdio transport");
        };
        assert_eq!(command, "npx");

        let positionals: Vec<&String> = args.iter().filter(|a| !a.starts_with('-')).collect();
        assert_eq!(
            positionals.len(),
            1,
            "npx takes exactly one executable positional, got {positionals:?}"
        );
        let exe = positionals[0];
        assert!(
            !exe.contains('@') && !exe.contains('/'),
            "the npx executable positional must be a bin name, not a package \
             specifier — got {exe:?}; `npx -y @ijfw/memory-server` exits 1 with \
             \"could not determine executable to run\" (#928)"
        );
        assert!(
            args.iter()
                .any(|a| a == &format!("--package={MEMORY_SERVER_PACKAGE}")),
            "the package must be supplied with --package= so npx installs it \
             while still running the named bin; args were {args:?}"
        );
        assert_eq!(exe, MEMORY_SERVER_BIN);
    }

    /// A command whose exit code is non-zero is NOT reachable.
    ///
    /// #928 root cause: this arm used to `return true` for *any* exit code,
    /// on the theory that "the process started" proves the binary is real.
    /// It does not. Through `npx` the child is the npm shim and on Windows the
    /// child is `cmd /C`, so the spawn succeeds for a command line that cannot
    /// work — the probe could never reject anything, the broken memory server
    /// was registered anyway, and every call failed afterwards.
    ///
    /// This spawns a REAL process (no fake, no shape assertion): on Unix
    /// `false --help` exits 1; on Windows `cmd /C <absent> --help` exits 1
    /// while `cmd` itself spawns fine, which is precisely the shape that
    /// defeated the old code.
    #[test]
    fn reachability_rejects_a_command_that_exits_non_zero() {
        let command = if cfg!(windows) {
            "wayland-ijfw-definitely-absent-binary-xyz"
        } else {
            "false"
        };
        let spec = McpServerSpec {
            name: "probe-test".to_string(),
            transport: McpTransport::Stdio {
                command: command.to_string(),
                args: vec![],
            },
            env: HashMap::new(),
        };
        assert!(
            !mcp_server_is_reachable(&spec),
            "a child that exits non-zero must not be reported reachable"
        );
    }

    /// Control for the test above: the strict exit check must not reject a
    /// command that genuinely works, or the probe becomes a gate that can
    /// never PASS — equally useless. `true --help` / `cmd /C rem --help` both
    /// exit 0 and ignore the trailing flag.
    #[test]
    fn reachability_accepts_a_command_that_exits_cleanly() {
        let command = if cfg!(windows) { "rem" } else { "true" };
        let spec = McpServerSpec {
            name: "probe-test".to_string(),
            transport: McpTransport::Stdio {
                command: command.to_string(),
                args: vec![],
            },
            env: HashMap::new(),
        };
        assert!(
            mcp_server_is_reachable(&spec),
            "a child that exits 0 must still be reported reachable"
        );
    }

    /// #928 — the skip must reach the user, not just the log file.
    ///
    /// `wcore-cli` caps its stderr writer at `Level::ERROR`, so the original
    /// `tracing::info!` meant a user with `RUST_LOG` unset was told nothing at
    /// all when memory was switched off. Asserting on the recorded LEVEL is
    /// the point: a regression to `info!`/`warn!` still "logs the failure" and
    /// still leaves the user staring at a dead Memory button.
    #[test]
    fn notice_for_unreachable_server_is_user_visible() {
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
        tracing::subscriber::with_default(subscriber, notify_server_unreachable);

        let levels = captured.0.lock().unwrap().clone();
        assert_eq!(
            levels.len(),
            1,
            "expected exactly one event, got {levels:?}"
        );
        assert_eq!(
            levels[0],
            tracing::Level::ERROR,
            "the unreachable-server notice must be ERROR: wcore-cli's stderr \
             writer is capped at ERROR, so anything quieter never reaches the \
             user (#928)"
        );
    }

    /// Live proof against the real npm registry. `#[ignore]`d and env-gated so
    /// it stays out of the default lane, per this repo's live-test convention.
    ///
    /// Asserts BOTH directions against the same probe: the shipped spec is
    /// reachable, and the pre-#928 invocation is not. Without the second half
    /// a green here would not distinguish the fix from the bug.
    ///
    /// `WAYLAND_IJFW_LIVE_NPX=1 cargo test -p wayland-ijfw -- --ignored`
    #[test]
    #[ignore = "needs network + npx; gated on WAYLAND_IJFW_LIVE_NPX=1"]
    fn live_npx_invocation_starts_the_memory_server() {
        assert_eq!(
            std::env::var("WAYLAND_IJFW_LIVE_NPX").ok().as_deref(),
            Some("1"),
            "declared intent to run this live case but WAYLAND_IJFW_LIVE_NPX is \
             not 1 — refusing to report a pass that measured nothing"
        );
        assert!(
            command_available("npx", "--version"),
            "live case requires npx on PATH"
        );

        let broken = McpServerSpec {
            name: SERVER_NAME.to_string(),
            transport: McpTransport::Stdio {
                command: "npx".to_string(),
                args: vec!["-y".to_string(), MEMORY_SERVER_PACKAGE.to_string()],
            },
            env: HashMap::new(),
        };
        assert!(
            !mcp_server_is_reachable(&broken),
            "the pre-#928 invocation `npx -y {MEMORY_SERVER_PACKAGE}` must be \
             rejected — it exits 1 with \"could not determine executable to run\""
        );

        assert!(
            mcp_server_is_reachable(&default_server_spec()),
            "the shipped invocation must be reachable against the live registry"
        );
    }

    // Issue #6: the probe must route through `cmd /C` on Windows so the
    // `npx.cmd` PATHEXT shim resolves; on Unix it spawns the program direct.
    #[test]
    fn shim_aware_command_routes_through_cmd_on_windows() {
        let cmd = shim_aware_command("npx");
        let program = cmd.get_program().to_string_lossy().to_string();
        let args: Vec<String> = cmd
            .get_args()
            .map(|a| a.to_string_lossy().to_string())
            .collect();
        #[cfg(windows)]
        {
            assert_eq!(program, "cmd");
            assert_eq!(args, vec!["/C".to_string(), "npx".to_string()]);
        }
        #[cfg(not(windows))]
        {
            assert_eq!(program, "npx");
            assert!(args.is_empty());
        }
    }

    #[test]
    fn command_available_is_false_for_absent_binary() {
        assert!(!command_available(
            "wayland-ijfw-definitely-absent-binary-xyz",
            "--version"
        ));
    }

    // RANK-57: the reachability probe must not run on the async worker.
    // We can't directly observe which thread it ran on, but we CAN assert
    // it completes promptly on a *single-threaded* current-thread runtime —
    // if the blocking poll were run inline on the worker (instead of via
    // `spawn_blocking`) the runtime could not also drive the join future.
    // The `node`-fast-path returns without spawning any process, so the
    // whole thing must finish well under the 2s probe budget.
    #[test]
    fn probe_reachability_node_fast_path_skips_for_absent_script() {
        use wcore_plugin_api::mcp_server_spec::McpTransport;
        // Absolute path that does not exist → fast path returns false, no
        // process spawned, no thread::sleep poll loop entered.
        let abs = if cfg!(windows) {
            "C:\\wayland-ijfw\\definitely\\absent\\server.js"
        } else {
            "/wayland-ijfw/definitely/absent/server.js"
        };
        let spec = McpServerSpec {
            name: "probe-test".to_string(),
            transport: McpTransport::Stdio {
                command: "node".to_string(),
                args: vec![abs.to_string()],
            },
            env: HashMap::new(),
        };
        assert!(!mcp_server_is_reachable(&spec));
    }

    // The probe is awaited via `spawn_blocking`, so it must drive to
    // completion on a current-thread runtime without deadlocking the worker.
    #[test]
    fn probe_offloads_to_blocking_thread_on_current_thread_runtime() {
        use wcore_plugin_api::mcp_server_spec::McpTransport;
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("build current-thread runtime");

        let abs = if cfg!(windows) {
            "C:\\wayland-ijfw\\definitely\\absent\\server.js"
        } else {
            "/wayland-ijfw/definitely/absent/server.js"
        };
        let spec = McpServerSpec {
            name: "probe-test".to_string(),
            transport: McpTransport::Stdio {
                command: "node".to_string(),
                args: vec![abs.to_string()],
            },
            env: HashMap::new(),
        };

        let reachable = rt.block_on(async move {
            tokio::task::spawn_blocking(move || probe_reachability(&spec))
                .await
                .unwrap_or(false)
        });
        // npx may or may not be present in CI, but the node fast-path inside
        // the spec we passed short-circuits to false regardless. What this
        // asserts is that the spawn_blocking offload joins cleanly on a
        // single-threaded runtime (no reactor starvation / deadlock).
        assert!(!reachable);
    }

    // ---------------------------------------------------------------------
    // wayland-core#340 — the reachability probe must not execute a registry
    // package before the malware gate has seen it.
    // ---------------------------------------------------------------------

    #[test]
    fn package_runners_are_recognised_including_windows_shims() {
        for command in [
            "npx",
            "npx.cmd",
            "NPX.EXE",
            "/usr/local/bin/uvx",
            "pipx",
            "pnpm",
            "yarn",
            "bunx",
            "bun",
            "uv",
            "npm",
            "deno",
            "npx.ps1",
        ] {
            assert!(
                is_package_runner(command),
                "{command} fetches from a registry"
            );
        }
        // Negative controls: these execute something already on disk.
        for command in ["node", "python3", "/opt/mcp/my-server", "sh", "docker"] {
            assert!(!is_package_runner(command), "{command} fetches nothing");
        }
    }

    /// The probe used to spawn `npx … --help`, which DOWNLOADS AND RUNS the
    /// package — before `wcore_mcp::malware_gate` sees the launch. A runner
    /// spec must now be reported reachable without any spawn at all.
    ///
    /// `npx.cmd` cannot be executed on Unix, so if the smoke test still ran
    /// for a runner this would spawn-error and return `false`.
    #[test]
    #[cfg(unix)]
    fn a_package_runner_spec_is_never_spawned_by_the_probe() {
        use wcore_plugin_api::mcp_server_spec::{McpServerSpec, McpTransport};
        let spec = McpServerSpec {
            name: "probe".into(),
            transport: McpTransport::Stdio {
                command: "npx.cmd".into(),
                args: vec![
                    "-y".into(),
                    "--package=@ferroxlabs/nonexistent-340".into(),
                    "nonexistent-bin".into(),
                ],
            },
            env: std::collections::HashMap::new(),
        };
        assert!(
            mcp_server_is_reachable(&spec),
            "a package runner must be registered without executing the package"
        );
    }

    /// Negative control for the test above: the smoke test still runs — and
    /// still rejects — for a command that is NOT a package runner. Without
    /// this, `mcp_server_is_reachable` returning `true` would prove nothing.
    #[test]
    fn a_non_runner_command_that_cannot_start_is_still_rejected() {
        use wcore_plugin_api::mcp_server_spec::{McpServerSpec, McpTransport};
        let spec = McpServerSpec {
            name: "probe".into(),
            transport: McpTransport::Stdio {
                command: "ferroxlabs-no-such-binary-340".into(),
                args: vec!["--serve".into()],
            },
            env: std::collections::HashMap::new(),
        };
        assert!(!mcp_server_is_reachable(&spec));
    }

    /// #928 used to be caught by spawning the package at every startup. It is
    /// a property of a constant, so it is graded here instead: `npx` only
    /// resolves a bin whose name equals the package's unscoped name, and
    /// `@ijfw/memory-server` ships no `memory-server` bin.
    #[test]
    fn default_spec_names_the_bin_explicitly() {
        use wcore_plugin_api::mcp_server_spec::McpTransport;
        let McpTransport::Stdio { command, args } = default_server_spec().transport else {
            panic!("the default IJFW MCP spec must be stdio");
        };
        assert_eq!(command, "npx");
        assert!(
            args.iter()
                .any(|a| a == &format!("--package={MEMORY_SERVER_PACKAGE}")),
            "the package must be passed with --package= (#928): {args:?}"
        );
        assert_eq!(
            args.last().map(String::as_str),
            Some(MEMORY_SERVER_BIN),
            "the bin must be named explicitly (#928): {args:?}"
        );
    }
}
