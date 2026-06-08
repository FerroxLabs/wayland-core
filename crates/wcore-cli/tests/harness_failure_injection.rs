//! USER-FLOW HARNESS — Layer 3: failure injection (the crown jewel).
//!
//! Layers 1 and 2 prove the binary's happy paths. This layer reproduces
//! the exact bug classes the audit named — the failure modes that caused
//! a 35-minute hang on a "gates green" build — and asserts the in-tree
//! fixes hold.
//!
//! Gated behind `--features harness-failure-injection` on `wcore-cli`
//! because the wedged-MCP scenario deliberately waits out a real 30s
//! connect timeout. That is too slow for the default `cargo test` pass
//! — mirrors how `wcore-browser` gates its live Chromium suite behind
//! `browser-live-tests`.
//!
//! Run with:
//!
//!     cargo test -p wcore-cli --features harness-failure-injection \
//!         --test harness_failure_injection -- --test-threads=1
//!
//! ## Scripted-provider / VCR investigation outcome
//!
//! The task brief asked whether the binary supports a scripted / VCR
//! replay provider mode so a "deliberately-slow tool" scenario could be
//! driven without an API key. The investigation outcome:
//!
//! * `crates/wcore-agent/src/vcr.rs` defines a `VcrLayer` with record /
//!   replay modes, gated on `VCR_MODE` + `VCR_CASSETTE` env vars.
//! * Grep for `VcrLayer` / `from_env` / `VCR_MODE` across the workspace
//!   returns zero call sites outside the module itself. The layer is
//!   never instantiated by `AgentBootstrap`, never wired into any
//!   provider (`crates/wcore-providers/src/*`), and never reachable from
//!   the binary. It is dead code at the binary level.
//! * The dispatch-timeout regression is therefore covered by the
//!   in-process engine test
//!   `wcore-agent::engine::tests::hung_tool_times_out_with_error_result`
//!   (a `tokio::test(start_paused = true)` that fast-forwards virtual
//!   time past the 30s `ToolCategory::Info` budget).
//!
//! End-to-end interruption coverage in this file therefore relies on
//! the Ctrl+C scenario below, which uses a real provider and is
//! API-key-gated (matching the `e2e.yml` pattern).

#![cfg(feature = "harness-failure-injection")]

use std::io::{Read, Write};
use std::path::Path;
use std::time::{Duration, Instant};

use portable_pty::{CommandBuilder, MasterPty, PtySize, native_pty_system};
use tempfile::TempDir;

/// Path to the debug binary under test.
fn binary() -> &'static str {
    env!("CARGO_BIN_EXE_wayland-core")
}

/// Seed `HOME` with a config that (a) has a valid provider table for
/// boot to succeed and (b) registers one wedged stdio MCP server.
///
/// The wedged server is `sh -c 'cat >/dev/null'` — it accepts every byte
/// the MCP manager writes on its stdin, never produces a response, and
/// never exits. This is the canonical "MCP server that never replies"
/// fixture; the bounded-handshake fix (`wcore-mcp::manager::
/// CONNECT_TIMEOUT = 30s`, audit C2) is the only thing standing between
/// it and an unbounded boot hang.
fn seed_config_with_wedged_mcp(home: &Path) {
    let macos_dir = home
        .join("Library")
        .join("Application Support")
        .join("wayland-core");
    let linux_dir = home.join(".config").join("wayland-core");
    let body = "[default]\n\
                provider = \"anthropic\"\n\
                model = \"claude-sonnet-4-20250514\"\n\
                \n\
                [providers.anthropic]\n\
                api_key = \"sk-ant-harness-not-real-key-0000000000\"\n\
                \n\
                # The wedged stdio MCP server fixture. `sh -c 'cat >/dev/null'`\n\
                # is a process that accepts stdin and never speaks — the\n\
                # MCP handshake hangs reading its response until the\n\
                # CONNECT_TIMEOUT fires (30s, audit C2). Pre-fix this\n\
                # call was unbounded and stranded boot indefinitely.\n\
                [mcp.servers.wedged]\n\
                transport = \"stdio\"\n\
                command = \"sh\"\n\
                args = [\"-c\", \"cat >/dev/null\"]\n";
    for dir in [&macos_dir, &linux_dir] {
        std::fs::create_dir_all(dir).expect("create config dir");
        std::fs::write(dir.join("config.toml"), body).expect("write config.toml");
    }
}

/// Seed `HOME` with a valid provider config — NO wedged MCP server,
/// just a baseline working TUI. Used by the Ctrl+C recovery sub-test
/// so the TUI boots fast.
///
/// `api_key` is written directly into the config because
/// `wcore-config::config::api_key_for` returns the config-file key
/// FIRST and only falls back to env vars when the file has no key
/// (config.rs ~line 1070). The Ctrl+C sub-test calls this with the
/// real `ANTHROPIC_API_KEY` value so the provider call actually
/// streams.
fn seed_config_plain(home: &Path, api_key: &str) {
    let macos_dir = home
        .join("Library")
        .join("Application Support")
        .join("wayland-core");
    let linux_dir = home.join(".config").join("wayland-core");
    let body = format!(
        "[default]\n\
         provider = \"anthropic\"\n\
         model = \"claude-sonnet-4-20250514\"\n\
         \n\
         [providers.anthropic]\n\
         api_key = \"{api_key}\"\n"
    );
    for dir in [&macos_dir, &linux_dir] {
        std::fs::create_dir_all(dir).expect("create config dir");
        std::fs::write(dir.join("config.toml"), &body).expect("write config.toml");
    }
}

/// Trimmed-down clone of the Layer-2 `PtyHarness`. Local to this file so
/// the failure-injection layer stays self-contained and the
/// happy-path Layer-2 harness can evolve independently.
struct PtyHarness {
    writer: Box<dyn Write + Send>,
    parser: std::sync::Arc<std::sync::Mutex<vt100::Parser>>,
    master: Box<dyn MasterPty + Send>,
    child: Box<dyn portable_pty::Child + Send + Sync>,
    _reader: std::thread::JoinHandle<()>,
}

impl PtyHarness {
    fn spawn(home: &Path, env_overrides: &[(&str, &str)]) -> Self {
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
        cmd.env("TERM", "xterm-256color");
        cmd.env_remove("API_KEY");
        cmd.env_remove("ANTHROPIC_API_KEY");
        cmd.env_remove("OPENAI_API_KEY");
        // Caller-supplied env wins — used to inject a real API key for
        // the Ctrl+C sub-test after the env_remove sweep above.
        for (k, v) in env_overrides {
            cmd.env(*k, *v);
        }
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

    fn screen_text(&self) -> String {
        self.parser.lock().expect("parser lock").screen().contents()
    }

    /// Returns `true` if `predicate` matched within `timeout`, `false`
    /// on timeout. The `_what` parameter is unused at runtime; it stays
    /// on the signature for documentation of intent at call sites and is
    /// surfaced by the asserter's own panic-on-false-message.
    fn wait_for<F: Fn(&str) -> bool>(&self, predicate: F, timeout: Duration, _what: &str) -> bool {
        let deadline = Instant::now() + timeout;
        while Instant::now() < deadline {
            if predicate(&self.screen_text()) {
                return true;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        false
    }

    fn send(&mut self, bytes: &[u8]) {
        self.writer.write_all(bytes).expect("write to PTY");
        self.writer.flush().ok();
    }

    fn wait_for_exit(&mut self, timeout: Duration) -> Option<portable_pty::ExitStatus> {
        let deadline = Instant::now() + timeout;
        while Instant::now() < deadline {
            match self.child.try_wait() {
                Ok(Some(status)) => return Some(status),
                Ok(None) => std::thread::sleep(Duration::from_millis(50)),
                Err(_) => return None,
            }
        }
        None
    }

    /// Avoid an unused-field warning for `master` while keeping the
    /// PTY's lifetime tied to the harness (the writer handle is taken
    /// from the master, which must outlive it).
    #[allow(dead_code)]
    fn pty_size(&self) -> Option<PtySize> {
        self.master.get_size().ok()
    }
}

impl Drop for PtyHarness {
    fn drop(&mut self) {
        if let Ok(None) = self.child.try_wait() {
            let _ = self.child.kill();
        }
    }
}

#[test]
fn wedged_mcp_server_does_not_hang_boot() {
    // AUDIT C2 regression test (the BIG one).
    //
    // Pre-fix: `McpManager::connect_server` ran the JSON-RPC handshake
    // with an unbounded `read` on the MCP server's stdout. A server
    // that accepts stdin but never replies (the `sh -c 'cat >/dev/null'`
    // fixture below) stranded boot forever — the literal 35-minute-hang
    // failure mode the user reported.
    //
    // Post-fix: each per-server connect is bounded by
    // `wcore-mcp::manager::CONNECT_TIMEOUT = 30s` and all connects run
    // concurrently. The wedged server times out, an error is logged,
    // boot continues. The TUI must reach the Workspace surface within
    // ~35s — 30s for the bounded handshake plus slack for the rest of
    // bootstrap (provider plugins, agent pack, etc.).
    //
    // If a regression re-introduces unbounded waiting, the TUI never
    // renders WAYLAND within 35s and this test fails with the very
    // failure-mode the audit named.
    let home = TempDir::new().expect("tempdir");
    seed_config_with_wedged_mcp(home.path());
    let start = Instant::now();
    let h = PtyHarness::spawn(home.path(), &[]);

    // (a) BOOT TERMINATES — the TUI must render the WAYLAND wordmark
    // within 35s despite the wedged server. The bound is chosen as
    // 30s (CONNECT_TIMEOUT) + 5s slack; any longer and a regression
    // has dropped the timeout or reintroduced a serial wait.
    let booted = h.wait_for(
        |s| s.contains("WAYLAND") && s.contains("Workspace"),
        Duration::from_secs(35),
        "TUI to reach Workspace despite a wedged MCP server",
    );
    let elapsed = start.elapsed();
    assert!(
        booted,
        "boot HUNG with a wedged MCP server: TUI did not reach Workspace within 35s.\n\
         elapsed={elapsed:?}\n--- last screen ---\n{}\n--- end ---",
        h.screen_text()
    );

    // (b) IT DOES NOT HANG FOREVER — already covered by (a)'s 35s bound,
    // but log the elapsed time for forensics. A jump from ~30s to 35s
    // in CI history would be an early warning that the timeout drifted.
    eprintln!("[harness] boot-with-wedged-MCP elapsed: {elapsed:?}");

    // Clean shutdown via the proven palette /exit path so the next
    // test does not race a live child.
    drop(h);
}

#[test]
fn ctrl_c_during_a_real_turn_does_not_brick_the_session() {
    // The direct regression test for this session's two engine bugs
    // (cooperative-cancel wiring + spinner-on-error). It requires a
    // real provider, so it is API-key-gated — same pattern as e2e.yml.
    //
    // Skips cleanly (no fail) when neither ANTHROPIC_API_KEY nor
    // API_KEY is set in the environment, with a printed reason matching
    // the e2e.yml "::warning::… is not set" format.
    let api_key = std::env::var("ANTHROPIC_API_KEY")
        .ok()
        .or_else(|| std::env::var("API_KEY").ok());
    let Some(api_key) = api_key else {
        eprintln!(
            "[harness] SKIP ctrl_c_during_a_real_turn_does_not_brick_the_session — \
             neither ANTHROPIC_API_KEY nor API_KEY is set. \
             Set ANTHROPIC_API_KEY to exercise this regression test."
        );
        return;
    };

    let home = TempDir::new().expect("tempdir");
    seed_config_plain(home.path(), api_key.as_str());
    // env override is belt-and-suspenders — the seeded config-file key
    // is the actual auth source for the provider (api_key_for returns
    // the file key first).
    let mut h = PtyHarness::spawn(home.path(), &[("ANTHROPIC_API_KEY", api_key.as_str())]);

    // Wait for the workspace to render.
    assert!(
        h.wait_for(
            |s| s.contains("WAYLAND") && s.contains("Workspace"),
            Duration::from_secs(60),
            "TUI to boot",
        ),
        "TUI did not boot in 60s"
    );

    // Type a prompt that will trigger at least one tool call (Read /
    // Grep / Glob — the built-in tools the binary always registers).
    // The exact response is irrelevant; the test only needs the engine
    // to be in flight — either streaming assistant tokens or about to
    // call a tool — when Ctrl+C lands.
    h.send(
        b"list the files in the current directory using the appropriate tool, then explain them\r",
    );

    // Wait for a SURFACE-LEVEL indicator that an assistant turn is
    // in flight. Three anchors land in the transcript only AFTER the
    // turn starts — none of them appear at boot:
    //   * `thinking…`  — `render_turns` emits this once `session.thinking`
    //     is non-empty (workspace.rs ~line 703);
    //   * `wayland` (the assistant role marker) — emitted once a turn
    //     has streamed any text (workspace.rs ~line 725);
    //   * the spawned tool's name in a tool card (`Glob` / `Read` / etc.).
    // The user message echo (`› list the files…`) is NOT enough — that
    // would render before the engine has done any real work.
    let streaming = h.wait_for(
        |s| {
            s.contains("thinking…")
                || s.contains("Glob")
                || s.contains("Grep")
                || s.contains("Read ")
                // The assistant role marker is `wayland` painted bold —
                // distinct from `WAYLAND` (the chrome wordmark, all caps).
                || s.contains("wayland")
        },
        Duration::from_secs(45),
        "engine to start streaming a turn (thinking… / wayland / tool card)",
    );
    assert!(
        streaming,
        "engine never started streaming within 45s — cannot test Ctrl+C mid-turn.\nlast screen:\n{}",
        h.screen_text()
    );

    // Send Ctrl+C while the engine is working. NOTE: the TUI's quit
    // chord is two presses — first arms (`quit_armed`), second quits.
    // A SINGLE press is the cancel affordance for a turn-in-flight, NOT
    // a quit. We send ONE Ctrl+C and immediately type any other key
    // (the chord disarms on any non-Ctrl-C key) to make sure the
    // session is not on the second-press-arms-to-quit edge.
    h.send(&[0x03]); // Ctrl+C
    std::thread::sleep(Duration::from_millis(500));
    // A no-op character to disarm the quit chord. ESC is safe: in the
    // workspace it triggers /cancel (the same cancel verb), which is
    // idempotent if no turn is in flight.
    h.send(b"\x1b");

    // The session must accept new input. The proven test for "the TUI
    // is alive" is the same exit path Layer 2 uses: open the palette,
    // type `exit`, Enter. If Ctrl+C wedged the session the palette
    // never opens and /exit never runs — the wait_for_exit times out.
    std::thread::sleep(Duration::from_millis(500));
    h.send(b"/");
    assert!(
        h.wait_for(
            |s| s.contains("/  command") || s.contains("command "),
            Duration::from_secs(5),
            "palette to open after Ctrl+C",
        ),
        "TUI did not accept `/` after Ctrl+C — the session is wedged.\nlast screen:\n{}",
        h.screen_text()
    );
    h.send(b"exit");
    std::thread::sleep(Duration::from_millis(300));
    h.send(b"\r");

    let status = h
        .wait_for_exit(Duration::from_secs(10))
        .expect("wayland-core did not exit within 10s of /exit after Ctrl+C — session bricked");
    assert!(
        status.success(),
        "expected clean exit after Ctrl+C + /exit; got {status:?}"
    );
}
