//! G.7 — register the IJFW MCP server.
//!
//! Spawns `@ijfw/memory-server` via stdio. The MCP server itself
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

/// Build the default IJFW MCP server spec. Operators override the
/// transport (npx vs locally-installed binary) via plugin config.
pub fn default_server_spec() -> McpServerSpec {
    McpServerSpec {
        name: SERVER_NAME.to_string(),
        transport: McpTransport::Stdio {
            command: "npx".to_string(),
            args: vec!["-y".to_string(), "@ijfw/memory-server".to_string()],
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
///    check the script path exists with `std::fs::metadata`. Otherwise the
///    spec is trusted and `wcore-mcp` surfaces any error at connect time —
///    core#340: pre-executing a package runner here ran the registry package
///    before the malware gate on the stdio launch path could see it.
///
/// On any probe failure we log INFO and return `Ok(())` — the MCP server
/// is optional infrastructure and must NOT block the engine.
///
/// RANK-57 hardening: `command_available` spawns a child process. Running that
/// directly on a tokio worker starves the reactor (guaranteed cold-start
/// latency, and on a single-threaded runtime it blocks every other task). We
/// therefore make `register` async and run the blocking probe inside
/// `tokio::task::spawn_blocking`, awaiting the result so no async worker is
/// ever blocked. Only the thread it runs on changes.
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
    if !mcp_server_is_reachable(spec) {
        tracing::info!(
            "ijfw-memory: MCP server did not start cleanly — skipping registration. \
             Run `npx @ijfw/memory-server --help` manually to diagnose."
        );
        return false;
    }

    true
}

/// Returns `true` if the MCP server is reachable / will start.
///
/// For `Stdio { command: "node", args: [script, …] }`: checks the script
/// file exists on disk (fast, no process spawn).
///
/// For every other stdio command: `true` without probing.
///
/// # Why this stopped spawning the server (core#340)
///
/// This used to run the configured command with `--help` and a 2-second
/// budget. For this plugin's own default spec that command is
/// `npx -y @ijfw/memory-server`, and `npx -y` FETCHES the package from the
/// public npm registry and executes its entry point — install scripts and
/// all. That happened here, at plugin `initialize()`, which is long before
/// `wcore_mcp`'s stdio transport (and therefore the OSV malware gate that
/// sits in front of it) ever sees the spec.
///
/// So the gate could refuse the launch all it liked: the package had already
/// run. A pre-exec probe of a package runner is not a cheap reachability
/// check, it is the execution the gate exists to gate.
///
/// The plugin cannot call the gate itself — audit F2 forbids a `wcore-mcp` or
/// `wcore-tools` dependency here, and the `wcore-plugin-api` build.rs lint
/// enforces it. The fix is therefore to stop pre-executing and let the ONE
/// gated launch path be the only one: registration now defers, exactly as the
/// SSE/HTTP arm below already does, and `wcore-mcp` surfaces a connect error
/// if the server does not start.
///
/// Stage 1 of `probe_reachability` is unchanged and still does the work this
/// probe was for: `npx --version` answers "is Node installed at all?" without
/// naming, fetching or running any registry package.
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

            // Everything else — `npx`, `uvx`, `pipx`, a local binary — is
            // NOT probed. Running it here would execute it outside the one
            // launch path the malware gate sits on; see the doc comment.
            let _ = (command, args);
            true
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
                assert!(args.iter().any(|a| a == "@ijfw/memory-server"));
            }
            _ => panic!("expected stdio transport for default IJFW MCP server"),
        }
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

    /// core#340 item 3 — the ONE arm that matters here.
    ///
    /// The reachability probe must not EXECUTE the configured command. For
    /// this plugin's own default spec that command is `npx -y
    /// @ijfw/memory-server`, which fetches the package from the public npm
    /// registry and runs its entry point — and it happened at plugin
    /// `initialize()`, before `wcore-mcp`'s stdio transport (and the OSV
    /// malware gate in front of it) ever saw the spec.
    ///
    /// The instrument is a marker file written by the child itself, not a
    /// return value: only the child can answer "did it run?".
    #[cfg(unix)]
    #[test]
    fn the_probe_does_not_execute_the_configured_package_runner() {
        use std::os::unix::fs::PermissionsExt;
        use wcore_plugin_api::mcp_server_spec::McpTransport;

        let dir = tempfile::tempdir().expect("tempdir");
        let marker = dir.path().join("PROBE-EXECUTED-THE-RUNNER");
        let runner = dir.path().join("npx");
        std::fs::write(
            &runner,
            format!("#!/bin/sh\ntouch '{}'\nexit 0\n", marker.display()),
        )
        .expect("write fake runner");
        let mut perms = std::fs::metadata(&runner).expect("stat").permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&runner, perms).expect("chmod");

        let spec = McpServerSpec {
            name: "probe-test".to_string(),
            transport: McpTransport::Stdio {
                command: runner.to_str().expect("utf-8 path").to_string(),
                args: vec!["-y".to_string(), "@ijfw/memory-server".to_string()],
            },
            env: HashMap::new(),
        };

        // KNOWN-POSITIVE CONTROL: the script IS executable and DOES write the
        // marker, so an empty marker below means "was not run", not "could not
        // run". Without this the assertion is satisfied by a broken fixture.
        let status = std::process::Command::new(&runner)
            .status()
            .expect("the fixture runner must be executable");
        assert!(status.success() && marker.exists(), "fixture is inert");
        std::fs::remove_file(&marker).expect("reset marker");

        let reachable = mcp_server_is_reachable(&spec);

        assert!(
            !marker.exists(),
            "the reachability probe EXECUTED the package runner — a registry \
             package ran before the OSV malware gate on the stdio launch path \
             could see it"
        );
        assert!(
            reachable,
            "with no probe, the spec is trusted and wcore-mcp reports connect \
             failures instead"
        );
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
}
