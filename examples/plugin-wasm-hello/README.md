# plugin-wasm-hello — canonical WASM plugin example

Minimum-viable Wayland WASM plugin: implements one tool that returns `"Hello, {input}!"`.

## v0.6.5 status

| Surface | Status |
|---|---|
| WIT contract (`wit/tool.wit` + `wit/wayland-host.wit`) | ✅ stable |
| Plugin source (`src/lib.rs`) | ✅ ready to compile to `wasm32-wasip1` |
| Manifest (`plugin.toml`) | ✅ valid against `PluginLoader::discover_on_disk` |
| Host-side execute round-trip | ⏳ **deferred to v0.6.6** |

The runtime composition (Engine config, ResourceLimiter, Linker, Component instantiation, fresh-instance-per-call, capability-gated host adapters) is fully wired and unit-tested at `crates/wcore-plugin-wasm/`. The remaining piece — implementing the bindgen-generated `wayland:host/host` `Host` trait on `HostState` so that `WaylandTool::add_to_linker` is satisfied — is ~150 LOC of method delegation from `HostState`'s existing `Gated*`/`Deny*` adapters to the wit-bindgen trait signatures. It lands in **v0.6.6**.

Plugin authors can still:
- Author plugins against this WIT contract today
- Test their plugin's WIT export shape with `wit-parser`
- Load their `.wasm` into the engine and observe discovery + signing
- Validate manifest schema against `PluginManifest::from_toml_str`

The v0.6.6 release lights up actual execution end-to-end.

## Build

```bash
rustup target add wasm32-wasip1
cargo install cargo-component
cargo component build --release
# produces target/wasm32-wasip1/release/plugin_wasm_hello.wasm
```

## Install

```bash
mkdir -p ~/.wayland/plugins/plugin-wasm-hello
cp plugin.toml ~/.wayland/plugins/plugin-wasm-hello/
cp target/wasm32-wasip1/release/plugin_wasm_hello.wasm ~/.wayland/plugins/plugin-wasm-hello/
```

## Sign (production)

Engines configured with `plugin_signature_verification = true` reject unsigned plugins. Generate an ed25519 signature over the `.wasm` bytes and save as `wayland-plugin.sig` in the plugin directory. Place the matching `.pub` key in `~/.wayland/trusted-keys/` OR in the engine's `plugins.toml::trusted_plugin_keys` array. See `docs/plugin-authors.md` for the full signing flow.
