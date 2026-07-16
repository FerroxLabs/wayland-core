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
    child: Child,
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

    #[cfg(target_os = "linux")]
    async fn launch_without_secure_store(
        env: &TempEnv,
        fixture: &RunningOpenAiFixture,
        session_id: &str,
    ) -> Self {
        Self::launch_with_secure_store(env, fixture, None, session_id, false).await
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

        let child = command.spawn();
        // SAFETY: after spawn, only the child may consume its inherited copy.
        if let Some(vault_fd) = vault_fd {
            assert_eq!(
                unsafe { libc::close(vault_fd) },
                0,
                "close parent vault pipe"
            );
        }
        let mut child = child.expect("spawn packaged wayland-core");
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

    fn panic_timeout(&self, expected: &str, observed: &[String]) -> ! {
        let stderr = self.stderr.lock().expect("lock Core stderr capture");
        panic!(
            "timed out waiting for {expected}; observed={observed:?}; stderr:\n{}",
            String::from_utf8_lossy(&stderr)
        );
    }

    async fn sigkill(mut self) -> Vec<u8> {
        let pid = self.child.id().expect("running Core pid");
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
    child: Box<dyn portable_pty::Child + Send + Sync>,
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

        let child = pty
            .slave
            .spawn_command(command)
            .expect("spawn packaged TUI");

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

impl Drop for TuiProcess {
    fn drop(&mut self) {
        if let Ok(None) = self.child.try_wait() {
            let _ = self.child.kill();
        }
    }
}

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
    let child = command.spawn();
    // SAFETY: after spawn, only the child may consume its inherited copy.
    assert_eq!(
        unsafe { libc::close(vault_fd) },
        0,
        "close parent seed vault pipe"
    );
    let output = child
        .expect("spawn recoverable-profile seeder")
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
        assert_eq!(&bytes[offset..offset + 4], b"WJ01", "journal frame magic");
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
        frames.push((
            frame,
            serde_json::from_slice(&bytes[body_start..body_end])
                .expect("decode journal frame JSON"),
        ));
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

#[cfg(target_os = "linux")]
#[tokio::test]
async fn isolated_profile_without_secure_store_fails_before_turn_or_provider_intent() {
    let fixture = OpenAiFixtureScript::new([OpenAiStep::text("MUST-NOT-DISPATCH")])
        .start()
        .await
        .expect("start secure-storage preflight fixture");
    let env = environment(&fixture);
    let session_id = "f1400000000000000000000000000000";
    let prompt = "F14-UNPROTECTED-PROMPT-MUST-NOT-BECOME-DURABLE";

    let mut process = CoreProcess::launch_without_secure_store(&env, &fixture, session_id).await;
    send_message(&mut process, "f14-no-secure-store", prompt).await;

    let error = process.next_type("error").await;
    assert_eq!(error["error"]["code"], "engine_error");
    assert_eq!(error["error"]["retryable"], false);
    let message = error["error"]["message"]
        .as_str()
        .unwrap_or_else(|| panic!("secure-storage error lacks a message: {error}"));
    assert!(
        message.contains("secure recovery storage is unavailable")
            && message.contains("OS keyring")
            && message.contains("encrypted credentials vault"),
        "secure-storage error is not actionable: {message}"
    );

    let terminal = process.next_type("stream_end").await;
    assert_eq!(terminal["msg_id"], "f14-no-secure-store");
    assert_eq!(terminal["finish_reason"], "error");
    assert!(
        fixture.observation().requests.is_empty(),
        "secure-storage preflight failure reached the provider"
    );

    let journal = env
        .home()
        .join("sessions")
        .join(format!("{session_id}.journal"));
    let journal_bytes = fs::read(&journal).expect("read preflight-failed session journal");
    let events = journal_frames(&journal_bytes);
    assert!(
        events.iter().all(|(_, envelope)| {
            envelope.pointer("/event/type").and_then(Value::as_str) != Some("turn_started")
        }),
        "secure-storage preflight failure durably started a turn: {events:?}"
    );
    assert!(
        byte_offsets(&journal_bytes, prompt.as_bytes()).is_empty(),
        "preflight-failed prompt became durable"
    );

    let _diagnostics = process.sigkill().await;
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
    assert_eq!(
        lifecycle["lifecycle"], "suspended",
        "accepted host cancellation cannot claim a terminal cancellation after a provider request was physically dispatched"
    );
    assert_eq!(
        lifecycle["reconcile_reason"], "provider_outcome_unknown",
        "the lifecycle receipt must publish the post-cancel durable recovery authority"
    );

    let after = resync_current(&mut process, session_id, "continue-stop-after").await;
    assert_eq!(after["lifecycle"], "suspended");
    assert_eq!(
        after["pending_turn"]["reconcile_reason"],
        "provider_outcome_unknown"
    );
    assert_ne!(after["cursor"], before["cursor"]);
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
