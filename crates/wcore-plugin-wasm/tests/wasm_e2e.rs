//! v0.6.5 Task 2.7 — WASM plugin host end-to-end smoke.
//!
//! ## E2E approach
//!
//! Full component-model round-trip (host instantiates a real `.wasm`
//! component, invokes the `tool.execute` export, and gets a response
//! back through the host imports) requires either:
//!   1. A `cargo-component` build step in CI (Task 4.4's plan).
//!   2. A hand-written WAT component that imports `wayland:host/host` —
//!      WAT component-model syntax is large and brittle to author inline.
//!
//! For Task 2.7 we ship the **PLACEHOLDER** path per the task brief: a
//! smoke test that exercises everything UP TO real component dispatch,
//! deferring the round-trip to Task 4.4's `.wasm` fixture. This proves:
//!   - `WasmPluginRunner::new()` builds an engine.
//!   - `WasmPluginRunner::load_from_bytes` rejects malformed bytes with
//!     the documented `LoadFailed` error (not a panic, not a stub).
//!   - `LoadedWasmPlugin::call_tool` is callable on the placeholder path
//!     and returns the documented `ExecuteFailed("not yet wired (Task 2.7…)")`
//!     surface — the dispatch seam compiles end-to-end.
//!
//! ## TODO Task 4.4
//!
//! Replace this file with a real component fixture: a 5-LOC Rust
//! tool plugin compiled to `target/wasm32-wasi/release/echo.wasm`
//! via `cargo-component`, loaded here, and round-tripped through
//! the host imports. The fixture lives under
//! `crates/wcore-plugin-wasm/tests/fixtures/echo-plugin/` and is built
//! by the `wasm-fixtures` CI job.

use std::sync::Arc;

use wcore_plugin_api::access_gate::PluginAccessGate;
use wcore_plugin_api::manifest::PluginManifest;
use wcore_plugin_wasm::{WasmPluginError, WasmPluginRunner};

fn fixture_manifest() -> PluginManifest {
    // TOML-parse mirrors the production load path; same fixture style as
    // `wcore-plugin-wasm::runner::tests::manifest_with`.
    let toml_str = r#"
[plugin]
name = "echo-test"
version = "0.0.0"
description = "task 2.7 e2e fixture"
entry = "echo-test"
license = "Apache-2.0"

[permissions]
"#;
    PluginManifest::from_toml_str(toml_str).expect("fixture toml parses")
}

#[tokio::test(flavor = "current_thread")]
async fn wasm_runner_construction_and_load_failure_paths_smoke() {
    let runner = WasmPluginRunner::new().expect("engine + epoch ticker boot");

    // 1. Bad-bytes path must surface `LoadFailed`, not panic.
    let manifest = fixture_manifest();
    let gate = Arc::new(PluginAccessGate);
    let err = runner
        .load_from_bytes(b"not-a-component", &manifest, gate.clone())
        .expect_err("malformed bytes must fail");
    assert!(
        matches!(err, WasmPluginError::LoadFailed(_)),
        "expected LoadFailed, got {err:?}"
    );
}

// Wave 6B.1: the legacy "returns documented placeholder until Task 4.4"
// test was removed when the dispatch stub was replaced by the real
// linker + instantiate + execute path. See `tests/wasm_real_execute.rs`
// for the replacement coverage.
