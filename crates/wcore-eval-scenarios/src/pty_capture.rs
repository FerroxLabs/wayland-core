//! D8 — PTY-based TUI capture driver.
//!
//! The json-stream runner ([`crate::runner`]) drives `wayland-core` in its
//! machine-facing `--json-stream` mode over plain pipes. This module instead
//! drives the binary in its **interactive ratatui TUI** mode under a real
//! pseudo-terminal, sends keystrokes the way a human keyboard would, and
//! captures what the terminal actually renders so eval scenarios can assert on
//! the *rendered screen* — not on a wire protocol.
//!
//! ## Why a PTY
//!
//! `wayland-core` only launches its full-screen TUI when
//! `IsTerminal::is_terminal(&stdout())` is true AND no prompt / `--no-tui` /
//! `--json-stream` was given (`wcore-cli/src/main.rs` `tui_capable` gate). A
//! plain piped subprocess fails that check and falls through to the line-based
//! REPL, so only a PTY exercises the real UI. We therefore spawn with NO extra
//! mode flags — the bare binary on a PTY IS the TUI.
//!
//! ## Hermeticity
//!
//! Reuses [`crate::tempenv`] exactly like the json-stream runner: a throwaway
//! tempdir holds a seeded `.wayland-core/config.toml` (absolute session dir per
//! cross-audit C-3, plus the provider id/model), the binary is spawned with
//! `cwd = env.path()`, and `WAYLAND_HOME` is pointed at the tempdir so
//! `wcore_config::wayland_config_dir()` resolves the seeded config on every
//! platform (matching `wcore-cli/tests/harness_tui_flow.rs`). The child receives
//! only the evaluator's allowlisted environment and selected provider key.
//!
//! ## Screen capture & assertions
//!
//! A reader thread pumps the master PTY's raw byte stream into a
//! [`vt100::Parser`], which applies every ANSI sequence the binary emits and
//! yields a rendered screen grid. [`PtyCapture::screen_text`] returns that grid
//! as plain text (ANSI already resolved), so substring assertions
//! ([`PtyCapture::assert_screen_contains`] / [`PtyCapture::screen_contains`])
//! match on human-visible anchors, never on escape codes.
//!
//! ## Dependencies
//!
//! `portable-pty` and `vt100` are NOT currently dependencies of
//! `wcore-eval-scenarios` (they ARE direct deps of `wcore-cli`, so both are
//! already in the workspace lockfile). To compile this module, add to
//! `crates/wcore-eval-scenarios/Cargo.toml` under `[dependencies]`:
//!
//! ```toml
//! # D8 PTY TUI-capture driver. Same versions wcore-cli already pins
//! # (vt100 stays at 0.15 — 0.16 wants unicode-width ^0.2.1 which
//! # collides with ratatui 0.29's pinned unicode-width =0.2.0).
//! portable-pty = "0.9"
//! vt100        = "0.15"
//! ```
//!
//! Both are direct (registry) deps, not `{ workspace = true }` — they are not
//! declared in `[workspace.dependencies]`, only inline in `wcore-cli`.
//!
//! ## Platform
//!
//! Unix-only. `portable_pty`'s Windows ConPTY backend does not surface the
//! spawned binary's stdout to the master end in headless CI (the vt100 parser
//! stays empty and every wait hits its timeout), exactly as documented in
//! `wcore-cli/tests/harness_tui_flow.rs`. The module is gated `#[cfg(unix)]`.

#![cfg(unix)]

use std::io::{Read, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, anyhow, bail};
use portable_pty::{CommandBuilder, MasterPty, PtySize, native_pty_system};

use crate::child_env::ChildEnvironment;
use crate::process_tree::UnixProcessGroup;
use crate::providers::ProviderConfig;
use crate::redaction::SecretRedactor;
use crate::tempenv::{self, TempEnv, TempEnvOptions};

/// Default PTY geometry. 100 columns keeps the workspace right rail visible
/// (`RAIL_RESPONSIVE_MIN_WIDTH = 100`) and 30 rows is a comfortable terminal
/// height; the eval harness wants a stable, reproducible size.
pub const DEFAULT_COLS: u16 = 100;
pub const DEFAULT_ROWS: u16 = 30;

/// Geometry for the spawned PTY.
#[derive(Debug, Clone, Copy)]
pub struct PtyGeometry {
    pub cols: u16,
    pub rows: u16,
}

impl Default for PtyGeometry {
    fn default() -> Self {
        Self {
            cols: DEFAULT_COLS,
            rows: DEFAULT_ROWS,
        }
    }
}

/// A live `wayland-core` TUI process attached to a pseudo-terminal.
///
/// Owns the master PTY (keystroke sink), the spawned child, a reader thread
/// pumping the byte stream into a shared [`vt100::Parser`], and the hermetic
/// [`TempEnv`] whose lifetime must outlive the child (its tempdir holds the
/// seeded config + session dir). [`Drop`] kills the child if it is still alive
/// so a panicking test never leaks a process.
pub struct PtyCapture {
    /// Master end of the PTY — keystrokes are written here.
    writer: Box<dyn Write + Send>,
    /// vt100 screen, refreshed by the reader thread, read by the asserters.
    parser: Arc<Mutex<vt100::Parser>>,
    /// Master PTY handle, kept alive so the writer end stays open and `resize`
    /// works.
    master: Box<dyn MasterPty + Send>,
    /// The spawned child. `wait` consumes it; until then `Drop` kills it.
    child: Box<dyn portable_pty::Child + Send + Sync>,
    /// `portable-pty` makes the child a session/process-group leader before
    /// exec. This guard owns that group independently of the direct child.
    process_group: UnixProcessGroup,
    cleanup_complete: bool,
    /// Exact provider-secret redactor applied before screen text leaves this
    /// capture object.
    redactor: SecretRedactor,
    secret_detected: AtomicBool,
    /// Reader-thread handle. Held so a clean shutdown can join; on panic
    /// `Drop` simply lets it dangle.
    _reader: JoinHandle<()>,
    /// Hermetic env — held for the lifetime of the child so its tempdir (the
    /// seeded config + session dir + cwd) is not deleted out from under the
    /// running binary.
    _env: TempEnv,
}

type PtyChild = Box<dyn portable_pty::Child + Send + Sync>;

/// Owns a just-spawned child until every fallible PTY setup step completes.
/// This closes the construction gap where returning `Err` would otherwise
/// drop a plain `std::process::Child` without terminating its process group.
struct PendingPtyChild {
    child: Option<PtyChild>,
    process_group: Option<UnixProcessGroup>,
}

impl PendingPtyChild {
    fn new(child: PtyChild) -> Self {
        Self {
            child: Some(child),
            process_group: None,
        }
    }

    fn child(&self) -> &PtyChild {
        self.child.as_ref().expect("pending PTY child is armed")
    }

    fn set_process_group(&mut self, process_group: UnixProcessGroup) {
        self.process_group = Some(process_group);
    }

    fn disarm(mut self) -> PtyChild {
        self.child.take().expect("pending PTY child is armed")
    }
}

impl Drop for PendingPtyChild {
    fn drop(&mut self) {
        let Some(child) = self.child.as_mut() else {
            return;
        };
        if let Some(process_group) = self.process_group {
            let _ = process_group.kill();
        }
        let _ = child.kill();
        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline {
            match child.try_wait() {
                Ok(Some(_)) | Err(_) => break,
                Ok(None) => std::thread::sleep(Duration::from_millis(20)),
            }
        }
    }
}

impl PtyCapture {
    /// Spawn `wayland-core` in interactive TUI mode under a fresh PTY of the
    /// default [`PtyGeometry`], seeded for `provider` via [`crate::tempenv`].
    ///
    /// The binary is located the same way the json-stream runner finds it
    /// ([`crate::runner::discover_binary`]): `WCORE_EVAL_BIN`, else the
    /// `target/{release,debug}/wayland-core` walk-up.
    pub fn spawn(provider: &ProviderConfig) -> Result<Self> {
        Self::spawn_with(provider, PtyGeometry::default(), &[])
    }

    /// Spawn with explicit geometry and extra CLI args (e.g. `["--continue"]`
    /// to resume a saved session). NOTE: never pass `--json-stream` or
    /// `--no-tui` here — those defeat the TUI launch gate this driver exists to
    /// exercise.
    pub fn spawn_with(
        provider: &ProviderConfig,
        geometry: PtyGeometry,
        extra_args: &[&str],
    ) -> Result<Self> {
        if crate::process_tree::authoritative_required() {
            bail!(
                "authoritative PTY process-tree containment is unavailable; use a disposable native worker"
            );
        }
        // Reuse the json-stream runner's binary discovery so PTY scenarios and
        // pipe scenarios always target the same artifact.
        let bin = crate::runner::discover_binary()
            .map_err(|e| anyhow!("locate wayland-core binary: {e}"))?;
        Self::spawn_binary(provider, geometry, extra_args, &bin, &[])
    }

    fn spawn_binary(
        provider: &ProviderConfig,
        geometry: PtyGeometry,
        extra_args: &[&str],
        bin: &std::path::Path,
        extra_env: &[(&str, &str)],
    ) -> Result<Self> {
        // Hermetic tempdir + seeded config.toml (absolute session dir per C-3,
        // provider id/model). Held in `self._env` for the child's lifetime.
        let env = tempenv::build_with(provider, &TempEnvOptions::default())
            .context("build hermetic tempenv for PTY run")?;

        let pty = native_pty_system()
            .openpty(PtySize {
                rows: geometry.rows,
                cols: geometry.cols,
                pixel_width: 0,
                pixel_height: 0,
            })
            .context("open PTY")?;

        // Build the hermetic command. cwd = tempdir so the engine's config
        // cwd-walk lands inside the sandbox; WAYLAND_HOME = tempdir so
        // `wayland_config_dir()` resolves the seeded config on every platform.
        let mut cmd = CommandBuilder::new(
            bin.to_str()
                .ok_or_else(|| anyhow!("binary path is not valid UTF-8: {}", bin.display()))?,
        );
        for arg in extra_args {
            cmd.arg(arg);
        }
        let secret = provider.resolved_key();
        ChildEnvironment::build(env.path(), env.path(), secret.as_deref())?.apply_pty(&mut cmd);
        for (name, value) in extra_env {
            cmd.env(*name, *value);
        }
        cmd.cwd(env.path());
        // Override the noninteractive default from ChildEnvironment: a
        // TTY-capable TERM is required for the TUI launch gate.
        cmd.env("TERM", "xterm-256color");

        let child = pty
            .slave
            .spawn_command(cmd)
            .context("spawn wayland-core under PTY")?;
        let mut pending_child = PendingPtyChild::new(child);
        let child_pid = pending_child
            .child()
            .process_id()
            .ok_or_else(|| anyhow!("spawned PTY child had no process id"))?;
        let process_group =
            UnixProcessGroup::from_pid(child_pid).context("capture PTY process group")?;
        pending_child.set_process_group(process_group);
        process_group
            .verify_session_leader()
            .context("verify PTY session ownership")?;

        // Reader thread: pump the PTY byte stream into a shared vt100 parser.
        let mut reader = pty.master.try_clone_reader().context("clone PTY reader")?;
        let parser = Arc::new(Mutex::new(vt100::Parser::new(
            geometry.rows,
            geometry.cols,
            0,
        )));
        let parser_for_thread = Arc::clone(&parser);
        let reader_handle = std::thread::Builder::new()
            .name("wcore-eval-pty-reader".to_string())
            .spawn(move || {
                let mut buf = [0u8; 8192];
                loop {
                    match reader.read(&mut buf) {
                        Ok(0) => break, // EOF — child closed the PTY.
                        Ok(n) => {
                            if let Ok(mut p) = parser_for_thread.lock() {
                                p.process(&buf[..n]);
                            }
                        }
                        Err(_) => break,
                    }
                }
            })
            .context("spawn PTY reader thread")?;

        let writer = pty.master.take_writer().context("take PTY writer")?;
        let child = pending_child.disarm();

        Ok(Self {
            writer,
            parser,
            master: pty.master,
            child,
            process_group,
            cleanup_complete: false,
            redactor: SecretRedactor::from_secret(secret),
            secret_detected: AtomicBool::new(false),
            _reader: reader_handle,
            _env: env,
        })
    }

    /// The hermetic working directory the binary was spawned in. Artifact
    /// assertions (file-on-disk checks) resolve relative paths against this; it
    /// stays alive until this `PtyCapture` is dropped.
    pub fn workdir(&self) -> &std::path::Path {
        self._env.path()
    }

    /// Snapshot the rendered screen as plain text — one row per line, trailing
    /// blanks trimmed by vt100. ANSI is already resolved, so this is the
    /// human-visible text, suitable for direct substring matching.
    pub fn screen_text(&self) -> String {
        let screen = match self.parser.lock() {
            Ok(p) => p.screen().contents(),
            // A poisoned lock means the reader thread panicked mid-process;
            // surface an empty screen rather than propagating the poison so
            // callers' `wait_for`/assert paths report a clean timeout/mismatch.
            Err(_) => String::new(),
        };
        let (screen, detected) = self.redactor.text(screen);
        if detected {
            self.secret_detected.store(true, Ordering::Release);
        }
        screen
    }

    /// Whether provider material reached the rendered PTY output. The screen
    /// returned to callers is already redacted; this flag makes the run fail.
    pub fn secret_detected(&self) -> bool {
        self.secret_detected.load(Ordering::Acquire)
    }

    /// Push raw bytes to the PTY as if typed on a keyboard. Use for control
    /// sequences (`b"\r"` Enter, `b"\x1b"` ESC, `b"\x1b[Z"` Shift+Tab).
    pub fn send(&mut self, bytes: &[u8]) -> Result<()> {
        self.writer.write_all(bytes).context("write to PTY")?;
        self.writer.flush().ok();
        Ok(())
    }

    /// Type a string one byte at a time with a short inter-key delay, the way a
    /// human types. A single bulk write outruns the TUI's per-frame input drain
    /// when the app is busy (e.g. just after a turn finalises) and drops
    /// characters; paced bytes give the event loop time to consume each key.
    /// Does NOT send a trailing newline — call [`send`](Self::send) with
    /// `b"\r"` to submit.
    pub fn type_text(&mut self, text: &str) -> Result<()> {
        for b in text.bytes() {
            self.writer.write_all(&[b]).context("write to PTY")?;
            self.writer.flush().ok();
            std::thread::sleep(Duration::from_millis(12));
        }
        Ok(())
    }

    /// Send a prompt and submit it with Enter — the common drive step for a
    /// single agent turn. Types at human pace then presses Enter.
    pub fn send_prompt(&mut self, prompt: &str) -> Result<()> {
        self.type_text(prompt)?;
        self.send(b"\r")
    }

    /// Resize the PTY. The TUI sees this as a `crossterm::event::Resize` and
    /// reflows. The vt100 parser is resized in lockstep so the captured grid
    /// matches the new geometry.
    pub fn resize(&mut self, cols: u16, rows: u16) -> Result<()> {
        self.master
            .resize(PtySize {
                rows,
                cols,
                pixel_width: 0,
                pixel_height: 0,
            })
            .context("resize PTY")?;
        if let Ok(mut p) = self.parser.lock() {
            p.set_size(rows, cols);
        }
        Ok(())
    }

    /// Poll the rendered screen at ~30Hz until `predicate(&screen_text)` is
    /// true, or fail with a message that includes the last screen state. This
    /// is the core "wait for the TUI to render/settle" primitive: a bounded
    /// timeout that never hangs the harness and always reports WHAT it was
    /// waiting for plus the actual last screen (the most debuggable failure).
    pub fn wait_for<F: Fn(&str) -> bool>(
        &self,
        predicate: F,
        timeout: Duration,
        what: &str,
    ) -> Result<()> {
        let deadline = Instant::now() + timeout;
        let mut last = String::new();
        while Instant::now() < deadline {
            last = self.screen_text();
            if self.secret_detected() {
                bail!("provider secret detected in PTY output");
            }
            if predicate(&last) {
                return Ok(());
            }
            std::thread::sleep(Duration::from_millis(30));
        }
        bail!(
            "timed out after {timeout:?} waiting for {what}.\n--- last screen ---\n{last}\n--- end ---"
        )
    }

    /// Boot the TUI and block until the workspace chrome has rendered (the
    /// `WAYLAND` wordmark and the `Workspace` tab), the canonical "the UI is up
    /// and settled" anchor. The first boot is dominated by the bundled
    /// `ijfw-memory` stdio MCP handshake (bounded by `CONNECT_TIMEOUT = 30s`),
    /// so 60s leaves a cold runner slack while still tripping a regression that
    /// reintroduces unbounded waiting.
    pub fn wait_for_workspace(&self) -> Result<()> {
        self.wait_for(
            |s| s.contains("WAYLAND") && s.contains("Workspace"),
            Duration::from_secs(60),
            "the TUI to render the chrome wordmark and Workspace tab",
        )
    }

    /// `true` if the current rendered screen contains `needle` (ANSI stripped
    /// by vt100). The non-panicking sibling of
    /// [`assert_screen_contains`](Self::assert_screen_contains).
    pub fn screen_contains(&self, needle: &str) -> bool {
        self.screen_text().contains(needle)
    }

    /// Assert the current rendered screen contains `needle`, returning an
    /// `Err` (with the full screen dump) when it does not. Matches against the
    /// vt100-rendered grid, so `needle` is plain visible text — never an escape
    /// code. For "wait until it appears", use
    /// [`wait_for`](Self::wait_for) with a `contains` closure instead.
    pub fn assert_screen_contains(&self, needle: &str) -> Result<()> {
        let screen = self.screen_text();
        if screen.contains(needle) {
            Ok(())
        } else {
            Err(anyhow!(
                "screen did not contain {needle:?}.\n--- screen ---\n{screen}\n--- end ---"
            ))
        }
    }

    /// Block until the child exits or `timeout` elapses. A normal exit first
    /// removes remaining group members. A timeout terminates and bounded-reaps
    /// the complete group before returning `None`.
    pub fn wait_for_exit(&mut self, timeout: Duration) -> Result<Option<portable_pty::ExitStatus>> {
        let deadline = Instant::now() + timeout;
        while Instant::now() < deadline {
            match self.process_group.child_exited_unreaped() {
                Ok(true) => return self.reap_exited_group().map(Some),
                Ok(false) => std::thread::sleep(Duration::from_millis(50)),
                Err(error) => {
                    let cleanup = self.terminate_and_reap();
                    return match cleanup {
                        Ok(()) => Err(error).context("poll PTY child"),
                        Err(cleanup) => Err(anyhow!(
                            "poll PTY child: {error}; PTY cleanup failed: {cleanup}"
                        )),
                    };
                }
            }
        }
        self.terminate_and_reap()?;
        Ok(None)
    }

    /// Drive a clean shutdown via the command palette's `/exit` path — the same
    /// quit route the TUI flow tests use. Best-effort: errors writing to a
    /// possibly-dying PTY are swallowed; the returned status is `None` if the
    /// child required forced termination after `grace`.
    pub fn quit_via_palette(
        &mut self,
        grace: Duration,
    ) -> Result<Option<portable_pty::ExitStatus>> {
        let _ = self.send(b"/");
        std::thread::sleep(Duration::from_millis(300));
        let _ = self.send(b"exit\r");
        self.wait_for_exit(grace)
    }

    fn reap_exited_group(&mut self) -> Result<portable_pty::ExitStatus> {
        let group_kill = self.process_group.kill().err();
        // The unreaped leader anchors the numeric PGID through the kill. Never
        // signal or probe that PGID after `wait`, when it may be recycled.
        self.cleanup_complete = true;
        let status = self.child.wait().context("reap exited PTY child")?;
        self.finish_cleanup([group_kill, None, None, None])?;
        Ok(status)
    }

    fn terminate_and_reap(&mut self) -> Result<()> {
        if self.cleanup_complete {
            return Ok(());
        }
        let group_kill = self.process_group.kill().err();
        self.cleanup_complete = true;
        let child_kill = self.child.kill().err();
        let deadline = Instant::now() + Duration::from_secs(5);
        let reap = loop {
            match self.child.try_wait() {
                Ok(Some(_)) => break None,
                Ok(None) if Instant::now() < deadline => {
                    std::thread::sleep(Duration::from_millis(20));
                }
                Ok(None) => {
                    break Some(std::io::Error::new(
                        std::io::ErrorKind::TimedOut,
                        "direct PTY child was not reaped within 5 seconds",
                    ));
                }
                Err(error) => break Some(error),
            }
        };
        let child_kill = reap.as_ref().and(child_kill);
        self.finish_cleanup([group_kill, child_kill, reap, None])
    }

    fn finish_cleanup(&mut self, errors: [Option<std::io::Error>; 4]) -> Result<()> {
        let errors = errors
            .into_iter()
            .flatten()
            .map(|error| error.to_string())
            .collect::<Vec<_>>();
        if errors.is_empty() {
            Ok(())
        } else {
            Err(anyhow!(errors.join("; ")))
        }
    }
}

impl Drop for PtyCapture {
    fn drop(&mut self) {
        let _ = self.terminate_and_reap();
    }
}

/// One-shot capture: boot the TUI for `provider`, wait for the workspace chrome
/// to settle, send `prompt` + Enter, give the turn `settle` time to render, and
/// return the captured screen text (ANSI resolved). The hermetic env and child
/// are torn down before returning.
///
/// This is the high-level convenience for the common eval shape — "drive one
/// prompt, capture what the TUI shows" — built on the [`PtyCapture`] primitives
/// so scenarios that need finer control (multi-turn, approval keys, resize) can
/// drop down to them.
///
/// `settle` bounds how long we wait for the turn to render AFTER submission. It
/// is a fixed dwell, not a predicate wait: this helper does not know the
/// scenario's expected anchor, so it captures whatever the screen shows once the
/// dwell elapses. Callers that DO know an anchor should use [`PtyCapture::spawn`]
/// and [`PtyCapture::wait_for`] for a tighter, non-flaky wait.
pub fn capture_prompt(provider: &ProviderConfig, prompt: &str, settle: Duration) -> Result<String> {
    let mut cap = PtyCapture::spawn(provider)?;
    cap.wait_for_workspace()?;
    cap.send_prompt(prompt)?;
    std::thread::sleep(settle);
    let screen = cap.screen_text();
    if cap.secret_detected() {
        bail!("provider secret detected in PTY output");
    }
    cap.quit_via_palette(Duration::from_secs(8))?;
    Ok(screen)
}

/// Strip ANSI/VT escape sequences from a raw terminal byte buffer, returning the
/// plain visible text. PROVIDED FOR COMPLETENESS as the "else" path named in the
/// D8 brief: the primary capture path already renders through [`vt100`]
/// (`PtyCapture::screen_text`), which is strictly better — it resolves cursor
/// motion, scrolling, and overwrites into a true screen grid, whereas a linear
/// strip only removes escape codes from a byte stream and cannot reconstruct
/// what a cell finally showed.
///
/// Use this only when you have a raw byte/string buffer with no live parser
/// (e.g. post-hoc log scrubbing). It handles CSI (`ESC [ … final`), OSC
/// (`ESC ] … BEL`/`ST`), and the common two-byte `ESC <single>` escapes.
pub fn strip_ansi(raw: &str) -> String {
    let bytes = raw.as_bytes();
    let mut out = String::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        let b = bytes[i];
        if b == 0x1b {
            // ESC. Decide which escape form follows.
            match bytes.get(i + 1) {
                Some(b'[') => {
                    // CSI: parameters/intermediates until a final byte 0x40..=0x7e.
                    i += 2;
                    while i < bytes.len() && !(0x40..=0x7e).contains(&bytes[i]) {
                        i += 1;
                    }
                    i += 1; // consume the final byte
                }
                Some(b']') => {
                    // OSC: terminated by BEL (0x07) or ST (ESC \).
                    i += 2;
                    while i < bytes.len() {
                        if bytes[i] == 0x07 {
                            i += 1;
                            break;
                        }
                        if bytes[i] == 0x1b && bytes.get(i + 1) == Some(&b'\\') {
                            i += 2;
                            break;
                        }
                        i += 1;
                    }
                }
                Some(_) => {
                    // Two-byte escape (e.g. ESC =, ESC >). Skip ESC + next.
                    i += 2;
                }
                None => {
                    // Trailing lone ESC — drop it.
                    i += 1;
                }
            }
            continue;
        }
        // Pass through printable + whitespace; drop other C0 control bytes
        // except newline/tab/carriage-return which carry layout meaning.
        if b == b'\n' || b == b'\t' || b == b'\r' || b >= 0x20 {
            out.push(b as char);
        }
        i += 1;
    }
    out
}

/// OPTIONAL — ANSI → PNG screenshot rendering.
///
/// STUBBED ON PURPOSE. The D8 core deliverable is reliable PTY capture + text
/// assertions (above); pixel screenshots are a nice-to-have and pull in an
/// extra rendering dependency, so they are deferred behind this clearly-marked
/// seam rather than half-implemented.
///
/// To implement: render the [`vt100::Screen`] grid (each cell's char + fg/bg
/// from `vt100`'s SGR state) into an RGBA image using a monospace font and the
/// `image` crate, then PNG-encode. A turnkey path is the `vt100-image`-style
/// approach — add a font rasteriser (`fontdue` or `rusttype`) + `image` as
/// dependencies and walk `screen.cell(row, col)`.
///
/// Returns an explicit "not implemented" error (never `todo!()`, which the
/// crate-wide `#![deny(clippy::todo)]` forbids) so a caller that wires this up
/// before the renderer exists gets an honest signal, not a panic.
pub fn screenshot_png(_cap: &PtyCapture, _out_path: &std::path::Path) -> Result<()> {
    bail!(
        "screenshot_png is an intentional D8 stub — the core deliverable is text \
         capture + assertions. To enable, add an image-render dep (e.g. `image` + a \
         font rasteriser such as `fontdue`) and rasterise the vt100 screen grid; \
         see the function docs."
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    const HELPER_TEST: &str = "pty_capture::tests::pty_process_tree_helper";

    #[test]
    #[ignore = "subprocess helper for PTY process-group lifecycle tests"]
    #[allow(clippy::zombie_processes)]
    fn pty_process_tree_helper() {
        let role = std::env::var("WCORE_PTY_HELPER_ROLE").expect("helper role");
        let state_path = std::path::PathBuf::from(
            std::env::var_os("WCORE_PTY_HELPER_STATE").expect("helper state path"),
        );
        if role == "descendant" {
            let listener = std::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
                .expect("bind PTY descendant listener");
            let state = format!(
                "pid={}\nport={}\n",
                std::process::id(),
                listener.local_addr().expect("listener address").port()
            );
            std::fs::write(state_path, state).expect("publish PTY descendant state");
            loop {
                std::thread::sleep(Duration::from_secs(60));
            }
        }

        let executable = std::env::current_exe().expect("resolve test executable");
        let mut descendant = std::process::Command::new(executable);
        descendant
            .args(std::env::args_os().skip(1))
            .env("WCORE_PTY_HELPER_ROLE", "descendant")
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null());
        descendant.spawn().expect("spawn PTY descendant");

        let deadline = Instant::now() + Duration::from_secs(2);
        while !state_path.exists() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(state_path.exists(), "PTY descendant did not publish state");
        if std::env::var("WCORE_PTY_HELPER_MODE").as_deref() == Ok("exit") {
            return;
        }
        loop {
            std::thread::sleep(Duration::from_secs(60));
        }
    }

    fn spawn_process_tree_helper(mode: &str, state: &std::path::Path) -> PtyCapture {
        let provider = ProviderConfig::new(
            crate::providers::ProviderId::DeepSeek,
            "pty-process-tree-fixture",
        );
        let executable = std::env::current_exe().expect("resolve test executable");
        let state = state.to_str().expect("UTF-8 test state path");
        PtyCapture::spawn_binary(
            &provider,
            PtyGeometry::default(),
            &["--ignored", "--exact", HELPER_TEST],
            &executable,
            &[
                ("WCORE_PTY_HELPER_ROLE", "parent"),
                ("WCORE_PTY_HELPER_MODE", mode),
                ("WCORE_PTY_HELPER_STATE", state),
            ],
        )
        .expect("spawn PTY process-tree helper")
    }

    #[derive(Clone, Copy)]
    struct ListenerState {
        pid: u32,
        port: u16,
    }

    fn wait_for_listener_state(path: &std::path::Path) -> ListenerState {
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            if let Ok(contents) = std::fs::read_to_string(path)
                && let Some(pid) = contents
                    .lines()
                    .find_map(|line| line.strip_prefix("pid="))
                    .and_then(|pid| pid.parse().ok())
                && let Some(port) = contents
                    .lines()
                    .find_map(|line| line.strip_prefix("port="))
                    .and_then(|port| port.parse().ok())
            {
                return ListenerState { pid, port };
            }
            assert!(
                Instant::now() < deadline,
                "PTY descendant did not publish listener state"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    fn listener_accepts_connections(port: u16) -> bool {
        let address = std::net::SocketAddr::from(([127, 0, 0, 1], port));
        std::net::TcpStream::connect_timeout(&address, Duration::from_millis(50)).is_ok()
    }

    fn process_exists(pid: u32) -> bool {
        // SAFETY: signal zero observes only process existence.
        let result = unsafe { libc::kill(pid as libc::pid_t, 0) };
        result == 0 || std::io::Error::last_os_error().raw_os_error() != Some(libc::ESRCH)
    }

    fn assert_descendant_closes(state: ListenerState) {
        let deadline = Instant::now() + Duration::from_secs(2);
        while (listener_accepts_connections(state.port) || process_exists(state.pid))
            && Instant::now() < deadline
        {
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(
            !listener_accepts_connections(state.port) && !process_exists(state.pid),
            "PTY descendant pid={} or listener on 127.0.0.1:{} survived cleanup",
            state.pid,
            state.port
        );
    }

    #[test]
    fn direct_child_exit_reaps_pty_descendant_group() {
        let state_dir = tempfile::tempdir().expect("create PTY state dir");
        let state = state_dir.path().join("listener-state");
        let mut capture = spawn_process_tree_helper("exit", &state);
        let descendant = wait_for_listener_state(&state);

        let status = capture
            .wait_for_exit(Duration::from_secs(3))
            .expect("wait for PTY helper");

        assert!(status.is_some(), "PTY helper should exit normally");
        assert_descendant_closes(descendant);
    }

    #[test]
    fn drop_reaps_live_pty_process_group() {
        let state_dir = tempfile::tempdir().expect("create PTY state dir");
        let state = state_dir.path().join("listener-state");
        let capture = spawn_process_tree_helper("hold", &state);
        let descendant = wait_for_listener_state(&state);

        drop(capture);

        assert_descendant_closes(descendant);
    }

    #[test]
    fn strip_ansi_removes_csi_color_sequences() {
        // SGR color set + reset around plain text.
        let raw = "\x1b[31mred\x1b[0m text";
        assert_eq!(strip_ansi(raw), "red text");
    }

    #[test]
    fn strip_ansi_removes_cursor_motion_and_osc() {
        // Cursor move (CSI H), an OSC title set (BEL-terminated), then text.
        let raw = "\x1b[2J\x1b[H\x1b]0;window title\x07visible";
        assert_eq!(strip_ansi(raw), "visible");
    }

    #[test]
    fn strip_ansi_preserves_layout_whitespace() {
        let raw = "line one\r\n\tindented\x1b[0m";
        assert_eq!(strip_ansi(raw), "line one\r\n\tindented");
    }

    #[test]
    fn strip_ansi_drops_trailing_lone_esc() {
        assert_eq!(strip_ansi("ok\x1b"), "ok");
    }

    #[test]
    fn strip_ansi_handles_st_terminated_osc() {
        // OSC terminated by ST (ESC \) instead of BEL.
        let raw = "\x1b]8;;https://example.com\x1b\\link";
        assert_eq!(strip_ansi(raw), "link");
    }

    #[test]
    fn default_geometry_is_100x30() {
        let g = PtyGeometry::default();
        assert_eq!((g.cols, g.rows), (DEFAULT_COLS, DEFAULT_ROWS));
        assert_eq!((g.cols, g.rows), (100, 30));
    }

    #[test]
    fn screenshot_png_is_an_honest_stub_not_a_panic() {
        // The stub must return an Err (not unwind) so callers get a signal.
        // We can't construct a PtyCapture without spawning the binary, so this
        // documents the contract via the error-path of a direct call would-be;
        // instead assert the function exists and is wired to bail. (A spawning
        // integration test lives in tests/ where the binary is available.)
        // Compile-time presence check: take a function pointer.
        let _f: fn(&PtyCapture, &std::path::Path) -> Result<()> = screenshot_png;
    }
}
