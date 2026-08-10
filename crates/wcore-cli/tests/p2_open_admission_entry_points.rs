//! P2 — the admits-everyone refusal, on EVERY entry point that loads an
//! `InboundPolicy`, driven through the real `wayland-core` binary.
//!
//! # Why this file exists
//!
//! The refusal lives inside `ChannelPolicySnapshot::from_configs`, which is
//! deliberately the one derivation every lifecycle passes through. That makes
//! the gate unavoidable — and it also makes its BLAST RADIUS wider than
//! "the gateway". `enable_inbound_dispatch(true)` is set at three places in
//! `wcore-cli/src/main.rs` and once more inside `gateway run`:
//!
//! | Entry point | Site | Covered by |
//! |---|---|---|
//! | headless one-shot / line-REPL (`wayland-core "<prompt>"`, `--no-tui`) | `main.rs` `.enable_inbound_dispatch(true)` on the REPL bootstrap | [`headless_one_shot_refuses_before_it_runs_a_turn`] |
//! | interactive TUI | the TUI bootstrap, same builder | shares `AgentBootstrap::build` with the row above; needs a TTY, see the residual note below |
//! | `--json-stream` host (the Wayland desktop app) | the json-stream bootstrap | [`json_stream_host_receives_one_clean_error_frame`] |
//! | `gateway run` | `channel_inbound_host::spawn` | [`gateway_run_refuses_even_with_the_webhook_disabled`] |
//! | `channel reload` on a running host | `InboundHost::reload_policies` | `wcore-agent`'s `p2_open_admission_refusal_test` LEG 8 |
//!
//! The json-stream row is the one with a protocol contract attached: a refusal
//! there must be a single, parseable `error` frame on STDOUT, arriving instead
//! of `ready` — not a silent non-zero exit and not a hang. A desktop host that
//! merely times out waiting for `ready` learns nothing about why.
//!
//! # Every negative here has a positive control
//!
//! A refusal is indistinguishable from a broken invocation unless the same
//! harness is shown to start cleanly on a BOUNDED channel. Each test pairs the
//! two, and the headless pair goes further: the bounded run must complete a
//! real agent turn against a mock provider, so "it did not refuse" is proved
//! by output the gate could not have produced.
//!
//! **Residual, named:** the TUI row is not driven here. It needs a PTY, and it
//! reaches the gate through the same `AgentBootstrap::build` call the headless
//! row exercises, with the same `enable_inbound_dispatch(true)`.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

#[path = "support/mod.rs"]
mod support;

/// A syntactically valid key so `Config::resolve` succeeds and the run reaches
/// the startup stage under test. It authenticates nothing.
///
/// Assembled from two halves rather than written whole because the repo's
/// pre-commit secret ratchet matches `sk-` followed by 16+ key characters, and
/// a placeholder tripping the guard that exists to catch real keys is a good
/// way to teach people to bypass it.
fn dummy_key() -> String {
    format!("{}{}", "sk-", "ant-p2-not-a-real-key-000000000000")
}

/// The scripted assistant text. Seeing it proves a real turn ran, which is
/// only possible if the gate let the process start.
const MOCK_MARKER: &str = "P2_MOCK_TURN_RAN";

/// `[inbound]` body for a channel that admits every account on the platform.
const OPEN_INBOUND: &str = "dm = \"open\"\ngroup = \"disabled\"\n";

/// `[inbound]` body for a channel that admits exactly one named sender.
const BOUNDED_INBOUND: &str =
    "dm = \"allowlist\"\ndm_allowlist = [\"U-NAMED\"]\ngroup = \"disabled\"\n";

/// The refusal's own first line, as the product emits it. Asserting on this
/// (rather than on "some error occurred") is what stops an unrelated startup
/// failure passing for the gate firing.
const REFUSAL: &str = "do not match their acknowledgement of open admission";

struct Case {
    _dir: tempfile::TempDir,
    home: PathBuf,
    project: PathBuf,
}

impl Case {
    /// Isolated profile home + project dir, with one channel whose `[inbound]`
    /// body is `inbound`.
    ///
    /// `base_url` points the anthropic provider at a local mock when given.
    /// `HOME`/`USERPROFILE` are redirected alongside `WAYLAND_HOME` because
    /// `dirs::home_dir()` reads `USERPROFILE` on Windows, so `WAYLAND_HOME`
    /// alone does not isolate every lookup.
    fn new(inbound: &str, base_url: Option<&str>) -> Self {
        let dir = tempfile::tempdir().expect("case tempdir");
        let home = dir.path().join("home");
        let project = dir.path().join("proj");
        std::fs::create_dir_all(home.join("channels")).expect("create channels dir");
        std::fs::create_dir_all(dir.path().join("fakehome")).expect("create fake home");
        std::fs::create_dir_all(&project).expect("create project dir");

        let provider_block = match base_url {
            Some(url) => format!(
                "[providers.anthropic]\napi_key = \"{}\"\nbase_url = \"{url}\"\n\n",
                dummy_key()
            ),
            None => String::new(),
        };
        std::fs::write(
            home.join("config.toml"),
            format!(
                "[default]\nprovider = \"anthropic\"\nmodel = \"claude-sonnet-4-20250514\"\n\n\
                 {provider_block}[session]\nenabled = false\n\n\
                 [inbound_webhook]\nenabled = false\n"
            ),
        )
        .expect("write config.toml");

        write_channel(&home.join("channels"), "p2chan", inbound);
        Self {
            _dir: dir,
            home,
            project,
        }
    }

    fn fake_home(&self) -> PathBuf {
        self.home
            .parent()
            .expect("home has a parent")
            .join("fakehome")
    }

    fn command(&self) -> Command {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_wayland-core"));
        cmd.env_remove("ANTHROPIC_API_KEY")
            .env_remove("OPENAI_API_KEY")
            .env_remove("API_KEY")
            .env_remove("FLUX_API_KEY")
            .env_remove("WAYLAND_VAULT_PASSPHRASE")
            .env_remove("WAYLAND_VAULT_PASSPHRASE_FD")
            .env("ANTHROPIC_API_KEY", dummy_key())
            .env("WAYLAND_HOME", &self.home)
            .env("HOME", self.fake_home())
            .env("USERPROFILE", self.fake_home())
            .env("TERM", "dumb")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        cmd
    }

    /// Run to completion with stdin closed, and capture both streams.
    fn run(&self, args: &[&str]) -> Capture {
        let mut child = self
            .command()
            .args(args)
            .spawn()
            .expect("spawn wayland-core");
        drop(child.stdin.take());
        let out = child.wait_with_output().expect("wait for wayland-core");
        Capture {
            stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
            status: out.status.code(),
        }
    }
}

fn write_channel(dir: &Path, name: &str, inbound: &str) {
    std::fs::write(
        dir.join(format!("{name}.toml")),
        format!(
            "name = \"{name}\"\nplatform = \"slack\"\nenabled = true\n\n\
             [options]\nworkspace_name = \"p2\"\ndefault_channel_id = \"D0\"\n\
             credential_handle_bot_token = \"slack.{name}.bot_token\"\n\
             credential_handle_signing_secret = \"slack.{name}.signing_secret\"\n\n\
             [inbound]\n{inbound}"
        ),
    )
    .expect("write channel config");
}

struct Capture {
    stdout: String,
    stderr: String,
    status: Option<i32>,
}

impl Capture {
    fn frames(&self) -> Vec<serde_json::Value> {
        self.stdout
            .lines()
            .filter(|l| !l.trim().is_empty())
            .map(|l| {
                serde_json::from_str::<serde_json::Value>(l)
                    .unwrap_or_else(|e| panic!("host cannot parse stdout line as JSON: {e}\n{l}"))
            })
            .collect()
    }

    fn frames_of_type(&self, ty: &str) -> Vec<serde_json::Value> {
        self.frames()
            .into_iter()
            .filter(|f| f.get("type").and_then(|t| t.as_str()) == Some(ty))
            .collect()
    }

    fn either(&self) -> String {
        format!("stdout:\n{}\nstderr:\n{}", self.stdout, self.stderr)
    }
}

// ---------------------------------------------------------------- json-stream

/// THE DESKTOP HOST. A refusal must be one parseable error frame on stdout,
/// arriving instead of `ready` — not a silent exit and not a hang.
#[test]
fn json_stream_host_receives_one_clean_error_frame() {
    let case = Case::new(OPEN_INBOUND, None);
    let cap = case.run(&[
        "--json-stream",
        "--project-dir",
        &case.project.display().to_string(),
    ]);

    assert_ne!(
        cap.status,
        Some(0),
        "an admits-everyone channel must refuse to start. {}",
        cap.either()
    );
    assert!(
        cap.frames_of_type("ready").is_empty(),
        "a refused start must never claim ready. {}",
        cap.either()
    );

    let errors = cap.frames_of_type("error");
    assert_eq!(
        errors.len(),
        1,
        "the host must receive EXACTLY one error frame, not zero (a silent exit) and not \
         several. {}",
        cap.either()
    );
    let err = &errors[0]["error"];
    assert_eq!(
        err["retryable"], false,
        "a configuration refusal is not retryable"
    );
    assert!(
        err["code"].as_str().is_some_and(|c| !c.is_empty()),
        "the error frame must carry a code"
    );
    let message = err["message"].as_str().expect("message is a string");
    assert!(
        message.contains(REFUSAL),
        "the host must learn this was the open-admission gate; got: {message}"
    );
    assert!(
        message.contains("p2chan"),
        "the host must learn WHICH channel is at fault; got: {message}"
    );
    assert!(
        message.contains("acknowledge_open_admission = [\"dm=open\"]"),
        "and must carry the exact remedy, or the desktop user has no way forward; got: {message}"
    );
}

/// POSITIVE CONTROL for the row above. Without it, a build that failed to
/// start for ANY reason would satisfy every assertion in it.
#[test]
fn json_stream_host_still_starts_on_a_bounded_channel() {
    let case = Case::new(BOUNDED_INBOUND, None);
    let cap = case.run(&[
        "--json-stream",
        "--project-dir",
        &case.project.display().to_string(),
    ]);

    assert_eq!(
        cap.status,
        Some(0),
        "a bounded channel must start cleanly. {}",
        cap.either()
    );
    assert_eq!(
        cap.frames_of_type("ready").len(),
        1,
        "and must emit exactly one ready frame. {}",
        cap.either()
    );
    assert!(
        cap.frames_of_type("error").is_empty(),
        "with no error frame. {}",
        cap.either()
    );
}

// ------------------------------------------------------------------- headless

/// `wayland-core "<prompt>"` — purely local codegen that never touches a
/// channel still passes through the gate, because the same bootstrap opts into
/// inbound dispatch. That is the blast radius, stated as a test rather than as
/// a caveat.
#[test]
fn headless_one_shot_refuses_before_it_runs_a_turn() {
    let rt = tokio::runtime::Runtime::new().expect("tokio runtime");
    let server = rt.block_on(support::mock_llm::MockLlm::new().text(MOCK_MARKER).start());

    let case = Case::new(OPEN_INBOUND, Some(&server.uri()));
    let cap = case.run(&[
        "--no-tui",
        "--project-dir",
        &case.project.display().to_string(),
        "say hello",
    ]);

    assert_ne!(cap.status, Some(0), "the run must fail. {}", cap.either());
    assert!(
        cap.stderr.contains(REFUSAL) && cap.stderr.contains("p2chan"),
        "and must fail with the open-admission refusal, naming the channel. {}",
        cap.either()
    );
    assert!(
        !cap.stdout.contains(MOCK_MARKER),
        "no turn may run: the refusal is a refusal to START, not a warning printed on the way \
         through. {}",
        cap.either()
    );
}

/// POSITIVE CONTROL. The same invocation over a bounded channel must complete
/// a REAL agent turn. Asserting on the mock's scripted text (which only the
/// provider round-trip can produce) rules out "it did not refuse because it
/// did not get that far".
#[test]
fn headless_one_shot_runs_a_real_turn_on_a_bounded_channel() {
    let rt = tokio::runtime::Runtime::new().expect("tokio runtime");
    let server = rt.block_on(support::mock_llm::MockLlm::new().text(MOCK_MARKER).start());

    let case = Case::new(BOUNDED_INBOUND, Some(&server.uri()));
    let cap = case.run(&[
        "--no-tui",
        "--project-dir",
        &case.project.display().to_string(),
        "say hello",
    ]);

    assert!(
        !cap.stderr.contains(REFUSAL),
        "a bounded channel must not trip the gate. {}",
        cap.either()
    );
    assert!(
        cap.stdout.contains(MOCK_MARKER),
        "and the turn must actually run against the provider. {}",
        cap.either()
    );
}

// -------------------------------------------------------------------- gateway

/// `gateway run` — the systemd/launchd surface. Its other inbound failures
/// DEGRADE when `[inbound_webhook] enabled = false`, because their consequence
/// is "this process receives nothing", which is safe. This one must not: its
/// consequence is "anyone can drive the agent". The webhook is disabled here
/// precisely so a mis-ordered match arm would degrade instead of refusing.
#[test]
fn gateway_run_refuses_even_with_the_webhook_disabled() {
    let case = Case::new(OPEN_INBOUND, None);
    let cap = case.run(&["gateway", "run"]);

    assert_ne!(
        cap.status,
        Some(0),
        "the gateway must refuse to start. {}",
        cap.either()
    );
    assert!(
        cap.stderr.contains(REFUSAL) && cap.stderr.contains("p2chan"),
        "and must say why, naming the channel. {}",
        cap.either()
    );
    assert!(
        !cap.stderr.contains("inbound dispatch unavailable"),
        "it must NOT take the degrade-and-carry-on arm — that arm leaves the open configuration \
         merely discouraged. {}",
        cap.either()
    );
}

/// POSITIVE CONTROL for the row above: on a bounded channel the same gateway
/// stays up. Without this, a gateway that could never start would pass the
/// refusal test.
#[test]
fn gateway_run_stays_up_on_a_bounded_channel() {
    let case = Case::new(BOUNDED_INBOUND, None);
    let mut child: Child = case
        .command()
        .args(["gateway", "run"])
        .spawn()
        .expect("spawn gateway");
    drop(child.stdin.take());

    // Give it long enough to pass bootstrap, then require it to still be alive.
    let deadline = Instant::now() + Duration::from_secs(30);
    let mut exited = None;
    while Instant::now() < deadline {
        match child.try_wait().expect("try_wait") {
            Some(status) => {
                exited = Some(status);
                break;
            }
            None => std::thread::sleep(Duration::from_millis(250)),
        }
    }

    let alive = exited.is_none();
    let _ = child.kill();
    let mut stderr = String::new();
    if let Some(mut pipe) = child.stderr.take() {
        let _ = pipe.read_to_string(&mut stderr);
    }
    let _ = child.wait();

    assert!(
        alive,
        "a bounded channel must let the gateway run; it exited with {exited:?}. stderr:\n{stderr}"
    );
    assert!(
        !stderr.contains(REFUSAL),
        "and must not have logged the open-admission refusal. stderr:\n{stderr}"
    );
}
