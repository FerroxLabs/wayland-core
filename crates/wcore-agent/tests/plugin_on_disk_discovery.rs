//! v0.6.5 Task 2.7b — on-disk plugin discovery + dispatch routing tests.
//!
//! These prove that [`PluginLoader::discover_on_disk`] reaches each of
//! the four `*PluginRunner::load` call sites (Wasm / Subprocess /
//! McpBridge) when handed a fixture manifest, and that malformed /
//! unsupported manifests are skipped cleanly rather than crashing
//! discovery.
//!
//! These tests deliberately use load FAILURES as the signal — the
//! point is to prove dispatch ROUTING fires, not that load succeeds
//! (real loads require live wasmtime / a real binary). The minimum-
//! viable wasm component header is reused from Task 2.6's runner tests.

use std::path::Path;
use std::sync::{Arc, Mutex, OnceLock};

use wcore_agent::plugins::loader::{PluginLoader, RuntimeDispatch};
use wcore_agent::plugins::runner::PluginRunner;
use wcore_config::plugins_config::PluginsConfig;
use wcore_plugin_api::PluginAccessGate;
use wcore_plugin_wasm::WasmPluginRunner;

/// Minimal-viable wasm component bytes — 8 bytes, magic + component-model
/// version. Verified against wasmtime 30.x in Task 2.6.
fn wasm_empty_component_bytes() -> Vec<u8> {
    vec![
        0x00, 0x61, 0x73, 0x6d, // \0asm magic
        0x0d, 0x00, 0x01, 0x00, // component model version
    ]
}

/// Create a plugin dir at `root/<name>/` and write `plugin.toml` + any
/// supplied entry payload. Returns the manifest path.
fn make_plugin_dir(
    root: &Path,
    name: &str,
    manifest_toml: &str,
    entry_filename: Option<&str>,
    entry_bytes: Option<&[u8]>,
) -> std::path::PathBuf {
    let dir = root.join(name);
    std::fs::create_dir_all(&dir).unwrap();
    let manifest = dir.join("plugin.toml");
    std::fs::write(&manifest, manifest_toml).unwrap();
    if let (Some(fname), Some(bytes)) = (entry_filename, entry_bytes) {
        std::fs::write(dir.join(fname), bytes).unwrap();
    }
    manifest
}

/// Process-global mutex to serialize env-mutating tests. `cargo test`
/// runs `#[test]`s on multiple threads within one process; the env vars
/// `WAYLAND_PLUGINS_DIR` / `WAYLAND_PLUGIN_TRUST_UNSIGNED` are global
/// and therefore must be held exclusively.
fn env_mutex() -> &'static Mutex<()> {
    static M: OnceLock<Mutex<()>> = OnceLock::new();
    M.get_or_init(|| Mutex::new(()))
}

/// Pin `$WAYLAND_PLUGINS_DIR` for the duration of a test. Helper
/// because env mutation is unsafe and we want to remove it on drop.
/// Holds the global env mutex for the lifetime of the guard.
struct EnvGuard {
    _lock: std::sync::MutexGuard<'static, ()>,
}
impl EnvGuard {
    fn set(dir: &Path) -> Self {
        let lock = env_mutex().lock().unwrap_or_else(|p| p.into_inner());
        unsafe {
            std::env::set_var("WAYLAND_PLUGINS_DIR", dir);
            // Disable signing so the focus stays on dispatch routing,
            // not key management (Task 1.3 has its own dedicated tests).
            std::env::set_var("WAYLAND_PLUGIN_TRUST_UNSIGNED", "1");
        }
        Self { _lock: lock }
    }
}
impl Drop for EnvGuard {
    fn drop(&mut self) {
        unsafe {
            std::env::remove_var("WAYLAND_PLUGINS_DIR");
            std::env::remove_var("WAYLAND_PLUGIN_TRUST_UNSIGNED");
        }
    }
}

fn config() -> PluginsConfig {
    PluginsConfig {
        plugin_signature_verification: false,
        ..Default::default()
    }
}

#[tokio::test]
async fn on_disk_wasm_plugin_discovered_and_dispatched() {
    let tmp = tempfile::TempDir::new().unwrap();
    let _guard = EnvGuard::set(tmp.path());

    let manifest_toml = r#"
[plugin]
name = "wasm-fixture"
version = "0.0.0"
description = "fixture"
entry = "wasm-fixture"
license = "Apache-2.0"

[permissions]

[runtime]
kind = "wasm"
"#;
    let _manifest = make_plugin_dir(
        tmp.path(),
        "wasm-fixture",
        manifest_toml,
        Some("plugin.wasm"),
        Some(&wasm_empty_component_bytes()),
    );

    let config = config();
    let mut loader = PluginLoader::discover(&config);
    let runner = PluginRunner::new();
    let wasm = WasmPluginRunner::new().expect("wasm runner builds");
    let gate = Arc::new(PluginAccessGate);

    loader.discover_on_disk(&runner, Some(&wasm), gate).await;

    let recs = loader.on_disk_dispatches();
    assert_eq!(recs.len(), 1, "expected one on-disk dispatch, got {recs:?}");
    let r = &recs[0];
    assert_eq!(r.plugin_name, "wasm-fixture");
    assert_eq!(r.dispatch, RuntimeDispatch::Wasm);
    // Load with the 8-byte minimal component MUST succeed — wasm runner
    // accepts it; this proves dispatch reached `WasmPluginRunner::load`.
    assert!(
        r.load_result.is_ok(),
        "wasm dispatch failed: {:?}",
        r.load_result
    );
    // Crash budget untouched on successful load.
    assert!(!runner.is_disabled("wasm-fixture"));
}

#[tokio::test]
async fn on_disk_subprocess_plugin_discovered_and_dispatched() {
    let tmp = tempfile::TempDir::new().unwrap();
    let _guard = EnvGuard::set(tmp.path());

    // Point at a non-existent binary — load WILL fail at spawn, but the
    // test only proves dispatch reached `SubprocessPluginRunner::load`.
    let manifest_toml = r#"
[plugin]
name = "subprocess-fixture"
version = "0.0.0"
description = "fixture"
entry = "subprocess-fixture"
license = "Apache-2.0"

[permissions]

[runtime]
kind = "subprocess"
[runtime.subprocess]
binary_path = "does-not-exist"
"#;
    let _manifest = make_plugin_dir(tmp.path(), "subprocess-fixture", manifest_toml, None, None);

    let config = config();
    let mut loader = PluginLoader::discover(&config);
    let runner = PluginRunner::new();
    let gate = Arc::new(PluginAccessGate);

    loader.discover_on_disk(&runner, None, gate).await;

    let recs = loader.on_disk_dispatches();
    assert_eq!(recs.len(), 1);
    let r = &recs[0];
    assert_eq!(r.plugin_name, "subprocess-fixture");
    assert_eq!(r.dispatch, RuntimeDispatch::Subprocess);
    // Load fails (binary missing); error message MUST come from the
    // subprocess runner to prove the dispatch routed there.
    let err = r.load_result.as_ref().unwrap_err();
    assert!(
        err.contains("SubprocessPluginRunner::load"),
        "expected SubprocessPluginRunner::load error, got: {err}"
    );
    // Crash budget incremented (1 of 3) — plugin NOT auto-disabled yet.
    assert!(!runner.is_disabled("subprocess-fixture"));
}

#[tokio::test]
async fn on_disk_mcp_bridge_plugin_discovered_and_dispatched() {
    let tmp = tempfile::TempDir::new().unwrap();
    let _guard = EnvGuard::set(tmp.path());

    let manifest_toml = r#"
[plugin]
name = "mcp-bridge-fixture"
version = "0.0.0"
description = "fixture"
entry = "mcp-bridge-fixture"
license = "Apache-2.0"

[permissions]

[runtime]
kind = "mcp-bridge"
[runtime.subprocess]
binary_path = "does-not-exist"
"#;
    let _manifest = make_plugin_dir(tmp.path(), "mcp-bridge-fixture", manifest_toml, None, None);

    let config = config();
    let mut loader = PluginLoader::discover(&config);
    let runner = PluginRunner::new();
    let gate = Arc::new(PluginAccessGate);

    loader.discover_on_disk(&runner, None, gate).await;

    let recs = loader.on_disk_dispatches();
    assert_eq!(recs.len(), 1);
    let r = &recs[0];
    assert_eq!(r.plugin_name, "mcp-bridge-fixture");
    assert_eq!(r.dispatch, RuntimeDispatch::McpBridge);
    let err = r.load_result.as_ref().unwrap_err();
    assert!(
        err.contains("McpBridgePluginRunner::load"),
        "expected McpBridgePluginRunner::load error, got: {err}"
    );
}

#[tokio::test]
async fn invalid_runtime_kind_skipped_with_log() {
    let tmp = tempfile::TempDir::new().unwrap();
    let _guard = EnvGuard::set(tmp.path());

    let manifest_toml = r#"
[plugin]
name = "weird"
version = "0.0.0"
description = "fixture"
entry = "weird"
license = "Apache-2.0"

[permissions]

[runtime]
kind = "garbage"
"#;
    let _manifest = make_plugin_dir(tmp.path(), "weird", manifest_toml, None, None);

    let config = config();
    let mut loader = PluginLoader::discover(&config);
    let runner = PluginRunner::new();
    let gate = Arc::new(PluginAccessGate);

    // MUST NOT panic — unknown runtime kind is a parse failure that's
    // captured into the dispatch record, not propagated.
    loader.discover_on_disk(&runner, None, gate).await;

    let recs = loader.on_disk_dispatches();
    assert_eq!(recs.len(), 1, "expected one record (skipped via record)");
    let r = &recs[0];
    assert!(
        r.load_result.is_err(),
        "garbage runtime kind must produce error record"
    );
    let err = r.load_result.as_ref().unwrap_err();
    assert!(
        err.contains("parse manifest") || err.contains("garbage"),
        "expected parse-error message, got: {err}"
    );
}

#[tokio::test]
async fn mixed_static_and_on_disk_coexist() {
    // Static-plugin path through inventory + on-disk path via dir scan.
    // The static side is exercised by every other test in this crate
    // (and would explode if the merge logic were broken); here we
    // confirm that constructing a loader against an EMPTY plugins dir
    // still yields a `discovered()` list from inventory AND that
    // `discover_on_disk` against a populated dir adds dispatches without
    // disturbing the static set.
    let tmp = tempfile::TempDir::new().unwrap();
    let _guard = EnvGuard::set(tmp.path());

    // Drop one on-disk plugin in the dir.
    let manifest_toml = r#"
[plugin]
name = "on-disk-only"
version = "0.0.0"
description = "fixture"
entry = "on-disk-only"
license = "Apache-2.0"

[permissions]

[runtime]
kind = "subprocess"
[runtime.subprocess]
binary_path = "does-not-exist"
"#;
    let _ = make_plugin_dir(tmp.path(), "on-disk-only", manifest_toml, None, None);

    let config = config();
    let mut loader = PluginLoader::discover(&config);
    let static_count_before = loader.discovered().len();

    let runner = PluginRunner::new();
    let gate = Arc::new(PluginAccessGate);
    loader.discover_on_disk(&runner, None, gate).await;

    // Static plugins via inventory: unchanged by on-disk pass.
    assert_eq!(loader.discovered().len(), static_count_before);
    // On-disk dispatch: ran for one plugin.
    let on_disk = loader.on_disk_dispatches();
    assert_eq!(on_disk.len(), 1);
    assert_eq!(on_disk[0].plugin_name, "on-disk-only");
    assert_eq!(on_disk[0].dispatch, RuntimeDispatch::Subprocess);
}

// ---------------------------------------------------------------------------
// v0.6.5 Task 6B.4 — path-traversal + symlink defense.
//
// Two attack surfaces in on-disk discovery:
//   1. A malicious `binary_path = "../../bin/curl"` could point signature
//      verification + loader handoff at an arbitrary file outside the
//      plugin dir. We reject any binary_path that is absolute or contains
//      a `..` component, and canonicalize+starts_with check when the
//      file exists (symlink escape).
//   2. A symlink in `~/.wayland/plugins/<name>` pointing at an
//      attacker-controlled directory bypasses the allowed-roots check.
//      We skip symlink entries in the plugins root with a warn log.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn traversal_relative_binary_path_rejected() {
    let tmp = tempfile::TempDir::new().unwrap();
    let _guard = EnvGuard::set(tmp.path());

    let manifest_toml = r#"
[plugin]
name = "evil-traversal"
version = "0.0.0"
description = "fixture"
entry = "evil-traversal"
license = "Apache-2.0"

[permissions]

[runtime]
kind = "subprocess"
[runtime.subprocess]
binary_path = "../../../etc/passwd"
"#;
    let _ = make_plugin_dir(tmp.path(), "evil-traversal", manifest_toml, None, None);

    let config = config();
    let mut loader = PluginLoader::discover(&config);
    let runner = PluginRunner::new();
    let gate = Arc::new(PluginAccessGate);
    loader.discover_on_disk(&runner, None, gate).await;

    let recs = loader.on_disk_dispatches();
    assert_eq!(recs.len(), 1);
    let r = &recs[0];
    let err = r
        .load_result
        .as_ref()
        .expect_err("traversal must be rejected");
    assert!(
        err.contains("entry path rejected") && err.contains("parent-dir"),
        "expected entry-path-rejected error, got: {err}"
    );
}

#[tokio::test]
async fn absolute_binary_path_rejected() {
    let tmp = tempfile::TempDir::new().unwrap();
    let _guard = EnvGuard::set(tmp.path());

    // Platform-specific absolute path. `/etc/passwd` is absolute on
    // Unix but NOT on Windows (Windows requires `C:\...` or `\\...`),
    // so on Windows `Path::is_absolute()` returns false for `/etc/...`
    // and the loader falls through to a different error class
    // (CI run 26402707210 job 77718648753 — caught
    // "subprocess spawn failed: The system cannot find the path
    // specified" instead of the expected "entry path rejected:
    // absolute"). Use a path that is absolute on the actual host so
    // we exercise the SAME gate logic on both platforms.
    #[cfg(unix)]
    let abs_path_literal = "/etc/passwd";
    #[cfg(windows)]
    let abs_path_literal = "C:\\\\Windows\\\\System32\\\\cmd.exe";

    let manifest_toml = format!(
        r#"
[plugin]
name = "evil-absolute"
version = "0.0.0"
description = "fixture"
entry = "evil-absolute"
license = "Apache-2.0"

[permissions]

[runtime]
kind = "subprocess"
[runtime.subprocess]
binary_path = "{abs_path_literal}"
"#
    );
    let _ = make_plugin_dir(tmp.path(), "evil-absolute", &manifest_toml, None, None);

    let config = config();
    let mut loader = PluginLoader::discover(&config);
    let runner = PluginRunner::new();
    let gate = Arc::new(PluginAccessGate);
    loader.discover_on_disk(&runner, None, gate).await;

    let recs = loader.on_disk_dispatches();
    assert_eq!(recs.len(), 1);
    let r = &recs[0];
    let err = r
        .load_result
        .as_ref()
        .expect_err("absolute path must be rejected");
    assert!(
        err.contains("entry path rejected") && err.contains("absolute"),
        "expected absolute-path rejection, got: {err}"
    );
}

#[tokio::test]
async fn legit_relative_binary_path_accepted() {
    // A clean relative `binary_path` that stays inside the plugin dir
    // MUST pass the traversal check. Load itself still fails (binary
    // doesn't exist / isn't a real subprocess plugin), but the failure
    // must come from SubprocessPluginRunner::load — NOT from the
    // entry-path gate.
    let tmp = tempfile::TempDir::new().unwrap();
    let _guard = EnvGuard::set(tmp.path());

    let manifest_toml = r#"
[plugin]
name = "legit"
version = "0.0.0"
description = "fixture"
entry = "legit"
license = "Apache-2.0"

[permissions]

[runtime]
kind = "subprocess"
[runtime.subprocess]
binary_path = "bin/plugin"
"#;
    let _ = make_plugin_dir(tmp.path(), "legit", manifest_toml, None, None);

    let config = config();
    let mut loader = PluginLoader::discover(&config);
    let runner = PluginRunner::new();
    let gate = Arc::new(PluginAccessGate);
    loader.discover_on_disk(&runner, None, gate).await;

    let recs = loader.on_disk_dispatches();
    assert_eq!(recs.len(), 1);
    let r = &recs[0];
    let err = r
        .load_result
        .as_ref()
        .expect_err("subprocess binary does not exist — load fails");
    assert!(
        !err.contains("entry path rejected"),
        "legit relative path must not be rejected by entry-path gate; got: {err}"
    );
    assert!(
        err.contains("SubprocessPluginRunner::load"),
        "expected the failure to come from the subprocess runner, got: {err}"
    );
}

// ---------------------------------------------------------------------------
// Path B step 1 — declarative on-disk plugin kind.
//
// A declarative plugin ships a `plugin.toml` with `runtime.kind =
// "declarative"`, `[[hooks]]`, and an optional `[mcp_server]` — NO executable.
// Discovery must route it to `RuntimeDispatch::Declarative`, succeed (no binary
// to sign/spawn), and carry the parsed hooks + mcp_server spec out on the
// handle.
// ---------------------------------------------------------------------------

/// T10 — a declarative manifest dispatches to `Declarative`, loads Ok, and the
/// handle carries the declared hooks + mcp_server spec.
#[tokio::test]
async fn on_disk_declarative_plugin_discovered_and_dispatched() {
    use wcore_agent::plugins::LoadedRuntimeHandle;
    use wcore_plugin_api::registry::hooks::HookPhase;

    let tmp = tempfile::TempDir::new().unwrap();
    let _guard = EnvGuard::set(tmp.path());

    let manifest_toml = r#"
[plugin]
name = "declarative-plug"
version = "0.0.0"
description = "fixture"
license = "Apache-2.0"

[permissions]
register_hooks = true
register_mcp_server = true

[runtime]
kind = "declarative"

[[hooks]]
phase = "session_start"
tool = "memory_prelude"

[mcp_server]
name = "declarative-plug-server"

[mcp_server.transport]
kind = "sse"
url = "https://example.invalid/sse"
"#;
    let _ = make_plugin_dir(tmp.path(), "declarative-plug", manifest_toml, None, None);

    let config = config();
    let mut loader = PluginLoader::discover(&config);
    let runner = PluginRunner::new();
    let gate = Arc::new(PluginAccessGate);

    loader.discover_on_disk(&runner, None, gate).await;

    let recs = loader.on_disk_dispatches();
    assert_eq!(recs.len(), 1, "expected one dispatch, got {recs:?}");
    let r = &recs[0];
    assert_eq!(r.plugin_name, "declarative-plug");
    assert_eq!(r.dispatch, RuntimeDispatch::Declarative);
    assert!(
        r.load_result.is_ok(),
        "declarative load must succeed (no binary): {:?}",
        r.load_result
    );
    match &r.handle {
        LoadedRuntimeHandle::Declarative { hooks, mcp_server } => {
            assert_eq!(hooks.len(), 1);
            assert_eq!(hooks[0].plugin, "declarative-plug");
            assert_eq!(hooks[0].phase, HookPhase::SessionStart);
            assert_eq!(hooks[0].name, "memory_prelude");
            let spec = mcp_server.as_ref().expect("mcp_server present on handle");
            assert_eq!(spec.name, "declarative-plug-server");
        }
        other => panic!("expected Declarative handle, got {other:?}"),
    }
    assert!(!runner.is_disabled("declarative-plug"));
}

/// T11 — a declarative plugin declaring `[[hooks]]` without `register_hooks`
/// fails to load (manifest validation rejects it before dispatch).
#[tokio::test]
async fn on_disk_declarative_hooks_without_permission_rejected() {
    let tmp = tempfile::TempDir::new().unwrap();
    let _guard = EnvGuard::set(tmp.path());

    let manifest_toml = r#"
[plugin]
name = "declarative-nogrant"
version = "0.0.0"
description = "fixture"
license = "Apache-2.0"

[runtime]
kind = "declarative"

[[hooks]]
phase = "session_start"
tool = "memory_prelude"
"#;
    let _ = make_plugin_dir(tmp.path(), "declarative-nogrant", manifest_toml, None, None);

    let config = config();
    let mut loader = PluginLoader::discover(&config);
    let runner = PluginRunner::new();
    let gate = Arc::new(PluginAccessGate);

    loader.discover_on_disk(&runner, None, gate).await;

    let recs = loader.on_disk_dispatches();
    assert_eq!(recs.len(), 1);
    let r = &recs[0];
    let err = r
        .load_result
        .as_ref()
        .expect_err("hooks-without-register_hooks must be rejected");
    assert!(
        err.contains("parse manifest") || err.contains("register_hooks"),
        "expected manifest-validation rejection, got: {err}"
    );
}

#[cfg(unix)]
#[tokio::test]
async fn symlink_entry_in_plugins_root_skipped() {
    use std::os::unix::fs::symlink;

    let tmp = tempfile::TempDir::new().unwrap();
    let _guard = EnvGuard::set(tmp.path());

    // Real plugin dir lives OUTSIDE the plugins root.
    let outside = tempfile::TempDir::new().unwrap();
    let real_dir = outside.path().join("attacker-controlled");
    std::fs::create_dir_all(&real_dir).unwrap();
    let manifest_toml = r#"
[plugin]
name = "attacker"
version = "0.0.0"
description = "fixture"
entry = "attacker"
license = "Apache-2.0"

[permissions]

[runtime]
kind = "subprocess"
[runtime.subprocess]
binary_path = "does-not-exist"
"#;
    std::fs::write(real_dir.join("plugin.toml"), manifest_toml).unwrap();

    // Symlink under the plugins root pointing at the attacker dir.
    let link = tmp.path().join("attacker");
    symlink(&real_dir, &link).unwrap();

    let config = config();
    let mut loader = PluginLoader::discover(&config);
    let runner = PluginRunner::new();
    let gate = Arc::new(PluginAccessGate);
    loader.discover_on_disk(&runner, None, gate).await;

    let recs = loader.on_disk_dispatches();
    assert!(
        recs.is_empty(),
        "symlink in plugins root must be skipped, got dispatches: {recs:?}"
    );
}
