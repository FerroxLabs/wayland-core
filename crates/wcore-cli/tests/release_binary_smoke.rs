//! V1 / RELEASE BINARY SMOKE — proves the shipped artifact behaves.
//!
//! The companion test `plugin_discovery_e2e.rs` already exercises the
//! plugin-inventory wiring against the **debug** binary that Cargo
//! exposes through `env!("CARGO_BIN_EXE_wayland-core")`. That is
//! necessary but not sufficient: the workspace builds `[profile.release]`
//! with `lto = "thin"` + `codegen-units = 1` (see root Cargo.toml). Both
//! settings rewire dead-code elimination — historically the exact knobs
//! that strip `inventory::submit!` items whose hosting crates are never
//! named. v0.2.0 BLOCKER #1 was that regression in disguise.
//!
//! This smoke test builds the **release** binary, runs `--help` /
//! `--version` to assert process plumbing survives optimization, then
//! drives the same `--json-stream` Ready handshake the debug-binary
//! test does, asserting on the exact capability flags that signal
//! plugin discovery survived linking + LTO:
//!
//! - `capabilities.browser_suite` (wayland-browser linked)
//! - `capabilities.computer_use` (wayland-cua linked — the flag derives from
//!   plugin presence via `PluginCapabilitySet::from_verified`, NOT from the
//!   runtime `HostCuaRegistrar.computer_use_advertised`; that inner registrar
//!   gate is covered by `wcore-agent/tests/capability_advertising_test.rs`.)
//! - `capabilities.plugins` == `true` (umbrella flag, has_plugins).
//!
//! Since `85b60a2f` those first two flags are gated on **backend liveness**,
//! not linkage alone, so they are asserted as a two-run polarity differential
//! (live probe inputs -> advertised, dead probe inputs -> withdrawn) rather
//! than unconditionally `true`. Restoring an unconditional `== true` would
//! re-introduce the false advertisement that narrowing fixed. Full reasoning
//! in `plugin_discovery_e2e.rs` and `.planning/CI-TRIAGE.md`.
//!
//! Any future regression that drops a `use wayland_<plugin> as _;` from
//! `wcore-cli/src/main.rs`, or any release-profile change that re-enables
//! `inventory` dead-code-strip, fails this test before the artifact ships.

use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use tempfile::TempDir;

#[path = "support/mod.rs"]
mod support;
use support::owned_tree::OwnedTree;

/// Walk up from `CARGO_MANIFEST_DIR` (= `<workspace>/crates/wcore-cli`)
/// to the workspace root so we can locate `target/release/<bin>`.
fn workspace_root() -> PathBuf {
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    manifest_dir
        .parent()
        .and_then(|p| p.parent())
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            panic!(
                "CARGO_MANIFEST_DIR ({}) has fewer than two ancestors; cannot locate workspace root",
                manifest_dir.display()
            )
        })
}

fn release_binary_path() -> PathBuf {
    let root = workspace_root();
    let bin_name = if cfg!(windows) {
        "wayland-core.exe"
    } else {
        "wayland-core"
    };
    root.join("target").join("release").join(bin_name)
}

/// Fail-fast variant: if the artifact is missing, panic with a message that
/// points at the CI pre-build step. Replaces the previous in-test
/// `cargo build --release` fallback, which under parallel nextest workers
/// caused cargo file-lock contention against `target/.rustc_info.json` and
/// the 60s-timeout flake closed by M2.7.
///
/// Contract:
/// - On CI: `vx cargo build --release -p wcore-cli` is invoked as a dedicated
///   pre-test step (see ci.yml "Pre-build wcore-cli release binary" step).
///   By the time this test runs, the binary already exists; the function
///   returns immediately.
/// - Locally: developers run `vx cargo build --release -p wcore-cli` once
///   themselves (or `just build-release`). Subsequent `cargo nextest run`
///   invocations are fast because the cache is warm.
fn ensure_release_binary_or_fail() -> PathBuf {
    let bin = release_binary_path();
    if bin.exists() {
        return bin;
    }
    panic!(
        "WCORE_PREBUILD_REQUIRED: release binary not found at {}\n\
         \n\
         The release_binary_smoke test depends on a pre-built artifact.\n\
         CI pre-builds it via the \"Pre-build wcore-cli release binary\"\n\
         step in .github/workflows/ci.yml. Locally, run:\n\
         \n\
             vx cargo build --release -p wcore-cli\n\
         \n\
         BEFORE running this test. The previous in-test cargo invocation\n\
         caused file-lock contention against parallel nextest workers and\n\
         was removed by M2.7.\n",
        bin.display()
    );
}

/// Skip-aware variant for the smoke tests themselves: on a dev checkout the
/// release binary is usually absent (`cargo test --workspace` never builds
/// it), and hard-failing there makes the whole workspace suite permanently
/// red (#190). Returns `None` to skip with a notice when the artifact is
/// missing — UNLESS `WCORE_SMOKE_REQUIRE_PREBUILT` is set, in which case a
/// missing binary is still the hard `WCORE_PREBUILD_REQUIRED` panic. CI sets
/// that variable on its "Release binary smoke" steps (after the dedicated
/// pre-build step), so the gate cannot silently degrade into a skip there.
fn release_binary_or_skip() -> Option<PathBuf> {
    let bin = release_binary_path();
    if bin.exists() {
        return Some(bin);
    }
    if std::env::var("WCORE_SMOKE_REQUIRE_PREBUILT").is_ok() {
        ensure_release_binary_or_fail();
    }
    eprintln!(
        "[release_binary_smoke] release binary not found at {} — skipping.\n\
         Build it first (`vx cargo build --release -p wcore-cli`), or set\n\
         WCORE_SMOKE_REQUIRE_PREBUILT=1 to make a missing artifact a failure.",
        bin.display()
    );
    None
}

/// Run the release binary with the given args; return (status, stdout, stderr).
fn run_with(bin: &PathBuf, args: &[&str]) -> (std::process::ExitStatus, String, String) {
    let output = Command::new(bin)
        .args(args)
        .output()
        .unwrap_or_else(|e| panic!("spawn {} {:?} failed: {e}", bin.display(), args));
    (
        output.status,
        String::from_utf8_lossy(&output.stdout).into_owned(),
        String::from_utf8_lossy(&output.stderr).into_owned(),
    )
}

#[test]
fn release_binary_help_and_version_succeed() {
    let Some(bin) = release_binary_or_skip() else {
        return;
    };

    let (help_status, help_stdout, help_stderr) = run_with(&bin, &["--help"]);
    assert!(
        help_status.success(),
        "--help must exit 0; got {help_status}; stderr: {help_stderr}"
    );
    assert!(
        !help_stdout.trim().is_empty(),
        "--help stdout must be non-empty; got empty (stderr: {help_stderr})"
    );

    let (ver_status, ver_stdout, ver_stderr) = run_with(&bin, &["--version"]);
    assert!(
        ver_status.success(),
        "--version must exit 0; got {ver_status}; stderr: {ver_stderr}"
    );
    assert!(
        !ver_stdout.trim().is_empty(),
        "--version stdout must be non-empty; got empty (stderr: {ver_stderr})"
    );

    // Both should agree on success behavior.
    assert_eq!(
        help_status.code(),
        ver_status.code(),
        "--help and --version exit codes must match: help={help_status} version={ver_status}"
    );
}

/// Which answer the backend-liveness probes should give for a run. See the
/// module docs and `plugin_discovery_e2e.rs` for why the capability flags are
/// asserted as a two-run polarity differential rather than `== true`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Backends {
    /// Every probe input names something that could start.
    Live,
    /// Every probe input is provably dead.
    Dead,
}

/// Plant the environment facts the probes read. The expected flag values come
/// from what is planted here, never from invoking the probe under test.
fn apply_backend_env(cmd: &mut Command, backends: Backends) {
    // A credentialed Browserbase build probes Indeterminate and keeps the flag
    // regardless of any local backend; clear it in BOTH legs so the Dead leg
    // cannot be silently unfalsifiable.
    cmd.env_remove("BROWSERBASE_API_KEY");
    cmd.env_remove("BROWSERBASE_PROJECT_ID");

    // The browser probe mirrors `ensure_ready`'s TWO real startup paths: a
    // resolvable sidecar program, OR an externally managed sidecar already
    // answering `/health` at the configured base URL. Only the first was ever
    // planted here; the second fell through to the shipped default
    // (`http://localhost:9377`), an ambient fact this test does not own. On a
    // host actually running a Camoufox sidecar there — a supported deployment,
    // and the standing state of the Linux build box — the Dead leg was not
    // dead, the probe correctly answered `Ready`, and the leg failed against a
    // product that was telling the truth.
    //
    // Pinned to a reserved loopback port in BOTH legs, so the only inputs that
    // move between them are the sidecar binary and the display. That makes the
    // Live leg a statement about the binary path specifically: if binary
    // resolution rots, Live goes red instead of being propped up by whatever
    // happens to be listening on 9377.
    cmd.env("WAYLAND_CAMOUFOX_URL", "http://127.0.0.1:1");

    match backends {
        Backends::Live => {
            // Resolved with `which`, which accepts an absolute path as given and
            // never executes it — the test binary is a valid sidecar stand-in.
            cmd.env(
                "WAYLAND_CAMOUFOX_BIN",
                std::env::current_exe().expect("resolve test binary path"),
            );
            cmd.env("DISPLAY", ":0");
        }
        Backends::Dead => {
            cmd.env(
                "WAYLAND_CAMOUFOX_BIN",
                "wayland-core-smoke-no-such-browser-binary",
            );
            cmd.env_remove("DISPLAY");
            cmd.env_remove("WAYLAND_DISPLAY");
        }
    }
}

/// Read a capability flag the way a host reads it. These fields are
/// `#[serde(skip_serializing_if = "is_false")]`, so a withdrawn capability is
/// omitted from the wire rather than sent as `false`; absent and `false` are
/// the same claim.
fn advertises(caps: &serde_json::Value, key: &str) -> bool {
    match &caps[key] {
        serde_json::Value::Null => false,
        v => v
            .as_bool()
            .unwrap_or_else(|| panic!("capabilities.{key} was not a bool: {v}")),
    }
}

/// Drive `--json-stream` against the release binary, capture the first
/// stdout line (the Ready event), and assert the plugin-capability flags
/// the v0.2.0 release-time dead-code-strip regression hid.
///
/// Mirrors `plugin_discovery_e2e.rs::first_ready_event` but targets the
/// release artifact at `target/release/wayland-core` instead of the
/// debug binary Cargo wires through `CARGO_BIN_EXE_wayland-core`.
fn first_ready_event_release(bin: &PathBuf, backends: Backends) -> serde_json::Value {
    // Clean cwd + HOME so no `.wayland-core.toml` from the dev environment
    // perturbs config resolution.
    let tmp = TempDir::new().expect("create tmp workspace");

    let mut cmd = Command::new(bin);
    cmd.args([
        "--json-stream",
        "--provider",
        "anthropic",
        "--api-key",
        "test-key-not-used-because-we-stop-before-message",
    ])
    .current_dir(tmp.path())
    // `HOME` alone does NOT isolate on Windows: `dirs::home_dir()` reads
    // `USERPROFILE` there, so the child loads the developer's real
    // `%APPDATA%\wayland-core` config. Measured on `SeanD@seandesktop`
    // 2026-07-29: with `HOME` only, this exact invocation produced 0 bytes of
    // stdout and exited before `ready`, because the host config carried
    // `storage.credentials.backend = "plaintext"` and durable session recovery
    // refuses to start on it. With `WAYLAND_HOME` — the crate's canonical
    // hermetic override, see `wcore_config::config::wayland_config_dir` — the
    // same invocation emitted `ready` in under a second.
    .env("HOME", tmp.path())
    .env("WAYLAND_HOME", tmp.path())
    .stdin(Stdio::piped())
    .stdout(Stdio::piped())
    .stderr(Stdio::piped());
    apply_backend_env(&mut cmd, backends);
    let mut child = OwnedTree::new(
        cmd.spawn()
            .expect("spawn release wayland-core --json-stream"),
    );

    let mut stdout = child.stdout.take().expect("capture stdout");
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        use std::io::{BufRead, BufReader};
        let mut reader = BufReader::new(&mut stdout);
        let mut line = String::new();
        let result = match reader.read_line(&mut line) {
            Ok(0) => Err("release child closed stdout before emitting Ready".to_string()),
            Ok(_) => Ok(line),
            Err(e) => Err(format!("release stdout read error: {e}")),
        };
        let _ = tx.send(result);
    });

    let first_line = rx
        .recv_timeout(Duration::from_secs(60))
        .expect("release binary did not produce stdout within 60s")
        .expect("release binary stdout read failed");

    // Best-effort clean shutdown so the child doesn't outlive the test.
    if let Some(mut stdin) = child.stdin.take() {
        let _ = writeln!(stdin, "{{\"type\":\"stop\"}}");
    }
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(50));
            }
            Ok(None) | Err(_) => {
                let _ = child.kill();
                break;
            }
        }
    }

    serde_json::from_str(&first_line)
        .unwrap_or_else(|e| panic!("release first stdout line was not JSON ({e}): {first_line:?}"))
}

/// Positive polarity. With both probes reporting Ready, neither can narrow, so
/// the flags are a pure linkage signal and this is the LTO dead-code-strip
/// guard the file was written to be.
#[test]
fn release_binary_ready_event_advertises_plugin_capabilities() {
    let Some(bin) = release_binary_or_skip() else {
        return;
    };
    let event = first_ready_event_release(&bin, Backends::Live);

    assert_eq!(
        event["type"], "ready",
        "first release stdout line should be the Ready event, got: {event}"
    );

    let caps = &event["capabilities"];
    assert!(
        caps.is_object(),
        "Ready event missing capabilities object: {event}"
    );

    // wayland-browser plugin inventory items survived release LTO.
    assert!(
        advertises(caps, "browser_suite"),
        "release binary: WAYLAND_CAMOUFOX_BIN resolves so liveness cannot narrow; withdrawn \
         means wayland-browser was stripped by release LTO; caps: {caps}"
    );

    // wayland-cua plugin presence flips this — independent of the
    // separate `HostCuaRegistrar.computer_use_advertised` runtime gate
    // (which defaults false and controls per-tool registration).
    assert!(
        advertises(caps, "computer_use"),
        "release binary: a display server is nominated so liveness cannot narrow; withdrawn \
         means wayland-cua was stripped by release LTO; caps: {caps}"
    );

    // Umbrella plugins flag — any discovered plugin trips it.
    assert!(
        advertises(caps, "plugins"),
        "release binary: expected capabilities.plugins=true (no plugins discovered at all); \
         caps: {caps}"
    );
}

/// Negative polarity, against the SHIPPED artifact. The release profile is
/// where 27-C2(b) actually reached users, so the honesty property is asserted
/// on the same binary the LTO guard above covers.
#[test]
fn release_binary_withdraws_plugin_capabilities_when_backends_cannot_start() {
    let Some(bin) = release_binary_or_skip() else {
        return;
    };
    let event = first_ready_event_release(&bin, Backends::Dead);

    assert_eq!(
        event["type"], "ready",
        "first release stdout line should be the Ready event, got: {event}"
    );
    let caps = &event["capabilities"];

    // Proves this leg differs from the positive leg ONLY in backend liveness.
    assert!(
        advertises(caps, "plugins"),
        "release binary: plugins=false means the plugin system is inert, so a withdrawn \
         browser_suite below would prove nothing; caps: {caps}"
    );

    assert!(
        !advertises(caps, "browser_suite"),
        "release binary: no browser backend can start yet browser_suite is still \
         advertised — this is the 27-C2(b) false advertisement, in the profile that \
         ships. (A `chromium`/`browserbase` build probes Indeterminate by design and \
         keeps the flag.) caps: {caps}"
    );

    // Indeterminate must NOT narrow, so the honest expectation is platform-dependent.
    let expected_cua = !cfg!(target_os = "linux");
    assert_eq!(
        advertises(caps, "computer_use"),
        expected_cua,
        "release binary: computer_use should be {expected_cua} with no display on {}; \
         Linux must narrow, macOS/Windows report Indeterminate and must not; caps: {caps}",
        std::env::consts::OS
    );
}

#[test]
fn release_binary_smoke_fails_fast_when_artifact_missing() {
    // CI pre-builds the binary via `vx cargo build --release -p wcore-cli` BEFORE
    // running this test, so on CI this case never triggers. Locally, a developer
    // who runs the test on a fresh checkout WITHOUT pre-building should see a
    // clear, fast error pointing at the pre-build step — NOT a 60s cargo build
    // inside the test body (M2.7).
    //
    // Verify by setting WCORE_SMOKE_REQUIRE_PREBUILT=1 in the env:
    //   WCORE_SMOKE_REQUIRE_PREBUILT=1 cargo nextest run -p wcore-cli --test release_binary_smoke
    // The test panics with the prebuild-required message instead of rebuilding.

    if std::env::var("WCORE_SMOKE_REQUIRE_PREBUILT").is_err() {
        eprintln!(
            "[release_binary_smoke] WCORE_SMOKE_REQUIRE_PREBUILT not set — skipping fast-fail test"
        );
        return;
    }

    let bin = release_binary_path();
    if bin.exists() {
        // Already built — nothing to fast-fail on. Treat as pass; the absence
        // of a rebuilt binary is what we're verifying, and "binary exists, no
        // rebuild needed" satisfies the contract trivially.
        return;
    }

    // The new ensure_release_binary_or_fail() should panic with a message
    // that points at the CI pre-build step.
    let result = std::panic::catch_unwind(ensure_release_binary_or_fail);
    let err =
        result.expect_err("expected ensure_release_binary_or_fail to panic when binary is missing");
    let msg = err
        .downcast_ref::<String>()
        .map(String::as_str)
        .unwrap_or("<non-string panic>");
    assert!(
        msg.contains("WCORE_PREBUILD_REQUIRED"),
        "panic message did not mention the prebuild contract: {msg}"
    );
}
