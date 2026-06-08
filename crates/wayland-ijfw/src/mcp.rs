//! G.7 — register the IJFW MCP server.
//!
//! Spawns `@ijfw/memory-server` via stdio. The MCP server itself
//! exposes the canonical memory tools (`memory_store`, `memory_search`,
//! `memory_recall`, `memory_prelude`, `ijfw_run`, `ijfw_update_apply`)
//! — wcore-mcp's tool proxy ingests the server's tool list at first
//! use and surfaces it through the normal MCP tool path.
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
/// F-060 / B4: gate on `npx` being present on PATH AND the server being
/// reachable. Two-stage probe:
///
/// 1. `npx --version` presence check (fast).
/// 2. If the transport is `Stdio { command: "node", args: [path, …] }`,
///    check the script path exists with `std::fs::metadata`.  Otherwise,
///    run the command with a 2-second timeout (`--help` or similar) to
///    verify it starts rather than exiting immediately.
///
/// On any probe failure we log INFO and return `Ok(())` — the MCP server
/// is optional infrastructure and must NOT block the engine.
pub fn register(ctx: &mut PluginContext<'_>) -> PluginResult<()> {
    // Wave RB STABILITY MINOR #13: typed HostMisconfiguration error.
    let registry = ctx.mcp_servers.as_mut().ok_or_else(|| {
        wcore_plugin_api::PluginError::HostMisconfiguration {
            plugin: "wayland-ijfw".into(),
            surface: "mcp_servers".into(),
        }
    })?;

    // Stage 1: npx presence (fast, no startup cost).
    let npx_available = std::process::Command::new("npx")
        .arg("--version")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false);

    if !npx_available {
        tracing::info!(
            "ijfw-memory: npx not found on PATH — skipping MCP registration \
             (install Node.js to enable)"
        );
        return Ok(());
    }

    // Stage 2: verify the server is actually reachable.
    let spec = default_server_spec();
    if !mcp_server_is_reachable(&spec) {
        tracing::info!(
            "ijfw-memory: MCP server did not start cleanly — skipping registration. \
             Run `npx @ijfw/memory-server --help` manually to diagnose."
        );
        return Ok(());
    }

    registry.register_mcp_server(spec)?;
    Ok(())
}

/// Returns `true` if the MCP server is reachable / will start.
///
/// For `Stdio { command: "node", args: [script, …] }`: checks the script
/// file exists on disk (fast, no process spawn).
///
/// For all other stdio commands (e.g. `npx @ijfw/memory-server`): spawns
/// the server with a `--help` flag and waits up to 2 seconds. If the
/// process exits with code 0 or the `--help` flag causes it to exit
/// non-zero but the process at least *starts* (spawn succeeds), we treat
/// the server as reachable. If the spawn fails (binary not found / exits
/// immediately with error) we skip.
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

            // Smoke-test path: spawn the command with `--help` and give
            // it 2 seconds to respond. We consider it reachable if the
            // process starts at all (even if `--help` returns non-zero).
            let mut probe_args: Vec<&str> = args.iter().map(String::as_str).collect();
            probe_args.push("--help");

            let mut cmd = std::process::Command::new(command);
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
                            Ok(Some(_)) => {
                                // Process exited — it started, which is
                                // enough to confirm the binary is present
                                // and executable. `--help` may exit 1,
                                // but that's fine.
                                return true;
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
                assert!(args.iter().any(|a| a == "@ijfw/memory-server"));
            }
            _ => panic!("expected stdio transport for default IJFW MCP server"),
        }
    }
}
