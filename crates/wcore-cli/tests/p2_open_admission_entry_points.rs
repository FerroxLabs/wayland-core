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
//! | Entry point | Site | Open-admission refusal | Unreadable config is fatal |
//! |---|---|---|---|
//! | headless one-shot / line-REPL (`wayland-core "<prompt>"`, `--no-tui`) | `main.rs` `.enable_inbound_dispatch(true)` on the REPL bootstrap | [`headless_one_shot_refuses_before_it_runs_a_turn`] | [`headless_one_shot_refuses_an_unparseable_channel_file`] |
//! | interactive TUI | the TUI bootstrap, same builder | [`interactive_tui_refuses_before_it_paints`] (unix, on a real PTY) | [`interactive_tui_refuses_an_unparseable_channel_file`] |
//! | `--json-stream` host (the Wayland desktop app) | the json-stream bootstrap | [`json_stream_host_receives_one_clean_error_frame`] | [`json_stream_host_receives_an_error_frame_for_an_unparseable_channel_file`] |
//! | `gateway run` | `channel_inbound_host::spawn` | [`gateway_run_refuses_even_with_the_webhook_disabled`] | [`gateway_run_refuses_an_unparseable_channel_file`] |
//! | `channel reload` on a running host | `InboundHost::reload_policies` | `wcore-agent`'s `p2_open_admission_refusal_test` LEG 8 | already strict before this change |
//!
//! # The second column exists because the gate could be switched off by
//! # deleting its input
//!
//! Every cold-start row used to load its channel configs through
//! `try_load_channel_policy_configs().unwrap_or_default()`, over a loader that
//! stops at the FIRST unparseable file. One junk `.toml` therefore emptied the
//! list, `refuse_open_admission([])` returned `Ok`, and an adjacent
//! `dm = "open"` channel started with no refusal and no warning. The admission
//! consequence was fail-CLOSED, so it was never an admits-everyone hole — but a
//! security gate that any stray file can silently satisfy is not a gate, and
//! the same typo silently converted a working gateway into universal denial at
//! the next restart.
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
//! The TUI row is driven on a real pseudo-terminal rather than reasoned about
//! from the headless row. It is the one entry point that enters the alternate
//! screen BEFORE `AgentBootstrap::build`, so "the same builder refuses" is not
//! enough: what an operator actually gets is whatever survives the terminal
//! being restored on the way out, and only a PTY can show that.
//!
//! **Residual, named:** the TUI legs are `#[cfg(unix)]`. `portable_pty`'s
//! ConPTY backend on a headless Windows runner does not surface the child's
//! output to the master end (see `goal_control_tui_pty.rs` for the same
//! constraint), so the Windows terminal leg of this row is NOT measured here.
//! Windows keeps the headless, json-stream and gateway rows, which are
//! cross-platform.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

/// How long a run gets before the harness calls it a failure.
///
/// A BOUND, not a convenience. Without one, [`Case::run`] was
/// `child.wait_with_output()` with no deadline, so a build in which the fatal
/// `OpenAdmission` arm had been removed did not fail — the mutated gateway kept
/// running and the test hung. Under nextest that eventually surfaced as a
/// 567-second red; under plain `cargo test` it hung forever, which is a gate
/// that cannot fail.
///
/// Set deliberately BELOW nextest's own kill for this binary (the default
/// profile's `slow-timeout = { period = "30s", terminate-after = 2 }`), so the
/// failure an engineer reads is this harness naming the invocation that never
/// returned rather than an anonymous "timed out" — and so the bound holds under
/// plain `cargo test`, which has no timeout at all. Measured basis: the slowest
/// real `run` in this file is the headless positive control at ~4s, including a
/// full agent turn against a local mock.
const RUN_TIMEOUT: Duration = Duration::from_secs(45);

#[path = "support/mod.rs"]
mod support;
use support::owned_tree::OwnedTree;

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
    ///
    /// Bounded by [`RUN_TIMEOUT`]: a process that does not exit is a FAILURE of
    /// this test, not a reason for it to wait. Both pipes are drained on their
    /// own threads so a child that fills one cannot deadlock against a harness
    /// that is waiting on the other.
    fn run(&self, args: &[&str]) -> Capture {
        let mut child = OwnedTree::new(
            self.command()
                .args(args)
                .spawn()
                .expect("spawn wayland-core"),
        );
        drop(child.stdin.take());
        let mut out_pipe = child.stdout.take().expect("stdout is piped");
        let mut err_pipe = child.stderr.take().expect("stderr is piped");
        let out_reader = std::thread::spawn(move || {
            let mut buf = Vec::new();
            let _ = out_pipe.read_to_end(&mut buf);
            buf
        });
        let err_reader = std::thread::spawn(move || {
            let mut buf = Vec::new();
            let _ = err_pipe.read_to_end(&mut buf);
            buf
        });

        let deadline = Instant::now() + RUN_TIMEOUT;
        let mut status = None;
        loop {
            match child.try_wait().expect("try_wait") {
                Some(s) => {
                    status = Some(s);
                    break;
                }
                None if Instant::now() >= deadline => break,
                None => std::thread::sleep(Duration::from_millis(100)),
            }
        }
        let timed_out = status.is_none();
        if timed_out {
            let _ = child.kill();
            let _ = child.wait();
        }
        let cap = Capture {
            stdout: String::from_utf8_lossy(&out_reader.join().unwrap_or_default()).into_owned(),
            stderr: String::from_utf8_lossy(&err_reader.join().unwrap_or_default()).into_owned(),
            status: status.and_then(|s| s.code()),
        };
        assert!(
            !timed_out,
            "wayland-core {args:?} did not exit within {RUN_TIMEOUT:?}. A refusal that never \
             returns is a gate that cannot fail. {}",
            cap.either()
        );
        cap
    }
}

/// Drop an unparseable `.toml` into the profile's channel directory.
fn write_junk(home: &Path) {
    std::fs::write(
        home.join("channels").join("junk.toml"),
        "this is not = valid toml [[[\n",
    )
    .expect("write unparseable channel file");
}

/// The first line of the unreadable-config refusal, as the product emits it.
const UNREADABLE: &str = "inbound channel configuration could not be loaded";

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
    // The version is read from the crate rather than written out: the literal
    // that used to be here silently went stale when the token gained the
    // `[options]` half, and a stale literal here is a test that stops checking
    // the thing it names.
    assert!(
        message.contains(&format!(
            "acknowledge_open_admission = [\"{} ",
            wcore_channels::ADMISSION_SHAPE_VERSION
        )) && message.contains("dm=open"),
        "and must carry the exact remedy — the whole-shape token, not prose — or the desktop \
         user has no way forward; got: {message}"
    );
}

/// V-C on the desktop host. An unparseable channel file is a refusal the host
/// can render, not an empty policy set the gate silently approves of.
#[test]
fn json_stream_host_receives_an_error_frame_for_an_unparseable_channel_file() {
    let case = Case::new(BOUNDED_INBOUND, None);
    write_junk(&case.home);
    let cap = case.run(&[
        "--json-stream",
        "--project-dir",
        &case.project.display().to_string(),
    ]);

    assert_ne!(cap.status, Some(0), "the run must fail. {}", cap.either());
    assert!(
        cap.frames_of_type("ready").is_empty(),
        "a refused start must never claim ready. {}",
        cap.either()
    );
    let errors = cap.frames_of_type("error");
    assert_eq!(errors.len(), 1, "exactly one error frame. {}", cap.either());
    let message = errors[0]["error"]["message"]
        .as_str()
        .expect("message is a string");
    assert!(
        message.contains(UNREADABLE) && message.contains("junk.toml"),
        "the host must learn the config could not be read, and WHICH file; got: {message}"
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

/// V-C on the headless/REPL/TUI bootstrap. The junk file sits next to a BOUNDED
/// channel, so nothing here is open — the point is that an unreadable config
/// directory is never silently treated as an empty one.
#[test]
fn headless_one_shot_refuses_an_unparseable_channel_file() {
    let rt = tokio::runtime::Runtime::new().expect("tokio runtime");
    let server = rt.block_on(support::mock_llm::MockLlm::new().text(MOCK_MARKER).start());

    let case = Case::new(BOUNDED_INBOUND, Some(&server.uri()));
    write_junk(&case.home);
    let cap = case.run(&[
        "--no-tui",
        "--project-dir",
        &case.project.display().to_string(),
        "say hello",
    ]);

    assert_ne!(cap.status, Some(0), "the run must fail. {}", cap.either());
    assert!(
        cap.stderr.contains(UNREADABLE) && cap.stderr.contains("junk.toml"),
        "and must name the file it could not parse. {}",
        cap.either()
    );
    assert!(
        !cap.stdout.contains(MOCK_MARKER),
        "no turn may run. {}",
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

/// V-C on the gateway, in BOTH directions.
///
/// First half: an unparseable sibling next to an OPEN channel must not switch
/// the open-admission gate off. Second half: the same sibling next to a BOUNDED
/// channel must still be fatal, because the alternative is a working gateway
/// that silently starts denying everyone after one typo.
#[test]
fn gateway_run_refuses_an_unparseable_channel_file() {
    for (label, inbound) in [("open", OPEN_INBOUND), ("bounded", BOUNDED_INBOUND)] {
        let case = Case::new(inbound, None);
        write_junk(&case.home);
        let cap = case.run(&["gateway", "run"]);

        assert_ne!(
            cap.status,
            Some(0),
            "{label}: the gateway must refuse to start. {}",
            cap.either()
        );
        assert!(
            cap.stderr.contains(UNREADABLE) && cap.stderr.contains("junk.toml"),
            "{label}: and must name the file it could not parse. {}",
            cap.either()
        );
        assert!(
            !cap.stderr.contains("inbound dispatch unavailable"),
            "{label}: it must NOT degrade — degrading here is what let one junk file switch the \
             open-admission gate off. {}",
            cap.either()
        );
    }
}

/// POSITIVE CONTROL for the row above: on a bounded channel the same gateway
/// stays up. Without this, a gateway that could never start would pass the
/// refusal test.
#[test]
fn gateway_run_stays_up_on_a_bounded_channel() {
    let case = Case::new(BOUNDED_INBOUND, None);
    let mut child: OwnedTree<Child> = OwnedTree::new(
        case.command()
            .args(["gateway", "run"])
            .spawn()
            .expect("spawn gateway"),
    );
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

// ------------------------------------------------------------------ TUI (PTY)

/// The TUI on a real pseudo-terminal.
///
/// `cwd` is the project dir rather than the profile home so the run has the
/// same two authorities every other case here has. `ANTHROPIC_API_KEY` is
/// injected through `extra_env` because the PTY helper strips provider keys out
/// of the child for hermeticity and re-applies caller overrides afterwards.
#[cfg(unix)]
fn tui(case: &Case) -> support::pty::Pty {
    support::pty::Pty::spawn_with_args_env(
        &case.home,
        &case.project,
        // Taller and wider than the default 40x120: the refusal is a
        // multi-paragraph message and the vt100 grid this is read off has no
        // scrollback, so a short screen would drop the first line — the one
        // carrying [`REFUSAL`] — and turn a real failure into a green test.
        100,
        200,
        &["--project-dir", &case.project.display().to_string()],
        &[("ANTHROPIC_API_KEY", dummy_key())],
    )
}

/// How long the TUI legs wait. Same reasoning as [`RUN_TIMEOUT`]: a refusal
/// that never returns is a gate that cannot fail.
#[cfg(unix)]
const TUI_TIMEOUT: Duration = Duration::from_secs(45);

/// THE INTERACTIVE SURFACE. The TUI enters the alternate screen BEFORE
/// `AgentBootstrap::build`, so this is the one row where a refusal has to
/// survive the terminal being handed back: the operator must end up looking at
/// the reason, not at a restored blank prompt or a half-painted frame.
#[cfg(unix)]
#[test]
fn interactive_tui_refuses_before_it_paints() {
    let case = Case::new(OPEN_INBOUND, None);
    let mut pty = tui(&case);

    pty.wait_for(
        |s| s.contains(REFUSAL) && s.contains("p2chan"),
        TUI_TIMEOUT,
        "the open-admission refusal, naming the channel",
    );
    let status = pty
        .wait_for_exit(TUI_TIMEOUT)
        .expect("the TUI must EXIT on a refusal rather than stay up on an open configuration");
    assert!(
        !status.success(),
        "and must exit non-zero. screen:\n{}",
        pty.screen_text()
    );
}

/// V-C on the TUI. One unparseable sibling used to empty the policy list, and
/// this surface never saw a gate at all.
#[cfg(unix)]
#[test]
fn interactive_tui_refuses_an_unparseable_channel_file() {
    for (label, inbound) in [("open", OPEN_INBOUND), ("bounded", BOUNDED_INBOUND)] {
        let case = Case::new(inbound, None);
        write_junk(&case.home);
        let mut pty = tui(&case);

        pty.wait_for(
            |s| s.contains(UNREADABLE) && s.contains("junk.toml"),
            TUI_TIMEOUT,
            "the unreadable-config refusal, naming the file",
        );
        let status = pty
            .wait_for_exit(TUI_TIMEOUT)
            .unwrap_or_else(|| panic!("{label}: the TUI must EXIT on an unreadable channel dir"));
        assert!(
            !status.success(),
            "{label}: and must exit non-zero. screen:\n{}",
            pty.screen_text()
        );
    }
}

/// POSITIVE CONTROL. Without it, a TUI that could never start on this harness
/// would pass both legs above: every assertion there is satisfied by a process
/// that refuses for an unrelated reason and dies.
#[cfg(unix)]
#[test]
fn interactive_tui_paints_on_a_bounded_channel() {
    let case = Case::new(BOUNDED_INBOUND, None);
    let mut pty = tui(&case);

    // The status bar carries the model out of this case's own `config.toml`,
    // so seeing it proves the run got past `AgentBootstrap::build` — the call
    // the gate lives inside — and painted. Asserted rather than "some glyph
    // appeared" because a half-painted splash would satisfy the weaker check.
    pty.wait_for(
        |s| {
            s.contains("claude-sonnet-4-20250514")
                && !s.contains(REFUSAL)
                && !s.contains(UNREADABLE)
        },
        TUI_TIMEOUT,
        "a painted TUI, showing this profile's model, with no refusal on it",
    );
    pty.quit();
}
