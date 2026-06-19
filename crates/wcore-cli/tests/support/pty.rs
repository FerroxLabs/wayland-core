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
        let pty = native_pty_system()
            .openpty(PtySize {
                rows,
                cols,
                pixel_width: 0,
                pixel_height: 0,
            })
            .expect("open PTY");

        let mut cmd = CommandBuilder::new(binary());
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
        cmd.cwd(home);
        let child = pty.slave.spawn_command(cmd).expect("spawn wayland-core");

        let mut reader = pty.master.try_clone_reader().expect("clone PTY reader");
        let parser = std::sync::Arc::new(std::sync::Mutex::new(vt100::Parser::new(rows, cols, 0)));
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
            _master: pty.master,
            child,
            _reader: reader_handle,
        }
    }

    pub fn screen_text(&self) -> String {
        let parser = self.parser.lock().expect("parser lock");
        parser.screen().contents()
    }

    pub fn wait_for<F: Fn(&str) -> bool>(&self, predicate: F, timeout: Duration, what: &str) {
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

    pub fn send(&mut self, bytes: &[u8]) {
        self.writer.write_all(bytes).expect("write to PTY");
        self.writer.flush().ok();
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
