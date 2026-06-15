//! PTY SMOKE — Lane F2: the `/plugins` marketplace overlay, end to end.
//!
//! Layer-2 sibling of `harness_tui_flow.rs`: spawn the compiled
//! `wayland-core` binary attached to a real pseudo-terminal, drive it with
//! keystrokes, parse the rendered byte stream with `vt100`, and assert on
//! stable text anchors. This file proves the `/plugins` overlay actually
//! works against a live render loop — browse → inspect (consent) → install
//! → switch to Installed → uninstall — not just in isolated unit/router
//! tests.
//!
//! HERMETIC / OFFLINE: the marketplace is a **local directory** fixture, so
//! every code path (`add_marketplace_source`, `resolve_and_plan`,
//! `commit_install`) short-circuits the network — `acquire_source` /
//! `acquire_marketplace` return a local `is_dir()` path verbatim, and the
//! single plugin's `source` is a `RelativePath` joined under the marketplace
//! root. No git clone, no `owner/repo` resolution. Safe to run in CI.
//!
//! WHY a PTY: `main.rs::tui_capable` checks `IsTerminal` before launching the
//! full-screen TUI; a plain piped subprocess falls through to the line REPL.
//! Only a PTY runs the real ratatui UI whose tick loop polls the overlay's
//! async resolve/install jobs (the loop ticks the overlay because
//! `tick_active` was extended for F2).
//!
//! WHY `#![cfg(unix)]`: identical to `harness_tui_flow.rs` — `portable_pty`'s
//! ConPTY backend on the headless `windows-latest` runner does not surface
//! the child's stdout to the master end, so the vt100 grid stays empty and
//! every `wait_for` times out. The overlay's non-PTY surface (parse, plan,
//! commit, uninstall) is covered cross-platform by `marketplace_install_e2e`
//! and the surface's own unit tests.

#![cfg(unix)]

use std::io::{Read, Write};
use std::path::Path;
use std::time::{Duration, Instant};

use portable_pty::{CommandBuilder, MasterPty, PtySize, native_pty_system};
use tempfile::TempDir;

use wcore_cli::plugin::marketplace::add_marketplace_source;

/// Path to the debug binary under test.
fn binary() -> &'static str {
    env!("CARGO_BIN_EXE_wayland-core")
}

/// Seed `<home>/config.toml` so the TUI first-run gate resolves to
/// `Workspace` (not `Onboarding`) without a real provider key. The spawn
/// helper sets `WAYLAND_HOME=<home>`, so `wcore_config::wayland_config_dir()`
/// returns `<home>` and `global_config_path()` is `<home>/config.toml` on all
/// platforms. The marketplace overlay never sends an agent turn, so no
/// network call is ever made. Mirrors `harness_tui_flow::seed_config`.
fn seed_config(home: &Path) {
    std::fs::write(
        home.join("config.toml"),
        "[default]\n\
         provider = \"anthropic\"\n\
         model = \"claude-sonnet-4-20250514\"\n\
         \n\
         [providers.anthropic]\n\
         api_key = \"sk-ant-smoke-not-a-real-key-000000000000\"\n",
    )
    .expect("write config.toml");
}

/// Build a local marketplace fixture: a dir with `.claude-plugin/marketplace.json`
/// listing one relative-path Claude Code plugin that ships a skill **and** a
/// stdio MCP server. The MCP server is what makes `commit_install` write a
/// `consent.json` spawn-consent sidecar (Lane E) and the consent surface
/// render a `spawns` line — so the smoke exercises the security-relevant path,
/// not just a skill-only plugin.
fn build_fixture(dir: &Path) {
    let write = |p: std::path::PathBuf, body: &str| {
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, body).unwrap();
    };
    write(
        dir.join(".claude-plugin/marketplace.json"),
        r#"{
          "name": "localmkt",
          "owner": { "name": "Smoke Tester" },
          "plugins": [
            { "name": "demo", "source": "./demo", "description": "demo smoke plugin" }
          ]
        }"#,
    );
    write(
        dir.join("demo/.claude-plugin/plugin.json"),
        r#"{"name":"demo","version":"0.1.0","description":"demo smoke plugin"}"#,
    );
    write(
        dir.join("demo/skills/hello/SKILL.md"),
        "---\nname: hello\ndescription: greets\n---\nSay hello.",
    );
    write(
        dir.join("demo/.mcp.json"),
        r#"{"mcpServers":{"demosrv":{"command":"${CLAUDE_PLUGIN_ROOT}/srv","args":[]}}}"#,
    );
}

/// Drives one PTY-attached `wayland-core` process. Trimmed copy of
/// `harness_tui_flow::PtyHarness` (the marketplace smoke needs only spawn /
/// screen / wait_for / send — no mock-LLM, no resize, no resume). Each
/// integration test is its own binary, so the harness is duplicated rather
/// than shared, matching `harness_regression`/`harness_tui_flow`.
struct PtyHarness {
    writer: Box<dyn Write + Send>,
    parser: std::sync::Arc<std::sync::Mutex<vt100::Parser>>,
    #[allow(dead_code)]
    master: Box<dyn MasterPty + Send>,
    child: Box<dyn portable_pty::Child + Send + Sync>,
    _reader: std::thread::JoinHandle<()>,
}

impl PtyHarness {
    /// Spawn `wayland-core` on a fresh 120x40 PTY with a hermetic env:
    /// `WAYLAND_HOME`/`HOME` at the tempdir, a TTY-capable `TERM`, and every
    /// inherited provider credential stripped.
    fn spawn(home: &Path) -> Self {
        let pty = native_pty_system()
            .openpty(PtySize {
                rows: 40,
                cols: 120,
                pixel_width: 0,
                pixel_height: 0,
            })
            .expect("open PTY");

        let mut cmd = CommandBuilder::new(binary());
        cmd.env("HOME", home);
        cmd.env("WAYLAND_HOME", home);
        cmd.env("TERM", "xterm-256color");
        cmd.env_remove("API_KEY");
        cmd.env_remove("ANTHROPIC_API_KEY");
        cmd.env_remove("OPENAI_API_KEY");
        cmd.cwd(home);
        let child = pty.slave.spawn_command(cmd).expect("spawn wayland-core");

        let mut reader = pty.master.try_clone_reader().expect("clone PTY reader");
        let parser = std::sync::Arc::new(std::sync::Mutex::new(vt100::Parser::new(40, 120, 0)));
        let parser_for_thread = std::sync::Arc::clone(&parser);
        let reader_handle = std::thread::spawn(move || {
            let mut buf = [0u8; 8192];
            loop {
                match reader.read(&mut buf) {
                    Ok(0) => break,
                    Ok(n) => {
                        if let Ok(mut p) = parser_for_thread.lock() {
                            p.process(&buf[..n]);
                        }
                    }
                    Err(_) => break,
                }
            }
        });

        let writer = pty.master.take_writer().expect("take PTY writer");

        Self {
            writer,
            parser,
            master: pty.master,
            child,
            _reader: reader_handle,
        }
    }

    /// Snapshot the screen as plain text (one row per line, trailing blanks
    /// trimmed by vt100).
    fn screen_text(&self) -> String {
        let parser = self.parser.lock().expect("parser lock");
        parser.screen().contents()
    }

    /// Wait until `predicate(&screen_text)` is true, polling ~30Hz. Panics
    /// with the last screen on timeout (the failure mode worth debugging).
    fn wait_for<F: Fn(&str) -> bool>(&self, predicate: F, timeout: Duration, what: &str) {
        let deadline = Instant::now() + timeout;
        let mut last = String::new();
        while Instant::now() < deadline {
            last = self.screen_text();
            if predicate(&last) {
                return;
            }
            std::thread::sleep(Duration::from_millis(30));
        }
        panic!(
            "timed out after {:?} waiting for {what}.\n--- last screen ---\n{}\n--- end ---",
            timeout, last
        );
    }

    /// Push raw bytes to the PTY (as if typed on the keyboard).
    fn send(&mut self, bytes: &[u8]) {
        self.writer.write_all(bytes).expect("write to PTY");
        self.writer.flush().ok();
    }

    /// Type a string one paced byte at a time so the per-frame input drain
    /// never overruns (a single bulk write drops chars when the app is busy).
    fn type_text(&mut self, text: &str) {
        for b in text.bytes() {
            self.writer.write_all(&[b]).expect("write to PTY");
            self.writer.flush().ok();
            std::thread::sleep(Duration::from_millis(12));
        }
    }
}

impl Drop for PtyHarness {
    fn drop(&mut self) {
        if let Ok(None) = self.child.try_wait() {
            let _ = self.child.kill();
        }
    }
}

/// Boot the TUI and block until the chrome wordmark + Workspace tab land.
/// Cold boot spins up the full agent bootstrap; allow 60s of slack.
fn boot_to_workspace(home: &Path) -> PtyHarness {
    let h = PtyHarness::spawn(home);
    h.wait_for(
        |s| s.contains("WAYLAND") && s.contains("Workspace"),
        Duration::from_secs(60),
        "TUI to render the chrome wordmark and Workspace tab",
    );
    h
}

/// Open the `/plugins` overlay through the command palette: `/` opens the
/// palette, typing `plugins` narrows it to the `/plugins` command, `Enter`
/// runs it → the router applies `OpenOverlay(Marketplace)`.
fn open_plugins_overlay(h: &mut PtyHarness) {
    h.send(b"/");
    h.wait_for(
        |s| s.contains("command") || s.contains("/plugins"),
        Duration::from_secs(3),
        "command palette to open",
    );
    h.type_text("plugins");
    // Let the fuzzy filter settle on `/plugins` as the highlighted row.
    std::thread::sleep(Duration::from_millis(300));
    h.send(b"\r");
}

#[test]
fn plugins_overlay_browse_install_uninstall_round_trip() {
    let home = TempDir::new().expect("tempdir");
    seed_config(home.path());

    // The overlay's store root is `profile_home()/plugins` == `<home>/plugins`
    // (WAYLAND_HOME override). Pre-register the local marketplace there so the
    // catalog cache exists and Browse is populated on `on_enter`.
    let store = home.path().join("plugins");
    let quarantine = store.join(".quarantine");
    std::fs::create_dir_all(&store).unwrap();
    let fixture = home.path().join("_fixture");
    build_fixture(&fixture);
    let meta = add_marketplace_source(&store, &quarantine, &fixture.to_string_lossy())
        .expect("register local marketplace fixture");
    assert_eq!(meta.name, "localmkt");

    let mut h = boot_to_workspace(home.path());

    // ── Browse ──────────────────────────────────────────────────────────
    open_plugins_overlay(&mut h);
    h.wait_for(
        |s| s.contains("Plugins") && s.contains("localmkt") && s.contains("demo"),
        Duration::from_secs(8),
        "marketplace overlay to render the localmkt catalog with the demo plugin",
    );
    // Footer affordances prove this is the Browse segment, not a stale tab.
    let browse = h.screen_text();
    assert!(
        browse.contains("inspect") && browse.contains("add"),
        "browse footer hints missing.\n--- screen ---\n{browse}\n--- end ---"
    );

    // ── Inspect (consent surface) ───────────────────────────────────────
    // `Enter` resolves the highlighted plugin (local path → no clone) and the
    // tick loop transitions Loading → Consent. The MCP server surfaces a
    // `spawns` line; the unofficial source surfaces the unsigned-source warn.
    h.send(b"\r");
    h.wait_for(
        |s| s.contains("install") && s.contains("demosrv") && s.contains("spawns"),
        Duration::from_secs(20),
        "consent surface to render the resolved plan (adds, spawns, install hint)",
    );
    let consent = h.screen_text();
    assert!(
        consent.contains("unsigned-source"),
        "unofficial marketplace must surface the unsigned-source warning.\n\
         --- screen ---\n{consent}\n--- end ---"
    );

    // ── Install ─────────────────────────────────────────────────────────
    h.send(b"\r");
    h.wait_for(
        |s| s.contains("installed"),
        Duration::from_secs(20),
        "install to complete and the status line to read `installed`",
    );

    // The native plugin dir + the spawn-consent sidecar must be on disk.
    let install_dir = store.join("demo@localmkt");
    assert!(
        install_dir.join("plugin.toml").is_file(),
        "expected generated plugin.toml at {}",
        install_dir.display()
    );
    assert!(
        install_dir.join("consent.json").is_file(),
        "expected spawn-consent sidecar at {}/consent.json",
        install_dir.display()
    );

    // ── Installed segment ───────────────────────────────────────────────
    h.send(b"\t");
    h.wait_for(
        |s| s.contains("demo@localmkt"),
        Duration::from_secs(5),
        "Installed segment to list the freshly installed plugin",
    );

    // ── Uninstall ───────────────────────────────────────────────────────
    h.send(b"u");
    h.wait_for(
        |s| s.contains("uninstalled"),
        Duration::from_secs(20),
        "uninstall to complete and the status line to read `uninstalled`",
    );
    assert!(
        !install_dir.exists(),
        "install dir must be gone after uninstall: {}",
        install_dir.display()
    );

    // Clean dismissal: Esc closes the overlay back to the workspace.
    h.send(b"\x1b");
    h.wait_for(
        |s| s.contains("Workspace") && !s.contains("uninstalled"),
        Duration::from_secs(5),
        "overlay to close back to the workspace",
    );
}
