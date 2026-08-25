//! Shared PTY harness for the wcore-cli integration test suite.
//!
//! Exposes [`Pty`], [`boot`], [`write_config`], [`harden_child_env`], and
//! [`STRIPPED_PROVIDER_ENV`] so multiple integration test binaries can share the
//! same hermetic harness without copy-paste.
//!
//! `Pty` and `boot` are Unix-only (`portable_pty` ConPTY cannot surface stdout
//! in headless GHA runners — see the module doc in `smoke_p0.rs`).
//! `write_config`, `harden_child_env`, and `STRIPPED_PROVIDER_ENV` are
//! cross-platform.
//!
//! This is a shared support module included into multiple integration test
//! binaries.  Each binary uses only a subset of the items, so dead-code
//! warnings per-binary are expected and suppressed here.
#![allow(dead_code)]

use std::path::Path;

// ===========================================================================
// Cross-platform helpers (no #[cfg(unix)] guard).
// ===========================================================================

/// Seed `<home>/config.toml` for a provider/model, optionally routing the
/// provider `base_url` at a local mock. `model: None` writes NO model line —
/// the exact catalog-provider shape the D002 GAP check needs.
pub fn write_config(home: &Path, provider: &str, model: Option<&str>, base_url: Option<&str>) {
    let mut toml = format!("[default]\nprovider = \"{provider}\"\n");
    if let Some(m) = model {
        toml.push_str(&format!("model = \"{m}\"\n"));
    }
    toml.push_str(&format!(
        "\n[providers.{provider}]\napi_key = \"sk-ant-harness-not-real-key-0000000000\"\n"
    ));
    if let Some(url) = base_url {
        toml.push_str(&format!("base_url = \"{url}\"\n"));
    }
    std::fs::write(home.join("config.toml"), toml).expect("write config.toml");
}

/// The full provider-credential env-var set every spawned child must NOT
/// inherit, so a run can neither read the developer's real keys nor have
/// onboarding auto-detect a stray dev credential. ONE source of truth used by
/// `run_headless`, the PTY spawn, and the `--json-stream` child — keeps the
/// strip set honest and uniform (M6). `AWS_*` / `VERTEX*` are stripped by name
/// (the concrete vars Bedrock/Vertex auth reads), not by glob.
pub const STRIPPED_PROVIDER_ENV: &[&str] = &[
    "API_KEY",
    // Core providers.
    "ANTHROPIC_API_KEY",
    "OPENAI_API_KEY",
    "AZURE_OPENAI_API_KEY",
    "GEMINI_API_KEY",
    "GOOGLE_API_KEY",
    "OPENROUTER_API_KEY",
    "DEEPSEEK_API_KEY",
    "GROQ_API_KEY",
    "XAI_API_KEY",
    "MISTRAL_API_KEY",
    "COHERE_API_KEY",
    "PERPLEXITY_API_KEY",
    "CEREBRAS_API_KEY",
    "TOGETHER_API_KEY",
    "FIREWORKS_API_KEY",
    "NVIDIA_API_KEY",
    "FLUX_API_KEY",
    "MOONSHOT_API_KEY",
    "DASHSCOPE_API_KEY",
    "ALIBABA_API_KEY",
    "MINIMAX_API_KEY",
    // Token-style credentials (not _API_KEY suffix).
    "REPLICATE_API_TOKEN",
    "HF_TOKEN",
    "HUGGINGFACE_API_KEY",
    "HUGGING_FACE_HUB_TOKEN",
    // AWS (Bedrock) — concrete vars the provider auth chain reads.
    "AWS_ACCESS_KEY_ID",
    "AWS_SECRET_ACCESS_KEY",
    "AWS_SESSION_TOKEN",
    "AWS_PROFILE",
    "AWS_REGION",
    "AWS_DEFAULT_REGION",
    // Google Vertex.
    "VERTEX_PROJECT",
    "VERTEX_LOCATION",
    "GOOGLE_APPLICATION_CREDENTIALS",
];

/// Apply the hermetic child env uniformly: point `WAYLAND_HOME` + `HOME` at the
/// throwaway tempdir, set a deterministic `TERM`, and strip every credential in
/// [`STRIPPED_PROVIDER_ENV`]. The single place that defines "hermetic child
/// env" so the headless / PTY / json-stream spawns can never drift apart (M6).
pub fn harden_child_env(cmd: &mut std::process::Command, home: &Path) {
    cmd.env("WAYLAND_HOME", home)
        .env("HOME", home)
        // Headless / json-stream children get a deterministic non-TTY term. The
        // PTY spawn (which needs a real terminal type) sets its own TERM and
        // does NOT route through this helper.
        .env("TERM", "dumb");
    for key in STRIPPED_PROVIDER_ENV {
        cmd.env_remove(key);
    }
}

// ===========================================================================
// PTY harness — Unix only.
// ===========================================================================

#[cfg(unix)]
use std::io::{Read, Write};
#[cfg(unix)]
use std::time::{Duration, Instant};

#[cfg(unix)]
use portable_pty::{CommandBuilder, MasterPty, PtySize, native_pty_system};

/// Path to the debug binary under test (Cargo wires this env var).
#[cfg(unix)]
fn binary() -> &'static str {
    env!("CARGO_BIN_EXE_wayland-core")
}

/// A minimal PTY harness — the proven shape from `harness_tui_flow.rs`,
/// re-derived here because integration test files compile as separate
/// binaries and cannot share a non-`support` module.
#[cfg(unix)]
pub struct Pty {
    writer: Box<dyn Write + Send>,
    parser: std::sync::Arc<std::sync::Mutex<vt100::Parser>>,
    _master: Box<dyn MasterPty + Send>,
    child: Box<dyn portable_pty::Child + Send + Sync>,
    _reader: std::thread::JoinHandle<()>,
    /// FerroxLabs/wayland#1109. What the reader thread has managed to do.
    /// When a `wait_for` gives up, the last 40-row screen alone cannot say
    /// whether the child stopped emitting or kept painting; those are
    /// different components, and that issue burned two CI runs failing to
    /// tell them apart.
    reader_stats: std::sync::Arc<std::sync::Mutex<ReaderStats>>,
    /// Every `wait_for` already SATISFIED on this terminal. The speed of the
    /// prefix is what separates process-wide starvation from a stall specific
    /// to one awaited thing — see [`timeout_report`].
    steps: std::sync::Mutex<Vec<StepTiming>>,
    /// The master side, kept for the `FIONREAD` probe in [`Pty::pending_bytes`].
    /// `None` on a backend that does not expose one, which is not a failure —
    /// the probe simply reports "unknown" and nothing is extended on it.
    master_fd: Option<std::os::fd::RawFd>,
}

/// What the PTY reader thread has managed to do so far.
#[cfg(unix)]
#[derive(Default)]
pub struct ReaderStats {
    pub bytes: u64,
    pub reads: u64,
    pub last_read: Option<Instant>,
    /// The read loop ended — EOF or an error on the master side, which on
    /// every platform this harness runs on means the child is gone.
    pub eof: bool,
}

/// One satisfied `wait_for`, kept so a later timeout can print the timeline.
#[cfg(unix)]
#[derive(Clone, Debug)]
pub struct StepTiming {
    pub what: String,
    pub elapsed: Duration,
    pub budget: Duration,
}

#[cfg(unix)]
impl Pty {
    pub fn spawn(home: &Path) -> Self {
        Self::spawn_sized(home, 40, 120)
    }

    /// Spawn the binary against `home` with an explicit terminal size.
    /// Used by the Proving Ground cell runner so each cell can declare its
    /// own `TermShape` (e.g. narrow columns for wrapping tests).
    pub fn spawn_sized(home: &Path, rows: u16, cols: u16) -> Self {
        let no_extra: &[(&str, &str)] = &[];
        Self::spawn_with_env(home, rows, cols, no_extra)
    }

    /// Spawn the binary against `home` with explicit terminal size and
    /// additional environment variable overrides injected into the child.
    /// Used by `run_cell` when `ConfigState::EnvKeysOnly` needs to inject
    /// `OPENAI_API_KEY` without writing a config file.
    pub fn spawn_with_env(
        home: &Path,
        rows: u16,
        cols: u16,
        extra_env: &[(impl AsRef<str>, impl AsRef<str>)],
    ) -> Self {
        let no_args: &[&str] = &[];
        Self::spawn_with_args_env(home, home, rows, cols, no_args, extra_env)
    }

    /// Spawn the binary against `home` with explicit terminal size, explicit
    /// ARGV, and additional environment overrides.
    ///
    /// The argv seam exists because a real terminal is not only the TUI's
    /// requirement: `wcore_agent::confirm::ToolConfirmer::check_for` denies any
    /// tool call that needs confirmation when `stdin` is not a terminal, so a
    /// `--no-tui` one-shot run driven over pipes can never execute a gated tool
    /// call at all. Driving the headless surface on a PTY is the only way to
    /// give that surface the approval channel a user at a terminal actually has.
    /// `cwd` is separate from `home` because the two are separate authorities:
    /// `home` is `WAYLAND_HOME` (config, sessions, durable state) while `cwd` is
    /// the repository the session governs. An isolated-mutation child's checkout
    /// root is derived under the session directory, and
    /// `WorktreeManager::new_with_workspace_root` refuses when that root's
    /// parent overlaps the repository — so a run in which the two are the same
    /// directory can never create a mutating child at all.
    pub fn spawn_with_args_env(
        home: &Path,
        cwd: &Path,
        rows: u16,
        cols: u16,
        args: &[impl AsRef<str>],
        extra_env: &[(impl AsRef<str>, impl AsRef<str>)],
    ) -> Self {
        let pty = native_pty_system()
            .openpty(PtySize {
                rows,
                cols,
                pixel_width: 0,
                pixel_height: 0,
            })
            .expect("open PTY");

        let mut cmd = CommandBuilder::new(binary());
        for arg in args {
            cmd.arg(arg.as_ref());
        }
        cmd.env("HOME", home);
        cmd.env("WAYLAND_HOME", home);
        // The TUI needs a real terminal type (not "dumb") to render; the
        // hermetic key-strip set is shared with the headless/json-stream
        // spawns via STRIPPED_PROVIDER_ENV (M6).
        cmd.env("TERM", "xterm-256color");
        for key in STRIPPED_PROVIDER_ENV {
            cmd.env_remove(key);
        }
        // Apply caller-supplied env overrides (e.g. OPENAI_API_KEY for EnvKeysOnly).
        // These are applied AFTER the strip pass so they intentionally survive
        // the hermetic strip (they are test-supplied, not developer credentials).
        for (k, v) in extra_env {
            cmd.env(k.as_ref(), v.as_ref());
        }
        cmd.cwd(cwd);
        let child = pty.slave.spawn_command(cmd).expect("spawn wayland-core");

        let mut reader = pty.master.try_clone_reader().expect("clone PTY reader");
        let parser = std::sync::Arc::new(std::sync::Mutex::new(vt100::Parser::new(
            rows, cols, 10_000,
        )));
        let parser_for_thread = std::sync::Arc::clone(&parser);
        let reader_stats = std::sync::Arc::new(std::sync::Mutex::new(ReaderStats::default()));
        let stats_for_thread = std::sync::Arc::clone(&reader_stats);
        let reader_handle = std::thread::spawn(move || {
            let mut buf = [0u8; 8192];
            loop {
                match reader.read(&mut buf) {
                    Ok(0) => {
                        if let Ok(mut s) = stats_for_thread.lock() {
                            s.eof = true;
                        }
                        break;
                    }
                    Ok(n) => {
                        // Liveness is recorded BEFORE the parse: a starved
                        // reader and a silent child differ only in whether
                        // bytes are still arriving, and #1109 needs that
                        // answer even when the vt100 parse is the slow part.
                        if let Ok(mut s) = stats_for_thread.lock() {
                            s.bytes = s.bytes.saturating_add(n as u64);
                            s.reads = s.reads.saturating_add(1);
                            s.last_read = Some(Instant::now());
                        }
                        if let Ok(mut p) = parser_for_thread.lock() {
                            p.process(&buf[..n]);
                        }
                    }
                    Err(_) => {
                        if let Ok(mut s) = stats_for_thread.lock() {
                            s.eof = true;
                        }
                        break;
                    }
                }
            }
        });

        let master_fd = pty.master.as_raw_fd();
        let writer = pty.master.take_writer().expect("take PTY writer");
        Self {
            writer,
            parser,
            _master: pty.master,
            child,
            _reader: reader_handle,
            reader_stats,
            steps: std::sync::Mutex::new(Vec::new()),
            master_fd,
        }
    }

    /// FerroxLabs/wayland#1126 — does this TUI produce terminal scrollback AT
    /// ALL? The proposed harness fix (give the parser scrollback, read the full
    /// buffer) is only buildable if the answer is yes. A full-screen
    /// alternate-screen app repaints in place, and content that falls outside a
    /// widget's own viewport is never emitted, so it would never reach
    /// scrollback either. Measure instead of assuming.
    pub fn scrollback_probe(&self) -> String {
        let Ok(mut parser) = self.parser.lock() else {
            return "parser lock poisoned".to_string();
        };
        let mut out = String::new();
        for offset in [0usize, 1, 5, 40] {
            parser.set_scrollback(offset);
            let contents = parser.screen().contents();
            let non_blank = contents.lines().filter(|l| !l.trim().is_empty()).count();
            out.push_str(&format!(
                "scrollback offset {offset}: {} bytes, {non_blank} non-blank rows\n",
                contents.len()
            ));
        }
        parser.set_scrollback(0);
        out
    }

    /// The OS pid of the child this harness drives, when the backend exposes
    /// one. FerroxLabs/wayland#1126 needs it to take an OS-level stack sample of
    /// a stalled child: no reading available INSIDE this process can distinguish
    /// "the child is blocked" from "the child answered and we did not see it".
    pub fn child_pid(&self) -> Option<u32> {
        self.child.process_id()
    }

    /// Bytes the child has written that NOTHING in this harness has read yet,
    /// via `FIONREAD` on the master. `None` when there is no fd to ask.
    ///
    /// This is the fact that separates "the child went quiet" from "this
    /// harness fell behind the child" — the two readings #1109 could not tell
    /// apart, because both look identical on a stale screen.
    pub fn pending_bytes(&self) -> Option<u64> {
        let fd = self.master_fd?;
        let mut n: libc::c_int = 0;
        // SAFETY: `fd` is the live master end of a pty this struct owns, and
        // `FIONREAD` writes one `c_int` through the pointer we hand it.
        let rc = unsafe { libc::ioctl(fd, libc::FIONREAD, &mut n) };
        if rc == -1 {
            None
        } else {
            Some(n.max(0) as u64)
        }
    }

    pub fn screen_text(&self) -> String {
        let parser = self.parser.lock().expect("parser lock");
        parser.screen().contents()
    }

    pub fn wait_for<F: Fn(&str) -> bool>(&self, predicate: F, timeout: Duration, what: &str) {
        self.wait_for_ctx(predicate, timeout, what, String::new);
    }

    /// `wait_for` plus a caller-supplied diagnostic, evaluated ONLY on the
    /// timeout path.
    ///
    /// FerroxLabs/wayland#1109 asked for the boundary tests to be instrumented
    /// so a CI timeout names the component that stalled. The one fact this
    /// harness cannot see for itself is what the mock PROVIDER received, which
    /// separates "the engine never dispatched the turn" from "the provider
    /// answered and the answer never reached the screen". The caller knows
    /// that; this seam lets it say so at the moment of failure.
    pub fn wait_for_ctx<F: Fn(&str) -> bool, C: Fn() -> String>(
        &self,
        predicate: F,
        timeout: Duration,
        what: &str,
        context: C,
    ) {
        let started = Instant::now();
        let deadline = started + timeout;
        // How many times this thread actually got to LOOK. Against the
        // `timeout / POLL_INTERVAL` it was entitled to, this measures whether
        // the harness thread itself was being scheduled — the reading #1109
        // needed and could not take.
        let mut polls = 0_u64;
        let mut last;
        loop {
            last = self.screen_text();
            polls += 1;
            if predicate(&last) {
                self.record_step(what, started.elapsed(), timeout);
                return;
            }
            if Instant::now() >= deadline {
                break;
            }
            std::thread::sleep(POLL_INTERVAL);
        }

        // #1109 GRACE. The deadline can expire while output the child ALREADY
        // wrote is still queued in the pty — the screen this loop has been
        // polling is then simply OLD, and failing on it reports our own
        // observation lag as the product's stall. Extend only on EVIDENCE of
        // unobserved output: bytes pending in the kernel buffer, or bytes that
        // landed moments ago. A genuinely silent child satisfies neither, so
        // this is not a budget increase for a stalled product — it is bounded
        // by GRACE_CAP either way.
        let grace_started = Instant::now();
        let mut extended = false;
        while should_extend(
            self.pending_bytes(),
            self.quiet_for(),
            grace_started.elapsed(),
            GRACE_CAP,
        ) {
            extended = true;
            std::thread::sleep(POLL_INTERVAL);
            last = self.screen_text();
            polls += 1;
            if predicate(&last) {
                let waited = started.elapsed();
                // Loud on purpose: a wait that only passed because the screen
                // was stale is a scheduling fact worth keeping, not a silent
                // rescue.
                eprintln!(
                    "[pty#1109] {what}: satisfied {:?} PAST its {timeout:?} budget, after \
                     draining output already written by the child. The screen was STALE, not \
                     the child.",
                    waited.saturating_sub(timeout)
                );
                self.record_step(what, waited, timeout);
                return;
            }
        }

        let (bytes, reads, last_read_age, eof) = self.reader_liveness();
        let steps = self
            .steps
            .lock()
            .map(|s| s.clone())
            .unwrap_or_else(|p| p.into_inner().clone());
        panic!(
            "{}",
            timeout_report(&TimeoutFacts {
                what,
                timeout,
                steps: &steps,
                polls,
                bytes,
                reads,
                last_read_age,
                pending: self.pending_bytes(),
                eof,
                extended,
                context: &context(),
                screen: &last,
            })
        );
    }

    fn record_step(&self, what: &str, elapsed: Duration, budget: Duration) {
        if let Ok(mut steps) = self.steps.lock() {
            steps.push(StepTiming {
                what: what.to_owned(),
                elapsed,
                budget,
            });
        }
    }

    /// How long since the newest byte arrived. `Duration::MAX` when none ever
    /// did, so "never spoke" and "long silent" fall on the same side of every
    /// comparison.
    fn quiet_for(&self) -> Duration {
        match self.reader_stats.lock() {
            Ok(s) => s.last_read.map_or(Duration::MAX, |t| t.elapsed()),
            Err(_) => Duration::MAX,
        }
    }

    /// `(bytes, reads, age of the newest byte, reader ended)`.
    fn reader_liveness(&self) -> (u64, u64, Option<Duration>, bool) {
        match self.reader_stats.lock() {
            Ok(s) => (s.bytes, s.reads, s.last_read.map(|t| t.elapsed()), s.eof),
            Err(_) => (0, 0, None, false),
        }
    }

    /// Type at the terminal.
    ///
    /// A PTY master write fails with `EIO` once no process holds the slave
    /// open, and on macOS that lands the instant the child exits. Two callers
    /// race it by design: `answer_approval_prompts` runs its full budget with
    /// `stop_on_exit: false` and keeps typing `y\r` after the TUI has gone,
    /// and `quit` sends `exit\r` 300ms after `/` — by which time the child may
    /// already have quit for its own reasons. Both are the desired end state,
    /// not a fault, so `corpus_fan_out` panicking here was the harness
    /// reporting a race as a failure.
    ///
    /// The tolerance is deliberately narrow. A write that fails while the
    /// child is STILL RUNNING is a real defect and still panics — blanket
    /// `.ok()` here would make every drive step in this harness vacuous, since
    /// a test could "type" at a terminal that never received a byte and still
    /// pass. A tolerated write is also not a silent pass: nothing was
    /// delivered, so any `wait_for` that depended on it still fails, with its
    /// screen dump intact.
    pub fn send(&mut self, bytes: &[u8]) {
        if let Err(e) = self.writer.write_all(bytes) {
            assert!(
                self.child_has_exited(),
                "write to PTY failed while the child was STILL RUNNING: {e}"
            );
            return;
        }
        self.writer.flush().ok();
    }

    /// Has the child gone? Bounded rather than a single `try_wait`: the failed
    /// write can beat reaping by a few milliseconds, and reading "not yet
    /// exited" in that window would turn the race back into the panic this
    /// exists to remove.
    fn child_has_exited(&mut self) -> bool {
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            match self.child.try_wait() {
                Ok(Some(_)) => return true,
                // The child cannot be observed at all, so it cannot be
                // asserted to be running.
                Err(_) => return true,
                Ok(None) if Instant::now() >= deadline => return false,
                Ok(None) => std::thread::sleep(Duration::from_millis(20)),
            }
        }
    }

    pub fn wait_for_exit(&mut self, timeout: Duration) -> Option<portable_pty::ExitStatus> {
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

    /// Clean shutdown via the proven palette quit path.
    pub fn quit(&mut self) {
        self.send(b"/");
        std::thread::sleep(Duration::from_millis(300));
        self.send(b"exit\r");
        let _ = self.wait_for_exit(Duration::from_secs(8));
    }
}

#[cfg(unix)]
impl Drop for Pty {
    fn drop(&mut self) {
        if let Ok(None) = self.child.try_wait() {
            let _ = self.child.kill();
        }
    }
}

/// Boot the TUI to the Workspace surface (chrome wordmark + tab painted).
#[cfg(unix)]
pub fn boot(home: &Path) -> Pty {
    let h = Pty::spawn(home);
    h.wait_for(
        |s| s.contains("WAYLAND") && s.contains("Workspace"),
        Duration::from_secs(60),
        "TUI to render the chrome wordmark and Workspace tab",
    );
    h
}

// ===========================================================================
// Timeout diagnostics — FerroxLabs/wayland#1109.
//
// #1109 spent two CI runs, and this file's own `threads-required` override in
// `.config/nextest.toml`, on a question the harness could not answer: when a
// PTY wait times out, WHICH component stopped — the child, the reader thread,
// or the test thread? The captured 40-row screen is the same picture in all
// three cases. Everything below exists to make the failure say which.
// ===========================================================================

/// How often a wait looks at the screen. Named because the diagnostic divides
/// by it to work out how many looks the wait was ENTITLED to.
#[cfg(unix)]
const POLL_INTERVAL: Duration = Duration::from_millis(30);

/// The hard ceiling on the post-deadline drain. Bounded so a stuck product can
/// never turn one wait into an unbounded one.
#[cfg(unix)]
const GRACE_CAP: Duration = Duration::from_secs(3);

/// A terminal quiet for longer than this has stopped emitting, for the
/// purposes of both the drain and the verdict. Comfortably longer than one
/// poll interval and than a vt100 repaint.
#[cfg(unix)]
const QUIET_WINDOW: Duration = Duration::from_millis(400);

/// A wait whose satisfied prefix used at most 1/`PREFIX_FAST_DIVISOR` of its
/// own budget was running at normal speed. A fifth is deliberately generous:
/// the PTY tests satisfy every prefix wait in about a second against 10-60s
/// budgets when unloaded.
#[cfg(unix)]
const PREFIX_FAST_DIVISOR: u32 = 5;

/// A wait that got at least this fraction of the looks it was entitled to was
/// being scheduled. Two thirds tolerates ordinary jitter while still catching
/// a thread that spent most of the budget off-CPU.
#[cfg(unix)]
const SCHEDULED_NUMERATOR: u64 = 2;
#[cfg(unix)]
const SCHEDULED_DENOMINATOR: u64 = 3;

/// Should a wait be extended past its deadline?
///
/// The whole point is the NEGATIVE case: a silent terminal returns `false`
/// immediately, so a stalled product still fails on its stated budget and this
/// is not a timeout increase. `true` requires positive evidence that the child
/// produced output this harness has not yet rendered.
#[cfg(unix)]
pub fn should_extend(
    pending: Option<u64>,
    quiet_for: Duration,
    extended_for: Duration,
    cap: Duration,
) -> bool {
    if extended_for >= cap {
        return false;
    }
    // Unread bytes in the kernel buffer: the child spoke and we have not
    // listened. Unambiguous.
    if pending.is_some_and(|n| n > 0) {
        return true;
    }
    // Bytes landed a moment ago: the parse may be mid-flight, or more is
    // coming. `Duration::MAX` (never spoke) fails this, as intended.
    quiet_for < QUIET_WINDOW
}

/// Everything known at the moment a `wait_for` gives up.
#[cfg(unix)]
pub struct TimeoutFacts<'a> {
    pub what: &'a str,
    pub timeout: Duration,
    /// The waits already satisfied on this terminal, in order.
    pub steps: &'a [StepTiming],
    /// How many times the waiting thread actually looked at the screen.
    pub polls: u64,
    pub bytes: u64,
    pub reads: u64,
    /// Age of the newest byte read off the master; `None` if none ever came.
    pub last_read_age: Option<Duration>,
    /// Unread bytes in the pty buffer; `None` when unknowable.
    pub pending: Option<u64>,
    pub eof: bool,
    /// Whether the post-deadline drain ran at all.
    pub extended: bool,
    pub context: &'a str,
    pub screen: &'a str,
}

/// Render the timeout diagnostic.
///
/// Pure on purpose: the classification it draws is the deliverable, so it has
/// to be gradeable without provoking a real 30s stall. It answers three
/// questions in order, and the three together name the component:
///
/// 1. **Was THIS THREAD scheduled?** `polls` against the `timeout /
///    POLL_INTERVAL` looks it was entitled to. Few looks means the test thread
///    itself was off-CPU, and nothing else in the report can be trusted.
/// 2. **Did the child keep talking?** EOF, unread bytes pending, silence, or
///    live output. Unread bytes mean the harness fell behind a child that was
///    fine — the "stale frame" reading `.config/nextest.toml` records.
/// 3. **Was the run slow BEFORE this wait?** Measured on `hetzner-dsm` against
///    `addb4f48`: pinning both boundary tests to 3 cores against 170 spinning
///    competitors stretched them from ~1.3s to ~20s TOTAL and they still
///    PASSED, because every wait slowed together. The CI failures #1109
///    reports took 31.148 / 31.147 / 31.194 / 31.202s total — a normal-speed
///    prefix plus one full 30s stall. Uniform CPU starvation does not produce
///    that shape.
#[cfg(unix)]
pub fn timeout_report(facts: &TimeoutFacts<'_>) -> String {
    let mut out = format!(
        "timed out after {:?} waiting for {}.\n",
        facts.timeout, facts.what
    );

    // 1. Was this thread scheduled?
    let entitled = (facts.timeout.as_millis() / POLL_INTERVAL.as_millis().max(1)).max(1) as u64;
    if facts.polls.saturating_mul(SCHEDULED_DENOMINATOR)
        >= entitled.saturating_mul(SCHEDULED_NUMERATOR)
    {
        out.push_str(&format!(
            "HARNESS SCHEDULED — this thread looked at the screen {} time(s) of the {} it was \
             entitled to, so it was on-CPU throughout. A verdict below is about the child, not \
             about our own scheduling.\n",
            facts.polls, entitled
        ));
    } else {
        out.push_str(&format!(
            "HARNESS STARVED — this thread looked at the screen only {} time(s) of the {} it \
             was entitled to, so the TEST THREAD was off-CPU for most of the budget. This is a \
             runner-capacity failure; nothing below is evidence about the product.\n",
            facts.polls, entitled
        ));
    }

    // 2. Did the child keep talking?
    out.push_str(&match (facts.eof, facts.pending, facts.last_read_age) {
        (true, _, _) => "TERMINAL CLOSED — the pty reached EOF, so the child exited before this \
                         wait could be satisfied.\n"
            .to_owned(),
        (false, Some(n), _) if n > 0 => format!(
            "TERMINAL BACKLOGGED — {n} byte(s) the child already wrote are still unread in the \
             pty, so the screen above is STALE. The stall is this harness falling behind the \
             child, not the child.\n"
        ),
        (false, _, None) => {
            "TERMINAL SILENT — the child never wrote one byte to the terminal.\n".to_owned()
        }
        (false, _, Some(age)) if age >= QUIET_WINDOW => format!(
            "TERMINAL SILENT — no byte arrived for the last {age:?} and nothing is queued, so \
             the child stopped emitting. The stall is upstream of the terminal, inside the \
             binary.\n"
        ),
        (false, _, Some(age)) => format!(
            "TERMINAL LIVE — a byte arrived {age:?} ago, so the child is still painting. The \
             awaited text simply never rendered.\n"
        ),
    });

    // 3. Was the run already slow?
    if facts.steps.is_empty() {
        out.push_str(
            "NO PRIOR STEPS — this is the first wait on this terminal, so there is no speed \
             baseline to compare it against.\n",
        );
    } else {
        let spent: Duration = facts.steps.iter().map(|s| s.elapsed).sum();
        let budget: Duration = facts.steps.iter().map(|s| s.budget).sum();
        let n = facts.steps.len();
        if spent * PREFIX_FAST_DIVISOR <= budget {
            out.push_str(&format!(
                "NOT UNIFORM SLOWDOWN — the {n} wait(s) before this one were satisfied in \
                 {spent:?} against {budget:?} of budget, so the run was at normal speed until \
                 this step. Process-wide CPU starvation is refuted; the stall is specific to \
                 what THIS step waits on.\n"
            ));
        } else {
            out.push_str(&format!(
                "UNIFORM SLOWDOWN — the {n} wait(s) before this one already needed {spent:?} of \
                 {budget:?} budget, so earlier waits were slow too. Consistent with process-wide \
                 starvation rather than a stall specific to this step.\n"
            ));
        }
    }

    out.push_str(&format!(
        "reader: {} byte(s) in {} read(s); pending {}; post-deadline drain {}.\n",
        facts.bytes,
        facts.reads,
        facts
            .pending
            .map_or_else(|| "unknown".to_owned(), |n| n.to_string()),
        if facts.extended { "ran" } else { "did not run" },
    ));

    out.push_str("step timeline:\n");
    if facts.steps.is_empty() {
        out.push_str("  (none)\n");
    }
    for step in facts.steps {
        out.push_str(&format!(
            "  {:?} of {:?} — {}\n",
            step.elapsed, step.budget, step.what
        ));
    }
    out.push_str(&format!(
        "  {:?} of {:?} — {} (TIMED OUT)\n",
        facts.timeout, facts.timeout, facts.what
    ));

    if !facts.context.is_empty() {
        out.push_str("caller context:\n");
        for line in facts.context.lines() {
            out.push_str(&format!("  {line}\n"));
        }
    }

    out.push_str(&format!(
        "--- last screen ---\n{}\n--- end ---",
        facts.screen
    ));
    out
}

#[cfg(all(unix, test))]
mod pty_diagnostics_tests {
    use super::*;

    fn step(what: &str, elapsed_ms: u64, budget_s: u64) -> StepTiming {
        StepTiming {
            what: what.to_owned(),
            elapsed: Duration::from_millis(elapsed_ms),
            budget: Duration::from_secs(budget_s),
        }
    }

    /// A wait that was scheduled throughout: 1000 looks of the 1000 a 30s
    /// budget at a 30ms interval entitles it to.
    fn facts<'a>(steps: &'a [StepTiming], last_read_age: Option<Duration>) -> TimeoutFacts<'a> {
        TimeoutFacts {
            what: "the turn to continue after the granted read",
            timeout: Duration::from_secs(30),
            steps,
            polls: 1000,
            bytes: 4096,
            reads: 12,
            last_read_age,
            pending: Some(0),
            eof: false,
            extended: false,
            context: "",
            screen: "SCREEN-SENTINEL",
        }
    }

    // --- the drain decision -------------------------------------------------

    /// Unread bytes in the pty are positive evidence that the screen is behind
    /// the child, so the wait extends.
    #[test]
    fn a_pending_input_extends_the_wait() {
        assert!(should_extend(
            Some(64),
            Duration::from_secs(20),
            Duration::ZERO,
            GRACE_CAP
        ));
    }

    /// THE CONTROL THAT MATTERS. A silent terminal with nothing queued must
    /// NOT extend. Without this the grace would be a blanket timeout increase,
    /// and a genuinely stalled product would get 3 extra seconds to hide in.
    #[test]
    fn a_control_a_silent_terminal_does_not_extend() {
        assert!(!should_extend(
            Some(0),
            Duration::from_secs(20),
            Duration::ZERO,
            GRACE_CAP
        ));
    }

    /// A byte that landed moments ago: the parse may be mid-flight.
    #[test]
    fn b_recently_arrived_bytes_extend_the_wait() {
        assert!(should_extend(
            Some(0),
            Duration::from_millis(50),
            Duration::ZERO,
            GRACE_CAP
        ));
    }

    /// CONTROL. The extension is capped however loud the terminal is — a child
    /// spewing output forever cannot turn one wait into an unbounded one.
    #[test]
    fn b_control_the_extension_is_capped() {
        assert!(!should_extend(
            Some(4096),
            Duration::ZERO,
            GRACE_CAP,
            GRACE_CAP
        ));
    }

    /// CONTROL. An unknowable pending count is not evidence of anything, so a
    /// backend without a `FIONREAD` fd behaves exactly like today.
    #[test]
    fn b_control_unknown_pending_is_not_evidence() {
        assert!(!should_extend(
            None,
            Duration::from_secs(20),
            Duration::ZERO,
            GRACE_CAP
        ));
    }

    // --- the verdict --------------------------------------------------------

    /// A child that stopped emitting, with nothing queued, is named as a stall
    /// inside the binary.
    #[test]
    fn c_a_stalled_child_is_named_as_a_silent_terminal() {
        let steps = [step("the approval card", 900, 30)];
        let report = timeout_report(&facts(&steps, Some(Duration::from_secs(29))));
        assert!(
            report.contains("TERMINAL SILENT"),
            "a terminal with no byte for 29s and nothing queued must be named SILENT; \
             got:\n{report}"
        );
        assert!(
            !report.contains("TERMINAL LIVE") && !report.contains("TERMINAL BACKLOGGED"),
            "a silent terminal must not also be reported live or backlogged; got:\n{report}"
        );
    }

    /// CONTROL. Bytes still arriving is the opposite finding and must not be
    /// reported as a silent child — otherwise the verdict is unfalsifiable.
    #[test]
    fn c_control_a_painting_child_is_named_as_a_live_terminal() {
        let steps = [step("the approval card", 900, 30)];
        let report = timeout_report(&facts(&steps, Some(Duration::from_millis(40))));
        assert!(
            report.contains("TERMINAL LIVE"),
            "a terminal still receiving bytes must be named LIVE; got:\n{report}"
        );
        assert!(
            !report.contains("TERMINAL SILENT"),
            "a live terminal must not also be reported SILENT; got:\n{report}"
        );
    }

    /// THE STALE-FRAME READING. `.config/nextest.toml` blames a starved reader
    /// thread leaving the parser behind the child. That state has a signature
    /// — unread bytes in the pty — and the report must name it rather than
    /// blaming the binary.
    #[test]
    fn d_unread_bytes_are_named_as_a_stale_screen() {
        let steps = [step("the approval card", 900, 30)];
        let mut f = facts(&steps, Some(Duration::from_secs(29)));
        f.pending = Some(512);
        let report = timeout_report(&f);
        assert!(
            report.contains("TERMINAL BACKLOGGED") && report.contains("STALE"),
            "unread pty bytes must be named as a stale screen; got:\n{report}"
        );
        assert!(
            !report.contains("inside the binary"),
            "a backlogged terminal must not be blamed on the binary; got:\n{report}"
        );
    }

    /// A thread that got its looks was on-CPU; the verdict about the child is
    /// then worth reading.
    #[test]
    fn e_a_thread_that_got_its_polls_is_named_scheduled() {
        let report = timeout_report(&facts(&[], Some(Duration::from_secs(29))));
        assert!(
            report.contains("HARNESS SCHEDULED"),
            "1000 polls of an entitled 1000 must read as scheduled; got:\n{report}"
        );
    }

    /// CONTROL, and the reading #1109 could never take. A test thread that was
    /// off-CPU explains a timeout by itself, and the report must say so
    /// instead of pointing at the product.
    #[test]
    fn e_control_a_thread_denied_its_polls_is_named_starved() {
        let mut f = facts(&[], Some(Duration::from_secs(29)));
        f.polls = 12;
        let report = timeout_report(&f);
        assert!(
            report.contains("HARNESS STARVED"),
            "12 polls of an entitled 1000 must read as starved; got:\n{report}"
        );
        assert!(
            !report.contains("HARNESS SCHEDULED"),
            "the starvation verdict must not also carry its own negation; got:\n{report}"
        );
    }

    /// THE DISCRIMINATOR. A fast prefix plus one full-budget stall is the CI
    /// shape (31.15s total against a 30s wait) and is NOT what process-wide
    /// starvation produces.
    #[test]
    fn f_a_fast_prefix_refutes_process_wide_starvation() {
        let steps = [
            step("the TUI chrome", 1_100, 60),
            step("the approval card", 120, 30),
            step("the folder name on the card", 60, 10),
        ];
        let report = timeout_report(&facts(&steps, Some(Duration::from_secs(29))));
        assert!(
            report.contains("NOT UNIFORM SLOWDOWN"),
            "a prefix that used ~1.3s of 100s of budget must refute uniform starvation; \
             got:\n{report}"
        );
    }

    /// CONTROL. When the earlier waits WERE slow the report must say so — a
    /// verdict that always refutes starvation would name nothing.
    #[test]
    fn f_control_a_slow_prefix_reports_uniform_starvation() {
        let steps = [
            step("the TUI chrome", 40_000, 60),
            step("the approval card", 20_000, 30),
        ];
        let report = timeout_report(&facts(&steps, Some(Duration::from_secs(29))));
        assert!(
            report.contains("UNIFORM SLOWDOWN"),
            "a prefix that burned 60s of 90s budget must report uniform starvation; \
             got:\n{report}"
        );
        assert!(
            !report.contains("NOT UNIFORM SLOWDOWN"),
            "the starvation verdict must not also carry its own negation; got:\n{report}"
        );
    }

    /// The fact the caller owns — what the mock provider received — has to
    /// reach the failure text, because nothing inside this harness can see it.
    #[test]
    fn g_the_caller_context_reaches_the_failure_text() {
        let steps = [step("the approval card", 900, 30)];
        let mut f = facts(&steps, Some(Duration::from_secs(29)));
        f.context = "mock provider received 1 request(s)";
        let report = timeout_report(&f);
        assert!(
            report.contains("mock provider received 1 request(s)"),
            "the diagnostic supplied by the caller must be rendered; got:\n{report}"
        );
    }

    /// Every satisfied wait must be named with its own duration: the timeline
    /// is what lets a reader locate the stall without a second CI run.
    #[test]
    fn g_the_step_timeline_names_every_satisfied_wait() {
        let steps = [
            step("the TUI chrome", 1_100, 60),
            step("the approval card", 120, 30),
        ];
        let report = timeout_report(&facts(&steps, Some(Duration::from_secs(29))));
        for entry in &steps {
            assert!(
                report.contains(&entry.what),
                "the timeline must name the wait {:?}; got:\n{report}",
                entry.what
            );
        }
        assert!(
            report.contains("TIMED OUT"),
            "the timeline must mark which wait failed; got:\n{report}"
        );
    }

    /// CONTROL / no-regression. The last screen was the ONLY diagnostic this
    /// harness used to print. It must survive.
    #[test]
    fn h_control_the_last_screen_survives() {
        let report = timeout_report(&facts(&[], None));
        assert!(
            report.contains("SCREEN-SENTINEL") && report.contains("--- last screen ---"),
            "the pre-existing screen dump must not be lost; got:\n{report}"
        );
        assert!(
            report.contains("NO PRIOR STEPS"),
            "a first wait must say it has no baseline rather than claiming a verdict; \
             got:\n{report}"
        );
    }
}
