//! D6 — MCP round-trip scenarios.
//!
//! Exercises a full MCP stdio round-trip through the REAL `wayland-core`
//! binary: the engine connects to a mock stdio MCP server, performs the
//! `initialize` / `tools/list` handshake, the model calls the advertised
//! `mcp_echo` tool, and we assert the call fired AND a sentinel string
//! round-tripped back through the tool result.
//!
//! ## Why a python script (not a cargo bin)
//!
//! The mock server is a self-contained `python3` script (stdlib only,
//! ~80 lines). `python3` resolves on every CI runner and dev box this
//! suite targets; a cargo bin target would need workspace wiring we are
//! explicitly not touching. The [`Scenario::setup`] hook writes the
//! script into the scenario tempdir at runtime and `chmod +x`'s it.
//!
//! ## How the engine discovers the server (config wiring)
//!
//! wayland-core reads MCP servers ONLY from `[mcp.servers.*]` in
//! `config.toml` (`wcore-config/src/config.rs` `McpConfig` /
//! `McpServerConfig`; there is NO project-level `.mcp.json` reader — grep
//! confirmed). `tempenv::build` seeds `<cwd>/.wayland-core/config.toml`
//! BEFORE the setup hook runs and the runner spawns the binary with
//! `cwd = tempdir`, so the setup hook simply **APPENDS** an
//! `[mcp.servers.mock_echo]` block to that already-seeded file. This needs
//! NO change to `tempenv.rs`.
//!
//! The appended block (values filled in at runtime with absolute paths):
//!
//! ```toml
//! [mcp.servers.mock_echo]
//! transport = "stdio"
//! command = "python3"
//! args = ["<tempdir>/mock_mcp_server.py"]
//! deferred = false
//! ```
//!
//! Field semantics (verified against `McpServerConfig`):
//!   - `transport = "stdio"` → launch a subprocess, talk JSON-RPC over its
//!     stdin/stdout. `TransportType` is `#[serde(rename_all = "kebab-case")]`
//!     so the literal string is `"stdio"`.
//!   - `command` / `args` → the program + argv. We pass `python3` as the
//!     command and the absolute script path as the sole arg.
//!   - `deferred = false` → **critical**. MCP tools default to
//!     `deferred = true`, which registers them as name-only stubs the model
//!     must first discover via `ToolSearch`. Setting `false` sends the full
//!     `mcp_echo` schema eagerly so a single-turn prompt can call it
//!     directly and deterministically.
//!
//! ## Tool name in the trace
//!
//! `mcp_echo` does not collide with any built-in tool, so the MCP proxy
//! registers it under its ORIGINAL name `mcp_echo` (collision would prefix
//! it to `mcp__mock_echo__mcp_echo` — see
//! `wcore-mcp/src/tool_proxy.rs::register_mcp_tools`). The trace's
//! `tool_name` is therefore `mcp_echo`, which is what `expect_tool` and the
//! `TraceAssertion` count against.
//!
//! ## WIRING NEEDED (lib.rs only — no tempenv change)
//!
//! Add to `crates/wcore-eval-scenarios/src/lib.rs`:
//! ```ignore
//! pub mod mcp_scenarios;
//! ```
//! and, if the binary aggregates scenario lists, fold `mcp_scenarios::all()`
//! into the master vec the same way `qa::all()` is folded. No `tempenv.rs`,
//! `runner.rs`, `scenario.rs`, or `Cargo.toml` edit is required — the setup
//! hook does all the per-run wiring.

use std::time::Duration;

use crate::assertions::{Assertion, TraceAssertion};
use crate::providers::ProviderChoice;
use crate::scenario::{Category, Platform, Scenario, Turn};

/// The mock MCP server script, embedded so the scenario is self-contained
/// (no dependency on the `tests/fixtures/` copy being present at the
/// runtime cwd). Kept byte-for-byte in sync with
/// `tests/fixtures/mock_mcp_server.py`.
const MOCK_MCP_SERVER_PY: &str = include_str!("../tests/fixtures/mock_mcp_server.py");

/// File name the setup hook writes the server script to, inside the cwd.
const SERVER_SCRIPT_NAME: &str = "mock_mcp_server.py";

/// Sentinel the prompt asks the agent to echo. Distinctive enough that a
/// `Contains` on the final text / tool output can't pass by coincidence.
const SENTINEL: &str = "WAYLAND-MCP-RTRIP-7F3A";

/// Scaffold the mock MCP server + wire it into the seeded config.
///
/// Runs in the scenario tempdir (`cwd`). Three steps:
/// 1. Write the python server script and (unix) `chmod +x` it.
/// 2. Append an `[mcp.servers.mock_echo]` stdio block to the config.toml
///    that `tempenv::build` already wrote at `.wayland-core/config.toml`.
fn setup_mock_mcp(cwd: &std::path::Path) -> anyhow::Result<()> {
    use std::io::Write as _;

    // 1. Write the server script into the cwd.
    let script_path = cwd.join(SERVER_SCRIPT_NAME);
    std::fs::write(&script_path, MOCK_MCP_SERVER_PY)?;

    // 2. Make it executable on unix (belt-and-suspenders; we launch via
    //    `python3 <script>` so the +x bit isn't strictly required, but a
    //    chmod keeps the fixture runnable standalone for debugging).
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = std::fs::metadata(&script_path)?.permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&script_path, perms)?;
    }

    // 3. Append the MCP server block to the seeded config.toml. tempenv
    //    has already created this file; we open it for append so we keep
    //    its [session] / [provider] blocks intact.
    let config_path = cwd.join(".wayland-core").join("config.toml");
    let script_abs = script_path.to_string_lossy().to_string();
    // Reuse tempenv's TOML basic-string escaper for the path arg so a
    // tempdir containing a backslash/quote (Windows, odd CI roots) stays
    // valid TOML.
    let script_escaped = crate::tempenv::escape_toml_basic(&script_abs);

    let block = format!(
        "\n\
         # D6 — mock stdio MCP server (appended by mcp_scenarios::setup_mock_mcp)\n\
         [mcp.servers.mock_echo]\n\
         transport = \"stdio\"\n\
         command = \"python3\"\n\
         args = [\"{script_escaped}\"]\n\
         deferred = false\n",
    );

    let mut f = std::fs::OpenOptions::new()
        .append(true)
        .open(&config_path)
        .map_err(|e| {
            anyhow::anyhow!(
                "mcp_scenarios setup: cannot append MCP block to {} \
                 (tempenv should have created it first): {e}",
                config_path.display()
            )
        })?;
    f.write_all(block.as_bytes())?;
    Ok(())
}

/// MCP round-trip: the agent must call the `mcp_echo` MCP tool with a
/// sentinel string and the sentinel must come back through the tool.
///
/// Asserts on three independent signals so a partial wiring can't pass:
///   - `expect_tool("mcp_echo")` → the MCP tool actually FIRED this turn
///     (proves connect + handshake + tools/list + dispatch all worked).
///   - `TraceAssertion::NoErrorsOnTool("mcp_echo")` → the `tools/call`
///     returned a non-error result (the server answered, not a transport
///     failure).
///   - `Assertion::Contains(SENTINEL)` on the final assistant text → the
///     sentinel round-tripped back to the model, which echoed it to the
///     user. (The model is instructed to repeat the tool's exact output.)
pub fn mcp_echo_roundtrip() -> Scenario {
    Scenario::new("mcp_echo_roundtrip", Category::Coverage)
        .platforms([Platform::Linux, Platform::Macos])
        .provider(ProviderChoice::ForceDeepSeek)
        .max_total_time(Duration::from_secs(120))
        .max_total_cost_usd(0.05)
        .setup(setup_mock_mcp)
        .turn(
            Turn::new(format!(
                "Use the `mcp_echo` tool to echo back this exact string: {SENTINEL}\n\
                 Call the tool with that string as the `text` argument, then reply to me \
                 with the tool's exact output and nothing else."
            ))
            .max_time(Duration::from_secs(100))
            .max_steps(6)
            .expect_tool("mcp_echo")
            .trace(TraceAssertion::CountAtLeast {
                tool: "mcp_echo",
                n: 1,
            })
            .trace(TraceAssertion::NoErrorsOnTool("mcp_echo"))
            .assert(Assertion::Contains(SENTINEL)),
        )
}

/// All D6 MCP scenarios, in a stable order.
pub fn all() -> Vec<Scenario> {
    vec![mcp_echo_roundtrip()]
}
