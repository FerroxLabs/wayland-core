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
//! fake API key and captures the startup events. Any future regression
//! that drops a `use ... as _;` fails it instead of silently shipping a
//! binary with an inert plugin system.
//!
//! # Why this is a two-run differential and not a `== true` assertion
//!
//! `85b60a2f` (ledger row 27-C2(b)) narrowed `capabilities.browser_suite` /
//! `.computer_use` from **linkage** to **backend liveness**, because a
//! headless host advertised `true` and the first operation died with
//! `spawn camoufox: No such file or directory`. That narrowing is correct
//! and this test must not undo it: restoring an unconditional `== true`
//! here would re-introduce the lie in order to make a test pass.
//!
//! So the flags are no longer a pure linkage signal, and the honest
//! question became *what should these tests assert instead?* Three
//! candidates were weighed (cross-audit panel, 3-of-3, plus an internal
//! adversarial pass; see `.planning/CI-TRIAGE.md`):
//!
//!   (a) the capability is present WHEN a backend is live;
//!   (b) the probe answers at all;
//!   (c) the advertisement MATCHES the probe.
//!
//! **(c) is the only one that would have caught the original lie**, and it
//! is implemented below as a two-run differential that asserts *polarity*,
//! not merely change. (a) is vacuous on precisely the headless host where
//! the defect shipped — the antecedent is false, so nothing is checked and
//! the test self-passes. (b) proves an instrument exists but never relates
//! it to the advertisement; under the old code the advertisement path never
//! consulted a probe at all, so (b) would pass while the lie stood.
//!
//! The expected values come from environment facts the test *plants*
//! (`WAYLAND_CAMOUFOX_BIN`, `WAYLAND_CAMOUFOX_URL`, `DISPLAY`), never from
//! calling the probe under test — otherwise the assertion would be
//! `f(x) == f(x)`. Every input the probe reads has to be one of them: leaving
//! the sidecar base URL to the ambient host made the Dead leg fail on any
//! machine with a Camoufox sidecar running on the default port. Asserting both
//! polarities rather than "the flag changed" matters because inverted
//! behaviour also changes.
//!
//! The original linkage guarantee survives, and is strengthened: a
//! dead-code-stripped plugin crate cannot be advertised even in the live
//! run, so [`ready_event_advertises_plugin_capabilities_when_backends_can_start`]
//! still fails on a dropped `use ... as _;`.

use std::io::Write;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use tempfile::TempDir;

#[path = "support/mod.rs"]
mod support;
use support::owned_tree::OwnedTree;

/// Which answer the two backend-liveness probes behind
/// `capabilities.browser_suite` / `.computer_use` should give for a run.
///
/// Both probes are non-executing and read their inputs from the environment,
/// so a test can drive them without installing a browser or starting an X
/// server — and without calling the probe itself to compute its own expected
/// value.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Backends {
    /// Every probe input names something that could start.
    Live,
    /// Every probe input is provably dead.
    Dead,
}

/// Plant the environment facts the probes read. This is the test's oracle:
/// expected flag values are derived from what is planted here, never from
/// invoking `wcore_browser::liveness` / `wcore_cua::liveness`.
fn apply_backend_env(cmd: &mut Command, backends: Backends) {
    // A credentialed Browserbase build probes `Indeterminate` and keeps the
    // browser flag regardless of any local backend, which would make the Dead
    // leg silently unfalsifiable. Clear it in BOTH legs so each leg's name
    // matches what is actually being measured.
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
            // The browser probe resolves this with `which`, which accepts an
            // absolute path as given and NEVER executes it. The running test
            // binary is therefore a valid stand-in for an installed sidecar.
            cmd.env(
                "WAYLAND_CAMOUFOX_BIN",
                std::env::current_exe().expect("resolve test binary path"),
            );
            // The CUA probe asks only whether a display server is nominated;
            // it does not connect. (If the ambient environment sets
            // WAYLAND_DISPLAY, that path reports Ready too — either way the
            // planted fact is "a display server exists".)
            cmd.env("DISPLAY", ":0");
        }
        Backends::Dead => {
            cmd.env(
                "WAYLAND_CAMOUFOX_BIN",
                "wayland-core-e2e-no-such-browser-binary",
            );
            cmd.env_remove("DISPLAY");
            cmd.env_remove("WAYLAND_DISPLAY");
        }
    }
}

/// Read a capability flag the way a host actually reads it.
///
/// `AdvertisedCapabilities` marks these fields
/// `#[serde(skip_serializing_if = "is_false")]`, so a withdrawn capability is
/// **omitted from the wire entirely** rather than sent as `false`. Absent and
/// `false` are therefore the same claim — "not advertised" — and comparing
/// against a literal `false` would fail against a correctly withdrawn flag.
/// Measured: the negative leg's `ready` event carries `plugins: true` with no
/// `browser_suite` key at all.
fn advertises(caps: &serde_json::Value, key: &str) -> bool {
    match &caps[key] {
        serde_json::Value::Null => false,
        v => v
            .as_bool()
            .unwrap_or_else(|| panic!("capabilities.{key} was not a bool: {v}")),
    }
}

/// Spawn the release-or-debug binary with the minimal flags needed to
/// reach `protocol_sink.emit_ready_with_plugins(...)` and return the
/// parsed Ready event and first capability activation. Additive policy receipts
/// may appear between them and must not make this inventory test order-fragile.
fn first_startup_events(backends: Backends) -> [serde_json::Value; 2] {
    // Use a clean, empty cwd so no `.wayland-core.toml` from the dev
    // environment perturbs config resolution. Also isolates the
    // session db / skills lookup from polluting the host project.
    let tmp = TempDir::new().expect("create tmp workspace");

    let bin = env!("CARGO_BIN_EXE_wayland-core");
    let mut cmd = Command::new(bin);
    cmd.args([
        "--json-stream",
        "--provider",
        "anthropic",
        "--api-key",
        "test-key-not-used-because-we-stop-before-message",
    ])
    .current_dir(tmp.path())
    // Defensive: empty HOME so per-user config doesn't sneak in.
    //
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
    let mut child = OwnedTree::new(cmd.spawn().expect("spawn wayland-core --json-stream"));

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

/// Positive polarity. With a resolvable browser binary and a nominated display,
/// NEITHER liveness probe can narrow, so both flags are once again a pure
/// linkage signal — which is the property this file was created to defend.
/// A dropped `use wayland_<plugin> as _;` fails here.
#[test]
fn ready_event_advertises_plugin_capabilities_when_backends_can_start() {
    let [event, activation] = first_startup_events(Backends::Live);

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
    assert!(
        advertises(caps, "browser_suite"),
        "WAYLAND_CAMOUFOX_BIN resolves, so the liveness probe reports Ready and cannot \
         narrow: browser_suite is pure linkage here. Withdrawn means the wayland-browser \
         plugin was not discovered (dropped `use wayland_browser as _;`?); caps: {caps}"
    );

    // The wayland-cua plugin must produce a true `computer_use` flag.
    assert!(
        advertises(caps, "computer_use"),
        "a display server is nominated, so the CUA probe reports Ready and cannot narrow: \
         computer_use is pure linkage here. Withdrawn means the wayland-cua plugin was \
         not discovered (dropped `use wayland_cua as _;`?); caps: {caps}"
    );

    // The umbrella `plugins` flag must be true once any plugin loaded. This one
    // is never narrowed, so it is an independent link anchor: if it is false the
    // whole inventory mechanism is inert, whatever the two flags above say.
    assert!(
        advertises(caps, "plugins"),
        "expected capabilities.plugins=true (no plugins loaded at all); caps: {caps}"
    );
}

/// Negative polarity — the assertion that would have caught 27-C2(b).
///
/// Same binary, same plugins, same linkage; only the probe inputs are dead.
/// Before `85b60a2f` both flags read `true` here, which is exactly the state
/// that made the desktop app render a capability whose first operation died.
#[test]
fn ready_event_withdraws_plugin_capabilities_when_backends_cannot_start() {
    let [event, _activation] = first_startup_events(Backends::Dead);

    assert_eq!(
        event["type"], "ready",
        "first stdout line should be the Ready event, got: {event}"
    );
    let caps = &event["capabilities"];

    // The plugin is still linked and still discovered — `plugins` proves it —
    // so a `false` below is liveness narrowing and nothing else. Without this
    // the negative leg could pass for the wrong reason (plugins missing
    // entirely), which would make the differential meaningless.
    assert!(
        advertises(caps, "plugins"),
        "the negative leg must differ from the positive leg ONLY in backend liveness; \
         plugins=false means the plugin system itself is inert and this leg proves \
         nothing; caps: {caps}"
    );

    assert!(
        !advertises(caps, "browser_suite"),
        "no browser backend can start (WAYLAND_CAMOUFOX_BIN does not resolve, no sidecar \
         on the healthcheck URL) yet browser_suite is still advertised — this is 27-C2(b), \
         the host is being shown a capability that cannot work. (If this build enables the \
         `chromium` or `browserbase` backend the probe returns Indeterminate by design and \
         keeps the flag; this leg then needs those backends disabled to stay meaningful.) \
         caps: {caps}"
    );

    // Only the Linux probe can prove a dead display server without launching
    // anything. macOS and Windows return Indeterminate, and Indeterminate
    // deliberately KEEPS the capability — under-advertising a working feature is
    // the same defect as over-advertising a broken one. So the honest expectation
    // is platform-dependent, and asserting "withdrawn" everywhere would be wrong.
    let expected_cua = !cfg!(target_os = "linux");
    assert_eq!(
        advertises(caps, "computer_use"),
        expected_cua,
        "computer_use should be {expected_cua} with no DISPLAY/WAYLAND_DISPLAY on \
         {}: Linux can prove the X11 backend cannot connect and must narrow, while \
         macOS/Windows report Indeterminate and must NOT narrow; caps: {caps}",
        std::env::consts::OS
    );
}
