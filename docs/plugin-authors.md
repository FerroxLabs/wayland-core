# Wayland Plugin Authors Guide

Wayland exposes a stable plugin API that lets third parties contribute tools, hooks, providers, skills, rules, MCP servers, and user-model backends without forking the engine. This guide is the canonical onboarding doc for v0.6.5. Read it once end-to-end before you write code; come back later for the manifest and permission reference tables.

The substrate is intentionally pluralist: four runtimes share one manifest schema and one permission model. Your job is to pick the right runtime, declare the right permissions, and let the host decide what actually gets granted.

---

## 1. Choosing a runtime (decision tree)

Every Wayland plugin runs in exactly one of four modes. Pick by answering these in order:

```
Are you on Wayland's first-party team and shipping with the engine binary?
   yes  -> static          (plug in tree, zero IPC overhead)
   no   -> next question

Do you need unrestricted OS access — raw sockets, child processes, native FS?
   yes  -> subprocess      (engine spawns a binary; inherits engine privileges)
   no   -> next question

Are you wrapping an existing Model Context Protocol server?
   yes  -> mcp-bridge      (zero plugin code; manifest only)
   no   -> WASM            (canonical sandboxed third-party path)
```

Practical guidance:

- **static** — Compile-time-linked Rust crate discovered via `inventory::submit!`. Identity is anchored to the engine binary's link table, so impersonation is impossible (`crates/wcore-plugin-api/src/manifest.rs:43-48`). No signing required. The only choice for first-party plugins where the team owns both the engine and the plugin.
- **wasm** — A `.wasm` Component-Model module instantiated fresh per tool call inside a wasmtime sandbox. The only runtime where host capabilities are opt-in via `Deny*` adapters (`crates/wcore-plugin-api/src/manifest.rs:79-84`). This is the canonical choice for the open ecosystem.
- **subprocess** — A native binary the engine spawns and talks to over JSON-Lines on stdio. Required when you need raw FS, raw network sockets, child processes, or non-Rust toolchains. Inherits engine privileges by default (`crates/wcore-plugin-api/src/manifest.rs:69-71`). Use sparingly.
- **mcp-bridge** — Manifest-only wrapper around an existing MCP server. Wayland synthesizes a `PluginTool` per discovered MCP tool. Zero plugin code; the MCP server you wrap is the implementation. Use this whenever someone has already built the integration in MCP — you do not need to port it.

When in doubt, write WASM. The sandbox is the default; you graduate to subprocess only when WASM cannot express what you need.

---

## 2. Capability vs ownership

This framing is lifted from OpenClaw research and is load-bearing for everything that follows.

A **plugin** is a vendor / feature boundary. One author, one team, one organizational unit, one manifest, one trust anchor.

A **capability** is a core contract. Tool, Hook, Skill, Provider, Agent, Rule, MCPServer, UserModel — these are the surfaces the engine offers. They are not plugins.

The two axes are orthogonal:

- One plugin can contribute many capabilities. The first-party OpenAI plugin contributes a Provider (chat completions), a Skill (transcription), and a Tool (image generation) under one manifest, one version, one set of permissions.
- Multiple plugins can implement the same capability type. The first-party OpenAI plugin contributes a Provider; a community Anthropic plugin contributes a Provider; the Spotify community plugin contributes only a Tool.

The manifest's `[permissions]` block declares which capability types this plugin wants to register — `register_tools`, `register_hooks`, `register_providers`, `register_agents`, `register_skills`, `register_rules`, `register_mcp_server`, `register_user_models` (`crates/wcore-plugin-api/src/manifest.rs:266-277`). Each flag is a request, not a grant. The host decides what to honor.

This means: do not name your plugin after a capability ("my-tool-plugin"). Name it after the vendor or feature boundary ("wayland-spotify", "wayland-honcho"). The capabilities are an implementation detail.

---

## 3. Plugin manifest reference

The manifest is a TOML file named `plugin.toml` at the root of your plugin directory. Every field below has an authoritative definition in `crates/wcore-plugin-api/src/manifest.rs`.

```toml
plugin_api_version = "1.0"                              # see line 191

[plugin]                                                # see line 251
name        = "wayland-spotify"
version     = "0.1.0"
description = "Spotify control surface as a Wayland tool"
entry       = "wayland_spotify::plugin"                 # static: Rust path; wasm: ignored
authors     = ["You <you@example.com>"]
license     = "Apache-2.0"
deferred    = false                                     # if true, initialize() runs on first use

[permissions]                                           # see line 266
register_tools         = true
register_hooks         = false
register_providers     = false
register_agents        = false
register_skills        = false
register_rules         = false
register_mcp_server    = false
register_user_models   = false
tool_namespace         = "spotify"                      # REQUIRED when register_tools = true
memory_partitions_writable = ["P3"]                     # P1..P5; P5 is system-write-only
memory_partitions_readable = ["P1", "P3"]
mcp_servers_visible    = "self_only"                    # "self_only" | "all"; default self_only

# WASM-only host capability flags (ignored by other runtimes).
# Each defaults to false / empty. Manifests stay byte-compatible with v0.6.4.
allow_network          = true                           # see line 296
allow_workspace_read   = false                          # see line 298
allow_workspace_write  = false                          # see line 300
allow_tool_invoke      = false                          # see line 302
permitted_secrets      = ["SPOTIFY_API_KEY"]            # existence-only — see line 306

[capabilities]                                          # see line 311
required = []                                           # host capabilities you cannot run without
optional = []                                           # host capabilities you can use if present

[runtime]                                               # see line 204
kind = "wasm"                                           # "static" | "wasm" | "subprocess" | "mcp-bridge"

[runtime.wasm]                                          # see line 229
component_path  = "./plugin.wasm"
fuel_per_call   = 100_000_000

[runtime.subprocess]                                    # see line 237
binary_path = "./wayland-spotify-bin"
args        = ["--mode=stdio"]

# mcp-bridge: set kind = "mcp-bridge" and use [runtime.subprocess] for the
# binary path — the loader reads subprocess.binary_path for both subprocess
# and mcp-bridge modes. [runtime.mcp_bridge] is reserved for future remote
# (non-stdio) variants (server_url field); not used by the current engine.
```

Validation rules the loader enforces (`crates/wcore-plugin-api/src/manifest.rs:323-377`):

- `register_tools = true` requires `tool_namespace`.
- Memory partitions must be `P1`..`P5`; `P5` is never writable from a plugin.
- `mcp_servers_visible` must be `"self_only"` or `"all"`.
- `[runtime].kind` must be one of `static`, `wasm`, `subprocess`, `mcp-bridge`. An omitted `[runtime]` block is equivalent to `kind = "static"`.

Missing `plugin_api_version` is treated as compatible with the current engine to keep older v0.6.4 manifests loadable (`crates/wcore-plugin-api/src/manifest.rs:389-399`).

---

## 4. Permission model

The manifest declares **requested** permissions. The host decides **effective** permissions. Plugins cannot self-elevate.

Three layers participate:

1. **The manifest** advertises what the plugin wants. Every flag defaults to `false` or empty, so a manifest that forgets a permission gets nothing instead of everything. This is fail-closed by construction.
2. **The host policy** is the engine config plus `PluginAccessGate`. It can refuse any requested permission. Configuration lives in `plugins.toml` and reads back into `PluginsConfig` at engine boot.
3. **The runtime** decides how the granted permissions become live wiring. For WASM this means composition-root selection between `Gated*` and `Deny*` host adapters — omitting a permission yields a `Deny*` adapter at component-link time, so even if a plugin tries to call the host import, the call returns a typed error instead of executing.

A few invariants worth knowing:

- **Default deny for every WASM host capability.** `allow_network`, `allow_workspace_read`, `allow_workspace_write`, `allow_tool_invoke`, and `permitted_secrets` all default to off. Composition is fail-closed: a plugin that forgets to ask gets a `Deny*` adapter (`crates/wcore-plugin-api/src/manifest.rs:283-307`).
- **Subprocess inherits engine privileges.** A subprocess plugin runs with the engine's process rights. This is an explicit trade-off documented at `crates/wcore-plugin-api/src/manifest.rs:69-71` and called out as A7 in the v0.6.5 cross-audit. If you do not need the privilege, do not pick subprocess.
- **Static plugins skip permission gating for host adapters.** The engine binary is the trust anchor; if you can link into it, you already have the engine's privileges by definition.
- **Secrets are existence-only across the boundary.** The WASM host exposes `secret-exists` and never `secret-value`. A plugin can learn that `SPOTIFY_API_KEY` is configured, but the value never crosses the sandbox boundary. The same constraint applies to subprocess plugins by convention.
- **Memory partitions are explicit.** `P5` (user model) is system-write-only — no plugin can list it as writable. Other partitions follow the writable/readable declaration in the manifest.

The mental model: manifest is a request, host is the gatekeeper, runtime is the enforcement layer. None of these substitute for the others.

---

## 5. Signing and distribution

Static plugins skip signing entirely. The engine binary is the trust anchor; impersonation requires recompiling the engine.

Dynamic plugins — WASM, subprocess, and mcp-bridge — must be signed. The trust anchor is the **union** of two sources (Task 1.3 union behavior):

- Files in `~/.wayland/trusted-keys/*.pub`. The default directory is anchored at `dirs::home_dir().join(".wayland").join("trusted-keys")` (`crates/wcore-agent/src/plugins/sig_verifier.rs:55-63`). Override via `WAYLAND_TRUSTED_KEYS_DIR` for tests.
- Entries in `plugins.toml::trusted_plugin_keys`. Loaded into `PluginsConfig` and resolved alongside filesystem keys (`crates/wcore-agent/src/plugins/loader.rs:494-500`).

A signed plugin ships a sidecar file `<plugin_dir>/wayland-plugin.sig` containing a raw 64-byte ed25519 signature over the plugin payload. The filename is the constant `PLUGIN_SIG_FILENAME` (`crates/wcore-agent/src/plugins/sig_verifier.rs:26`).

To produce a signature:

```bash
# 1. Generate a keypair (one-time, per author).
openssl genpkey -algorithm ed25519 -out signing.key
openssl pkey -in signing.key -pubout -out signing.pub

# 2. Drop signing.pub into the trust anchor directory.
mkdir -p ~/.wayland/trusted-keys
cp signing.pub ~/.wayland/trusted-keys/

# 3. Sign the plugin payload (the binary or .wasm).
openssl pkeyutl -sign -inkey signing.key \
    -in plugin.wasm -rawin -out wayland-plugin.sig
```

A development override exists: setting `WAYLAND_PLUGIN_TRUST_UNSIGNED=1` allows the loader to accept unsigned path-based plugins (`crates/wcore-agent/src/plugins/sig_verifier.rs:29` and `crates/wcore-agent/src/plugins/loader.rs:140`). The engine logs a warning every time this happens; never set this in production.

Discovery roots, in priority order (`crates/wcore-agent/src/plugins/loader.rs:614`):

1. `$WAYLAND_PLUGINS_DIR` when set and non-empty. Constant `ENV_PLUGINS_DIR` (`crates/wcore-agent/src/plugins/loader.rs:39`).
2. `<data_dir>/wayland/plugins/` otherwise (`crates/wcore-plugin-api/src/manifest.rs:139-144`).

The loader canonicalizes the manifest path and refuses any plugin whose path is not a prefix of an allowed root (`crates/wcore-plugin-api/src/manifest.rs:107-134`). This blocks the classic "drop a manifest in `~/Downloads`" impersonation vector.

---

## 6. Versioning

`PLUGIN_API_VERSION` is currently `"1.0"` (`crates/wcore-plugin-api/src/lib.rs:67`). Manifests that declare a different value are rejected with `PluginError::VersionMismatch` (`crates/wcore-plugin-api/src/manifest.rs:393-398`).

Patch and minor bumps stay compatible as long as the WIT files do not break. **WIT files are semver.** Major changes to any `wayland:host@x.y.z` package require a manifest `plugin_api_version` bump and a coordinated migration. Treat them like a public API contract.

Manifests that omit `plugin_api_version` are accepted as compatible with the current engine. This is intentional backward-compatibility for v0.6.4 manifests; new plugins should always set the field explicitly.

---

## 7. Crash budget

Every plugin has a 3-strike per-session crash budget. After three consecutive `Err`s from `Plugin::initialize` or any `register_*` invocation, the runner auto-disables the plugin for the remainder of the session.

- Threshold: `CRASH_THRESHOLD = 3` (`crates/wcore-agent/src/plugins/runner.rs:180`).
- The counter is consecutive — a success resets it to zero.
- The runner emits `tracing::warn!("auto-disabled after 3 consecutive failures")` at the moment of the trip (`crates/wcore-agent/src/plugins/runner.rs:226`).
- Reset happens on session start via `PluginRunner::reset_budget` (`crates/wcore-agent/src/plugins/runner.rs:256`). Restarting the engine restores the plugin.

Practical consequence: a plugin that throws on every initialize will be disabled three boots in. Failures inside a single tool call do not yet increment the counter — that lands in v0.6.6+ (see deferrals below). Until then, the budget protects against the pathological case where initialize is broken, not against per-call flakiness.

---

## 8. Debugging tips

Each runtime has its own debugging path. Read this section once per runtime you ship.

**Static.** Static plugins are Rust code in the engine workspace. Verify registration with:

```bash
cargo test -p wcore-agent --test inventory_discovery
```

The test enumerates every `inventory::submit!` entry the binary knows about. If your plugin compiles but does not appear, you forgot the `submit!` line. Use `RUST_LOG=wcore_agent::plugins=debug cargo run -p wcore-cli` to see registration tracing live.

**WASM.** Build with:

```bash
cargo component build --release
```

The output `.wasm` is what you sign and ship. Load it into a dev engine, watch the tracing logs:

```bash
RUST_LOG=wcore_plugin_wasm=debug,wcore_agent::plugins=debug \
  WAYLAND_PLUGIN_TRUST_UNSIGNED=1 \
  cargo run -p wcore-cli -- plugin list
```

If the component fails to link, the failure points at a missing or `Deny*` adapter. Check the corresponding `allow_*` permission in your manifest. The composition root logs which adapters were selected at instantiation time.

**Subprocess.** A subprocess plugin is just a binary that speaks JSON-Lines on stdio. Spawn it manually and pipe a canned RPC at it:

```bash
echo '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}' \
  | ./wayland-spotify-bin --mode=stdio
```

If it does not echo a well-formed response, the engine will not load it either. Debug it like any other CLI.

**mcp-bridge.** All existing MCP debugging tooling applies — `mcp-inspector`, JSON-RPC tracing, server-side stdout. If the underlying MCP server works in any MCP client, it works in Wayland.

---

## 9. What is deferred to v0.6.6+

These are known gaps in v0.6.5. They are explicit deferrals, not bugs.

- **Per-tool spinner.** The UI's per-tool progress indicator does not yet thread `call_id` through plugin tool dispatch.
- **WASM streaming output.** Current tool model is one-shot result. Streaming output lands when the WIT contract grows a streaming export.
- **WASM hot-reload.** Components are instantiated fresh per call but the loader does not yet detect on-disk changes mid-session. Restart the engine to pick up a rebuilt `.wasm`.
- **Public plugin registry / marketplace.** Distribution is currently DIY — direct download, drop into the plugins directory, sign with a trusted key. A registry is on the v0.7 roadmap.
- **Tool-dispatch error increments into crash budget.** Today only `initialize` and `register_*` errors count toward the 3-strike threshold (`crates/wcore-agent/src/plugins/runner.rs:175-180`). Tool-call failures are out of scope until the counter learns to distinguish flakiness from breakage.
- **End-to-end WASM execute with a real component.** The Task 4.4 fixture lands the canonical reference component; until you exercise that example, treat WASM execute as covered by unit tests rather than an end-to-end demo.

---

## 10. Quickstart links

Three paths in three lengths. Pick the runtime from section 1, then run the matching command.

**Static plugin in 30 seconds:**

```bash
cargo generate --git https://github.com/FerroxLabs/wayland-core \
  --template templates/plugin-static --name my-plugin
cd my-plugin
cargo test
```

**WASM plugin in five minutes:**

```bash
rustup target add wasm32-wasip1
cargo install cargo-component
cargo generate --git https://github.com/FerroxLabs/wayland-core \
  --template templates/plugin-wasm --name my-wasm-plugin
cd my-wasm-plugin
cargo component build --release
```

Drop the resulting `.wasm` plus a signed `wayland-plugin.sig` into `~/.wayland/plugins/my-wasm-plugin/`.

**Wrap an MCP server in 60 seconds:**

```bash
mkdir -p ~/.wayland/plugins/mcp-time
cp examples/plugin-subprocess-mcp/plugin.toml ~/.wayland/plugins/mcp-time/
# Edit the [runtime.subprocess] block to point at your MCP server binary.
```

No Rust code required. The engine discovers tools by introspecting the MCP server on the other side of the bridge.

---

Ship the smallest thing that works. Pick the most-restrictive runtime that lets you do your job. Declare only the permissions you need. The substrate punishes over-asking and rewards minimalism.
