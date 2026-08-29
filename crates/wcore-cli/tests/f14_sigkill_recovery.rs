//! F14 packaged-process crash/restart proof.
//!
//! These tests deliberately kill the real `wayland-core` binary with
//! `SIGKILL`, reopen the same durable session, and inspect only the public
//! JSON-stream recovery contract. The loopback provider records request
//! arrivals independently of Core, so a second provider call cannot hide
//! behind a plausible recovery event.

#![cfg(unix)]

use std::io::{Read, Write};
use std::os::unix::io::RawFd;
use std::os::unix::process::ExitStatusExt;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use std::{fs, io};

use portable_pty::{CommandBuilder, MasterPty, PtySize, native_pty_system};
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader, Lines};
use tokio::process::{Child, ChildStdin, ChildStdout, Command};
use tokio::task::JoinHandle;
use wcore_eval_scenarios::fixtures::openai::{
    OpenAiFixtureScript, OpenAiStep, RunningOpenAiFixture,
};
use wcore_eval_scenarios::providers::{ProviderConfig, ProviderId};
use wcore_eval_scenarios::tempenv::{self, TempEnv};

#[path = "support/mod.rs"]
mod support;
use support::owned_tree::OwnedTree;

const EVENT_TIMEOUT: Duration = Duration::from_secs(20);
const PROCESS_TIMEOUT: Duration = Duration::from_secs(10);
const FIXTURE_MODEL: &str = "fixture-chat-v1";
const FIXTURE_KEY: &str = "fixture-local-token";
const GENESIS_DIGEST: &str = "0000000000000000000000000000000000000000000000000000000000000000";

fn binary() -> &'static str {
    env!("CARGO_BIN_EXE_wayland-core")
}

struct VaultSecret(String);

impl VaultSecret {
    fn new() -> Self {
        Self(format!(
            "{}{}",
            uuid::Uuid::new_v4().simple(),
            uuid::Uuid::new_v4().simple()
        ))
    }

    fn as_str(&self) -> &str {
        &self.0
    }

    fn inheritable_pipe(&self) -> RawFd {
        let mut pipe = [0; 2];
        // SAFETY: `pipe` points to two valid integers. Plain `pipe(2)` is
        // intentional: the read end must survive exec into packaged Core.
        assert_eq!(
            unsafe { libc::pipe(pipe.as_mut_ptr()) },
            0,
            "create vault pipe"
        );
        let mut written = 0;
        while written < self.0.len() {
            // SAFETY: the write descriptor belongs to this process and the
            // source slice remains valid for the duration of the call.
            let count = unsafe {
                libc::write(
                    pipe[1],
                    self.0.as_bytes()[written..].as_ptr().cast(),
                    self.0.len() - written,
                )
            };
            assert!(count > 0, "write vault passphrase pipe");
            written += count as usize;
        }
        // SAFETY: the writer is no longer needed after the complete secret is
        // buffered; closing it also gives Core an unambiguous EOF.
        assert_eq!(
            unsafe { libc::close(pipe[1]) },
            0,
            "close vault pipe writer"
        );
        pipe[0]
    }
}

struct CoreProcess {
    child: OwnedTree<Child>,
    stdin: ChildStdin,
    stdout: Lines<BufReader<ChildStdout>>,
    stderr: Arc<Mutex<Vec<u8>>>,
    stderr_task: JoinHandle<()>,
}

impl CoreProcess {
    async fn launch(
        env: &TempEnv,
        fixture: &RunningOpenAiFixture,
        vault: &VaultSecret,
        session_id: &str,
        resume: bool,
    ) -> Self {
        Self::launch_with_secure_store(env, fixture, Some(vault), session_id, resume).await
    }

    async fn launch_with_secure_store(
        env: &TempEnv,
        fixture: &RunningOpenAiFixture,
        vault: Option<&VaultSecret>,
        session_id: &str,
        resume: bool,
    ) -> Self {
        let vault_fd = vault.map(VaultSecret::inheritable_pipe);
        let mut command = Command::new(binary());
        command
            .arg("--json-stream")
            .arg("--provider")
            .arg("openai")
            .arg("--model")
            .arg(FIXTURE_MODEL)
            .arg("--base-url")
            .arg(fixture.base_url());
        if resume {
            command.arg("--resume").arg(session_id);
        } else {
            command.arg("--session-id").arg(session_id);
        }
        command
            .current_dir(env.path())
            .env("HOME", env.path())
            .env("WAYLAND_HOME", env.home())
            .env("OPENAI_API_KEY", FIXTURE_KEY)
            .env_remove("WAYLAND_VAULT_PASSPHRASE")
            .env_remove("WAYLAND_VAULT_PASSPHRASE_FD")
            .env_remove("ANTHROPIC_API_KEY")
            .env_remove("DEEPSEEK_API_KEY")
            .env_remove("GEMINI_API_KEY")
            .env_remove("GOOGLE_API_KEY")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);

        if let Some(vault_fd) = vault_fd {
            command.env("WAYLAND_VAULT_PASSPHRASE_FD", vault_fd.to_string());
        } else {
            // Make the Linux Secret Service probe deterministically unavailable
            // rather than depending on the worker's desktop/session state.
            command.env(
                "DBUS_SESSION_BUS_ADDRESS",
                format!(
                    "unix:path={}",
                    env.path().join("missing-secret-service-bus").display()
                ),
            );
        }

        let mut child = OwnedTree::new({
            let child = command.spawn();
            // SAFETY: after spawn, only the child may consume its inherited copy.
            if let Some(vault_fd) = vault_fd {
                assert_eq!(
                    unsafe { libc::close(vault_fd) },
                    0,
                    "close parent vault pipe"
                );
            }
            child.expect("spawn packaged wayland-core")
        });
        let stdin = child.stdin.take().expect("Core stdin pipe");
        let stdout = BufReader::new(child.stdout.take().expect("Core stdout pipe")).lines();
        let mut child_stderr = child.stderr.take().expect("Core stderr pipe");
        let stderr = Arc::new(Mutex::new(Vec::new()));
        let stderr_capture = Arc::clone(&stderr);
        let stderr_task = tokio::spawn(async move {
            let mut chunk = [0_u8; 4096];
            while let Ok(read) = child_stderr.read(&mut chunk).await {
                if read == 0 {
                    break;
                }
                let mut output = stderr_capture.lock().expect("lock Core stderr capture");
                let remaining = 128 * 1024_usize - output.len().min(128 * 1024);
                output.extend_from_slice(&chunk[..read.min(remaining)]);
            }
        });
        let mut process = Self {
            child,
            stdin,
            stdout,
            stderr,
            stderr_task,
        };
        let ready = process.next_type("ready").await;
        assert_eq!(
            ready.get("session_id").and_then(Value::as_str),
            Some(session_id),
            "packaged Core opened a different session: {ready}"
        );
        // THE OTHER HALF of the degraded assertion in
        // `without_secure_store_the_default_runs_degraded_and_leaves_nothing_durable`.
        // Every vault-backed launch in this file runs through here, so the
        // durable posture is proved on the same wire, by the same binary, as
        // the degraded one — which is what makes the two distinguishable
        // rather than merely differently asserted.
        assert_eq!(
            ready["session_persistence"], "durable",
            "a launch that opened a journaled session must say so: {ready}"
        );
        process
    }

    async fn send(&mut self, command: Value) {
        let mut bytes = serde_json::to_vec(&command).expect("serialize host command");
        bytes.push(b'\n');
        self.stdin
            .write_all(&bytes)
            .await
            .expect("write host command");
        self.stdin.flush().await.expect("flush host command");
    }

    async fn next_type(&mut self, expected: &str) -> Value {
        let deadline = Instant::now() + EVENT_TIMEOUT;
        let mut observed = Vec::new();
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                self.panic_timeout(expected, &observed);
            }
            let line = match tokio::time::timeout(remaining, self.stdout.next_line()).await {
                Ok(line) => line.expect("read Core protocol stdout"),
                Err(_) => self.panic_timeout(expected, &observed),
            };
            let Some(line) = line else {
                let stderr = self.stderr.lock().expect("lock Core stderr capture");
                panic!(
                    "Core exited while waiting for {expected}; stderr:\n{}",
                    String::from_utf8_lossy(&stderr)
                );
            };
            let Ok(event) = serde_json::from_str::<Value>(&line) else {
                continue;
            };
            if event.get("type").and_then(Value::as_str) == Some(expected) {
                return event;
            }
            observed.push(format!(
                "type={:?} request_id={:?} reason={:?}",
                event.get("type"),
                event.get("request_id"),
                event.get("reason")
            ));
            assert_ne!(
                event.get("type").and_then(Value::as_str),
                Some("error"),
                "Core refused the command while waiting for {expected}: {event}"
            );
        }
    }

    /// Wait for an `info` frame whose message contains `needle`.
    ///
    /// [`Self::next_type`] matches on the frame type alone, which is not enough
    /// here: `info` is a general-purpose channel and other machinery emits on
    /// it, so a type-only match could return an unrelated frame and pass. The
    /// `stream_end` guard makes the wait bounded by the turn rather than only
    /// by the clock — if the turn finishes without the notice, that is a
    /// failure to report, not a slow report, and it should say so.
    #[cfg(target_os = "linux")]
    async fn next_info_containing(&mut self, needle: &str, msg_id: &str) -> Value {
        let mut seen = Vec::new();
        loop {
            let frame = self.next_type_in(&["info", "stream_end"]).await;
            match frame.get("type").and_then(Value::as_str) {
                Some("info") => {
                    let message = frame
                        .get("message")
                        .and_then(Value::as_str)
                        .unwrap_or_default();
                    if message.contains(needle) {
                        return frame;
                    }
                    seen.push(message.to_string());
                }
                _ => panic!(
                    "turn {msg_id} ended without an info frame containing {needle:?}; \
                     info frames seen: {seen:?}"
                ),
            }
        }
    }

    /// `next_type`, widened to a set, so a caller can wait for one of several
    /// frames and decide which arrived.
    #[cfg(target_os = "linux")]
    async fn next_type_in(&mut self, expected: &[&str]) -> Value {
        let deadline = Instant::now() + EVENT_TIMEOUT;
        let label = expected.join("|");
        let mut observed = Vec::new();
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                self.panic_timeout(&label, &observed);
            }
            let line = match tokio::time::timeout(remaining, self.stdout.next_line()).await {
                Ok(line) => line.expect("read Core protocol stdout"),
                Err(_) => self.panic_timeout(&label, &observed),
            };
            let Some(line) = line else {
                let stderr = self.stderr.lock().expect("lock Core stderr capture");
                panic!(
                    "Core exited while waiting for {label}; stderr:\n{}",
                    String::from_utf8_lossy(&stderr)
                );
            };
            let Ok(event) = serde_json::from_str::<Value>(&line) else {
                continue;
            };
            let event_type = event.get("type").and_then(Value::as_str);
            if event_type.is_some_and(|ty| expected.contains(&ty)) {
                return event;
            }
            observed.push(format!("type={event_type:?}"));
            assert_ne!(
                event_type,
                Some("error"),
                "Core refused the command while waiting for {label}: {event}"
            );
        }
    }

    fn panic_timeout(&self, expected: &str, observed: &[String]) -> ! {
        let stderr = self.stderr.lock().expect("lock Core stderr capture");
        panic!(
            "timed out waiting for {expected}; observed={observed:?}; stderr:\n{}",
            String::from_utf8_lossy(&stderr)
        );
    }

    async fn sigkill(mut self) -> Vec<u8> {
        let pid = self.child.child_mut().id().expect("running Core pid");
        // SAFETY: `pid` came from the live child owned by this harness. The
        // signal has no attacker-controlled component and targets that exact
        // process only.
        let result = unsafe { libc::kill(pid as libc::pid_t, libc::SIGKILL) };
        assert_eq!(result, 0, "send SIGKILL to packaged Core");
        let status = tokio::time::timeout(PROCESS_TIMEOUT, self.child.wait())
            .await
            .expect("Core exited after SIGKILL")
            .expect("wait for SIGKILLed Core");
        assert_eq!(status.signal(), Some(libc::SIGKILL));
        self.stderr_task.await.expect("join Core stderr capture");
        Arc::try_unwrap(self.stderr)
            .expect("sole Core stderr owner")
            .into_inner()
            .expect("unlock Core stderr capture")
    }
}

struct TuiProcess {
    writer: Box<dyn Write + Send>,
    parser: Arc<Mutex<vt100::Parser>>,
    _master: Box<dyn MasterPty + Send>,
    // Held only for its `Drop`, which kills and reaps the whole process tree
    // (FerroxLabs/wayland-core#352); nothing else in this file reads it.
    #[allow(dead_code)]
    child: OwnedTree<Box<dyn portable_pty::Child + Send + Sync>>,
    _reader: std::thread::JoinHandle<()>,
}

impl TuiProcess {
    fn launch(
        env: &TempEnv,
        fixture: &RunningOpenAiFixture,
        vault: &VaultSecret,
        session_id: &str,
    ) -> Self {
        let pty = native_pty_system()
            .openpty(PtySize {
                rows: 40,
                cols: 2_000,
                pixel_width: 0,
                pixel_height: 0,
            })
            .expect("open packaged TUI PTY");
        let mut command = CommandBuilder::new(binary());
        command.arg("--resume");
        command.arg(session_id);
        command.arg("--provider");
        command.arg("openai");
        command.arg("--model");
        command.arg(FIXTURE_MODEL);
        command.arg("--base-url");
        command.arg(fixture.base_url());
        command.cwd(env.path());
        command.env("HOME", env.path());
        command.env("WAYLAND_HOME", env.home());
        command.env("TERM", "xterm-256color");
        command.env("OPENAI_API_KEY", FIXTURE_KEY);
        // portable-pty deliberately closes arbitrary inherited descriptors.
        // This child is a hermetic test process with a fresh throwaway secret,
        // so use the legacy environment transport only at this PTY boundary.
        command.env("WAYLAND_VAULT_PASSPHRASE", vault.as_str());
        command.env_remove("WAYLAND_VAULT_PASSPHRASE_FD");
        command.env_remove("ANTHROPIC_API_KEY");
        command.env_remove("DEEPSEEK_API_KEY");
        command.env_remove("GEMINI_API_KEY");
        command.env_remove("GOOGLE_API_KEY");

        let child = OwnedTree::new(
            pty.slave
                .spawn_command(command)
                .expect("spawn packaged TUI"),
        );

        let mut reader = pty.master.try_clone_reader().expect("clone TUI PTY reader");
        let parser = Arc::new(Mutex::new(vt100::Parser::new(40, 2_000, 0)));
        let parser_for_thread = Arc::clone(&parser);
        let reader = std::thread::spawn(move || {
            let mut bytes = [0_u8; 8_192];
            loop {
                match reader.read(&mut bytes) {
                    Ok(0) => break,
                    Ok(read) => parser_for_thread
                        .lock()
                        .expect("lock TUI parser")
                        .process(&bytes[..read]),
                    Err(_) => break,
                }
            }
        });
        let writer = pty.master.take_writer().expect("take TUI PTY writer");
        let process = Self {
            writer,
            parser,
            _master: pty.master,
            child,
            _reader: reader,
        };
        process.wait_for(
            |screen| screen.contains("WAYLAND") && screen.contains("Workspace"),
            Duration::from_secs(60),
            "packaged TUI workspace",
        );
        process
    }

    fn screen_text(&self) -> String {
        self.parser
            .lock()
            .expect("lock TUI parser")
            .screen()
            .contents()
    }

    fn wait_for<F: Fn(&str) -> bool>(&self, predicate: F, timeout: Duration, label: &str) {
        let deadline = Instant::now() + timeout;
        let mut last = String::new();
        while Instant::now() < deadline {
            last = self.screen_text();
            if predicate(&last) {
                return;
            }
            std::thread::sleep(Duration::from_millis(30));
        }
        panic!("timed out waiting for {label}; last TUI screen:\n{last}");
    }

    fn type_command(&mut self, command: &str) {
        for byte in command.bytes() {
            self.writer.write_all(&[byte]).expect("type into TUI PTY");
            self.writer.flush().expect("flush TUI PTY");
            std::thread::sleep(Duration::from_millis(12));
        }
        self.writer.write_all(b"\r").expect("submit TUI command");
        self.writer.flush().expect("flush submitted TUI command");
    }

    fn recovery_projection(&self) -> Value {
        self.wait_for(
            |screen| screen.contains("RECOVERY_V1 "),
            EVENT_TIMEOUT,
            "RECOVERY_V1 projection",
        );
        let screen = self.screen_text();
        let marker = "RECOVERY_V1 ";
        let start = screen
            .rfind(marker)
            .map(|offset| offset + marker.len())
            .unwrap_or_else(|| panic!("recovery marker disappeared from TUI screen:\n{screen}"));
        let candidate = &screen[start..];
        serde_json::Deserializer::from_str(candidate)
            .into_iter::<Value>()
            .next()
            .expect("TUI recovery JSON value")
            .unwrap_or_else(|error| {
                let preview = candidate.chars().take(512).collect::<String>();
                panic!("decode TUI recovery JSON: {error}; candidate prefix={preview:?}")
            })
    }
}

// No `impl Drop for TuiProcess`: `child` is an `OwnedTree`, whose own `Drop`
// kills the whole process tree and reaps it — strictly stronger than the
// leaf-only kill that used to live here (FerroxLabs/wayland-core#352).

fn environment(fixture: &RunningOpenAiFixture) -> TempEnv {
    let provider = ProviderConfig::new(ProviderId::OpenAI, FIXTURE_MODEL)
        .with_api_key(FIXTURE_KEY)
        .with_known_free_cost()
        .with_base_url(fixture.base_url());
    tempenv::build(&provider).expect("build hermetic Core environment")
}

const SEED_SESSION_ID: &str = "WAYLAND_F14_SEED_SESSION_ID";
const SEED_TURN_ID: &str = "WAYLAND_F14_SEED_TURN_ID";
const SEED_PROMPT: &str = "WAYLAND_F14_SEED_PROMPT";
const SEED_BASE_URL: &str = "WAYLAND_F14_SEED_BASE_URL";
const SEED_WORKSPACE: &str = "WAYLAND_F14_SEED_WORKSPACE";
const SEED_DESKTOP_LAUNCH: &str = "WAYLAND_F14_SEED_DESKTOP_LAUNCH";

/// Re-exec helper used by packaged recovery tests. Running the seeder in a
/// child process keeps the confidential-store unlock descriptor and all HOME
/// overrides process-local even when the integration binary runs in parallel.
#[ignore]
#[tokio::test]
async fn f14_seed_recoverable_turn_helper() {
    let session_id = std::env::var(SEED_SESSION_ID).expect("seed session id");
    let turn_id = std::env::var(SEED_TURN_ID).expect("seed turn id");
    let prompt = std::env::var(SEED_PROMPT).expect("seed prompt");
    let base_url = std::env::var(SEED_BASE_URL).expect("seed base URL");
    let workspace = PathBuf::from(std::env::var(SEED_WORKSPACE).expect("seed workspace"));
    let wayland_home = PathBuf::from(std::env::var("WAYLAND_HOME").expect("seed WAYLAND_HOME"));
    let desktop_launch = std::env::var(SEED_DESKTOP_LAUNCH).as_deref() == Ok("1");
    let cli = wcore_config::config::CliArgs {
        provider: Some("openai".to_string()),
        api_key: Some(FIXTURE_KEY.to_string()),
        base_url: Some(base_url),
        model: Some(FIXTURE_MODEL.to_string()),
        project_dir: Some(workspace.clone()),
        ..Default::default()
    };
    let mut config = wcore_config::config::Config::resolve(&cli).expect("resolve seed config");
    config.session.enabled = true;
    config.session.directory = wayland_home.join("sessions").to_string_lossy().into_owned();
    let manager = wcore_agent::session::SessionManager::new(
        PathBuf::from(&config.session.directory),
        config.session.max_sessions,
    );
    let workspace = workspace.to_string_lossy().into_owned();
    let active = manager
        .create_for_run("openai", FIXTURE_MODEL, &workspace, Some(&session_id))
        .expect("create seed session");
    manager
        .persist_first_message(&active.session)
        .expect("publish seed session");
    let provider = Arc::new(wcore_agent::test_utils::ScriptedProvider::single_text_turn(
        "unused seed response",
    ));
    let approval_manager = Arc::new(wcore_protocol::ToolApprovalManager::new());
    let execution = wcore_cli::packaged_runtime::resolve_local_execution(
        &config,
        false,
        false,
        wcore_types::execution_policy::DEFAULT_DANGEROUS_SESSION_TTL_SECS,
        desktop_launch,
    )
    .expect("resolve packaged Desktop execution authority");
    let bootstrap = execution
        .apply(wcore_agent::bootstrap::AgentBootstrap::new(
            config,
            workspace,
            Arc::new(wcore_agent::output::null_sink::NullSink),
        ))
        .provider(provider)
        .with_approval_manager(approval_manager)
        .enable_inbound_dispatch(true)
        .resume(active)
        .build()
        .await
        .expect("build production-shaped seed engine");
    let mut engine = bootstrap.engine;
    engine
        .prepare_recoverable_turn_for_test(&turn_id, &prompt)
        .await
        .expect("persist recoverable seed checkpoint");
}

async fn seed_recoverable_profile(
    env: &TempEnv,
    fixture: &RunningOpenAiFixture,
    vault: &VaultSecret,
    session_id: &str,
    turn_id: &str,
    prompt: &str,
    desktop_launch: bool,
) {
    let vault_fd = vault.inheritable_pipe();
    let mut command = Command::new(std::env::current_exe().expect("current F14 test binary"));
    command
        .arg("--exact")
        .arg("f14_seed_recoverable_turn_helper")
        .arg("--ignored")
        .arg("--nocapture")
        .current_dir(env.path())
        .env("HOME", env.path())
        .env("WAYLAND_HOME", env.home())
        .env("OPENAI_API_KEY", FIXTURE_KEY)
        .env("WAYLAND_VAULT_PASSPHRASE_FD", vault_fd.to_string())
        .env_remove("WAYLAND_VAULT_PASSPHRASE")
        .env(SEED_SESSION_ID, session_id)
        .env(SEED_TURN_ID, turn_id)
        .env(SEED_PROMPT, prompt)
        .env(SEED_BASE_URL, fixture.base_url())
        .env(SEED_WORKSPACE, env.path())
        .env(SEED_DESKTOP_LAUNCH, if desktop_launch { "1" } else { "0" })
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let child = OwnedTree::new({
        let child = command.spawn();
        // SAFETY: after spawn, only the child may consume its inherited copy.
        assert_eq!(
            unsafe { libc::close(vault_fd) },
            0,
            "close parent seed vault pipe"
        );
        child.expect("spawn recoverable-profile seeder")
    });
    let output = child
        .wait_with_output()
        .await
        .expect("wait for recoverable-profile seeder");
    assert!(
        output.status.success(),
        "recoverable-profile seeder failed: status={:?}\nstdout:\n{}\nstderr:\n{}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

fn preserve_crash_evidence(env: &TempEnv) -> tempfile::TempDir {
    let destination = tempfile::Builder::new()
        .prefix("waylandcore-f14-")
        .tempdir()
        .expect("create private F14 evidence directory");
    copy_directory(env.home(), destination.path()).expect("preserve F14 crash profile");
    destination
}

fn copy_directory(source: &Path, destination: &Path) -> io::Result<()> {
    fs::create_dir_all(destination)?;
    for entry in fs::read_dir(source)? {
        let entry = entry?;
        let source_path = entry.path();
        let destination_path = destination.join(entry.file_name());
        let metadata = fs::symlink_metadata(&source_path)?;
        if metadata.is_dir() {
            copy_directory(&source_path, &destination_path)?;
        } else if metadata.is_file() {
            fs::copy(&source_path, &destination_path)?;
        }
    }
    Ok(())
}

struct ConfidentialProbe<'a> {
    label: &'a str,
    value: &'a str,
}

fn byte_offsets(bytes: &[u8], needle: &[u8]) -> Vec<usize> {
    if needle.is_empty() || needle.len() > bytes.len() {
        return Vec::new();
    }
    bytes
        .windows(needle.len())
        .enumerate()
        .filter_map(|(offset, candidate)| (candidate == needle).then_some(offset))
        .collect()
}

fn collect_profile_files(root: &Path, current: &Path, files: &mut Vec<PathBuf>) {
    for entry in fs::read_dir(current).expect("read preserved profile directory") {
        let entry = entry.expect("read preserved profile entry");
        let path = entry.path();
        let metadata = fs::symlink_metadata(&path).expect("stat preserved profile entry");
        if metadata.is_dir() {
            collect_profile_files(root, &path, files);
        } else if metadata.is_file() {
            files.push(
                path.strip_prefix(root)
                    .expect("profile file below evidence root")
                    .to_path_buf(),
            );
        }
    }
}

fn journal_frames(bytes: &[u8]) -> Vec<(usize, Value)> {
    let mut frames = Vec::new();
    let mut offset = 0;
    let mut frame = 1;
    while offset + 12 <= bytes.len() {
        // A session journal interleaves two frame kinds that share the same
        // 12-byte header + body + 32-byte digest layout (session_journal.rs
        // `encode_frame` / `encode_snapshot_authority_frame`): `WJ01` event
        // envelopes and `WSA1` snapshot-authority bindings. The product parser
        // (`parse_complete_frames`) accepts both and keeps them in separate
        // collections. These F14 recovery assertions reason only about event
        // envelopes, so decode `WJ01` frames and structurally walk past `WSA1`
        // binding frames.
        let magic = &bytes[offset..offset + 4];
        assert!(magic == b"WJ01" || magic == b"WSA1", "journal frame magic");
        let is_event_frame = magic == b"WJ01";
        let length = u32::from_be_bytes(
            bytes[offset + 4..offset + 8]
                .try_into()
                .expect("journal frame length"),
        ) as usize;
        let body_start = offset + 12;
        let body_end = body_start + length;
        if body_end + 32 > bytes.len() {
            break;
        }
        if is_event_frame {
            frames.push((
                frame,
                serde_json::from_slice(&bytes[body_start..body_end])
                    .expect("decode journal frame JSON"),
            ));
        }
        offset = body_end + 32;
        frame += 1;
    }
    frames
}

fn assert_global_secret_absence(
    evidence: &Path,
    diagnostics: &[&[u8]],
    probes: &[ConfidentialProbe<'_>],
) {
    let mut files = Vec::new();
    collect_profile_files(evidence, evidence, &mut files);
    files.sort();

    let mut leaks = Vec::new();
    for relative in files {
        let path = evidence.join(&relative);
        let bytes = fs::read(&path).expect("read preserved profile file");
        for probe in probes {
            let offsets = byte_offsets(&bytes, probe.value.as_bytes());
            if offsets.is_empty() {
                continue;
            }
            leaks.push(format!(
                "{}: durable profile file {} raw_offsets={offsets:?}",
                probe.label,
                relative.display()
            ));
        }
    }
    for (stream, bytes) in diagnostics.iter().enumerate() {
        for probe in probes {
            let offsets = byte_offsets(bytes, probe.value.as_bytes());
            if !offsets.is_empty() {
                leaks.push(format!(
                    "{}: diagnostics stream {stream} raw_offsets={offsets:?}",
                    probe.label
                ));
            }
        }
    }

    assert!(
        leaks.is_empty(),
        "credential material leaked plaintext:\n{}",
        leaks.join("\n")
    );
}

fn assert_provider_checkpoint_sealed(evidence: &Path, session_id: &str, prepared_sentinel: &str) {
    let journal = evidence
        .join("sessions")
        .join(format!("{session_id}.journal"));
    let bytes = fs::read(&journal).expect("read preserved F14 journal");
    let provider_checkpoints = journal_frames(&bytes)
        .into_iter()
        .filter(|(_, envelope)| {
            envelope.pointer("/event/type").and_then(Value::as_str) == Some("checkpoint_committed")
                && envelope
                    .pointer("/event/state/next_action")
                    .and_then(Value::as_str)
                    == Some("provider_dispatch")
        })
        .collect::<Vec<_>>();
    assert_eq!(
        provider_checkpoints.len(),
        1,
        "expected exactly one provider-dispatch recovery checkpoint in {}",
        journal.display()
    );

    let (frame, envelope) = &provider_checkpoints[0];
    let state = envelope
        .pointer("/event/state")
        .and_then(Value::as_object)
        .unwrap_or_else(|| panic!("frame {frame} checkpoint state must be an object"));
    assert!(
        !state.contains_key("prepared_request"),
        "frame {frame} $.event.state.prepared_request must not persist plaintext"
    );
    let sealed = state
        .get("sealed_prepared_request")
        .and_then(Value::as_object)
        .unwrap_or_else(|| {
            panic!("frame {frame} $.event.state.sealed_prepared_request must be an envelope")
        });
    assert_eq!(
        sealed.get("envelope_version").and_then(Value::as_u64),
        Some(1),
        "frame {frame} sealed request envelope version"
    );
    assert_eq!(
        sealed.get("algorithm").and_then(Value::as_str),
        Some("xchacha20-poly1305"),
        "frame {frame} sealed request algorithm"
    );
    let ciphertext = sealed
        .get("ciphertext")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| panic!("frame {frame} sealed request ciphertext must be non-empty"));
    assert!(
        !ciphertext.contains(prepared_sentinel),
        "frame {frame} sealed request ciphertext exposed prepared-request material"
    );
    assert_eq!(
        state
            .get("request_digest")
            .and_then(Value::as_str)
            .map(str::len),
        Some(64),
        "frame {frame} sealed request must retain its digest binding"
    );
    assert!(
        state
            .get("dispatch_id")
            .and_then(Value::as_str)
            .is_some_and(|value| !value.is_empty()),
        "frame {frame} sealed request must retain its dispatch binding"
    );

    let serialized = serde_json::to_vec(envelope).expect("serialize provider checkpoint");
    assert!(
        byte_offsets(&serialized, prepared_sentinel.as_bytes()).is_empty(),
        "frame {frame} checkpoint_committed embeds prepared-request plaintext outside the sealed envelope"
    );
}

async fn wait_for_requests(fixture: &RunningOpenAiFixture, expected: usize) {
    let deadline = Instant::now() + EVENT_TIMEOUT;
    while fixture.observation().requests.len() < expected {
        assert!(
            Instant::now() < deadline,
            "fixture did not observe {expected} request(s)"
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

async fn send_message(process: &mut CoreProcess, msg_id: &str, prompt: &str) {
    process
        .send(json!({
            "type": "message",
            "msg_id": msg_id,
            "content": prompt,
            "files": [],
        }))
        .await;
}

async fn resync_current(process: &mut CoreProcess, session_id: &str, request_id: &str) -> Value {
    process
        .send(json!({
            "type": "session_resync",
            "recovery_version": 1,
            "request_id": request_id,
            "session_id": session_id,
        }))
        .await;
    let event = process.next_type("session_recovery_snapshot").await;
    assert_eq!(event["request_id"], request_id);
    assert_eq!(event["session_id"], session_id);
    assert!(
        event["cursor"]["journal_sequence"].as_u64().is_some(),
        "recovered session must have a committed cursor: {event}"
    );
    assert_eq!(
        event["cursor"]["journal_digest"].as_str().map(str::len),
        Some(64),
        "cursor digest must be a SHA-256 hex digest: {event}"
    );
    event
}

async fn resync_from_genesis(
    process: &mut CoreProcess,
    session_id: &str,
    request_id: &str,
) -> (Value, Value) {
    process
        .send(json!({
            "type": "session_resync",
            "recovery_version": 1,
            "request_id": request_id,
            "session_id": session_id,
            "after": {
                "journal_sequence": null,
                "journal_digest": GENESIS_DIGEST,
            },
        }))
        .await;
    let snapshot = process.next_type("session_recovery_snapshot").await;
    let replay = process.next_type("session_recovery_replay").await;
    let genesis = json!({"journal_digest": GENESIS_DIGEST});
    assert_eq!(snapshot["request_id"], request_id);
    assert_eq!(replay["request_id"], request_id);
    assert_eq!(snapshot["session_id"], session_id);
    assert_eq!(replay["session_id"], session_id);
    assert_eq!(snapshot["cursor"], genesis);
    assert_eq!(replay["from"], genesis);
    (snapshot, replay)
}

async fn resync_after(
    process: &mut CoreProcess,
    session_id: &str,
    request_id: &str,
    after: &Value,
) -> (Value, Value) {
    process
        .send(json!({
            "type": "session_resync",
            "recovery_version": 1,
            "request_id": request_id,
            "session_id": session_id,
            "after": after,
        }))
        .await;
    let snapshot = process.next_type("session_recovery_snapshot").await;
    let replay = process.next_type("session_recovery_replay").await;
    assert_eq!(snapshot["request_id"], request_id);
    assert_eq!(replay["request_id"], request_id);
    assert_eq!(snapshot["cursor"], *after);
    assert_eq!(replay["from"], *after);
    (snapshot, replay)
}

fn assert_contiguous_replay(after: &Value, replay: &Value) {
    let first_expected = after["journal_sequence"].as_u64().map_or(0, |seq| seq + 1);
    let items = replay["items"]
        .as_array()
        .filter(|items| !items.is_empty())
        .unwrap_or_else(|| panic!("replay must contain a suffix: {replay}"));
    for (offset, item) in items.iter().enumerate() {
        let expected = first_expected + offset as u64;
        assert_eq!(item["cursor"]["journal_sequence"], expected);
        let digest = item["cursor"]["journal_digest"]
            .as_str()
            .unwrap_or_else(|| panic!("replay item lacks a digest: {item}"));
        assert_eq!(digest.len(), 64, "replay item digest must be SHA-256 hex");
        assert!(digest.as_bytes().iter().all(u8::is_ascii_hexdigit));
    }
    assert_eq!(
        replay["through"],
        items.last().expect("non-empty replay suffix")["cursor"]
    );
}

fn journal_events(home: &Path, session_id: &str) -> Vec<Value> {
    let bytes = fs::read(home.join("sessions").join(format!("{session_id}.journal")))
        .expect("read packaged recovery journal");
    journal_frames(&bytes)
        .into_iter()
        .map(|(_, envelope)| envelope["event"].clone())
        .collect()
}

fn latest_budget_authority(events: &[Value]) -> &Value {
    events
        .iter()
        .rev()
        .find(|event| event["type"] == "budget_authority_committed")
        .and_then(|event| event.get("authority"))
        .unwrap_or_else(|| panic!("journal has no committed budget authority: {events:?}"))
}

/// The two provider meters a durable budget authority carries for one actor:
/// what is still admitted but unsettled, and what has actually been charged.
///
/// Read straight off the journal frame rather than through a live coordinator,
/// because binding a coordinator IS the restart reconciliation — only the file
/// can answer what the dead process left behind.
#[derive(Debug, Clone, PartialEq)]
struct ProviderBooks {
    /// `(session_id, input_tokens, output_tokens, usd)` per in-flight reservation.
    reserved: Vec<(String, u64, u64, f64)>,
    /// `(session_id, tokens, usd)` per settled session, `tokens` = input + output.
    charged: Vec<(String, u64, f64)>,
}

impl ProviderBooks {
    fn charged_for(&self, session_id: &str) -> (u64, f64) {
        self.charged
            .iter()
            .find(|(id, _, _)| id == session_id)
            .map(|(_, tokens, usd)| (*tokens, *usd))
            .unwrap_or((0, 0.0))
    }
}

fn provider_books(authority: &Value) -> ProviderBooks {
    let tracker = &authority["provider_tracker"];
    let reserved = tracker["reservations"]
        .as_array()
        .unwrap_or_else(|| panic!("durable authority has no reservation ledger: {authority}"))
        .iter()
        .map(|entry| {
            let reservation = &entry["reservation"];
            (
                reservation["session_id"]
                    .as_str()
                    .unwrap_or_else(|| panic!("reservation without a session: {entry}"))
                    .to_owned(),
                reservation["input_tokens"].as_u64().unwrap_or_default(),
                reservation["output_tokens"].as_u64().unwrap_or_default(),
                reservation["usd"].as_f64().unwrap_or_default(),
            )
        })
        .collect();
    let charged = tracker["per_session"]
        .as_object()
        .unwrap_or_else(|| panic!("durable authority has no per-session ledger: {authority}"))
        .iter()
        .map(|(session_id, totals)| {
            (
                session_id.clone(),
                totals["tokens"].as_u64().unwrap_or_default(),
                totals["usd"].as_f64().unwrap_or_default(),
            )
        })
        .collect();
    ProviderBooks { reserved, charged }
}

fn provider_dispatch_bindings(events: &[Value]) -> Vec<Value> {
    events
        .iter()
        .filter(|event| {
            matches!(
                event["type"].as_str(),
                Some(
                    "provider_attempt_prepared_v2"
                        | "provider_attempt_started"
                        | "provider_attempt_finished_v2"
                        | "provider_attempt_not_started_v2"
                )
            )
        })
        .cloned()
        .collect()
}

fn assert_content_free(events: &[&Value], forbidden: &[&str]) {
    let serialized = serde_json::to_string(events).expect("serialize recovery events");
    for secret in forbidden {
        assert!(
            !serialized.contains(secret),
            "recovery projection leaked forbidden content {secret:?}: {serialized}"
        );
    }
}

fn sanitize_recovery_projection(mut projection: Value) -> Value {
    // Each independently booted transport commits a fresh budget-authority
    // frame containing wall-clock evidence, so identical crash-profile clones
    // reach the same cursor sequence with different valid SHA-256 digests.
    // Preserve and compare the sequence; validate, then normalize only that
    // process-specific digest before comparing the semantic projection.
    let digest = projection
        .pointer("/cursor/journal_digest")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("recovery projection lacks cursor digest: {projection}"))
        .to_string();
    assert_eq!(digest.len(), 64, "recovery cursor digest length");
    assert!(
        digest.as_bytes().iter().all(u8::is_ascii_hexdigit),
        "recovery cursor digest must be hex: {digest}"
    );
    *projection
        .pointer_mut("/cursor/journal_digest")
        .expect("recovery cursor digest field") = Value::String("<journal-digest>".into());
    projection
}

fn assert_one_provider_request(fixture: &RunningOpenAiFixture) {
    let observation = fixture.observation();
    assert_eq!(
        observation.requests.len(),
        1,
        "recovery duplicated provider dispatch: {observation:?}"
    );
}

async fn assert_provider_request_count_stable(fixture: &RunningOpenAiFixture, expected: usize) {
    assert_eq!(fixture.observation().requests.len(), expected);
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(
        fixture.observation().requests.len(),
        expected,
        "provider dispatch occurred after the recovery action was terminal"
    );
}

async fn resume_turn_continue(
    process: &mut CoreProcess,
    snapshot: &Value,
    session_id: &str,
    request_id: &str,
) {
    process
        .send(json!({
            "type": "resume_turn",
            "recovery_version": 1,
            "request_id": request_id,
            "session_id": session_id,
            "turn_id": snapshot["pending_turn"]["turn_id"],
            "cursor": snapshot["cursor"],
            "action": "continue",
        }))
        .await;
}

#[test]
fn packaged_local_surfaces_pin_distinct_policy_provenance() {
    let config = wcore_config::config::Config::default();
    let host = wcore_cli::packaged_runtime::resolve_local_execution(
        &config,
        false,
        false,
        wcore_types::execution_policy::DEFAULT_DANGEROUS_SESSION_TTL_SECS,
        true,
    )
    .expect("resolve packaged host policy source");
    let tui = wcore_cli::packaged_runtime::resolve_local_execution(
        &config,
        false,
        false,
        wcore_types::execution_policy::DEFAULT_DANGEROUS_SESSION_TTL_SECS,
        false,
    )
    .expect("resolve packaged TUI policy source");

    assert_eq!(
        host.baseline().source(),
        wcore_types::execution_policy::PolicySource::DesktopLocalLaunch
    );
    assert_eq!(
        tui.baseline().source(),
        wcore_types::execution_policy::PolicySource::LocalCliLaunch
    );
}

/// Declare `[session] require_durability = true` in a hermetic profile.
///
/// The key is inserted INTO the existing `[session]` table rather than appended
/// as a new one: `tempenv::build` already emits `[session]`, and a second table
/// with the same name is a TOML duplicate-key error, which the product reports
/// as a config-parse refusal. That refusal is indistinguishable at the exit
/// code from the policy refusal under test, so appending would have produced a
/// test that passed for the wrong reason — it did, on the first run.
#[cfg(target_os = "linux")]
fn require_durability(env: &TempEnv) {
    let path = env.home().join("config.toml");
    let config = fs::read_to_string(&path).expect("read seeded profile config");
    assert!(
        config.contains("[session]\n"),
        "the seeded profile no longer has a [session] table to extend:\n{config}"
    );
    let patched = config.replacen("[session]\n", "[session]\nrequire_durability = true\n", 1);
    assert_ne!(patched, config, "durability policy was not inserted");
    fs::write(&path, patched).expect("write durability policy into profile config");
}

/// Launch packaged Core with NO secure store — no OS keyring, no unlocked
/// vault — and return its `ready` frame.
///
/// This function used to exist because a keyless launch could not satisfy
/// [`CoreProcess::launch`]'s assertion that `ready` names the requested
/// session: the host-forced degrade turned durable sessions off, so there was
/// no session to name and `ProtocolEvent::Ready.session_id` was simply OMITTED
/// (`Option<String>` + `skip_serializing_if`). A `--json-stream` host received a
/// frame byte-identical in shape to a legacy producer's and was told nothing.
///
/// It now asserts the identity itself, because a keyless host HAS a session:
/// the journal is not encrypted and never needed a key, so it opens, and the
/// only thing given up is the sealed replay copy of the provider request. The
/// separate launcher survives only because the environment it builds differs —
/// it must actively BREAK the keyring rather than supply a vault.
#[cfg(target_os = "linux")]
async fn launch_keyless(
    env: &TempEnv,
    fixture: &RunningOpenAiFixture,
    session_id: &str,
) -> (CoreProcess, Value) {
    let (process, ready) = spawn_keyless(keyless_command(env, fixture, Some(session_id))).await;
    // THE HOST-FACING CONTRACT, read the way a host reads it: off the wire,
    // from the real packaged binary, on a real keyring-less profile.
    //
    // This assertion is the inverse of the one it replaces
    // (`ready.get("session_id") == None`). That line pinned the DEFECT: a host
    // was told nothing, in a frame indistinguishable from a legacy producer's,
    // about a deployment that had just silently lost its audit trail. Now the
    // keyless launch opens the session it was asked for and says which one, and
    // the residue assertions downstream prove that identity is backed by real
    // artifacts rather than being another unbacked claim.
    assert_eq!(
        ready.get("session_id").and_then(Value::as_str),
        Some(session_id),
        "a keyless host still journals, so its ready frame must name the session \
         it opened: {ready}"
    );
    // …and it must NOT call that session `durable`. This is the assertion the
    // whole fourth enum value exists for, made against the real binary rather
    // than a unit mapping: `durable` is what a host reads to decide whether to
    // WAIT for an interrupted turn to recover itself, and this session cannot.
    // Naming the session while over-claiming its recovery is the same defect as
    // dropping the key, moved one field along.
    assert_eq!(
        ready["session_persistence"], "journaled_without_replay",
        "a keyless host names its session but must not promise crash replay: {ready}"
    );
    (process, ready)
}

/// The launch environment that makes this host keyless: no vault unlock
/// material, and a `DBUS_SESSION_BUS_ADDRESS` pointed at a socket that does not
/// exist so the Linux Secret Service probe fails deterministically rather than
/// depending on the worker's desktop state.
#[cfg(target_os = "linux")]
fn keyless_command(
    env: &TempEnv,
    fixture: &RunningOpenAiFixture,
    session_id: Option<&str>,
) -> Command {
    let mut command = Command::new(binary());
    command
        .arg("--json-stream")
        .arg("--provider")
        .arg("openai")
        .arg("--model")
        .arg(FIXTURE_MODEL)
        .arg("--base-url")
        .arg(fixture.base_url());
    if let Some(session_id) = session_id {
        command.arg("--session-id").arg(session_id);
    }
    command
        .current_dir(env.path())
        .env("HOME", env.path())
        .env("WAYLAND_HOME", env.home())
        .env("OPENAI_API_KEY", FIXTURE_KEY)
        .env_remove("WAYLAND_VAULT_PASSPHRASE")
        .env_remove("WAYLAND_VAULT_PASSPHRASE_FD")
        .env_remove("ANTHROPIC_API_KEY")
        .env_remove("DEEPSEEK_API_KEY")
        .env_remove("GEMINI_API_KEY")
        .env_remove("GOOGLE_API_KEY")
        .env(
            "DBUS_SESSION_BUS_ADDRESS",
            format!(
                "unix:path={}",
                env.path().join("missing-secret-service-bus").display()
            ),
        )
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    command
}

/// THE CONTROL ARM for every presence this file now asserts about a keyless
/// run: the same keyless host, with `[session] enabled = false`, which must
/// still leave nothing.
#[cfg(target_os = "linux")]
async fn launch_sessions_off(
    env: &TempEnv,
    fixture: &RunningOpenAiFixture,
) -> (CoreProcess, Value) {
    let (process, ready) = spawn_keyless(keyless_command(env, fixture, None)).await;
    // THE THIRD LIVE POSTURE, and the one that keeps the other two honest. This
    // profile is on the SAME keyring-less host as `launch_keyless` — the only
    // difference is `[session] enabled = false` — so if the producer attributed
    // a null session to the host rather than the operator, this is where it
    // would show. It would send an operator hunting for a keyring to restore a
    // journal they switched off themselves.
    assert_eq!(
        ready["session_id"],
        Value::Null,
        "an operator who turned sessions off has no session to name: {ready}"
    );
    assert_eq!(
        ready["session_persistence"], "disabled_by_operator",
        "a keyless host must not claim credit for a choice the operator made: {ready}"
    );
    (process, ready)
}

/// Declare `[session] enabled = false` in a hermetic profile.
///
/// Inserted INTO the existing `[session]` table for the reason
/// [`require_durability`] documents: a second table with the same name is a
/// TOML duplicate-key error, which the product reports as a config-parse
/// refusal — indistinguishable at the exit code from the behaviour under test.
#[cfg(target_os = "linux")]
fn disable_sessions(env: &TempEnv) {
    let path = env.home().join("config.toml");
    let config = fs::read_to_string(&path).expect("read seeded profile config");
    assert!(
        config.contains("[session]\n"),
        "the seeded profile no longer has a [session] table to extend:\n{config}"
    );
    let patched = config.replacen("[session]\n", "[session]\nenabled = false\n", 1);
    assert_ne!(patched, config, "sessions-off policy was not inserted");
    fs::write(&path, patched).expect("write sessions-off policy into profile config");
}

#[cfg(target_os = "linux")]
async fn spawn_keyless(mut command: Command) -> (CoreProcess, Value) {
    let mut child = OwnedTree::new(command.spawn().expect("spawn keyless wayland-core"));
    let stdin = child.stdin.take().expect("Core stdin pipe");
    let stdout = BufReader::new(child.stdout.take().expect("Core stdout pipe")).lines();
    let mut child_stderr = child.stderr.take().expect("Core stderr pipe");
    let stderr = Arc::new(Mutex::new(Vec::new()));
    let stderr_capture = Arc::clone(&stderr);
    let stderr_task = tokio::spawn(async move {
        let mut chunk = [0_u8; 4096];
        while let Ok(read) = child_stderr.read(&mut chunk).await {
            if read == 0 {
                break;
            }
            let mut output = stderr_capture.lock().expect("lock Core stderr capture");
            let remaining = 128 * 1024_usize - output.len().min(128 * 1024);
            output.extend_from_slice(&chunk[..read.min(remaining)]);
        }
    });
    let mut process = CoreProcess {
        child,
        stdin,
        stdout,
        stderr,
        stderr_task,
    };
    let ready = process.next_type("ready").await;
    (process, ready)
}

/// Every file under a profile home, relative to it, with its bytes.
///
/// Deliberately NOT a glob for `sessions/*.journal`. The journal family is six
/// artifacts, not one: `<id>.journal`, `<id>.wal`, `<id>.journal.snapshot`,
/// `<id>.journal.authority`, the `.<id>.journal.effects/` checkpoint directory
/// and the `.<digest>.<pid>.<seq>.tmp` files inside it
/// (`session_journal.rs:1075`, `snapshot.rs:262`/`:334`,
/// `session_journal.rs:1172`/`:735`). A test that inspected only the first
/// would miss five sixths of what a run leaves behind — in either direction.
#[cfg(target_os = "linux")]
fn profile_contents(home: &Path) -> Vec<(PathBuf, Vec<u8>)> {
    if !home.exists() {
        return Vec::new();
    }
    let mut relative = Vec::new();
    collect_profile_files(home, home, &mut relative);
    relative.sort();
    relative
        .into_iter()
        .map(|path| {
            let bytes = fs::read(home.join(&path)).unwrap_or_default();
            (path, bytes)
        })
        .collect()
}

/// How many entries the profile's `sessions/` directory holds.
///
/// A missing directory counts as 0, so this is meaningful whether or not the
/// harness pre-created it.
#[cfg(target_os = "linux")]
fn session_directory_entries(home: &Path) -> usize {
    fs::read_dir(home.join("sessions"))
        .map(|entries| entries.count())
        .unwrap_or(0)
}

/// Which files under a profile carry a SEALED prepared provider request —
/// meaning the field is present AND carries a value.
///
/// This is the one thing a keyless host must not have written, and the one
/// thing that separates "journal without the seal" from "journal exactly as
/// before". Matched on the serialized field rather than on ciphertext, because
/// ciphertext is unrecognisable by construction: an absence of anything
/// unreadable is not evidence, whereas an absence of the field that carries it
/// is.
///
/// INSTRUMENT DEFECT, found by this test and repaired rather than written up:
/// v1 matched the field NAME alone and reported a keyless run as having sealed
/// a request. `RecoveryCheckpoint.sealed_prepared_request` has no
/// `skip_serializing_if`, so EVERY checkpoint serializes the key — including
/// the terminal `CommitTurn` checkpoint that closes a keyless turn, which
/// writes `"sealed_prepared_request":null`. The name is structural; only the
/// value is evidence. A needle that matches on every profile can neither pass
/// nor fail meaningfully, and the vault control would have hidden it by
/// agreeing.
///
/// Paired with a vault control in every caller: on a profile WITH a key this
/// must return files, or the emptiness asserted on the keyless profile is just
/// a needle nothing ever writes.
#[cfg(target_os = "linux")]
fn sealed_request_artifacts(home: &Path) -> Vec<PathBuf> {
    profile_contents(home)
        .into_iter()
        .filter(|(_, bytes)| {
            String::from_utf8_lossy(bytes)
                .split("\"sealed_prepared_request\":")
                .skip(1)
                .any(|value| !value.trim_start().starts_with("null"))
        })
        .map(|(path, _)| path)
        .collect()
}

/// The control for [`sealed_request_artifacts`]'s own repair: how many
/// checkpoints mention the field at all, sealed or not.
///
/// Without this, the fixed probe returning empty on a keyless profile is
/// indistinguishable from a profile whose checkpoints do not carry the field —
/// which is what the original defect looked like from the other side.
#[cfg(target_os = "linux")]
fn checkpoints_mentioning_the_seal(home: &Path) -> usize {
    profile_contents(home)
        .into_iter()
        .map(|(_, bytes)| byte_offsets(&bytes, b"\"sealed_prepared_request\":").len())
        .sum()
}

/// The distinct journal event types recorded for a session, as a set.
///
/// Read from the journal FILE, not from a live reducer: the question is what a
/// dead process left on disk for whoever has to reconcile it, and only the file
/// can answer that.
#[cfg(target_os = "linux")]
fn journal_event_types(home: &Path, session_id: &str) -> std::collections::BTreeSet<String> {
    journal_events(home, session_id)
        .into_iter()
        .filter_map(|event| event["type"].as_str().map(str::to_owned))
        .collect()
}

/// Grade one profile for durable-session residue: which artifacts of the
/// journal family exist, and in which files the prompt appears.
///
/// Returns `(artifact paths, files containing the prompt)` so a caller can
/// assert either direction AND a control caller can prove the same walker
/// answers the other way on a profile built to. Without that control every
/// "zero" is also what a broken walker, a wrong path, or a home that never
/// existed would return — and every non-zero is what a walker that matched
/// everything would return.
#[cfg(target_os = "linux")]
fn durable_residue(home: &Path, prompt: &str) -> (Vec<PathBuf>, Vec<PathBuf>) {
    const FAMILY: &[&str] = &[
        ".journal",
        ".wal",
        ".snapshot",
        ".authority",
        ".effects",
        ".tmp",
    ];
    let contents = profile_contents(home);
    let artifacts = contents
        .iter()
        .filter(|(path, _)| {
            let text = path.to_string_lossy();
            text.starts_with("sessions/") || FAMILY.iter().any(|suffix| text.contains(suffix))
        })
        .map(|(path, _)| path.clone())
        .collect();
    let leaked = contents
        .iter()
        .filter(|(_, bytes)| !byte_offsets(bytes, prompt.as_bytes()).is_empty())
        .map(|(path, _)| path.clone())
        .collect();
    (artifacts, leaked)
}

/// DEGRADE FORBIDDEN. An operator who declared this deployment requires durable
/// sessions gets the July-2026 posture back, unchanged where it matters: the
/// provider is never reached and no turn is ever started.
///
/// This is the half of the pair that must NOT be allowed to erode. The
/// host-forced degrade that shipped on 2026-07-30 reversed the decision
/// `906287e1` took, and the natural repair — rewrite the old test to assert the
/// new behaviour — would have widened the degrade path silently and left
/// nothing asserting that refusing is still reachable at all.
///
/// What it deliberately does NOT preserve: the old test also read the session
/// journal back and asserted it contained no `turn_started`. Under the policy
/// the refusal happens during config resolution, upstream of every engine, so
/// there is no journal to read. That is a stronger property than the one it
/// replaces — nothing at all was written — and it is asserted below rather
/// than dropped.
#[cfg(target_os = "linux")]
#[tokio::test]
async fn without_secure_store_an_operator_who_requires_durability_gets_a_refusal() {
    let fixture = OpenAiFixtureScript::new([OpenAiStep::text("MUST-NOT-DISPATCH")])
        .start()
        .await
        .expect("start secure-storage policy fixture");
    let env = environment(&fixture);
    require_durability(&env);
    let session_id = "f1400000000000000000000000000000";
    let prompt = "F14-UNPROTECTED-PROMPT-MUST-NOT-BECOME-DURABLE";

    let mut command = Command::new(binary());
    command
        .arg("--json-stream")
        .arg("--provider")
        .arg("openai")
        .arg("--model")
        .arg(FIXTURE_MODEL)
        .arg("--base-url")
        .arg(fixture.base_url())
        .arg("--session-id")
        .arg(session_id)
        .current_dir(env.path())
        .env("HOME", env.path())
        .env("WAYLAND_HOME", env.home())
        .env("OPENAI_API_KEY", FIXTURE_KEY)
        .env_remove("WAYLAND_VAULT_PASSPHRASE")
        .env_remove("WAYLAND_VAULT_PASSPHRASE_FD")
        .env_remove("ANTHROPIC_API_KEY")
        .env_remove("DEEPSEEK_API_KEY")
        .env_remove("GEMINI_API_KEY")
        .env_remove("GOOGLE_API_KEY")
        // Make the Linux Secret Service probe deterministically unavailable
        // rather than depending on the worker's desktop/session state.
        .env(
            "DBUS_SESSION_BUS_ADDRESS",
            format!(
                "unix:path={}",
                env.path().join("missing-secret-service-bus").display()
            ),
        )
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = OwnedTree::new(command.spawn().expect("spawn packaged wayland-core"));
    drop(child.stdin.take());
    let output = tokio::time::timeout(EVENT_TIMEOUT, child.wait_with_output())
        .await
        .expect("required-durability launch terminated")
        .expect("collect required-durability launch output");
    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();

    assert_ne!(
        output.status.code(),
        Some(0),
        "a host that cannot deliver required durability must not start. \
         stdout:\n{stdout}\nstderr:\n{stderr}"
    );

    // THE ASSERTION CARRIED FORWARD FROM 906287e1, VERBATIM IN MEANING: the
    // provider is never reached.
    assert!(
        fixture.observation().requests.is_empty(),
        "a required-durability refusal reached the provider: {:?}",
        fixture.observation().requests
    );

    let frames: Vec<Value> = stdout
        .lines()
        .filter(|line| !line.trim().is_empty())
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .collect();
    assert!(
        !frames
            .iter()
            .any(|frame| frame.get("type").and_then(Value::as_str) == Some("ready")),
        "a refused start must not claim ready: {stdout}"
    );
    let errors: Vec<&Value> = frames
        .iter()
        .filter(|frame| frame.get("type").and_then(Value::as_str) == Some("error"))
        .collect();
    assert_eq!(
        errors.len(),
        1,
        "the host must receive exactly one refusal frame on stdout: {stdout}"
    );
    assert_eq!(errors[0]["error"]["retryable"], false);
    let message = errors[0]["error"]["message"]
        .as_str()
        .unwrap_or_else(|| panic!("refusal frame lacks a message: {}", errors[0]));
    for needle in [
        "require_durability",
        "OS keyring",
        "credentials vault",
        "WAYLAND_VAULT_PASSPHRASE",
    ] {
        assert!(
            message.contains(needle),
            "the refusal must tell the host {needle:?}; got: {message}"
        );
    }

    // Stronger than the journal check it replaces: nothing durable exists.
    let (artifacts, leaked) = durable_residue(env.home(), prompt);
    assert!(
        artifacts.is_empty(),
        "a refused start left durable session artifacts: {artifacts:?}"
    );
    assert!(
        leaked.is_empty(),
        "a refused start persisted the prompt: {leaked:?}"
    );
}

/// DEGRADE ALLOWED. The default on a keyless host: the turn runs, it IS
/// recorded, the operator is told what was actually given up, and the ONLY
/// thing missing from disk is the sealed copy of the provider request.
///
/// The second half of the pair, and the half that changed. It used to assert
/// that a keyless run leaves NOTHING durable — which was true, and was the
/// defect: the journal is not encrypted, the confidential store protects one
/// field, and giving up the whole audit trail to protect that one field made
/// "suppress the keyring" a way to obtain unrecorded execution.
///
/// So this now asserts three things a single "artifacts exist" check cannot
/// separate:
///
/// 1. artifacts EXIST — the journal family is really on disk;
/// 2. the write-ahead pairs that prove what executed are really IN them;
/// 3. `sealed_prepared_request` is NOT — the degrade is real, not a no-op.
///
/// (3) is what stops (1) and (2) passing on a build where nothing degraded at
/// all, and the vault control at the end is what stops (3) passing on a needle
/// nothing ever writes.
#[cfg(target_os = "linux")]
#[tokio::test]
async fn without_secure_store_the_default_journals_every_effect_but_seals_nothing() {
    let fixture = OpenAiFixtureScript::new([
        OpenAiStep::text("F14-KEYLESS-REPLY-1"),
        OpenAiStep::text("F14-KEYLESS-REPLY-2"),
    ])
    .start()
    .await
    .expect("start keyless-run fixture");
    let env = environment(&fixture);
    let session_id = "f1400000000000000000000000000005";
    let prompt = "F14-KEYLESS-PROMPT-MUST-BECOME-DURABLE";

    let (mut process, _ready) = launch_keyless(&env, &fixture, session_id).await;

    // TWO turns, deliberately. A startup notice is indistinguishable from a
    // per-turn notice if you only ever run one turn, and "the notice fired
    // once, three weeks ago" is precisely the defect. The second turn is the
    // only assertion that can tell them apart.
    for (index, msg_id) in ["f14-keyless-1", "f14-keyless-2"].into_iter().enumerate() {
        send_message(&mut process, msg_id, prompt).await;
        let notice = process
            .next_info_containing("crash replay protection is OFF", msg_id)
            .await;
        assert_eq!(
            notice["msg_id"], msg_id,
            "the replay-degrade notice must be correlated to the turn it concerns"
        );
        // The notice must say what is STILL true, not only what is lost. Its
        // predecessor said "this turn is not being recorded" — now false, and
        // the more alarming half. A notice that overstates the loss is filtered
        // as noise, and the real loss goes unread with it.
        let text = notice["message"]
            .as_str()
            .unwrap_or_else(|| panic!("degrade notice carries no text: {notice}"));
        assert!(
            text.contains("IS being recorded"),
            "the notice must state what survives, not only what is lost: {text}"
        );
        let terminal = process.next_type("stream_end").await;
        assert_eq!(terminal["msg_id"], msg_id);
        assert_eq!(
            terminal["finish_reason"], "stop",
            "keyless turn {index} must actually complete: {terminal}"
        );
    }
    assert_eq!(
        fixture.observation().requests.len(),
        2,
        "both keyless turns must reach the provider, exactly once each"
    );

    let stderr = String::from_utf8_lossy(&process.sigkill().await).into_owned();
    assert!(
        stderr.contains("crash replay protection is OFF"),
        "a keyless run must tell the operator; stderr was:\n{stderr}"
    );

    // (1) THE JOURNAL IS REALLY THERE.
    let (artifacts, leaked) = durable_residue(env.home(), prompt);
    assert!(
        !artifacts.is_empty(),
        "a keyless run wrote NO durable session artifacts. That is the amnesia \
         this posture exists to end: the journal needs no key, and giving it up \
         to protect one sealed field made suppressing a keyring a way to obtain \
         unrecorded execution"
    );
    assert!(
        !leaked.is_empty(),
        "the prompt is nowhere on disk, so no conversation was journaled: \
         artifacts={artifacts:?}"
    );
    assert_ne!(
        session_directory_entries(env.home()),
        0,
        "a keyless run left the sessions directory empty"
    );

    // (2) AND IT CONTAINS THE WRITE-AHEAD PAIRS, not merely bytes.
    //
    // An artifact count cannot tell a real journal from an empty file with the
    // right name. These are the keyless v1 boundaries — no dispatch id needed,
    // no key involved — that let someone reconcile what actually executed.
    let types = journal_event_types(env.home(), session_id);
    for required in [
        "turn_started",
        "provider_attempt_prepared_v2",
        "provider_attempt_started",
        "turn_committed",
    ] {
        assert!(
            types.contains(required),
            "the keyless journal is missing the {required:?} write-ahead record, \
             so it cannot say what executed. Present: {types:?}"
        );
    }

    // (3) AND THE ONE FIELD THAT NEEDS A KEY CARRIES NOTHING.
    let sealed = sealed_request_artifacts(env.home());
    assert!(
        sealed.is_empty(),
        "a host with no keyring sealed a prepared request anyway, into {sealed:?}. \
         Either the degrade did not happen, or something wrote ciphertext under a \
         key it cannot have"
    );
    // …and the probe was looking at checkpoints that DO carry the field, so the
    // emptiness above is about the value and not about the field being absent.
    assert_ne!(
        checkpoints_mentioning_the_seal(env.home()),
        0,
        "no checkpoint on this profile mentions sealed_prepared_request at all, \
         so the empty result above says nothing about whether anything was sealed"
    );

    // KNOWN-POSITIVE CONTROL, in the same test, for the same three probes.
    //
    // (3) is an ABSENCE, and an absence is also what a misspelled needle, a
    // walker pointed at the wrong path, or a home that was never created
    // returns. So run the identical arm WITH vault unlock material and require
    // the seal to appear. Without this, a build that never seals ANYTHING —
    // i.e. one where the repair silently broke durable mode too — passes.
    let control_fixture = OpenAiFixtureScript::new([OpenAiStep::text("F14-DURABLE-REPLY")])
        .start()
        .await
        .expect("start durable control fixture");
    let control_env = environment(&control_fixture);
    let control_vault = VaultSecret::new();
    let control_prompt = "F14-DURABLE-PROMPT-MUST-BECOME-DURABLE";
    let mut control = CoreProcess::launch(
        &control_env,
        &control_fixture,
        &control_vault,
        "f1400000000000000000000000000006",
        false,
    )
    .await;
    send_message(&mut control, "f14-durable-control", control_prompt).await;
    let control_terminal = control.next_type("stream_end").await;
    assert_eq!(control_terminal["finish_reason"], "stop");
    let _ = control.sigkill().await;

    assert!(
        !sealed_request_artifacts(control_env.home()).is_empty(),
        "CONTROL FAILED: a profile WITH an unlocked vault sealed nothing either, \
         so the keyless absence asserted above proves nothing about the degrade — \
         it only proves the needle is never written"
    );

    // SECOND CONTROL, for the opposite direction: the residue walker must still
    // be capable of returning EMPTY. Every assertion in (1) is a presence now,
    // and a walker that matched every file would satisfy all of them. An
    // operator who turned sessions off is the profile that must stay clean.
    let off_fixture = OpenAiFixtureScript::new([OpenAiStep::text("F14-SESSIONS-OFF-REPLY")])
        .start()
        .await
        .expect("start sessions-off control fixture");
    let off_env = environment(&off_fixture);
    let off_prompt = "F14-SESSIONS-OFF-PROMPT-MUST-NOT-BECOME-DURABLE";
    disable_sessions(&off_env);
    let (mut off, _off_ready) = launch_sessions_off(&off_env, &off_fixture).await;
    send_message(&mut off, "f14-sessions-off", off_prompt).await;
    let off_terminal = off.next_type("stream_end").await;
    assert_eq!(off_terminal["finish_reason"], "stop");
    let _ = off.sigkill().await;

    let (off_artifacts, off_leaked) = durable_residue(off_env.home(), off_prompt);
    assert!(
        off_artifacts.is_empty() && off_leaked.is_empty(),
        "CONTROL FAILED: the residue walker reports artifacts on a profile with \
         [session] enabled = false, so the presences asserted above are what this \
         walker returns for everything: artifacts={off_artifacts:?} \
         leaked={off_leaked:?}"
    );
}

/// HIGH-1. A session whose sealed state cannot be opened is refused BY NAME,
/// and the refusal is scoped to that session — not to the host, not to the
/// process, and not to the profile.
///
/// # What was measured before this
///
/// Seed a profile with a real recoverable turn under a vault, delete the vault,
/// relaunch with `--resume <that id>`:
///
/// ```text
/// ARM_RC[journal-exists-key-gone]=0
/// FRAME_TYPES: ["ready(session_id='da00000000000000000000000000005')"]
/// SESSION_ENTRIES=6   (unchanged, unread)
/// ```
///
/// It started, emitted `ready` CARRYING that session id, and proceeded, while
/// six journal artifacts sat on disk untouched and unread. That is worse than
/// an anonymous degrade: a host told nothing retries or asks; a host told
/// `ready(session_id=X)` builds its entire session view on a continuity claim
/// that is false.
///
/// # The three arms, and why all three are needed
///
/// 1. **REFUSED** — `--resume` the locked session, keyless. No `ready`, one
///    non-retryable error naming the session, non-zero exit, and the journal
///    left byte-identical because it is evidence.
/// 2. **NOT A BRICK** — the SAME profile on the SAME keyless host, launched
///    without naming that session, starts and journals normally. This is the
///    arm that separates this remedy from the one it replaces: a refusal that
///    took the whole process down would hand anyone who can stop a D-Bus a
///    complete availability kill, converting an attack on confidentiality into
///    an attack on availability.
/// 3. **NOT PERMANENT** — the SAME profile, the SAME session, WITH the vault
///    restored, resumes. Without this, arm 1 also passes on a build that
///    refuses every resume, and the "restore the key and resume again" promise
///    in the refusal text would be untested.
#[cfg(target_os = "linux")]
#[tokio::test]
async fn a_session_whose_key_is_gone_is_refused_by_name_and_only_that_session() {
    let fixture = OpenAiFixtureScript::new([OpenAiStep::text("MUST-NOT-DISPATCH")])
        .start()
        .await
        .expect("start locked-session fixture");
    let env = environment(&fixture);
    let vault = VaultSecret::new();
    let session_id = "f1400000000000000000000000000009";
    let prompt = "F14-LOCKED-SESSION-PROMPT";

    // The product's own production-shaped seeder, in a child process, under a
    // vault: a real interrupted turn at a real sealed provider-dispatch
    // checkpoint. Nothing here is a hand-written journal.
    seed_recoverable_profile(
        &env,
        &fixture,
        &vault,
        session_id,
        "turn-f14-locked",
        prompt,
        false,
    )
    .await;
    let journal_path = env
        .home()
        .join("sessions")
        .join(format!("{session_id}.journal"));
    let seeded_journal = fs::read(&journal_path).expect("read the seeded journal");
    let seeded_sessions = session_directory_entries(env.home());
    assert_ne!(
        seeded_sessions, 0,
        "the seeder produced no session artifacts, so there is nothing to lock"
    );
    assert!(
        !sealed_request_artifacts(env.home()).is_empty(),
        "the seeded profile carries no sealed prepared request, so the arm below \
         would be refusing a session that never needed a key"
    );

    // ARM 1 — REFUSED. The vault is gone; the journal is not.
    let mut command = keyless_command(&env, &fixture, None);
    command
        .arg("--resume")
        .arg(session_id)
        .stdin(Stdio::null())
        .kill_on_drop(false);
    let output = tokio::time::timeout(
        EVENT_TIMEOUT,
        OwnedTree::new(
            command
                .spawn()
                .expect("spawn keyless resume of a locked session"),
        )
        .wait_with_output(),
    )
    .await
    .expect("locked-session resume terminated")
    .expect("collect locked-session resume output");
    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();

    assert_ne!(
        output.status.code(),
        Some(0),
        "resuming a session whose sealed state cannot be opened must not succeed. \
         stdout:\n{stdout}\nstderr:\n{stderr}"
    );
    let frames: Vec<Value> = stdout
        .lines()
        .filter(|line| !line.trim().is_empty())
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .collect();
    // THE DEFECT, pinned in the one place it showed: no `ready`, at all. The
    // frame was the whole of the false claim.
    assert!(
        !frames
            .iter()
            .any(|frame| frame.get("type").and_then(Value::as_str) == Some("ready")),
        "a refused resume claimed ready, which is the continuity claim this fixes: \
         {stdout}"
    );
    let errors: Vec<&Value> = frames
        .iter()
        .filter(|frame| frame.get("type").and_then(Value::as_str) == Some("error"))
        .collect();
    assert_eq!(
        errors.len(),
        1,
        "the host must receive exactly one refusal frame: {stdout}"
    );
    assert_eq!(errors[0]["error"]["retryable"], false);
    let message = errors[0]["error"]["message"]
        .as_str()
        .unwrap_or_else(|| panic!("refusal frame lacks a message: {}", errors[0]));
    for needle in [
        session_id,
        "cannot be resumed",
        "Only THIS session is refused",
        "starting a new session on this host works normally",
        "WAYLAND_VAULT_PASSPHRASE",
    ] {
        assert!(
            message.contains(needle),
            "the refusal must tell the host {needle:?}; got: {message}"
        );
    }

    // THE JOURNAL IS EVIDENCE. A refusal that consumed, truncated or rotated
    // what it could not read would destroy the only record of the interrupted
    // turn — and would make arm 3 impossible.
    //
    // NOT byte equality, and not over the whole profile. Both were tried and
    // both red for reasons that are not the product misbehaving: a launch that
    // opens a session acquires a writer lease and pins a confidential-backend
    // marker, and `SessionJournal::open` legitimately folds a leftover WAL into
    // the journal, so the file grows on its first open after the seeder died.
    // Byte equality would forbid recovery from doing its job. What must hold is
    // that nothing was LOST: the record only ever grows, the interrupted turn's
    // own prompt is still in it, and the sealed material that caused the
    // refusal is still there. Arm 3 then proves the stronger property end to
    // end by resuming from it.
    let after = fs::read(&journal_path).expect("read the journal after the refusal");
    assert!(
        after.len() >= seeded_journal.len(),
        "the refused resume SHRANK the journal it could not read: {} -> {} bytes",
        seeded_journal.len(),
        after.len()
    );
    assert!(
        !byte_offsets(&after, prompt.as_bytes()).is_empty(),
        "the refused resume erased the interrupted turn's own prompt from the \
         journal, so the evidence it refused to act on is gone"
    );
    assert!(
        !sealed_request_artifacts(env.home()).is_empty(),
        "the refused resume destroyed the sealed material that made it refuse"
    );

    // ARM 2 — NOT A BRICK. Same host, same profile, same missing key, one
    // second later. A launch that did not ask for the locked session must be
    // completely unaffected.
    let live_fixture = OpenAiFixtureScript::new([OpenAiStep::text("F14-LOCKED-SIBLING-REPLY")])
        .start()
        .await
        .expect("start locked-session sibling fixture");
    let sibling_id = "f1400000000000000000000000000010";
    let (mut sibling, _sibling_ready) = launch_keyless(&env, &live_fixture, sibling_id).await;
    send_message(
        &mut sibling,
        "f14-locked-sibling",
        "F14-LOCKED-SIBLING-PROMPT",
    )
    .await;
    let sibling_terminal = sibling.next_type("stream_end").await;
    assert_eq!(
        sibling_terminal["finish_reason"], "stop",
        "a keyless host with one locked session must still complete turns in \
         another: {sibling_terminal}"
    );
    let _ = sibling.sigkill().await;
    assert_ne!(
        session_directory_entries(env.home()),
        seeded_sessions,
        "the sibling session journaled nothing, so the refusal did brick this \
         profile after all"
    );

    // ARM 3 — NOT PERMANENT. The same session, with the key back.
    let restore_fixture = OpenAiFixtureScript::new([OpenAiStep::text("F14-LOCKED-RESTORED-REPLY")])
        .start()
        .await
        .expect("start locked-session restore fixture");
    let restored = CoreProcess::launch(&env, &restore_fixture, &vault, session_id, true).await;
    // `CoreProcess::launch` already asserts `ready` names this session, which is
    // the whole of arm 3: with the key present the identical resume that was
    // refused above now opens.
    drop(restored);
}

/// The crash boundaries a keyless run is killed at.
///
/// A keyless run that completes cleanly and leaves a journal proves very
/// little: a completed turn has nothing half-written by construction, and its
/// record is written at leisure. The property this posture rests on is that a
/// run killed MID-EFFECT still leaves a record of the effect — because the
/// whole argument for journaling without the seal is that the ambiguous case,
/// the one where nobody can say whether the effect landed, is exactly the case
/// a record is for.
#[cfg(target_os = "linux")]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DegradedCrashBoundary {
    /// Ready, no message yet. The floor: opening a degraded session must not
    /// itself create anything.
    BeforeAnyTurn,
    /// The provider request has left, no response headers have arrived. The
    /// worst case for ambiguity — the effect may or may not have landed.
    ProviderRequestSentNoHeaders,
    /// Response headers arrived and the stream is partially consumed.
    ProviderStreamPartiallyConsumed,
    /// The model asked for a tool and the approval gate is open.
    AwaitingToolApproval,
    /// The tool was approved and its child process is running.
    ToolExecuting,
}

#[cfg(target_os = "linux")]
impl DegradedCrashBoundary {
    fn label(self) -> &'static str {
        match self {
            Self::BeforeAnyTurn => "before-any-turn",
            Self::ProviderRequestSentNoHeaders => "provider-request-sent-no-headers",
            Self::ProviderStreamPartiallyConsumed => "provider-stream-partially-consumed",
            Self::AwaitingToolApproval => "awaiting-tool-approval",
            Self::ToolExecuting => "tool-executing",
        }
    }
}

/// Drive a keyless run to `boundary`, SIGKILL it there, and return the profile
/// residue plus the number of provider requests the fixture observed.
///
/// The request count is returned so the caller can assert the run REACHED the
/// boundary it named. Without that, a boundary whose setup silently failed
/// would produce a profile with no mid-effect record in it and the failure
/// would be indistinguishable from the product not writing one — the same shape
/// as a concurrency test in which a participant never started.
#[cfg(target_os = "linux")]
async fn crash_degraded_at(boundary: DegradedCrashBoundary) -> (Vec<PathBuf>, Vec<PathBuf>, usize) {
    let label = boundary.label();
    let prompt = format!("F14-CRASH-{label}-PROMPT-MUST-BECOME-DURABLE");

    let seed = OpenAiFixtureScript::new([OpenAiStep::text("unused")])
        .start()
        .await
        .expect("start crash-boundary seed fixture");
    let env = environment(&seed);
    let marker = env.path().join(format!("f14-crash-{label}.log"));
    let pid_file = env.path().join(format!("f14-crash-{label}.pid"));
    let tool_command = format!(
        "printf '%s\\n' \"$$\" > {} && printf 'started\\n' >> {} && exec sleep 60",
        shell_quote(&pid_file),
        shell_quote(&marker),
    );
    seed.shutdown().await.expect("stop crash-boundary seed");

    let steps = match boundary {
        DegradedCrashBoundary::BeforeAnyTurn => vec![OpenAiStep::text("MUST-NOT-DISPATCH")],
        DegradedCrashBoundary::ProviderRequestSentNoHeaders => {
            vec![OpenAiStep::stall_before_headers(60_000)]
        }
        DegradedCrashBoundary::ProviderStreamPartiallyConsumed => {
            vec![OpenAiStep::text_then_stall(
                "F14-CRASH-PARTIAL-DELTA",
                60_000,
            )]
        }
        DegradedCrashBoundary::AwaitingToolApproval | DegradedCrashBoundary::ToolExecuting => {
            vec![
                OpenAiStep::tool_call(
                    "f14-crash-bash",
                    "Bash",
                    json!({"command": tool_command, "timeout": 120_000}),
                ),
                OpenAiStep::text("MUST-NOT-DISPATCH-AGAIN"),
            ]
        }
    };
    let fixture = OpenAiFixtureScript::new(steps)
        .start()
        .await
        .expect("start crash-boundary fixture");
    let _tool_guard = ToolProcessGuard {
        pid_file: pid_file.clone(),
    };

    let (mut process, _ready) =
        launch_keyless(&env, &fixture, "f1400000000000000000000000000007").await;
    let msg_id = format!("f14-crash-{label}");
    if boundary != DegradedCrashBoundary::BeforeAnyTurn {
        send_message(&mut process, &msg_id, &prompt).await;
    }
    match boundary {
        DegradedCrashBoundary::BeforeAnyTurn => {}
        DegradedCrashBoundary::ProviderRequestSentNoHeaders => {
            wait_for_requests(&fixture, 1).await;
        }
        DegradedCrashBoundary::ProviderStreamPartiallyConsumed => {
            wait_for_requests(&fixture, 1).await;
            let delta = process.next_type("text_delta").await;
            assert_eq!(
                delta["text"], "F14-CRASH-PARTIAL-DELTA",
                "{label}: the stream did not reach the partial-consumption boundary"
            );
        }
        DegradedCrashBoundary::AwaitingToolApproval => {
            wait_for_requests(&fixture, 1).await;
            let approval = process.next_type("approval_required").await;
            assert_eq!(approval["call_id"], "f14-crash-bash", "{label}");
        }
        DegradedCrashBoundary::ToolExecuting => {
            wait_for_requests(&fixture, 1).await;
            let approval = process.next_type("approval_required").await;
            assert_eq!(approval["call_id"], "f14-crash-bash", "{label}");
            process
                .send(json!({"type": "tool_approve", "call_id": "f14-crash-bash"}))
                .await;
            let running = process.next_type("tool_running").await;
            assert_eq!(running["call_id"], "f14-crash-bash", "{label}");
            // The tool's own marker file, not a protocol event: the child must
            // genuinely be executing, or this boundary is a different one.
            wait_for_file(&marker).await;
        }
    }
    let observed = fixture.observation().requests.len();
    let _diagnostics = process.sigkill().await;

    // Read the profile AFTER the kill, with no clean shutdown having run, so
    // any temp file, WAL or partially-renamed artifact is still on disk.
    let (artifacts, leaked) = durable_residue(env.home(), &prompt);
    assert_ne!(
        session_directory_entries(env.home()),
        0,
        "{label}: a keyless crash left the sessions directory empty, so nothing \
         durable survived the kill"
    );
    // The seal is the one thing that must still be missing at EVERY boundary,
    // including the ones where a durable run would have written a
    // `ProviderDispatch` checkpoint. Asserting it per boundary, not once at the
    // end, is what catches a mode that seals only on some paths.
    let sealed = sealed_request_artifacts(env.home());
    assert!(
        sealed.is_empty(),
        "{label}: a keyless crash left a sealed prepared request in {sealed:?}"
    );
    (artifacts, leaked, observed)
}

/// TASK: SIGKILL at each effect boundary; RECOVERABLE RESIDUE at every one of
/// them, of the whole journal family and not merely a `sessions/` entry.
///
/// The inverse of the matrix it replaces, and the inversion is the repair. A
/// keyless run used to leave nothing at any boundary, which was asserted as a
/// virtue; it is the failure. The boundary that matters most is
/// `provider-request-sent-no-headers` — the request has left and no answer has
/// come back, so nobody can say whether it landed. That is precisely the state a
/// durable record exists to bound, and it is the state the old posture recorded
/// nothing about.
///
/// `BeforeAnyTurn` is the one boundary that legitimately has no PROMPT on disk —
/// no message was ever sent — so it is graded on artifacts alone. Folding it in
/// with the rest would have meant weakening the prompt assertion for all five.
///
/// Five boundaries run here. The unrun cells are named rather than quietly
/// omitted — outbound channel delivery is the notable one, because this harness
/// has no channel and inventing one would prove less than saying so.
#[cfg(target_os = "linux")]
#[tokio::test]
async fn a_keyless_run_killed_at_any_effect_boundary_leaves_recoverable_residue() {
    let boundaries = [
        DegradedCrashBoundary::BeforeAnyTurn,
        DegradedCrashBoundary::ProviderRequestSentNoHeaders,
        DegradedCrashBoundary::ProviderStreamPartiallyConsumed,
        DegradedCrashBoundary::AwaitingToolApproval,
        DegradedCrashBoundary::ToolExecuting,
    ];
    let mut graded = 0usize;
    let mut dispatched = 0usize;
    for boundary in boundaries {
        let label = boundary.label();
        let (artifacts, leaked, observed) = crash_degraded_at(boundary).await;
        assert!(
            !artifacts.is_empty(),
            "{label}: a keyless crash left NO durable artifacts, so nothing records \
             that this boundary was ever crossed"
        );
        if boundary != DegradedCrashBoundary::BeforeAnyTurn {
            assert!(
                !leaked.is_empty(),
                "{label}: the prompt is nowhere on disk after the kill, so the turn \
                 that reached this boundary left no recoverable trace of itself"
            );
        }
        // Prove the run REACHED its boundary. A boundary whose setup silently
        // failed still opens a session and still writes SOMETHING, so the
        // artifact assertion above would pass on a run that never got near the
        // effect it names.
        let expected_requests = usize::from(boundary != DegradedCrashBoundary::BeforeAnyTurn);
        assert_eq!(
            observed, expected_requests,
            "{label}: the run did not reach the boundary it claims to test"
        );
        dispatched += observed;
        graded += 1;
    }
    assert_eq!(graded, boundaries.len(), "every boundary must be graded");
    assert_eq!(
        dispatched, 4,
        "four of the five boundaries must have dispatched to the provider; a total \
         of 0 would mean nothing was ever exercised"
    );

    // KNOWN-NEGATIVE CONTROL for the crash path specifically, and it is now the
    // control that has to exist. The five presences above are what a walker
    // that matched every file in a profile would also return — every profile
    // has a config.toml. So kill a run at the same partial-stream boundary with
    // `[session] enabled = false`, where nothing durable may exist, and require
    // the identical functions to come back empty.
    let fixture = OpenAiFixtureScript::new([OpenAiStep::text_then_stall(
        "F14-CRASH-CONTROL-DELTA",
        60_000,
    )])
    .start()
    .await
    .expect("start crash-control fixture");
    let env = environment(&fixture);
    let control_prompt = "F14-CRASH-CONTROL-PROMPT-MUST-NOT-BECOME-DURABLE";
    disable_sessions(&env);
    let (mut control, _control_ready) = launch_sessions_off(&env, &fixture).await;
    send_message(&mut control, "f14-crash-control", control_prompt).await;
    wait_for_requests(&fixture, 1).await;
    let delta = control.next_type("text_delta").await;
    assert_eq!(delta["text"], "F14-CRASH-CONTROL-DELTA");
    let _ = control.sigkill().await;

    let (control_artifacts, control_leaked) = durable_residue(env.home(), control_prompt);
    assert!(
        control_artifacts.is_empty() && control_leaked.is_empty(),
        "CONTROL FAILED: killing a run with [session] enabled = false at the same \
         partial-stream boundary ALSO left residue, so the five presences above \
         prove nothing about journaling — they only prove the walker matches \
         everything: artifacts={control_artifacts:?} leaked={control_leaked:?}"
    );
    assert_eq!(
        session_directory_entries(env.home()),
        0,
        "CONTROL FAILED: the sessions directory has entries after a run the \
         operator turned sessions off for"
    );
}

#[tokio::test]
async fn sigkill_during_model_stream_resumes_as_provider_reconciliation_without_redispatch() {
    let partial = "F14-MODEL-PARTIAL-CONTENT-MUST-NOT-REPLAY";
    let prompt_sentinel = "F14-MODEL-PROMPT-CONTENT-MUST-NOT-PERSIST";
    let prepared_sentinel = "F14-MODEL-PREPARED-REQUEST-MUST-NOT-PERSIST";
    let prompt = format!("{prompt_sentinel}\n{prepared_sentinel}");
    let fixture = OpenAiFixtureScript::new([
        OpenAiStep::text_then_stall(partial, 60_000),
        OpenAiStep::text("DUPLICATE-PROVIDER-DISPATCH"),
    ])
    .start()
    .await
    .expect("start model-stream fixture");
    let env = environment(&fixture);
    let vault = VaultSecret::new();
    let session_id = "f1400000000000000000000000000001";

    let mut first = CoreProcess::launch(&env, &fixture, &vault, session_id, false).await;
    send_message(&mut first, "f14-model-msg", &prompt).await;
    wait_for_requests(&fixture, 1).await;
    let delta = first.next_type("text_delta").await;
    assert_eq!(delta["text"], partial);
    let first_diagnostics = first.sigkill().await;
    let evidence = preserve_crash_evidence(&env);

    let mut resumed = CoreProcess::launch(&env, &fixture, &vault, session_id, true).await;
    let current = resync_current(&mut resumed, session_id, "model-current").await;
    assert_eq!(current["lifecycle"], "suspended");
    assert_eq!(
        current["pending_turn"]["reconcile_reason"],
        "provider_outcome_unknown"
    );
    let (baseline, replay) = resync_from_genesis(&mut resumed, session_id, "model-replay").await;
    assert_contiguous_replay(&baseline["cursor"], &replay);
    assert_eq!(replay["through"], current["cursor"]);
    assert_content_free(
        &[&current, &baseline, &replay],
        &[prompt_sentinel, prepared_sentinel, partial],
    );
    assert_one_provider_request(&fixture);
    let resumed_diagnostics = resumed.sigkill().await;
    assert_provider_checkpoint_sealed(evidence.path(), session_id, prepared_sentinel);
    assert_global_secret_absence(
        evidence.path(),
        &[&first_diagnostics, &resumed_diagnostics],
        &[
            ConfidentialProbe {
                label: "provider_api_key",
                value: FIXTURE_KEY,
            },
            ConfidentialProbe {
                label: "vault_unlock_secret",
                value: vault.as_str(),
            },
        ],
    );
}

/// F21-04-02. An in-flight provider reservation must SURVIVE a real process
/// kill and be CHARGED by the restart — never silently handed back.
///
/// Phase 21's attribution corpus measured `reserved_totals == (0, 0.0)` and
/// `release() == false` after a restart and escalated a suspected durability
/// defect: "a crash silently returns spent budget". This test drives the real
/// packaged binary through a real `SIGKILL` while a paid dispatch is in flight
/// and reads the journal FILE on both sides of it, which is the only place the
/// two candidate explanations separate:
///
/// * *not persisted* — the crashed process's journal carries no reservation, so
///   the money simply vanished with the process; or
/// * *persisted, then deliberately reconciled* — the journal carries the
///   reservation, and the restart converts it into a charge because the send is
///   journalled as having reached the provider.
///
/// The assertions below are written so those two outcomes cannot be confused:
/// the reservation is required to be present, on its own session, at a non-zero
/// amount, BEFORE any restart; and the restart is required to move exactly that
/// amount from the reserved meter to the charged one. A restart that refunded
/// instead — the failure the finding feared — leaves the charged meter where it
/// was and fails the final assertion.
#[tokio::test]
async fn sigkill_mid_dispatch_charges_the_surviving_reservation_instead_of_refunding_it() {
    let partial = "F21-0402-STREAM-STALLED-WITH-BUDGET-IN-FLIGHT";
    let fixture = OpenAiFixtureScript::new([
        OpenAiStep::text_then_stall(partial, 60_000),
        OpenAiStep::text("F21-0402-MUST-NOT-REDISPATCH"),
    ])
    .start()
    .await
    .expect("start stalled-dispatch fixture");
    let env = environment(&fixture);
    let vault = VaultSecret::new();
    let session_id = "f2100000000000000000000000000402";

    let mut first = CoreProcess::launch(&env, &fixture, &vault, session_id, false).await;
    send_message(&mut first, "f21-0402-msg", "reserve a dispatch, then die").await;
    wait_for_requests(&fixture, 1).await;
    // The stream has started, so the reservation is admitted and the physical
    // send has demonstrably reached the provider. Killing here is the exact
    // window the finding is about.
    assert_eq!(first.next_type("text_delta").await["text"], partial);
    let _first_diagnostics = first.sigkill().await;

    // LEG 1 — the reservation is IN the dead process's journal, on the session
    // that made it, for a non-zero amount. This is what "does not survive a
    // process restart" would have to contradict.
    let crashed_events = journal_events(env.home(), session_id);
    let crashed_authority = latest_budget_authority(&crashed_events).clone();
    let crashed_books = provider_books(&crashed_authority);
    assert_eq!(
        crashed_books.reserved.len(),
        1,
        "the killed process must leave exactly one in-flight reservation in its journal: \
         {crashed_books:?}"
    );
    let (reserved_session, reserved_input, reserved_output, reserved_usd) =
        crashed_books.reserved[0].clone();
    assert!(
        reserved_input + reserved_output > 0,
        "a surviving reservation with no admitted tokens cannot distinguish a charge from a \
         refund: {crashed_books:?}"
    );
    assert_eq!(
        crashed_authority["provider_reservations"]
            .as_object()
            .map(serde_json::Map::len),
        Some(1),
        "the dispatch-to-reservation binding must survive the kill alongside the reservation: \
         {crashed_authority}"
    );
    let charged_before = crashed_books.charged_for(&reserved_session);

    // LEG 2 — the restart. Binding the durable authority in a fresh real
    // process IS the reconciliation.
    let mut resumed = CoreProcess::launch(&env, &fixture, &vault, session_id, true).await;
    let current = resync_current(&mut resumed, session_id, "f21-0402-current").await;
    assert_eq!(current["lifecycle"], "suspended");

    let restarted_events = journal_events(env.home(), session_id);
    let restarted_books = provider_books(latest_budget_authority(&restarted_events));
    assert!(
        restarted_books.reserved.is_empty(),
        "the restart must not carry a dead process's reservation forward as still-admitted: \
         {restarted_books:?}"
    );
    let charged_after = restarted_books.charged_for(&reserved_session);
    let expected_tokens = charged_before.0 + reserved_input + reserved_output;
    assert_eq!(
        charged_after.0, expected_tokens,
        "the restart returned the admitted tokens instead of charging them: the session was \
         charged {} before the kill and {} after the restart, against {reserved_input} input + \
         {reserved_output} output admitted and lost in flight",
        charged_before.0, charged_after.0
    );
    assert!(
        (charged_after.1 - (charged_before.1 + reserved_usd)).abs() < 1e-9,
        "the restart returned the admitted cost instead of charging it: ${:.6} before the kill, \
         ${:.6} after the restart, against ${reserved_usd:.6} admitted and lost in flight",
        charged_before.1,
        charged_after.1
    );
    assert_one_provider_request(&fixture);
    let _resumed_diagnostics = resumed.sigkill().await;
}

#[tokio::test]
async fn packaged_fresh_process_reopens_sealed_request_and_dispatches_once() {
    let continued = "F14-SEALED-REQUEST-CONTINUED-ONCE";
    let prepared_sentinel = "F14-SEALED-PREPARED-REQUEST";
    let fixture = OpenAiFixtureScript::new([OpenAiStep::text(continued)])
        .start()
        .await
        .expect("start sealed-request restart fixture");
    let env = environment(&fixture);
    let vault = VaultSecret::new();
    let session_id = "f1400000000000000000000000000009";
    let turn_id = "f14-sealed-request-restart-turn";

    seed_recoverable_profile(
        &env,
        &fixture,
        &vault,
        session_id,
        turn_id,
        prepared_sentinel,
        true,
    )
    .await;
    assert!(
        fixture.observation().requests.is_empty(),
        "the seeded prepared checkpoint must precede provider acceptance"
    );
    let seed_evidence = preserve_crash_evidence(&env);
    assert_provider_checkpoint_sealed(seed_evidence.path(), session_id, prepared_sentinel);

    let mut process = CoreProcess::launch(&env, &fixture, &vault, session_id, true).await;
    let before = resync_current(&mut process, session_id, "sealed-restart-before").await;
    assert_eq!(before["lifecycle"], "ready");
    assert_eq!(before["pending_turn"]["turn_id"], turn_id);
    resume_turn_continue(&mut process, &before, session_id, "sealed-restart").await;

    let delta = process.next_type("text_delta").await;
    assert_eq!(delta["text"], continued);
    let terminal = process.next_type("stream_end").await;
    assert_eq!(terminal["msg_id"], "sealed-restart");
    let lifecycle = process.next_type("turn_recovery_lifecycle").await;
    assert_eq!(lifecycle["turn_id"], turn_id);
    assert_eq!(lifecycle["lifecycle"], "completed");
    let completed = resync_current(&mut process, session_id, "sealed-restart-complete").await;
    assert_eq!(completed["lifecycle"], "ready");
    assert!(completed["pending_turn"].is_null());
    assert_provider_request_count_stable(&fixture, 1).await;
    let _diagnostics = process.sigkill().await;
}

#[tokio::test]
async fn stop_during_active_host_continue_preserves_unknown_provider_authority() {
    let partial = "F14-CONTINUE-STOP-PARTIAL";
    let fixture = OpenAiFixtureScript::new([
        OpenAiStep::text_then_stall(partial, 60_000),
        OpenAiStep::text("MUST-NOT-REDISPATCH-AFTER-STOP"),
    ])
    .start()
    .await
    .expect("start active-continue cancellation fixture");
    let env = environment(&fixture);
    let vault = VaultSecret::new();
    let session_id = "f1400000000000000000000000000005";
    let turn_id = "f14-active-continue-stop-turn";

    seed_recoverable_profile(
        &env,
        &fixture,
        &vault,
        session_id,
        turn_id,
        "continue from this committed boundary",
        true,
    )
    .await;

    let mut process = CoreProcess::launch(&env, &fixture, &vault, session_id, true).await;
    let before = resync_current(&mut process, session_id, "continue-stop-before").await;
    assert_eq!(before["lifecycle"], "ready");
    assert_eq!(before["pending_turn"]["turn_id"], turn_id);

    resume_turn_continue(&mut process, &before, session_id, "continue-stop").await;
    let delta = process.next_type("text_delta").await;
    wait_for_requests(&fixture, 1).await;
    assert_eq!(delta["text"], partial);
    process.send(json!({"type": "stop"})).await;

    let terminal = process.next_type("stream_end").await;
    assert_eq!(terminal["msg_id"], "continue-stop");
    assert_eq!(terminal["finish_reason"], "stop");
    let lifecycle = process.next_type("turn_recovery_lifecycle").await;
    assert_eq!(lifecycle["turn_id"], turn_id);
    // The TURN is terminally cancelled: the host asked to stop and it stopped.
    // What must never be claimed is that the physically dispatched REQUEST was
    // cancelled — that is asserted on the durable receipt below, which is where
    // the claim actually lives. This used to read "suspended", which kept the
    // distinction only by never closing the turn at all, and so left the
    // session refusing every later message for the rest of its life.
    assert_eq!(lifecycle["lifecycle"], "cancelled");

    let after = resync_current(&mut process, session_id, "continue-stop-after").await;
    assert_eq!(
        after["lifecycle"], "ready",
        "a stopped turn must leave the session able to accept the next message"
    );
    assert!(after["pending_turn"].is_null());
    assert_ne!(after["cursor"], before["cursor"]);

    // THE INVARIANT THIS TEST EXISTS FOR, and the reason the assertions above
    // could be relaxed without relaxing it: an accepted provider request is
    // never recorded as cancelled or as never-started. Its receipt says the
    // outcome was not observed, in words, and carries the digest of exactly the
    // bytes that did arrive.
    let events = journal_events(env.home(), session_id);
    let receipt = events
        .iter()
        .rev()
        .find(|event| {
            event["type"] == "provider_attempt_finished_v2"
                || event["type"] == "provider_attempt_finished"
        })
        .unwrap_or_else(|| {
            panic!("the dispatched attempt must take a durable receipt: {events:?}")
        });
    assert_eq!(
        receipt["outcome"]["status"], "failed",
        "a dispatched request must never be recorded as cancelled or not-started: {receipt}"
    );
    assert!(
        receipt["outcome"]["error"].as_str().is_some_and(
            |error| error.contains("may have served it in full, in part, or not at all")
        ),
        "the receipt must say in words that the provider's outcome is unknown: {receipt}"
    );
    assert!(
        receipt["response_digest"].is_string(),
        "a partial capture must pin the bytes it captured: {receipt}"
    );

    // And the stopped turn must not have re-dispatched: the fixture's second
    // step is never consumed.
    assert_provider_request_count_stable(&fixture, 1).await;
    let _diagnostics = process.sigkill().await;
}

#[tokio::test]
async fn packaged_host_continue_and_non_genesis_reconnect_are_exactly_once() {
    let continued = "F14-HOST-CONTINUED-ONCE";
    let fixture = OpenAiFixtureScript::new([OpenAiStep::text(continued)])
        .start()
        .await
        .expect("start packaged host continuation fixture");
    let env = environment(&fixture);
    let vault = VaultSecret::new();
    let session_id = "f1400000000000000000000000000006";
    let turn_id = "f14-host-continue-turn";

    seed_recoverable_profile(
        &env,
        &fixture,
        &vault,
        session_id,
        turn_id,
        "resume this exact committed turn",
        true,
    )
    .await;

    let mut process = CoreProcess::launch(&env, &fixture, &vault, session_id, true).await;
    let before = resync_current(&mut process, session_id, "host-continue-before").await;
    assert_eq!(before["lifecycle"], "ready");
    assert_eq!(before["pending_turn"]["turn_id"], turn_id);

    resume_turn_continue(&mut process, &before, session_id, "host-continue").await;
    let delta = process.next_type("text_delta").await;
    assert_eq!(delta["text"], continued);
    let terminal = process.next_type("stream_end").await;
    assert_eq!(terminal["msg_id"], "host-continue");
    let lifecycle = process.next_type("turn_recovery_lifecycle").await;
    assert_eq!(lifecycle["turn_id"], turn_id);
    assert_eq!(lifecycle["lifecycle"], "completed");

    let completed = resync_current(&mut process, session_id, "host-continue-complete").await;
    assert_eq!(completed["lifecycle"], "ready");
    assert!(completed["pending_turn"].is_null());
    assert_ne!(completed["cursor"], before["cursor"]);
    assert_provider_request_count_stable(&fixture, 1).await;
    let completed_events = journal_events(env.home(), session_id);
    let completed_budget_authority = latest_budget_authority(&completed_events).clone();
    let completed_dispatch_bindings = provider_dispatch_bindings(&completed_events);
    let _first_diagnostics = process.sigkill().await;

    let mut reconnected = CoreProcess::launch(&env, &fixture, &vault, session_id, true).await;
    let reconnect_head = resync_current(&mut reconnected, session_id, "host-reconnect-head").await;
    assert_eq!(
        reconnect_head["cursor"]["journal_sequence"].as_u64(),
        completed["cursor"]["journal_sequence"]
            .as_u64()
            .map(|seq| seq + 1),
        "restart reconciliation must commit exactly one durable authority epoch"
    );
    assert_ne!(reconnect_head["state_digest"], completed["state_digest"]);
    assert_eq!(reconnect_head["budget"], completed["budget"]);
    let reconnect_events = journal_events(env.home(), session_id);
    assert_eq!(reconnect_events.len(), completed_events.len() + 1);
    let reconnect_budget_authority = latest_budget_authority(&reconnect_events);
    assert_eq!(
        reconnect_budget_authority["authority_epoch"].as_u64(),
        completed_budget_authority["authority_epoch"]
            .as_u64()
            .map(|epoch| epoch + 1)
    );
    assert_eq!(
        reconnect_budget_authority["prior_cursor"]["journal_sequence"],
        completed["cursor"]["journal_sequence"]
    );
    assert_eq!(
        reconnect_budget_authority["prior_cursor"]["journal_checksum"],
        completed["cursor"]["journal_digest"]
    );
    assert_eq!(
        provider_dispatch_bindings(&reconnect_events),
        completed_dispatch_bindings,
        "restart reconciliation must not rewrite the provider dispatch binding"
    );

    let (baseline, replay) = resync_after(
        &mut reconnected,
        session_id,
        "host-reconnect-suffix",
        &before["cursor"],
    )
    .await;
    assert_contiguous_replay(&before["cursor"], &replay);
    assert_eq!(replay["through"], reconnect_head["cursor"]);

    let (duplicate_baseline, duplicate_replay) = resync_after(
        &mut reconnected,
        session_id,
        "host-reconnect-suffix",
        &before["cursor"],
    )
    .await;
    assert_eq!(duplicate_baseline, baseline);
    assert_eq!(duplicate_replay, replay);
    let after_duplicate =
        resync_current(&mut reconnected, session_id, "host-reconnect-after").await;
    assert_eq!(after_duplicate["cursor"], reconnect_head["cursor"]);
    assert_eq!(
        after_duplicate["state_digest"],
        reconnect_head["state_digest"]
    );
    assert_eq!(after_duplicate["budget"], reconnect_head["budget"]);
    assert_eq!(
        journal_events(env.home(), session_id),
        reconnect_events,
        "subsequent resync in the same process must not mutate durable authority"
    );
    assert_provider_request_count_stable(&fixture, 1).await;
    let _reconnect_diagnostics = reconnected.sigkill().await;
}

#[tokio::test]
async fn sigkill_while_awaiting_approval_restores_gate_without_provider_or_tool_replay() {
    let prompt_sentinel = "F14-APPROVAL-PROMPT-CONTENT-MUST-NOT-PERSIST";
    let prepared_sentinel = "F14-APPROVAL-PREPARED-REQUEST-MUST-NOT-PERSIST";
    let prompt = format!("{prompt_sentinel}\n{prepared_sentinel}");
    let target = "F14-APPROVAL-TARGET-MUST-NOT-REPLAY.txt";
    let fixture = OpenAiFixtureScript::new([
        OpenAiStep::tool_call(
            "f14-approval-call",
            "Write",
            json!({"file_path": target, "content": "MUST-NOT-BE-WRITTEN"}),
        ),
        OpenAiStep::text("DUPLICATE-PROVIDER-DISPATCH"),
    ])
    .start()
    .await
    .expect("start approval fixture");
    let env = environment(&fixture);
    let vault = VaultSecret::new();
    let session_id = "f1400000000000000000000000000002";

    let mut first = CoreProcess::launch(&env, &fixture, &vault, session_id, false).await;
    send_message(&mut first, "f14-approval-msg", &prompt).await;
    wait_for_requests(&fixture, 1).await;
    let approval = first.next_type("approval_required").await;
    assert_eq!(approval["call_id"], "f14-approval-call");
    let first_diagnostics = first.sigkill().await;
    let evidence = preserve_crash_evidence(&env);

    let mut resumed = CoreProcess::launch(&env, &fixture, &vault, session_id, true).await;
    let current = resync_current(&mut resumed, session_id, "approval-current").await;
    assert_eq!(current["lifecycle"], "awaiting_approval");
    assert_eq!(current["pending_turn"]["lifecycle"], "awaiting_approval");
    let (baseline, replay) = resync_from_genesis(&mut resumed, session_id, "approval-replay").await;
    assert_contiguous_replay(&baseline["cursor"], &replay);
    assert_eq!(replay["through"], current["cursor"]);
    assert_content_free(
        &[&current, &baseline, &replay],
        &[
            prompt_sentinel,
            prepared_sentinel,
            target,
            "MUST-NOT-BE-WRITTEN",
        ],
    );
    assert!(!env.path().join(target).exists(), "unapproved tool ran");
    assert_one_provider_request(&fixture);

    resumed
        .send(json!({
            "type": "resolve_interrupted_approval",
            "recovery_version": 1,
            "request_id": "approval-deny",
            "session_id": session_id,
            "turn_id": current["pending_turn"]["turn_id"],
            "cursor": current["cursor"],
            "approval_id": current["pending_turn"]["pending_call_id"],
            "decision": "deny",
        }))
        .await;
    let terminal = resumed.next_type("stream_end").await;
    assert_eq!(terminal["msg_id"], "approval-deny");
    let lifecycle = resumed.next_type("turn_recovery_lifecycle").await;
    assert_eq!(lifecycle["lifecycle"], "completed");
    assert_eq!(lifecycle["turn_id"], current["pending_turn"]["turn_id"]);
    assert!(
        !env.path().join(target).exists(),
        "denied recovered approval executed the tool"
    );
    assert_one_provider_request(&fixture);
    let resumed_diagnostics = resumed.sigkill().await;
    assert_provider_checkpoint_sealed(evidence.path(), session_id, prepared_sentinel);
    assert_global_secret_absence(
        evidence.path(),
        &[&first_diagnostics, &resumed_diagnostics],
        &[
            ConfidentialProbe {
                label: "provider_api_key",
                value: FIXTURE_KEY,
            },
            ConfidentialProbe {
                label: "vault_unlock_secret",
                value: vault.as_str(),
            },
        ],
    );
}

#[tokio::test]
async fn recovered_approval_approve_executes_effect_once_and_continues_once() {
    let seed = OpenAiFixtureScript::new([OpenAiStep::text("unused")])
        .start()
        .await
        .expect("start recovered-approval seed fixture");
    let env = environment(&seed);
    let marker = env.path().join("f14-recovered-approval-effect.log");
    let command = format!("printf 'effect\\n' >> {}", shell_quote(&marker));
    seed.shutdown()
        .await
        .expect("stop recovered-approval seed fixture");

    let fixture = OpenAiFixtureScript::new([
        OpenAiStep::tool_call(
            "f14-recovered-approval-call",
            "Bash",
            json!({"command": command}),
        ),
        OpenAiStep::text("F14-RECOVERED-APPROVAL-CONTINUED"),
    ])
    .start()
    .await
    .expect("start recovered-approval fixture");
    let vault = VaultSecret::new();
    let session_id = "f1400000000000000000000000000007";

    let mut first = CoreProcess::launch(&env, &fixture, &vault, session_id, false).await;
    send_message(
        &mut first,
        "f14-approval-approve-msg",
        "request one approval-bound effect",
    )
    .await;
    wait_for_requests(&fixture, 1).await;
    let approval = first.next_type("approval_required").await;
    assert_eq!(approval["call_id"], "f14-recovered-approval-call");
    let _first_diagnostics = first.sigkill().await;
    assert!(!marker.exists(), "tool ran before recovered approval");

    let mut resumed = CoreProcess::launch(&env, &fixture, &vault, session_id, true).await;
    let current = resync_current(&mut resumed, session_id, "approval-approve-current").await;
    assert_eq!(current["lifecycle"], "awaiting_approval");
    assert_eq!(
        current["pending_turn"]["pending_call_id"],
        "f14-recovered-approval-call"
    );
    resumed
        .send(json!({
            "type": "resolve_interrupted_approval",
            "recovery_version": 1,
            "request_id": "approval-approve",
            "session_id": session_id,
            "turn_id": current["pending_turn"]["turn_id"],
            "cursor": current["cursor"],
            "approval_id": current["pending_turn"]["pending_call_id"],
            "decision": "approve",
        }))
        .await;

    let running = resumed.next_type("tool_running").await;
    assert_eq!(running["call_id"], "f14-recovered-approval-call");
    let result = resumed.next_type("tool_result").await;
    assert_eq!(result["call_id"], "f14-recovered-approval-call");
    let delta = resumed.next_type("text_delta").await;
    assert_eq!(delta["text"], "F14-RECOVERED-APPROVAL-CONTINUED");
    let terminal = resumed.next_type("stream_end").await;
    assert_eq!(terminal["msg_id"], "approval-approve");
    let lifecycle = resumed.next_type("turn_recovery_lifecycle").await;
    assert_eq!(lifecycle["lifecycle"], "completed");

    let effects = fs::read_to_string(&marker).expect("read recovered approval marker");
    assert_eq!(effects.lines().collect::<Vec<_>>(), ["effect"]);
    assert_provider_request_count_stable(&fixture, 2).await;
    let completed = resync_current(&mut resumed, session_id, "approval-approve-done").await;
    assert_eq!(completed["lifecycle"], "ready");
    assert!(completed["pending_turn"].is_null());
    let _resumed_diagnostics = resumed.sigkill().await;
}

struct ToolProcessGuard {
    pid_file: PathBuf,
}

impl Drop for ToolProcessGuard {
    fn drop(&mut self) {
        let Ok(pid) = std::fs::read_to_string(&self.pid_file) else {
            return;
        };
        let Ok(pid) = pid.trim().parse::<libc::pid_t>() else {
            return;
        };
        // SAFETY: the fixture writes its own shell PID into a private tempdir.
        // ESRCH is the expected result when Core's process containment already
        // reaped the tool.
        unsafe {
            libc::kill(pid, libc::SIGKILL);
        }
    }
}

fn shell_quote(path: &Path) -> String {
    format!("'{}'", path.to_string_lossy().replace('\'', "'\\''"))
}

async fn wait_for_file(path: &Path) {
    let deadline = Instant::now() + EVENT_TIMEOUT;
    while !path.exists() {
        assert!(
            Instant::now() < deadline,
            "timed out waiting for tool marker {}",
            path.display()
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

#[tokio::test]
async fn sigkill_during_tool_execution_requires_reconciliation_without_reexecution() {
    let seed = OpenAiFixtureScript::new([OpenAiStep::text("unused")])
        .start()
        .await
        .expect("start seed fixture");
    let env = environment(&seed);
    let marker = env.path().join("f14-tool-started.log");
    let pid_file = env.path().join("f14-tool.pid");
    let command = format!(
        "printf '%s\\n' \"$$\" > {} && printf 'started\\n' >> {} && exec sleep 60",
        shell_quote(&pid_file),
        shell_quote(&marker),
    );
    seed.shutdown().await.expect("stop seed fixture");

    let fixture = OpenAiFixtureScript::new([
        OpenAiStep::tool_call(
            "f14-running-bash",
            "Bash",
            json!({"command": command, "timeout": 120_000}),
        ),
        OpenAiStep::text("DUPLICATE-PROVIDER-DISPATCH"),
    ])
    .start()
    .await
    .expect("start tool fixture");
    let _tool_guard = ToolProcessGuard {
        pid_file: pid_file.clone(),
    };
    let session_id = "f1400000000000000000000000000003";
    let prompt_sentinel = "F14-TOOL-PROMPT-CONTENT-MUST-NOT-PERSIST";
    let prepared_sentinel = "F14-TOOL-PREPARED-REQUEST-MUST-NOT-PERSIST";
    let prompt = format!("{prompt_sentinel}\n{prepared_sentinel}");
    let vault = VaultSecret::new();

    let mut first = CoreProcess::launch(&env, &fixture, &vault, session_id, false).await;
    send_message(&mut first, "f14-tool-msg", &prompt).await;
    wait_for_requests(&fixture, 1).await;
    let approval = first.next_type("approval_required").await;
    assert_eq!(approval["call_id"], "f14-running-bash");
    first
        .send(json!({
            "type": "tool_approve",
            "call_id": "f14-running-bash",
        }))
        .await;
    let running = first.next_type("tool_running").await;
    assert_eq!(running["call_id"], "f14-running-bash");
    wait_for_file(&marker).await;
    let first_diagnostics = first.sigkill().await;
    let evidence = preserve_crash_evidence(&env);

    let mut resumed = CoreProcess::launch(&env, &fixture, &vault, session_id, true).await;
    let current = resync_current(&mut resumed, session_id, "tool-current").await;
    assert_eq!(current["lifecycle"], "reconciliation_required");
    assert_eq!(
        current["pending_turn"]["reconcile_reason"],
        "tool_outcome_unknown"
    );
    let (baseline, replay) = resync_from_genesis(&mut resumed, session_id, "tool-replay").await;
    assert_contiguous_replay(&baseline["cursor"], &replay);
    assert_eq!(replay["through"], current["cursor"]);
    assert_content_free(
        &[&current, &baseline, &replay],
        &[
            prompt_sentinel,
            prepared_sentinel,
            &command,
            &marker.to_string_lossy(),
        ],
    );
    let starts = std::fs::read_to_string(&marker).expect("read tool marker");
    assert_eq!(starts.lines().collect::<Vec<_>>(), ["started"]);
    assert_one_provider_request(&fixture);
    let resumed_diagnostics = resumed.sigkill().await;
    assert_provider_checkpoint_sealed(evidence.path(), session_id, prepared_sentinel);
    assert_global_secret_absence(
        evidence.path(),
        &[&first_diagnostics, &resumed_diagnostics],
        &[
            ConfidentialProbe {
                label: "provider_api_key",
                value: FIXTURE_KEY,
            },
            ConfidentialProbe {
                label: "vault_unlock_secret",
                value: vault.as_str(),
            },
        ],
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn packaged_tui_restart_projection_matches_json_host() {
    let partial = "F14-PARITY-PARTIAL-CONTENT-MUST-NOT-REPLAY";
    let prompt_sentinel = "F14-PARITY-PROMPT-CONTENT-MUST-NOT-PROJECT";
    let prepared_sentinel = "F14-PARITY-PREPARED-REQUEST-MUST-NOT-PROJECT";
    let prompt = format!("{prompt_sentinel}\n{prepared_sentinel}");
    let fixture = OpenAiFixtureScript::new([OpenAiStep::text_then_stall(partial, 60_000)])
        .start()
        .await
        .expect("start parity fixture");
    let env = environment(&fixture);
    let vault = VaultSecret::new();
    let session_id = "f1400000000000000000000000000004";

    let mut first = CoreProcess::launch(&env, &fixture, &vault, session_id, false).await;
    send_message(&mut first, "f14-parity-msg", &prompt).await;
    wait_for_requests(&fixture, 1).await;
    let delta = first.next_type("text_delta").await;
    assert_eq!(delta["text"], partial);
    let first_diagnostics = first.sigkill().await;
    let evidence = preserve_crash_evidence(&env);

    let mut host = CoreProcess::launch(&env, &fixture, &vault, session_id, true).await;
    let host_snapshot = resync_current(&mut host, session_id, "parity-current").await;
    let host_projection = json!({
        "session_id": host_snapshot["session_id"].clone(),
        "cursor": host_snapshot["cursor"].clone(),
        "lifecycle": host_snapshot["lifecycle"].clone(),
        "pending_turn": host_snapshot["pending_turn"].clone(),
    });
    let host_diagnostics = host.sigkill().await;

    fs::remove_dir_all(env.home()).expect("reset mutated host recovery profile");
    copy_directory(evidence.path(), env.home()).expect("restore identical crashed profile for TUI");
    let mut tui = TuiProcess::launch(&env, &fixture, &vault, session_id);
    tui.type_command("/recover json");
    let tui_projection = tui.recovery_projection();

    assert_eq!(
        sanitize_recovery_projection(tui_projection.clone()),
        sanitize_recovery_projection(host_projection.clone()),
        "packaged TUI and JSON-stream projected different recovery authority"
    );
    assert_content_free(
        &[&host_projection, &tui_projection],
        &[prompt_sentinel, prepared_sentinel, partial],
    );
    assert_one_provider_request(&fixture);
    assert_global_secret_absence(
        evidence.path(),
        &[&first_diagnostics, &host_diagnostics],
        &[
            ConfidentialProbe {
                label: "provider_api_key",
                value: FIXTURE_KEY,
            },
            ConfidentialProbe {
                label: "vault_unlock_secret",
                value: vault.as_str(),
            },
        ],
    );
}
