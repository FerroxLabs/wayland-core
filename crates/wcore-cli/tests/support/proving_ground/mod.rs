//! Proving Ground harness — `Session`, `Cell`, `run_cell`, `ConfigState`,
//! `TermShape`, and `RunRecord`.
//!
//! This module is the foundation scaffold: many public items exist for later
//! Tasks 3 and 4 and are not yet used in Task 2's single test.
#![allow(dead_code)]
//!
//! # Design
//!
//! A *cell* is the unit of test work: it declares its config state, its
//! terminal shape, and a script closure that drives the PTY.  `run_cell`
//! materializes the config, boots the binary in a throw-away tempdir, runs
//! the script, captures a `RunRecord`, and cleans up.
//!
//! The harness is Unix-only (`portable_pty` cannot surface stdout in
//! headless GHA runners on Windows — see `pty.rs`).

#[cfg(unix)]
pub use super::pty::Pty;
#[allow(unused_imports)]
// Re-exported for non-PTY (headless / json-stream) spawns only.
// The PTY path (`Pty::spawn_with_env`) does its own STRIPPED_PROVIDER_ENV
// strip directly; `harden_child_env` is for `std::process::Command` children.
pub use super::pty::harden_child_env;
pub mod record;
pub use record::RunRecord;

use std::path::Path;
use tempfile::TempDir;

// ---------------------------------------------------------------------------
// Session
// ---------------------------------------------------------------------------

/// Owns one throw-away home directory. Every `launch()` call spawns the real
/// binary against the same home — so the same `Session` can be used for
/// *relaunch* scenarios (e.g. "boot, quit, reboot to the same home").
pub struct Session {
    home: TempDir,
}

impl Session {
    /// Create a new session with a fresh temporary home directory.
    pub fn new() -> Self {
        Self {
            home: TempDir::new().expect("tempdir"),
        }
    }

    /// The path to this session's home directory.
    pub fn home(&self) -> &Path {
        self.home.path()
    }

    /// Spawn the real binary against this session's home.
    ///
    /// Calling `launch()` more than once on the same session re-uses the same
    /// home directory (the binary reads whatever config/state is there), which
    /// is how *relaunch* journeys are tested.
    ///
    /// If `ConfigState::EnvKeysOnly.materialize(home)` was called before this
    /// launch, the `.proving-ground-env` sidecar written by `materialize` is
    /// read and its `KEY=VALUE` pairs are injected into the child's environment
    /// so the binary sees the fake provider key without a real credential.
    #[cfg(unix)]
    pub fn launch(&self) -> Pty {
        let env_overrides = self.read_env_sidecar();
        if env_overrides.is_empty() {
            Pty::spawn(self.home.path())
        } else {
            Pty::spawn_with_env(self.home.path(), 40, 120, &env_overrides)
        }
    }

    /// Read key=value pairs from the `.proving-ground-env` sidecar file, if
    /// present.  Returns an empty vec when the file does not exist.
    #[cfg(unix)]
    fn read_env_sidecar(&self) -> Vec<(String, String)> {
        let path = self.home.path().join(ENV_SIDECAR);
        let Ok(contents) = std::fs::read_to_string(&path) else {
            return Vec::new();
        };
        contents
            .lines()
            .filter_map(|line| {
                let (k, v) = line.split_once('=')?;
                Some((k.trim().to_string(), v.trim().to_string()))
            })
            .collect()
    }

    /// Spawn the real binary against this session's home with a specific
    /// terminal size. Task 4 uses this for layout/wrapping tests.
    #[cfg(unix)]
    pub fn launch_sized(&self, term: TermShape) -> Pty {
        let env_overrides = self.read_env_sidecar();
        if env_overrides.is_empty() {
            Pty::spawn_sized(self.home.path(), term.rows, term.cols)
        } else {
            Pty::spawn_with_env(self.home.path(), term.rows, term.cols, &env_overrides)
        }
    }
}

impl Default for Session {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// ConfigState
// ---------------------------------------------------------------------------

/// Declares what (if any) configuration exists in the session home before
/// the binary is launched.
#[derive(Clone, Copy, Debug)]
pub enum ConfigState {
    /// No config file and no credential env vars — the binary sees a clean
    /// slate and will enter the onboarding flow.
    Fresh,

    /// No config file, but `OPENAI_API_KEY` is present in the child's
    /// environment. Tests the "key-from-env, no config file" path.
    EnvKeysOnly,

    /// A minimal OpenAI config is written (`gpt-4o`, dummy base_url) so the
    /// binary boots directly to the Workspace surface without onboarding.
    ConfiguredOpenAi,

    /// `config.toml` is written with deliberately invalid TOML bytes.
    /// Tests the "corrupt config" error path.
    CorruptConfig,
}

/// Name of the side-channel env file that `ConfigState::EnvKeysOnly`
/// writes so `Session::launch()` can inject env vars without the caller
/// needing to change the `launch()` call site.
pub const ENV_SIDECAR: &str = ".proving-ground-env";

impl ConfigState {
    /// Write (or not write) the config file for this state into `home`.
    ///
    /// For `EnvKeysOnly`, writes a side-channel env file (`ENV_SIDECAR`)
    /// so that `Session::launch()` can inject the fake provider key into
    /// the child process without the test needing to call a different
    /// spawn method.  This is how `session.launch()` can see the key even
    /// though the caller only calls `materialize(session.home())`.
    pub fn materialize(&self, home: &Path) {
        match self {
            ConfigState::Fresh => {
                // No config file — leave the home directory empty.
            }
            ConfigState::EnvKeysOnly => {
                // Write the env sidecar so Session::launch() picks it up.
                // The fake key is safe to write to disk: it is a
                // well-known test sentinel with no real credentials.
                std::fs::write(
                    home.join(ENV_SIDECAR),
                    "OPENAI_API_KEY=sk-test-harness-envonly-00000000\n",
                )
                .expect("write .proving-ground-env");
            }
            ConfigState::ConfiguredOpenAi => {
                // Dummy base_url points at a port that is not listening; the
                // binary does NOT need the provider to be reachable to render
                // the Workspace surface — it only hits the provider when the
                // user sends a prompt.
                super::pty::write_config(
                    home,
                    "openai",
                    Some("gpt-4o"),
                    Some("http://127.0.0.1:1"),
                );
            }
            ConfigState::CorruptConfig => {
                std::fs::write(home.join("config.toml"), b"this is not valid toml {{{")
                    .expect("write corrupt config.toml");
            }
        }
    }

    /// Additional environment variable overrides that must be set on the child
    /// process for this config state.  Used by `run_cell` when spawning via
    /// `Pty::spawn_with_env`.
    pub fn env_overrides(&self) -> &[(&'static str, &'static str)] {
        match self {
            ConfigState::EnvKeysOnly => &[("OPENAI_API_KEY", "sk-test-harness-envonly-00000000")],
            _ => &[],
        }
    }
}

// ---------------------------------------------------------------------------
// TermShape
// ---------------------------------------------------------------------------

/// Terminal dimensions for the PTY.
#[derive(Clone, Copy, Debug)]
pub struct TermShape {
    pub rows: u16,
    pub cols: u16,
}

impl Default for TermShape {
    fn default() -> Self {
        Self { rows: 40, cols: 120 }
    }
}

// ---------------------------------------------------------------------------
// Cell
// ---------------------------------------------------------------------------

/// A single test cell: the static metadata + script that `run_cell` executes.
#[cfg(unix)]
pub struct Cell {
    /// Human-readable name used in diagnostics. Must be unique within the
    /// test file.
    pub name: &'static str,

    /// Config state to materialize before launching the binary.
    pub config: ConfigState,

    /// Terminal dimensions for the PTY.
    pub term: TermShape,

    /// The test script: drives the PTY, then returns.  `run_cell` calls
    /// `pty.quit()` and captures the `RunRecord` after the script returns.
    pub script: fn(&mut Pty, &Session),
}

// ---------------------------------------------------------------------------
// run_cell
// ---------------------------------------------------------------------------

/// Execute a `Cell` end-to-end and return the captured `RunRecord`.
///
/// 1. Creates a fresh `Session` (throw-away tempdir).
/// 2. Calls `cell.config.materialize(session.home())`.
/// 3. Spawns the binary (with any env overrides from `ConfigState`).
/// 4. Runs `cell.script(&mut pty, &session)`.
/// 5. Calls `pty.quit()`.
/// 6. Captures and returns a `RunRecord`.
#[cfg(unix)]
pub fn run_cell(cell: &Cell) -> RunRecord {
    let session = Session::new();
    cell.config.materialize(session.home());

    let env_overrides: Vec<(String, String)> = cell
        .config
        .env_overrides()
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect();

    let mut pty = if env_overrides.is_empty() {
        Pty::spawn_sized(session.home(), cell.term.rows, cell.term.cols)
    } else {
        Pty::spawn_with_env(session.home(), cell.term.rows, cell.term.cols, &env_overrides)
    };

    (cell.script)(&mut pty, &session);

    // Phase 1: snapshot the screen while the script's final UI state is
    // visible, BEFORE quit() sends the /exit command and scrolls/clears.
    let final_screen = record::redact(&pty.screen_text());

    // Phase 2: clean shutdown — sends /exit, waits for process exit.
    // After this the CrashSentinel Drop has fired, so .dirty-death is gone
    // for a normal run.
    pty.quit();

    // Phase 3: read filesystem state (config.toml, .dirty-death) now that
    // the process has fully exited and its cleanup has run.
    RunRecord::capture_post_quit(session.home(), &mut pty, final_screen)
}
