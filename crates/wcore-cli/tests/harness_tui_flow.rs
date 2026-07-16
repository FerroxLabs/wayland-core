//! USER-FLOW HARNESS — Layer 2: TUI flow via PTY.
//!
//! Layer 1 covers the non-interactive subcommands. This layer drives the
//! **real ratatui TUI** through a pseudo-terminal: spawn the compiled
//! `wayland-core` binary attached to a PTY, send keystrokes the way a
//! human's keyboard would, parse the rendered byte stream into a screen
//! grid with `vt100`, and assert on stable text anchors.
//!
//! Why a PTY: the binary checks `IsTerminal::is_terminal(&stdout())`
//! before launching the TUI (`main.rs::tui_capable`). A plain piped
//! subprocess would always fall through to the line-based REPL — only a
//! PTY makes the real full-screen UI run.
//!
//! Hermetic environment: every test points `WAYLAND_HOME` at a tempdir
//! (F-010 hermetic-sandbox env in `wcore-config::wayland_config_dir()`)
//! and pre-writes a minimal `config.toml` so the TUI's first-run gate
//! resolves to `Workspace` (not `Onboarding`) without any real provider
//! key being required (the binary boots the engine but never makes a
//! network call until a user prompt is sent).
//!
//! Stable anchors only: the renderer paints colour cells via crossterm
//! escape sequences and the spinner cycles every few frames, so byte-for-
//! byte comparison is brittle. Every assertion targets the human-visible
//! text that `vt100`'s screen grid yields after applying the ANSI stream.
//!
//! WHY `#![cfg(unix)]`: `portable_pty`'s Windows backend (ConPTY) in the
//! headless GHA `windows-latest` runner does not surface the spawned
//! binary's stdout to the master PTY end — the vt100 parser stays empty
//! and every `wait_for` hits its 60s timeout (round 13 caught this with
//! "timed out after 60s waiting for TUI to render the chrome wordmark
//! and Workspace tab" + empty screen). The binary itself runs fine on
//! Windows (`harness_cli_surface` and `harness_regression` cover its
//! CLI surface there); only the PTY-driven full-screen TUI harness is
//! Unix-only.

#![cfg(unix)]

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use portable_pty::{CommandBuilder, MasterPty, PtySize, native_pty_system};
use tempfile::TempDir;
use wcore_agent::session_journal::{ApprovalResolution, SessionEvent, SessionJournal};

// Shared mock-LLM harness support (the scriptable Anthropic-shaped server the
// real provider talks to). Included by path so this binary and
// `harness_mock_llm.rs` share one copy without a crate-level module.
#[path = "support/mod.rs"]
mod support;

/// Path to the debug binary under test.
fn binary() -> &'static str {
    env!("CARGO_BIN_EXE_wayland-core")
}

fn only_session_journal(home: &Path) -> PathBuf {
    let sessions = home.join("sessions");
    let journals = std::fs::read_dir(&sessions)
        .unwrap_or_else(|error| panic!("read session directory {sessions:?}: {error}"))
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.extension().and_then(|ext| ext.to_str()) == Some("journal"))
        .collect::<Vec<_>>();
    assert_eq!(
        journals.len(),
        1,
        "expected one session journal under {sessions:?}, found {journals:?}"
    );
    journals.into_iter().next().unwrap()
}

/// Seed `<home>/config.toml` with a minimal valid config so the TUI's
/// first-run gate (`!global_config_path().exists()`) resolves to
/// `Workspace`. The spawn helper sets `WAYLAND_HOME=<home>` (F-010), so
/// `wcore_config::wayland_config_dir()` returns `<home>` directly —
/// `global_config_path()` is then `<home>/config.toml` on all three
/// platforms. The api key is a non-credential placeholder; nothing in
/// the test path sends a network request.
///
/// WHY this isn't HOME-based: on Windows `dirs::config_dir()` resolves
/// via `%APPDATA%`, not `HOME`, so a `HOME=tempdir` override does not
/// redirect the binary's config path and the seeded file is never read.
/// Round 12 (`8c446ca`) caught this for `harness_cli_surface`; round 13
/// applies the same fix here. Pattern matches `harness_regression.rs`.
fn seed_config(home: &Path) {
    std::fs::write(
        home.join("config.toml"),
        "[default]\n\
         provider = \"anthropic\"\n\
         model = \"claude-sonnet-4-20250514\"\n\
         \n\
         [providers.anthropic]\n\
         api_key = \"sk-ant-harness-not-real-key-0000000000\"\n",
    )
    .expect("write config.toml");
}

/// Like [`seed_config`] but points the Anthropic provider's `base_url` at a
/// local mock server. The real provider POSTs to `{base_url}/v1/messages`
/// (`wcore_providers::anthropic::stream`), so this routes every agent turn the
/// spawned binary makes to the in-test [`support::mock_llm::MockLlm`] instead
/// of a live endpoint — letting a PTY test drive the *real* agent loop
/// deterministically. The provider path is not SSRF-guarded (unlike MCP /
/// browser), so a `127.0.0.1` base_url is dialed normally.
fn seed_config_with_base_url(home: &Path, base_url: &str) {
    std::fs::write(
        home.join("config.toml"),
        format!(
            "[default]\n\
             provider = \"anthropic\"\n\
             model = \"claude-sonnet-4-20250514\"\n\
             \n\
             [providers.anthropic]\n\
             api_key = \"sk-ant-harness-not-real-key-0000000000\"\n\
             base_url = \"{base_url}\"\n"
        ),
    )
    .expect("write config.toml");
}

/// Drives one PTY-attached `wayland-core` process for a single test.
///
/// Owns the master PTY, the spawned child, a reader thread that pumps
/// the byte stream into a `vt100::Parser`, and a guard against runaway
/// process leaks (`Drop` kills the child if it is still alive).
struct PtyHarness {
    /// Master end of the PTY — keystrokes are written here.
    writer: Box<dyn Write + Send>,
    /// vt100 screen — refreshed by `flush_screen`, read by the asserters.
    parser: std::sync::Arc<std::sync::Mutex<vt100::Parser>>,
    /// Master PTY handle, kept alive so the writer end stays open and
    /// `resize` calls work.
    master: Box<dyn MasterPty + Send>,
    /// The spawned child. `wait` consumes it; until then `Drop` kills it.
    child: Box<dyn portable_pty::Child + Send + Sync>,
    /// Reader-thread join handle. Kept so the test can join after the
    /// child exits (clean shutdown), but `Drop` lets it dangle on panic.
    _reader: std::thread::JoinHandle<()>,
}

impl PtyHarness {
    /// Spawn `wayland-core` on a fresh PTY sized 120x40.
    ///
    /// 120 columns is wide enough that the right rail stays visible
    /// (`RAIL_RESPONSIVE_MIN_WIDTH = 100` in `workspace.rs`); the
    /// resize-handling test below shrinks the PTY through that
    /// threshold and re-asserts.
    fn spawn(home: &Path) -> Self {
        Self::spawn_with_args(home, &[])
    }

    /// Like [`spawn`](Self::spawn) but passes extra CLI args to the binary
    /// (e.g. `["--continue"]` to resume the most-recent saved session). Used
    /// by the session-resume journey, which re-boots a second process against
    /// the same `WAYLAND_HOME` to assert the prior conversation is restored.
    fn spawn_with_args(home: &Path, args: &[&str]) -> Self {
        let pty = native_pty_system()
            .openpty(PtySize {
                rows: 40,
                cols: 120,
                pixel_width: 0,
                pixel_height: 0,
            })
            .expect("open PTY");

        // Build a hermetic command: tempdir HOME, a TTY-capable TERM, no
        // ambient provider key. Cwd is the tempdir so any `.wayland-core.toml`
        // walk-up search never finds a developer's project config.
        let mut cmd = CommandBuilder::new(binary());
        for arg in args {
            cmd.arg(arg);
        }
        cmd.env("HOME", home);
        // F-010 hermetic-sandbox override — wcore-config's
        // `wayland_config_dir()` honours this before falling back to
        // platform-native paths, so the seeded `<home>/config.toml`
        // resolves cleanly on Windows (where `HOME` alone would leak
        // to `%APPDATA%\wayland-core\`).
        cmd.env("WAYLAND_HOME", home);
        cmd.env("TERM", "xterm-256color");
        // Strip any inherited credentials — the harness never needs a
        // real key, and an accidental hit on the provider would be a
        // hidden network call from a test.
        cmd.env_remove("API_KEY");
        cmd.env_remove("ANTHROPIC_API_KEY");
        cmd.env_remove("OPENAI_API_KEY");
        cmd.cwd(home);
        let vault = support::vault::configure_pty(&mut cmd);
        let child = pty.slave.spawn_command(cmd);
        drop(vault);
        let child = child.expect("spawn wayland-core");

        // The reader thread pumps the PTY's byte stream into a shared
        // vt100 parser; tests query the screen grid by locking the
        // parser and rendering its current contents.
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

    /// Snapshot the screen as plain text (one row per line, trailing
    /// blanks trimmed). Called between key sends; vt100 applies every
    /// ANSI sequence the binary has emitted so far.
    fn screen_text(&self) -> String {
        let parser = self.parser.lock().expect("parser lock");
        parser.screen().contents()
    }

    /// Wait until `predicate(&screen_text)` returns `true`, polling at
    /// ~30Hz. Fails with a clear message that includes the last screen
    /// state if `timeout` elapses — the failure mode the rubric flags
    /// as the most important to debug (timeouts are flaky-test poison).
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

    /// Type a string one byte at a time with a short delay between keystrokes,
    /// the way a human types. A single bulk write outruns the TUI's per-frame
    /// input drain when the app is busy (e.g. just after a turn finalises),
    /// dropping characters; paced bytes give the event loop time to consume
    /// each key. Does NOT send a trailing newline — call `send(b"\r")` to
    /// submit.
    fn type_text(&mut self, text: &str) {
        for b in text.bytes() {
            self.writer.write_all(&[b]).expect("write to PTY");
            self.writer.flush().ok();
            std::thread::sleep(Duration::from_millis(12));
        }
    }

    /// Resize the PTY. The TUI sees this as a `crossterm::event::Resize`
    /// and reflows.
    fn resize(&mut self, cols: u16, rows: u16) {
        self.master
            .resize(PtySize {
                rows,
                cols,
                pixel_width: 0,
                pixel_height: 0,
            })
            .expect("resize PTY");
    }

    /// Block until the child exits or `timeout` elapses. Returns the
    /// child's exit status, or `None` on timeout (the caller decides
    /// whether to fail).
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
}

impl Drop for PtyHarness {
    fn drop(&mut self) {
        // Last-ditch cleanup: if the test panicked mid-flow, kill the
        // child so it never outlives the test process. `try_wait` first
        // to skip the kill on a clean exit.
        if let Ok(None) = self.child.try_wait() {
            let _ = self.child.kill();
        }
    }
}

/// Boot the TUI and block until `WAYLAND` (the chrome wordmark) lands on
/// screen. Reused by every test in this file.
///
/// The first boot has to spin up the full agent bootstrap (plugin
/// discovery, tool registry, any configured stdio MCP servers). Each MCP
/// handshake is bounded by `wcore-mcp::manager::CONNECT_TIMEOUT = 30s`
/// (audit C2) and runs concurrently with the rest of bootstrap. Allow 60s
/// so a cold runner has slack but a regression that re-introduces
/// unbounded waiting still trips the assert.
fn boot_to_workspace(home: &Path) -> PtyHarness {
    let h = PtyHarness::spawn(home);
    h.wait_for(
        |s| s.contains("WAYLAND") && s.contains("Workspace"),
        Duration::from_secs(60),
        "TUI to render the chrome wordmark and Workspace tab",
    );
    h
}

#[test]
fn tui_renders_the_chrome_and_every_tab_on_boot() {
    let home = TempDir::new().expect("tempdir");
    seed_config(home.path());
    let h = boot_to_workspace(home.path());

    let screen = h.screen_text();
    // The hybrid-branded wordmark sits at the top-left of every surface.
    assert!(
        screen.contains("WAYLAND"),
        "chrome wordmark missing on boot.\n--- screen ---\n{screen}\n--- end ---"
    );

    // All six tab labels paint inline next to the wordmark. These mirror
    // `SurfaceId::TABS` (Plugins/Marketplace moved to the `/plugins` overlay,
    // so they are no longer tab chrome). The
    // `widgets/header.rs::top_chrome_shows_the_wordmark_and_every_tab` unit
    // test asserts this in isolation; here we assert the same anchors land on a
    // real PTY-rendered screen.
    for tab in [
        "Workspace",
        "Sub-Agents",
        "Plan",
        "Config",
        "Diagnostics",
        "Workflows",
    ] {
        assert!(
            screen.contains(tab),
            "tab label `{tab}` missing on boot.\n--- screen ---\n{screen}\n--- end ---"
        );
    }

    // The bottom status bar sits on every surface. It renders the session
    // identity as `model │ mode │ ctx │ cost │ elapsed`; the consistent
    // anchor is the model name seeded into the config (the status bar shows
    // the model, not the provider label).
    assert!(
        screen.contains("claude-sonnet-4-20250514"),
        "status bar missing the configured model.\n--- screen ---\n{screen}\n--- end ---"
    );
}

#[test]
fn tab_key_navigates_to_subagents_and_back() {
    let home = TempDir::new().expect("tempdir");
    seed_config(home.path());
    let mut h = boot_to_workspace(home.path());

    // `Tab` is the global next-surface key (keybind.rs `next.surface`).
    // From Workspace (TABS index 0) one press lands on Sub-Agents (1).
    // The Sub-Agents surface paints a header with a "Sub-agents" anchor
    // distinct from the tab label, but the tab label itself becomes the
    // highlighted/active tab. We assert on a Sub-Agents surface-only
    // anchor: the empty-state text the surface paints when no spawns
    // exist (see surfaces/subagents.rs).
    h.send(b"\t");
    h.wait_for(
        // The Sub-Agents surface paints distinct content not present on
        // the Workspace surface. The header tab is shared, so we look
        // for a Sub-Agents-only anchor.
        |s| s.contains("Sub-Agents") && s.to_lowercase().contains("sub-agent"),
        Duration::from_secs(5),
        "Sub-Agents surface to activate after Tab",
    );

    // `Shift+Tab` (BackTab) returns to Workspace. crossterm encodes
    // BackTab as `ESC [ Z` in xterm-style sequences; the PTY pipeline
    // accepts the raw bytes.
    h.send(b"\x1b[Z");
    h.wait_for(
        |s| s.contains("Workspace") && (s.contains("⌃B") || s.contains("Path map")),
        Duration::from_secs(5),
        "Workspace surface to re-activate after Shift+Tab",
    );
}

#[test]
fn slash_exit_via_palette_terminates_the_session_cleanly() {
    let home = TempDir::new().expect("tempdir");
    seed_config(home.path());
    let mut h = boot_to_workspace(home.path());

    // The Workspace surface intercepts `/` on an empty composer and
    // opens the command palette overlay (workspace.rs ~line 272). The
    // palette's fuzzy filter narrows on each typed char; once a command
    // is the highlighted row, `Enter` runs it.
    //
    // Sequence: `/` opens palette, typing "exit" narrows the list to
    // `/exit` as the only sensible match (Palette::refilter sets
    // `selected` to the first command row each refilter), `Enter` runs
    // `/exit` → `app.quit = true` (surfaces/mod.rs ~line 668).
    h.send(b"/");
    h.wait_for(
        |s| s.contains("/  command") || s.contains("command "),
        Duration::from_secs(3),
        "palette overlay to open",
    );
    h.send(b"exit");
    // Give vt100 a frame to apply the filtered list before committing.
    std::thread::sleep(Duration::from_millis(300));
    h.send(b"\r");

    // The session must exit within a few seconds — the quit path is
    // synchronous from `app.quit = true` through the loop exit and
    // terminal restore. Any longer and a regression has stranded the
    // loop on a pending future.
    let status = h
        .wait_for_exit(Duration::from_secs(8))
        .expect("wayland-core did not exit within 8s of /exit");
    assert!(
        status.success(),
        "expected a clean exit from /exit; got status {status:?}"
    );
}

#[test]
fn narrow_terminal_resize_stays_coherent_without_panicking() {
    // v0.9.1.2 F15 removed the rail's `Path map` panel and W8 removed the
    // `Tools` panel; the rail's sole remaining tenant is `Activity`, which
    // renders ONLY when there is active work (running tools or system
    // notices). A hermetic boot-and-resize with no agent turns produces no
    // activity, so the rail never paints and the old `Path map` anchor is
    // gone — the rail's width-responsive hide (`rail_effectively_visible`,
    // RAIL_RESPONSIVE_MIN_WIDTH = 100) is unit-tested in `workspace.rs`.
    //
    // What this PTY test still uniquely guards: the LIVE binary survives an
    // aggressive narrow resize and back without panicking, and the chrome
    // stays coherent throughout (the surface never corrupts on tight rows).
    let home = TempDir::new().expect("tempdir");
    seed_config(home.path());
    let mut h = boot_to_workspace(home.path());

    // Shrink well below RAIL_RESPONSIVE_MIN_WIDTH = 100. 80 cols is the
    // canonical "narrow terminal" size. A render-primitive panic on tight
    // rows would crash the child here.
    h.resize(80, 40);
    h.wait_for(
        |s| s.contains("WAYLAND") && s.contains("Workspace"),
        Duration::from_secs(5),
        "chrome to stay painted after shrinking to 80 cols",
    );

    // Sanity: the screen is still coherent — the surface didn't panic and
    // the tabs are still painted.
    let screen = h.screen_text();
    assert!(
        screen.contains("Workspace"),
        "Workspace tab vanished after resize — surface state corrupt.\n{screen}"
    );

    // Restoring width must keep the chrome coherent — the symmetric half of
    // the contract. A resize handler that corrupted state on the way down
    // and never recovered would fail here.
    h.resize(120, 40);
    h.wait_for(
        |s| s.contains("WAYLAND") && s.contains("Workspace"),
        Duration::from_secs(5),
        "chrome to stay coherent after resizing back to 120 cols",
    );

    // Clean shutdown so the next test (or the dropping panic guard) is
    // not racing a live child. Use the proven palette quit path.
    h.send(b"/");
    std::thread::sleep(Duration::from_millis(300));
    h.send(b"exit\r");
    let _ = h.wait_for_exit(Duration::from_secs(8));
}

/// THE keystone E2E: a real agent turn, end to end, through the shipped TUI.
///
/// Every other test in this file drives the chrome (boot, tabs, palette,
/// resize) WITHOUT an LLM. This one wires the scriptable `MockLlm` server into
/// the spawned binary via the provider `base_url` and proves the core value
/// prop: the user types a prompt, the REAL agent loop runs a REAL turn against
/// the (mock) provider, and the streamed assistant text renders into the
/// transcript. This is the seam every daily-driver journey test builds on.
#[test]
fn agent_turn_streams_mock_assistant_text_into_the_transcript() {
    let home = TempDir::new().expect("tempdir");

    // A held multi-thread runtime keeps the wiremock server serving for the
    // whole test; the spawned binary (a separate process) POSTs to it over
    // real loopback TCP. Both `rt` and `server` are bound for the function's
    // lifetime so the endpoint stays up until the assertion passes.
    let rt = tokio::runtime::Runtime::new().expect("tokio runtime");
    let server = rt.block_on(
        support::mock_llm::MockLlm::new()
            .text("WAYLAND_MOCK_STREAM_OK the agent loop is wired end to end")
            .start(),
    );

    seed_config_with_base_url(home.path(), &server.uri());

    let mut h = boot_to_workspace(home.path());

    // A plain prompt (no leading `/`, so it's a message not a command),
    // submitted with Enter — this drives a real agent turn against the mock.
    h.send(b"say hello\r");

    // The mock's scripted assistant text must stream into the transcript.
    h.wait_for(
        |s| s.contains("WAYLAND_MOCK_STREAM_OK"),
        Duration::from_secs(30),
        "mock-scripted assistant text to render in the transcript after a real agent turn",
    );
    h.wait_for(
        |s| !s.contains("Esc interrupt"),
        Duration::from_secs(30),
        "mock-scripted agent turn to reach its idle terminal state",
    );

    // `/provider` (bare) opens the arrow-key picker OVERLAY — reaching its real
    // handler, not the LLM. Drive it through the palette, assert the picker
    // paints a provider row, then `esc` to close before the shutdown path.
    h.send(b"/");
    h.wait_for(
        |s| s.contains("/  command") || s.contains("command "),
        Duration::from_secs(3),
        "palette overlay to open before selecting /provider",
    );
    // Pace the query bytes. The initial unfiltered palette already contains
    // the `/provider` row, so a screen-only wait can otherwise return before
    // a bulk `provider\r` burst has crossed the PTY input boundary.
    h.type_text("provider");
    h.wait_for(
        |s| s.contains("/ provider") && s.contains("/provider") && s.contains("switch provider"),
        Duration::from_secs(3),
        "provider query to render with the exact /provider palette row selected",
    );
    // Keep the query and the control key on separate PTY input boundaries.
    // This is the same stabilization interval used by the other palette
    // journeys in this harness.
    std::thread::sleep(Duration::from_millis(300));
    h.send(b"\r");
    h.wait_for(
        |s| s.to_lowercase().contains("anthropic"),
        Duration::from_secs(6),
        "/provider to open the provider picker overlay",
    );
    h.send(b"\x1b"); // esc closes the overlay
    std::thread::sleep(Duration::from_millis(300));

    // Clean shutdown via the proven palette quit path. `rt`/`server` drop
    // naturally at end of scope, after the assertion above.
    h.send(b"/");
    std::thread::sleep(Duration::from_millis(300));
    h.send(b"exit\r");
    let _ = h.wait_for_exit(Duration::from_secs(8));
}

/// Phase 1 journey #1 — the full approval round-trip end to end through the
/// shipped TUI: the model asks to run a tool that is NOT auto-approved, the
/// approval card renders, the user presses the approve key, the tool actually
/// executes (writes a real file), and the agent turn continues to its closing
/// assistant text. This is the highest-risk daily-driver path that the keystone
/// did not yet cover (the keystone proved a plain text turn; this proves the
/// tool → approval-UI → execute → continue loop).
///
/// `Write` is deliberately chosen over `Read`: read-only tools (`Read`/`Grep`/
/// `Glob`) are in the default auto-approve allow-list (`wcore-config` config.rs)
/// and would never raise the dialog. `Write` is gated, and — unlike `Bash` — it
/// executes with no sandbox/network dependency, so the round-trip is fully
/// deterministic and we can assert the file landed in the workspace as hard
/// proof the tool ran (not merely that text streamed afterwards).
#[test]
fn tool_call_renders_approval_then_executes_and_continues_on_approve() {
    let home = TempDir::new().expect("tempdir");

    // The Write tool refuses relative paths by design (`validate_user_path`,
    // Wave SD #14), so the script hands it an ABSOLUTE path inside the tempdir
    // workspace. The name avoids the substring "approve" on purpose: the file
    // path is echoed in the approval card, and the screen anchor below matches
    // on "approve"/"deny" — a name containing "approve" would false-positive
    // that anchor before any UI rendered.
    let note_name = "note_for_review.txt";
    let note_body = "WROTE_VIA_GATE_OK";
    let note_path = home.path().join(note_name);
    let note_path_arg = note_path.to_str().expect("tempdir path utf-8").to_string();

    // Held runtime + server keep the mock serving for the test's lifetime (same
    // pattern as the keystone). Script: turn 1 = a gated Write tool call; turn 2
    // (served after the tool result is POSTed back) = the closing text. The mock
    // replays by cursor, so the second POST — which only happens if the tool was
    // approved and executed — yields the continuation text.
    let rt = tokio::runtime::Runtime::new().expect("tokio runtime");
    let server = rt.block_on(
        support::mock_llm::MockLlm::new()
            .tool_use(
                "Write",
                serde_json::json!({ "file_path": note_path_arg, "content": note_body }),
            )
            .text("WAYLAND_TOOL_DONE the write was approved and the turn continued")
            .start(),
    );

    seed_config_with_base_url(home.path(), &server.uri());

    let mut h = boot_to_workspace(home.path());

    // Submit a plain prompt — drives the real turn, which returns the Write call.
    h.send(b"write the review note\r");

    // 1. The approval dialog must render: the gated Write call suspends the loop
    //    awaiting consent. The inline card carries an approve/deny key row.
    h.wait_for(
        |s| s.contains("approve") && s.contains("deny"),
        Duration::from_secs(30),
        "the approval dialog (approve/deny key row) to render for the gated Write tool",
    );

    // 2. Approve once with `y` (the TUI routes approval keys while a card awaits
    //    consent; `y`/Enter == approve-once per workspace.rs::handle_approval_key).
    h.send(b"y");

    // 3. The turn must CONTINUE: only an approved+executed tool lets the engine
    //    POST the tool result and pull the mock's scripted continuation text.
    h.wait_for(
        |s| s.contains("WAYLAND_TOOL_DONE"),
        Duration::from_secs(30),
        "the agent turn to continue with the closing assistant text after approval",
    );

    // 4. Hard proof the tool actually EXECUTED (not just that text streamed):
    //    the file the Write call named is now on disk in the workspace.
    let on_disk = std::fs::read_to_string(&note_path).unwrap_or_default();
    assert!(
        on_disk.contains(note_body),
        "approved Write should have created {note_name} containing {note_body:?}; \
         read {on_disk:?} from {note_path:?}"
    );

    // Clean shutdown via the proven palette quit path.
    h.send(b"/");
    std::thread::sleep(Duration::from_millis(300));
    h.send(b"exit\r");
    let _ = h.wait_for_exit(Duration::from_secs(8));
}

/// F14 cancellation authority: Esc while a tool approval is pending cancels
/// the whole turn cooperatively. Core must resolve the durable approval as
/// cancelled before terminalizing the turn, and the proposed tool must never
/// cross its execution boundary.
#[test]
fn esc_on_pending_approval_durably_cancels_turn_without_tool_execution() {
    let home = TempDir::new().expect("tempdir");
    let output_path = home.path().join("must_not_be_written.txt");
    let output_arg = output_path
        .to_str()
        .expect("tempdir path utf-8")
        .to_string();

    let rt = tokio::runtime::Runtime::new().expect("tokio runtime");
    let server = rt.block_on(
        support::mock_llm::MockLlm::new()
            .tool_use(
                "Write",
                serde_json::json!({
                    "file_path": output_arg,
                    "content": "F14_TOOL_MUST_NOT_EXECUTE"
                }),
            )
            .start(),
    );
    seed_config_with_base_url(home.path(), &server.uri());

    let mut h = boot_to_workspace(home.path());
    h.send(b"propose the guarded write\r");
    h.wait_for(
        |s| s.contains("approve") && s.contains("deny"),
        Duration::from_secs(30),
        "the Write approval card to become pending",
    );

    h.send(b"\x1b");
    h.wait_for(
        |s| s.contains("Cancellation requested"),
        Duration::from_secs(5),
        "Esc to request cooperative turn cancellation",
    );
    h.wait_for(
        |s| s.contains("User aborted the session"),
        Duration::from_secs(15),
        "Core to durably finalize the cancelled approval turn",
    );

    assert!(
        !output_path.exists(),
        "the pending Write crossed its execution boundary during cancellation"
    );

    let entries = SessionJournal::replay(only_session_journal(home.path()))
        .expect("replay cancelled TUI turn journal");
    let approval_cancelled = entries
        .iter()
        .position(|entry| {
            matches!(
                &entry.event,
                SessionEvent::ApprovalResolved {
                    resolution: ApprovalResolution::Cancelled,
                    ..
                }
            )
        })
        .expect("pending approval must resolve as Cancelled");
    let turn_cancelled = entries
        .iter()
        .position(|entry| matches!(&entry.event, SessionEvent::TurnCancelled { .. }))
        .expect("turn must terminalize as TurnCancelled");
    assert!(
        approval_cancelled < turn_cancelled,
        "approval cancellation must be durable before TurnCancelled"
    );
    assert!(
        entries.iter().all(|entry| !matches!(
            &entry.event,
            SessionEvent::ToolExecutionStarted { .. } | SessionEvent::ToolExecutionFinished { .. }
        )),
        "cancelled approval produced a physical tool execution receipt"
    );

    h.send(b"/");
    std::thread::sleep(Duration::from_millis(300));
    h.send(b"exit\r");
    let _ = h.wait_for_exit(Duration::from_secs(8));
}

/// Phase 1 journey #2 — a transient provider error recovers via the REAL retry
/// path, end to end through the TUI. The mock returns `503` on the first POST;
/// the Anthropic provider's `builder_send_with_retry` retries 5xx inline (~250ms
/// backoff) and the second POST serves the success turn, whose text streams into
/// the transcript. Proves the daily-driver "the API blipped" case is invisible
/// to the user: no error surfaces, the turn just completes.
#[test]
fn provider_transient_error_retries_then_streams_the_response() {
    let home = TempDir::new().expect("tempdir");

    // Script: POST 1 → 503 (retried inline by the provider), POST 2 → the
    // success text. The mock advances its cursor per POST, so the retry pulls
    // the next turn exactly as a recovered request would.
    let rt = tokio::runtime::Runtime::new().expect("tokio runtime");
    let server = rt.block_on(
        support::mock_llm::MockLlm::new()
            .http_error(503)
            .text("WAYLAND_RETRY_OK recovered after a transient 503")
            .start(),
    );

    seed_config_with_base_url(home.path(), &server.uri());
    let mut h = boot_to_workspace(home.path());

    h.send(b"say hello after a blip\r");

    // The success turn must stream — which only happens if the provider retried
    // past the 503 rather than surfacing it as a failure.
    h.wait_for(
        |s| s.contains("WAYLAND_RETRY_OK"),
        Duration::from_secs(30),
        "assistant text to stream after the provider retried past a transient 503",
    );

    // Structural proof the retry actually happened (not that the mock just
    // skipped the error): the provider POSTed to /v1/messages at least twice —
    // the 503 attempt plus the recovering attempt. Asserting on the recorded
    // requests is robust, unlike screen-scraping for the absence of "503":
    // retry diagnostics now go through `tracing` (this journey caught them
    // leaking to the TUI via `eprintln!`; fixed in wcore-providers/retry.rs),
    // so they are correctly invisible to the transcript either way.
    let posts = rt
        .block_on(server.received_requests())
        .expect("mock records requests")
        .into_iter()
        .filter(|r| r.url.path() == "/v1/messages")
        .count();
    assert!(
        posts >= 2,
        "expected >=2 POSTs (503 + retry); the provider made {posts}"
    );

    // Clean shutdown via the proven palette quit path.
    h.send(b"/");
    std::thread::sleep(Duration::from_millis(300));
    h.send(b"exit\r");
    let _ = h.wait_for_exit(Duration::from_secs(8));
}

/// Phase 1 journey #3 — a real multi-turn conversation (3 rounds) through the
/// TUI, proving both that successive turns render AND that the engine threads
/// prior history into each request. Each user prompt drives one agent turn /
/// one POST; the mock serves a distinct ack per round. Beyond asserting all
/// three acks persist in the transcript, the test inspects the THIRD recorded
/// request body and asserts it still carries the FIRST prompt — hard proof the
/// conversation history is replayed to the provider, not silently dropped.
#[test]
fn multi_turn_conversation_threads_history_across_three_rounds() {
    let home = TempDir::new().expect("tempdir");

    let rt = tokio::runtime::Runtime::new().expect("tokio runtime");
    let server = rt.block_on(
        support::mock_llm::MockLlm::new()
            .text("ACK_ALPHA round one")
            .text("ACK_BETA round two")
            .text("ACK_GAMMA round three")
            .start(),
    );

    seed_config_with_base_url(home.path(), &server.uri());
    let mut h = boot_to_workspace(home.path());

    // Each round types at human pace then submits, and waits for that round's
    // ack before starting the next — mirroring real sequential use (a user
    // reads the reply before typing again). Typing the next prompt as one bulk
    // write right as the prior turn finalises drops characters (the TUI drains
    // input per frame); `type_text` paces the bytes so every key lands.

    // Round 1 — a distinctive word ("alpha") we later look for in round 3's
    // request body to prove history threading.
    h.type_text("first prompt alpha");
    h.send(b"\r");
    h.wait_for(
        |s| s.contains("ACK_ALPHA"),
        Duration::from_secs(30),
        "round-one assistant text",
    );

    // Round 2 — small settle so the turn-1 finalise doesn't eat the first keys.
    std::thread::sleep(Duration::from_millis(600));
    h.type_text("second prompt beta");
    h.send(b"\r");
    h.wait_for(
        |s| s.contains("ACK_BETA"),
        Duration::from_secs(30),
        "round-two assistant text",
    );

    // Round 3.
    std::thread::sleep(Duration::from_millis(600));
    h.type_text("third prompt gamma");
    h.send(b"\r");
    h.wait_for(
        |s| s.contains("ACK_GAMMA"),
        Duration::from_secs(30),
        "round-three assistant text",
    );

    // 1. All three rounds persist in the transcript (the conversation is not
    //    clobbered turn-to-turn).
    let screen = h.screen_text();
    for ack in ["ACK_ALPHA", "ACK_BETA", "ACK_GAMMA"] {
        assert!(
            screen.contains(ack),
            "transcript should retain every round; missing {ack}\n--- screen ---\n{screen}"
        );
    }

    // 2. Hard proof of history threading: the third request body must still
    //    carry the FIRST prompt's distinctive word. The Anthropic request
    //    serialises the full messages array, so a dropped-history regression
    //    (sending only the latest turn) would fail this.
    let requests = rt
        .block_on(server.received_requests())
        .expect("mock records requests");
    let third = requests
        .iter()
        .filter(|r| r.url.path() == "/v1/messages")
        .nth(2)
        .expect("a third /v1/messages POST");
    let third_body = String::from_utf8_lossy(&third.body);
    assert!(
        third_body.contains("alpha"),
        "round-three request must replay the round-one prompt (history threading); \
         body did not contain \"alpha\""
    );

    // Clean shutdown via the proven palette quit path.
    h.send(b"/");
    std::thread::sleep(Duration::from_millis(300));
    h.send(b"exit\r");
    let _ = h.wait_for_exit(Duration::from_secs(8));
}

/// Phase 1 journey #4 — session save → exit → resume → history threaded. A real
/// turn in one process is persisted to `$WAYLAND_HOME/sessions/`, the process
/// exits, and a SECOND process booted with `--continue` resumes it. This is the
/// "I closed my laptop and came back" path; the data-integrity guarantee is
/// what matters most: the conversation is saved completely AND a resumed process
/// reloads it into context.
///
/// We prove resume restored the prior conversation by inspecting the request
/// the resumed process makes: its next turn must replay the earlier user prompt
/// AND assistant reply to the provider. That is observable and load-bearing
/// even though the resumed TUI transcript does NOT yet repaint the old messages
/// (a known unimplemented feature — see the `#[ignore]`'d companion test
/// `resume_repaints_prior_conversation_into_the_transcript`).
#[test]
fn session_save_resume_threads_prior_history_into_the_next_request() {
    let home = TempDir::new().expect("tempdir");

    let user_marker = "SAVED_TOKEN_42";
    let assistant_marker = "ASSISTANT_REPLY_PERSISTED";

    // Two success turns share one server (one cursor) across both processes:
    // process 1 pops turn 0 (the saved reply); process 2's post-resume prompt
    // pops turn 1. The server stays up for the whole test so process 2 can run
    // a turn whose request body we inspect for the restored history.
    let rt = tokio::runtime::Runtime::new().expect("tokio runtime");
    let server = rt.block_on(
        support::mock_llm::MockLlm::new()
            .text(assistant_marker)
            .text("RESUMED_TURN_OK")
            .start(),
    );
    seed_config_with_base_url(home.path(), &server.uri());

    // --- Process 1: one turn, then clean exit (per-turn save persists it). ---
    {
        let mut h = boot_to_workspace(home.path());
        h.type_text(&format!("remember {user_marker}"));
        h.send(b"\r");
        h.wait_for(
            |s| s.contains(assistant_marker),
            Duration::from_secs(30),
            "the first process to complete and render the assistant reply",
        );
        // Let the per-turn save flush before tearing the process down.
        std::thread::sleep(Duration::from_millis(500));
        h.send(b"/");
        std::thread::sleep(Duration::from_millis(300));
        h.send(b"exit\r");
        let _ = h.wait_for_exit(Duration::from_secs(8));
    }

    // The session must be on disk with BOTH sides of the exchange:
    // $WAYLAND_HOME/sessions/<date>_<id>.json (index.json is the catalog).
    let sessions_dir = home.path().join("sessions");
    let saved: Vec<_> = std::fs::read_dir(&sessions_dir)
        .map(|rd| {
            rd.filter_map(|e| e.ok())
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .filter(|n| n.ends_with(".json") && n != "index.json")
                .collect()
        })
        .unwrap_or_default();
    assert!(
        !saved.is_empty(),
        "process 1 should have persisted a session JSON under {sessions_dir:?}; found none"
    );
    let saved_json =
        std::fs::read_to_string(sessions_dir.join(&saved[0])).expect("read saved session json");
    assert!(
        saved_json.contains(assistant_marker) && saved_json.contains(user_marker),
        "saved session JSON should contain both sides of the exchange; got:\n{saved_json}"
    );

    // --- Process 2: resume with `--continue`, then run one turn. ---
    let mut h2 = PtyHarness::spawn_with_args(home.path(), &["--continue"]);
    h2.wait_for(
        |s| s.contains("WAYLAND") && s.contains("Workspace"),
        Duration::from_secs(60),
        "the resumed process to boot to the workspace",
    );
    h2.type_text("and now continue");
    h2.send(b"\r");
    h2.wait_for(
        |s| s.contains("RESUMED_TURN_OK"),
        Duration::from_secs(30),
        "the resumed process to complete a turn",
    );

    // Proof resume RESTORED the conversation into engine context: the resumed
    // process's request replays the prior user prompt AND assistant reply to
    // the provider. (The transcript repaint is the separate, not-yet-wired
    // gap; the context restore — the part that protects against data loss — is
    // what this asserts, and it is fully working.)
    let last_post = rt
        .block_on(server.received_requests())
        .expect("mock records requests")
        .into_iter()
        .rfind(|r| r.url.path() == "/v1/messages")
        .expect("at least one POST from the resumed process");
    let body = String::from_utf8_lossy(&last_post.body);
    assert!(
        body.contains(user_marker) && body.contains(assistant_marker),
        "resumed request must replay the restored history (both {user_marker} and \
         {assistant_marker}); the resume did not reload the prior conversation"
    );

    // Clean shutdown of the resumed process.
    h2.send(b"/");
    std::thread::sleep(Duration::from_millis(300));
    h2.send(b"exit\r");
    let _ = h2.wait_for_exit(Duration::from_secs(8));
}

/// Phase 1 journey #4 (companion) — the resumed TUI REPAINTS the prior
/// conversation into the transcript, so a user who reopens with `--continue`
/// sees their history, not a blank screen. Boot-time resume restores engine
/// context (proven by `session_save_resume_threads_prior_history_into_the_next_request`)
/// AND now rebuilds the transcript via `protocol_bridge::hydrate_history`,
/// seeded into the initial `App` in `run_tui_mode`.
#[test]
fn resume_repaints_prior_conversation_into_the_transcript() {
    let home = TempDir::new().expect("tempdir");
    let user_marker = "SAVED_TOKEN_42";
    let assistant_marker = "ASSISTANT_REPLY_PERSISTED";

    let rt = tokio::runtime::Runtime::new().expect("tokio runtime");
    let server = rt.block_on(
        support::mock_llm::MockLlm::new()
            .text(assistant_marker)
            .start(),
    );
    seed_config_with_base_url(home.path(), &server.uri());

    {
        let mut h = boot_to_workspace(home.path());
        h.type_text(&format!("remember {user_marker}"));
        h.send(b"\r");
        h.wait_for(
            |s| s.contains(assistant_marker),
            Duration::from_secs(30),
            "the first process to render the assistant reply",
        );
        std::thread::sleep(Duration::from_millis(500));
        h.send(b"/");
        std::thread::sleep(Duration::from_millis(300));
        h.send(b"exit\r");
        let _ = h.wait_for_exit(Duration::from_secs(8));
    }

    let h2 = PtyHarness::spawn_with_args(home.path(), &["--continue"]);
    // The restored assistant reply must repaint into the transcript on resume.
    h2.wait_for(
        |s| s.contains(assistant_marker),
        Duration::from_secs(60),
        "the resumed transcript to repaint the prior assistant reply",
    );
    let restored = h2.screen_text();
    assert!(
        restored.contains(user_marker),
        "resumed transcript should also show the prior user prompt ({user_marker}); \
         screen:\n{restored}"
    );

    let mut h2 = h2;
    h2.send(b"/");
    std::thread::sleep(Duration::from_millis(300));
    h2.send(b"exit\r");
    let _ = h2.wait_for_exit(Duration::from_secs(8));
}

/// Phase 1 journey #5 — interrupt a turn mid-stream (ESC, the in-flight cancel
/// affordance) and prove the session SURVIVES: it cancels cleanly and accepts a
/// fresh turn afterward. The engine emits `StreamStart` at turn-submission
/// (engine.rs, before the provider call), so a held-back HTTP response leaves
/// the turn genuinely in flight and interruptible for the whole delay — no real
/// provider needed. (`harness_failure_injection.rs::ctrl_c_during_a_real_turn`
/// covers the real-provider Ctrl-C path; this is the deterministic mock twin and
/// adds the recover-and-continue assertion.)
#[test]
fn esc_on_an_accepted_provider_stream_surfaces_recovery_and_blocks_redispatch() {
    let home = TempDir::new().expect("tempdir");

    // Turn 0 is SLOW (reply held ~4s) so we can interrupt it after the
    // physical provider boundary. A second scripted response exists only to
    // prove the blocked follow-up cannot consume it before reconciliation.
    let rt = tokio::runtime::Runtime::new().expect("tokio runtime");
    let server = rt.block_on(
        support::mock_llm::MockLlm::new()
            .slow_text("WAYLAND_SHOULD_NOT_APPEAR slow turn body", 4000)
            .text("WAYLAND_RECOVERED_OK the session still works after cancel")
            .start(),
    );
    seed_config_with_base_url(home.path(), &server.uri());
    let mut h = boot_to_workspace(home.path());

    // Start the slow turn.
    h.type_text("start a slow turn");
    h.send(b"\r");

    // Wait until the slow POST has actually reached the mock — the turn is now
    // genuinely in flight (StreamStart fired, `streaming_active` is true), so
    // ESC maps to /cancel. Polling the recorded requests removes any timing race
    // between "we typed Enter" and "the turn is cancellable".
    {
        let deadline = Instant::now() + Duration::from_secs(20);
        loop {
            let posts = rt
                .block_on(server.received_requests())
                .map(|rs| rs.iter().filter(|r| r.url.path() == "/v1/messages").count())
                .unwrap_or(0);
            if posts >= 1 {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "the slow turn's POST never reached the mock"
            );
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    // Interrupt: ESC is the in-flight cancel affordance (workspace.rs maps
    // `Esc while streaming` → /cancel). The TUI acknowledges the request
    // immediately while Core retains the run future long enough to durably
    // finalize cancellation or preserve unknown provider authority.
    h.send(b"\x1b");
    h.wait_for(
        |s| s.contains("Cancellation requested"),
        Duration::from_secs(15),
        "the in-flight turn to acknowledge the cancellation request",
    );

    // The slow body must NOT have rendered — we interrupted before it arrived.
    let after_cancel = h.screen_text();
    assert!(
        !after_cancel.contains("WAYLAND_SHOULD_NOT_APPEAR"),
        "the cancelled turn's body must not render; screen:\n{after_cancel}"
    );

    // F14: local Esc is not proof that an already-accepted provider request
    // stopped. Provider attempts do not have the tool-effect reconciliation
    // authority behind `/recover reconcile`, so the TUI must surface the
    // durable unknown outcome and expose no unsafe recovery action.
    h.wait_for(
        |s| {
            let screen = s.to_ascii_lowercase();
            screen.contains("suspended")
                && screen.contains("provideroutcomeunknown")
                && screen.contains("allowed: none (fail-closed)")
        },
        Duration::from_secs(15),
        "Esc to surface the fail-closed provider-outcome-unknown state",
    );

    // A normal message while the provider outcome is unresolved must fail
    // closed and must not trigger the fixture's second provider response.
    h.type_text("now a normal turn");
    h.send(b"\r");
    h.wait_for(
        |s| {
            let screen = s.to_ascii_lowercase();
            screen.contains("session persistence authority unavailable")
                && screen.contains("journal cursor")
        },
        Duration::from_secs(15),
        "a pre-reconciliation follow-up to fail closed",
    );
    let requests = rt
        .block_on(server.received_requests())
        .expect("read provider fixture requests");
    assert_eq!(
        requests
            .iter()
            .filter(|request| request.url.path() == "/v1/messages")
            .count(),
        1,
        "pre-reconciliation follow-up dispatched the provider"
    );
    assert!(
        !h.screen_text().contains("WAYLAND_RECOVERED_OK"),
        "the blocked second provider response became visible"
    );

    // Clean shutdown via the proven palette quit path.
    h.send(b"/");
    std::thread::sleep(Duration::from_millis(300));
    h.send(b"exit\r");
    let _ = h.wait_for_exit(Duration::from_secs(8));
}

#[test]
fn newly_wired_slash_commands_reach_real_handlers_not_the_llm() {
    // The 9 commands wired on 2026-06-01 were each a stub that forwarded the
    // literal slash command to the LLM. Driven through the REAL palette on a
    // live PTY, each must paint its OWN honest output — proof the command
    // reaches its handler end-to-end, not the LLM-forward path (which, with
    // the fake harness key, would error or echo the slash back rather than
    // render these strings). The home is a hermetic empty tempdir, so every
    // empty-state below is deterministic: no skills/MCP/hooks/profiles/
    // sessions, no `.git`, an empty tree to index.
    let home = TempDir::new().expect("tempdir");
    seed_config(home.path());
    let mut h = boot_to_workspace(home.path());

    // (palette filter word, a lowercase anchor only that command's real
    // handler emits). The anchors are state-independent: skills/MCP/hooks load
    // bundled extensions so they may be populated, hence the shared
    // header substring (`skills loaded` matches both "No skills loaded" and
    // "Skills loaded (N)"). The fresh tempdir guarantees the empty state for
    // sessions/profiles/checkpoints (D019: /rewind is checkpoint-backed, not
    // git — bare /rewind lists snapshots and the empty store says "No checkpoints").
    let cases = [
        ("skills", "skills loaded"),
        ("mcp", "mcp servers"),
        ("hooks", "hooks registered"),
        ("resume", "saved sessions"),
        ("profile", "profiles configured"),
        // NOTE: `/provider` is NOT in this text-anchor loop — it opens the
        // arrow-key picker OVERLAY (D022), not a text listing, so it is driven
        // separately after the loop (an open overlay would also swallow the
        // next iteration's `/`).
        ("replay", "--replay"),
        ("rewind", "no checkpoints"),
        ("repomap", "indexing the project"),
    ];

    for (word, expect) in cases {
        // `/` opens the palette; typing the full command word narrows it to
        // the single match; `Enter` runs it. (Full words disambiguate the
        // shared `re*` prefix: /resume /rewind /replay /repomap.)
        h.send(b"/");
        std::thread::sleep(Duration::from_millis(400));
        h.send(word.as_bytes());
        std::thread::sleep(Duration::from_millis(300));
        h.send(b"\r");
        // Let Enter run the command AND close the palette before polling.
        // Without this settle the first poll can catch the still-open palette
        // and match a command's *description* (e.g. /mcp's "manage MCP
        // servers") instead of its output — a false match that desyncs the
        // next command. Once the palette is closed, the anchor only matches
        // real handler output.
        std::thread::sleep(Duration::from_millis(500));
        h.wait_for(
            |s| s.to_lowercase().contains(expect),
            Duration::from_secs(6),
            &format!("/{word} to render its real handler output (`{expect}`)"),
        );
    }

    // Clean shutdown via the proven palette quit path.
    h.send(b"/");
    std::thread::sleep(Duration::from_millis(300));
    h.send(b"exit\r");
    let _ = h.wait_for_exit(Duration::from_secs(8));
}
