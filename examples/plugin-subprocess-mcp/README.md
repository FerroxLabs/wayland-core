# Example: wrap an MCP server as a Wayland plugin

This example demonstrates the **"wrap an MCP server, get a Wayland plugin for free"** story introduced in v0.6.5.

The Wayland engine ships with an `mcp-bridge` plugin runtime. Drop a manifest like the one in this directory into `~/.wayland/plugins/<name>/plugin.toml` and the engine will:

1. Spawn the binary you point at.
2. Perform the standard MCP `initialize` + `notifications/initialized` + `tools/list` handshake over stdio.
3. Synthesize one Wayland `PluginTool` per MCP tool the server advertises.
4. Register each synthesized tool under the manifest's `tool_namespace`.

No per-server adapter code is required — any conformant MCP server works.

## What's in this directory

```
plugin-subprocess-mcp/
├── README.md                      <- you are here
├── plugin.toml                    <- Wayland manifest (kind = "mcp-bridge")
└── mcp-server-time/               <- standalone demo MCP server
    ├── Cargo.toml                 <-   not a workspace member
    ├── README.md
    └── src/main.rs                <-   ~140 LOC tokio JSON-RPC loop
```

The `mcp-server-time` crate is intentionally **outside** the main Wayland workspace — it represents a downstream third-party MCP server that knows nothing about Wayland.

## Try it end-to-end

### 1. Build the MCP server

```bash
cd examples/plugin-subprocess-mcp/mcp-server-time
cargo build --release
```

The binary lands at `examples/plugin-subprocess-mcp/mcp-server-time/target/release/mcp-server-time`.

### 2. Install the plugin

```bash
mkdir -p ~/.wayland/plugins/mcp-time
cp plugin.toml ~/.wayland/plugins/mcp-time/
```

### 3. Adjust `binary_path` to an absolute path

Open `~/.wayland/plugins/mcp-time/plugin.toml` and change `runtime.subprocess.binary_path` to the absolute path of the built binary, for example:

```toml
[runtime.subprocess]
binary_path = "/Users/you/dev/wayland/examples/plugin-subprocess-mcp/mcp-server-time/target/release/mcp-server-time"
args = []
```

The default value in the shipped manifest (`./mcp-server-time/target/release/mcp-server-time`) is relative to the manifest directory — it only resolves correctly if you run Wayland directly out of this examples folder, which most users won't.

### 4. Restart the Wayland engine

After restart, the tool **`Time::get_time`** should appear in the tool catalog. The namespace `Time` comes from `permissions.tool_namespace` in the manifest; the tool name `get_time` comes from the MCP server's `tools/list` response.

## Discovery flow (loader integration)

The engine's plugin loader (v0.6.5 Task 2.7b) walks `~/.wayland/plugins/*/plugin.toml` at startup. For each manifest:

1. It parses the manifest with `wcore_plugin_api::manifest::PluginManifest`.
2. It inspects `runtime.kind`.
3. When `kind == "mcp-bridge"`, it dispatches to `wcore_plugin_subprocess::mcp_bridge::McpBridgePluginRunner::load(manifest_path, manifest, gate)`.
4. The returned `LoadedMcpBridgePlugin.tools()` is folded into the engine's `InitializeOutcome.tools` surface alongside statically-linked plugin tools.

From the apply pipeline's perspective, an MCP-bridged tool is indistinguishable from any other plugin tool — same `PluginTool` shape, same `execute` closure contract, same `ToolCategory::Mcp` classification.

## Tests

This example is also exercised by `crates/wcore-plugin-subprocess/tests/mcp_bridge_real_subprocess.rs`, which builds `mcp-server-time` via `cargo` and drives it through `McpBridgePluginRunner::load` end-to-end (gated behind the `EXAMPLE_REAL_SUBPROCESS=1` env var to keep CI fast).
