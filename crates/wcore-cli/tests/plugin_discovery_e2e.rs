//! Z1 / VALIDATION MAJOR #5: end-to-end CLI test that proves the plugin
//! inventory mechanism survives all the way from `inventory::submit!`
//! through linker → `PluginLoader::discover` → bootstrap → Ready event.
//!
//! This test exists because BLOCKER #1 in the v0.2.0 validation pass
//! showed that `crates/wcore-cli/Cargo.toml` listing `wayland-browser`,
//! `wayland-cua`, `wayland-ollama` as dependencies was
//! NOT sufficient — Rust's linker dead-code-strips entire crates whose
//! items are never named in the binary, including the `link_section`
//! static items `inventory::submit!` emits. The fix is the
//! `use wayland_<plugin> as _;` lines at the top of `src/main.rs`.
//!
//! This test spawns the real CLI binary in `--json-stream` mode with a
//! fake API key, captures the first stdout line (the Ready event), and
//! asserts the per-plugin capability flags are set to `true`. Any future
//! regression that drops a `use ... as _;` will fail this test instead
//! of silently shipping a binary with an inert plugin system.

use std::io::Write;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use tempfile::TempDir;

/// Spawn the release-or-debug binary with the minimal flags needed to
/// reach `protocol_sink.emit_ready_with_plugins(...)` and return the
/// parsed Ready event and first capability activation. Additive policy receipts
/// may appear between them and must not make this inventory test order-fragile.
fn first_startup_events() -> [serde_json::Value; 2] {
    // Use a clean, empty cwd so no `.wayland-core.toml` from the dev
    // environment perturbs config resolution. Also isolates the
    // session db / skills lookup from polluting the host project.
    let tmp = TempDir::new().expect("create tmp workspace");

    let bin = env!("CARGO_BIN_EXE_wayland-core");
    let mut child = Command::new(bin)
        .args([
            "--json-stream",
            "--provider",
            "anthropic",
            "--api-key",
            "test-key-not-used-because-we-stop-before-message",
        ])
        .current_dir(tmp.path())
        // Defensive: empty HOME so per-user config doesn't sneak in.
        .env("HOME", tmp.path())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn wayland-core --json-stream");

    // Read the first stdout line on a worker thread so we can enforce
    // a wall-clock timeout against a child that never emits.
    let mut stdout = child.stdout.take().expect("capture stdout");
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        use std::io::{BufRead, BufReader};
        let mut reader = BufReader::new(&mut stdout);
        let mut events = Vec::with_capacity(4);
        let result = (|| {
            for _ in 0..16 {
                let mut line = String::new();
                match reader.read_line(&mut line) {
                    Ok(0) => return Err("child closed stdout during startup events".to_string()),
                    Ok(_) => {
                        let event: serde_json::Value =
                            serde_json::from_str(&line).map_err(|e| {
                                format!("startup stdout line was not JSON ({e}): {line:?}")
                            })?;
                        let found_activation = event["type"] == "capability_activation";
                        events.push(event);
                        if found_activation {
                            break;
                        }
                    }
                    Err(e) => return Err(format!("stdout read error: {e}")),
                }
            }
            let ready = events
                .iter()
                .find(|event| event["type"] == "ready")
                .cloned()
                .ok_or_else(|| "startup did not emit ready".to_string())?;
            let activation = events
                .iter()
                .find(|event| event["type"] == "capability_activation")
                .cloned()
                .ok_or_else(|| "startup did not emit a capability activation".to_string())?;
            Ok([ready, activation])
        })();
        let _ = tx.send(result);
    });

    let events = rx
        .recv_timeout(Duration::from_secs(30))
        .expect("did not receive any stdout line within 30s")
        .expect("stdout read failed");

    // Tell the engine to shut down cleanly so the test process tree
    // doesn't outlive this function. We don't care about subsequent
    // events; only the Ready event proves plugin discovery wired up.
    if let Some(mut stdin) = child.stdin.take() {
        let _ = writeln!(stdin, "{{\"type\":\"stop\"}}");
    }

    // Bound the wait so a bug in shutdown doesn't hang CI forever.
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(50));
            }
            Ok(None) => {
                let _ = child.kill();
                break;
            }
            Err(_) => {
                let _ = child.kill();
                break;
            }
        }
    }

    events
}

#[test]
fn ready_event_has_plugin_capability_flags() {
    let [event, activation] = first_startup_events();

    assert_eq!(
        event["type"], "ready",
        "first stdout line should be the Ready event, got: {event}"
    );

    let caps = &event["capabilities"];
    assert!(
        caps.is_object(),
        "Ready event missing capabilities object: {event}"
    );
    assert_eq!(
        activation["type"], "capability_activation",
        "capability activation must follow Ready, got: {activation}"
    );

    // The wayland-browser plugin must produce a true `browser_suite` flag.
    assert_eq!(
        caps["browser_suite"], true,
        "expected capabilities.browser_suite=true (wayland-browser plugin not discovered); \
         caps: {caps}"
    );

    // The wayland-cua plugin must produce a true `computer_use` flag.
    assert_eq!(
        caps["computer_use"], true,
        "expected capabilities.computer_use=true (wayland-cua plugin not discovered); \
         caps: {caps}"
    );

    // The umbrella `plugins` flag must be true once any plugin loaded.
    assert_eq!(
        caps["plugins"], true,
        "expected capabilities.plugins=true (no plugins loaded at all); caps: {caps}"
    );
}
