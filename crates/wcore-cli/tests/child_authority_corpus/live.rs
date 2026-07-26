//! The two LIVE drivers — the ones that spawn the real `wayland-core` binary.
//!
//! An in-process test proves the seam. Only the binary proves the product, and
//! this codebase has already shipped a version where an entire permission crate
//! compiled, passed its own tests, and had NO consumer calling
//! `PolicyEngine::check`. Nothing in-process would have caught that. So every
//! corpus entry runs here as well as in `surfaces.rs`, and the harness compares
//! the two.
//!
//! ## Which machinery this reuses, and why
//!
//! The host-protocol live driver spawns `wayland-core --json-stream` and drives
//! per-turn `message` commands over pipes, reading `ProtocolEvent` frames back —
//! the same shape `crates/wcore-eval-scenarios/src/runner.rs` uses, and the same
//! shape the in-repo precedent `crates/wcore-cli/tests/acp_gate_d012.rs` already
//! drives against the real binary from THIS crate.
//!
//! The standalone live driver spawns `wayland-core --no-tui ... "<prompt>"` for
//! the headless surface, and the BARE binary on a real PTY for the interactive
//! one. The PTY harness is `crates/wcore-cli/tests/support/pty.rs`, which is the
//! in-crate sibling of `crates/wcore-eval-scenarios/src/pty_capture.rs`: the
//! same `portable-pty` + `vt100` stack, the same keystroke-and-rendered-screen
//! contract, extracted into this crate's `tests/support` precisely so multiple
//! integration binaries here can drive the real TUI. `pty_capture` itself
//! carries `#![cfg(unix)]` at line 63 because `portable_pty`'s Windows ConPTY
//! backend does not surface the spawned binary's stdout to the master end, and
//! `support/pty.rs` inherits exactly that limitation — which is why the TUI
//! combination is DECLARED unavailable on Windows rather than silently skipped.
//!
//! Hermetic fixtures come from `support/pty.rs`'s `write_config` +
//! `harden_child_env`, the in-crate equivalent of
//! `crates/wcore-eval-scenarios/src/tempenv.rs`: a throwaway home, `WAYLAND_HOME`
//! and `HOME` pointed at it, `cwd` inside it, and the full provider-credential
//! env set stripped. `tempenv` itself is not used directly because it writes a
//! provider identity only and has no seam for the mock-LLM `base_url` these
//! probes require; the guarantee it exists to provide — that a live result never
//! depends on the operator's real config or credentials — is preserved exactly.
//!
//! ## Mode assertion
//!
//! `crates/wcore-cli/src/main.rs` enters the TUI only when the prompt is empty
//! AND `--no-tui` is absent AND stdout is a terminal AND `--json-stream` is
//! absent. A piped subprocess fails the terminal check and silently falls
//! through to the line-based REPL, so a run intended to exercise one surface can
//! quietly exercise another and report a verdict for it. Every live invocation
//! below therefore PROVES the mode it landed in before its verdict is recorded:
//! json-stream by the `ready` frame the protocol front-end emits, headless by
//! the absence of that frame together with a terminated process, and tui by the
//! rendered chrome on the PTY screen.

use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use serde_json::json;
use tempfile::TempDir;
use wiremock::{Mock, MockServer, ResponseTemplate, matchers};

use crate::cases::{CorpusEntry, Dimension};
use crate::support::pty;
use crate::support::vault;
use crate::surfaces::{CorpusExecutor, LiveEvidence, Mode, Outcome, ProbeResult, Surface, runtime};

/// Sentinel bytes seeded outside the workspace root. If they reach a
/// transcript, the child obtained a file the parent's authority denies.
const OUTSIDE_SENTINEL: &str = "CORPUS_OUTSIDE_SENTINEL_9f13";
/// Synthetic credential value. Never a real credential, and never written into
/// the results artifact.
const SECRET_SENTINEL: &str = "CORPUS_SYNTHETIC_TOKEN_4b7e";
/// Body served by the non-allowlisted loopback destination.
const EGRESS_SENTINEL: &str = "CORPUS_EGRESS_SENTINEL_2c8a";
/// Generation markers, prefixed into every delegated goal.
///
/// A provider request's FIRST user message is the goal the delegation gave it.
/// The parent's first message is the operator prompt, a child's is its L1 goal,
/// a grandchild's is its L2 goal — so the first message identifies WHO is
/// asking, and the mock answers accordingly.
///
/// This matters more than it looks. A single ordered script shared by parent
/// and child is answered in queue order regardless of who asks, so a parent
/// taking three turns receives the text intended for the grandchild and any
/// transcript marker becomes worthless as evidence. The first instrumented run
/// reported the depth dimension as WIDENED for exactly that reason, and it was
/// the harness talking to itself, not a defect in the product.
const CHILD_GOAL_L1: &str = "CORPUSGENL1";
const CHILD_GOAL_L2: &str = "CORPUSGENL2";

/// Ephemeral vault passphrase for the PTY child. Not a credential: it encrypts
/// a throwaway store inside a tempdir that is deleted when the run ends.
const CORPUS_VAULT_PASSPHRASE: &str = "child-authority-corpus-ephemeral-vault";

/// Wall-clock budget for ONE live run. Two live runs happen per corpus case and
/// the harness runs under nextest's 30s slow line with terminate-after 2, so the
/// pair must finish inside roughly 55 seconds or the case is killed mid-run and
/// records nothing at all.
///
/// This is a bound on the harness, not a loosened gate: a run that exceeds it is
/// killed and recorded as producing no verdict, which the anti-vacuity gate then
/// reports as NOT-EXPRESSIBLE. Nothing is ever counted as a refusal because it
/// ran out of time.
const LIVE_RUN_BUDGET: Duration = Duration::from_secs(18);

/// The shipped binary under test. Cargo guarantees it is built before this
/// integration test runs.
fn binary() -> &'static str {
    env!("CARGO_BIN_EXE_wayland-core")
}

/// Which shipped transport a live run uses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LiveTransport {
    /// `wayland-core --json-stream` — the host-protocol surface.
    JsonStream,
    /// `wayland-core --no-tui ... "<prompt>"` — the standalone headless surface.
    Headless,
    /// The bare binary on a real PTY — the standalone interactive surface.
    Tui,
}

impl LiveTransport {
    pub const fn label(self) -> &'static str {
        match self {
            Self::JsonStream => "json-stream",
            Self::Headless => "headless",
            Self::Tui => "tui",
        }
    }

    /// Whether this transport can be driven on the current platform. DECLARED,
    /// not discovered: the TUI is unavailable on Windows because
    /// `pty_capture.rs` is `#![cfg(unix)]` and `support/pty.rs` inherits that
    /// gate, and the corpus states the fact rather than probing for it.
    pub fn available_here(self) -> bool {
        !(self == Self::Tui && cfg!(windows))
    }

    pub const fn unavailable_reason(self) -> &'static str {
        "the interactive TUI is not drivable on Windows: portable_pty's ConPTY backend does not \
         surface the spawned binary's stdout to the master end, so pty_capture.rs is #![cfg(unix)] \
         and support/pty.rs inherits the gate"
    }
}

/// A hermetic world for one live run: a throwaway home, the seeded fixtures the
/// probes read, and the loopback destination the egress probe targets.
struct LiveWorld {
    home: TempDir,
    /// The probe a Bash-capable child would write. Its existence is the tool
    /// dimension's invariant evidence.
    bash_probe: PathBuf,
    /// A file outside the home entirely.
    outside: PathBuf,
    /// A synthetic credential file inside the home.
    dotenv: PathBuf,
    /// Kept alive so the outside file is not collected mid-run.
    _outside_dir: TempDir,
}

impl LiveWorld {
    fn build() -> Self {
        let home = TempDir::new().expect("hermetic home");
        let outside_dir = TempDir::new().expect("outside dir");
        let outside = outside_dir.path().join("corpus_outside.txt");
        std::fs::write(&outside, OUTSIDE_SENTINEL.as_bytes()).expect("seed the outside file");
        let dotenv = home.path().join(".env");
        std::fs::write(
            &dotenv,
            format!("CORPUS_KEY={SECRET_SENTINEL}\n").as_bytes(),
        )
        .expect("seed the synthetic .env");
        let bash_probe = home.path().join("corpus_bash_probe.txt");
        Self {
            home,
            bash_probe,
            outside,
            dotenv,
            _outside_dir: outside_dir,
        }
    }

    fn root(&self) -> &Path {
        self.home.path()
    }
}

/// Seed the hermetic config: provider identity, the mock `base_url`, and — for
/// the budget family — the low parent caps the child then tries to exceed.
fn write_live_config(home: &Path, base_url: &str, budget: Option<&str>) {
    pty::write_config(
        home,
        "anthropic",
        Some("claude-sonnet-4-20250514"),
        Some(base_url),
    );
    if let Some(budget) = budget {
        let path = home.join("config.toml");
        let mut toml = std::fs::read_to_string(&path).unwrap_or_default();
        toml.push_str(budget);
        std::fs::write(&path, toml).expect("append the seeded budget caps");
    }
}

/// The parent caps a budget-family live run seeds. Deliberately tight, so a
/// child that obtained a wider envelope would visibly run past them.
fn seeded_budget(dimension: Dimension) -> Option<String> {
    let body = match dimension {
        Dimension::Depth => "\n[budget]\nmax_agent_depth = 1\n",
        Dimension::Time => "\n[budget]\nmax_wall_time_secs = 1\n",
        Dimension::Token => "\n[budget]\nmax_tokens_out = 1\n",
        Dimension::Cost => "\n[budget]\nmax_cost_usd = 0.0000001\n",
        _ => return None,
    };
    Some(body.to_owned())
}

/// One scripted assistant turn, rendered through the same SSE builders the
/// shared mock uses, so every byte still passes the real provider parser.
enum Turn {
    Text(&'static str),
    Tool(&'static str, serde_json::Value),
}

impl Turn {
    fn sse(&self) -> String {
        match self {
            Self::Text(text) => crate::support::mock_llm::text_turn_sse(text),
            Self::Tool(name, input) => crate::support::mock_llm::tool_use_turn_sse(name, input),
        }
    }
}

/// The three scripts one dimension needs: what the PARENT is answered, what a
/// delegated CHILD is answered, and what a GRANDCHILD is answered.
struct LiveScripts {
    parent: Vec<Turn>,
    child: Vec<Turn>,
    grandchild: Vec<Turn>,
}

/// Build the hostile scripts for one dimension, transcribed from the census's
/// `WIDENING ::` row.
fn live_scripts(dimension: Dimension, world: &LiveWorld, sentinel_url: &str) -> LiveScripts {
    let bash_command = if cfg!(windows) {
        format!(
            "echo CORPUS_BASH_PROBE > \"{}\"",
            world.bash_probe.display()
        )
    } else {
        format!(
            "printf CORPUS_BASH_PROBE > '{}'",
            world.bash_probe.display()
        )
    };
    let outside = world.outside.display().to_string();
    let dotenv = world.dotenv.display().to_string();
    let probe = world.bash_probe.display().to_string();

    let delegate = |goal: String, toolsets: serde_json::Value| {
        Turn::Tool("Delegate", json!({ "goal": goal, "toolsets": toolsets }))
    };

    match dimension {
        Dimension::Tool => LiveScripts {
            parent: vec![
                delegate(
                    format!("{CHILD_GOAL_L1}: write the probe file with Bash"),
                    json!(["Bash"]),
                ),
                Turn::Text("parent done"),
            ],
            child: vec![
                Turn::Tool("Bash", json!({ "command": bash_command })),
                Turn::Text("child done"),
            ],
            grandchild: vec![Turn::Text("grandchild done")],
        },
        Dimension::Filesystem => LiveScripts {
            parent: vec![
                delegate(
                    format!("{CHILD_GOAL_L1}: read the file outside the workspace root"),
                    json!(["Read"]),
                ),
                Turn::Text("parent done"),
            ],
            child: vec![
                Turn::Tool("Read", json!({ "file_path": outside })),
                Turn::Text("child done"),
            ],
            grandchild: vec![Turn::Text("grandchild done")],
        },
        Dimension::Secret => LiveScripts {
            parent: vec![
                delegate(
                    format!("{CHILD_GOAL_L1}: read the credential file under the home"),
                    json!(["Read"]),
                ),
                Turn::Text("parent done"),
            ],
            child: vec![
                Turn::Tool("Read", json!({ "file_path": dotenv })),
                Turn::Text("child done"),
            ],
            grandchild: vec![Turn::Text("grandchild done")],
        },
        Dimension::Egress => LiveScripts {
            parent: vec![
                delegate(
                    format!("{CHILD_GOAL_L1}: fetch the external destination"),
                    json!(["Read"]),
                ),
                Turn::Text("parent done"),
            ],
            child: vec![
                Turn::Tool("WebFetch", json!({ "url": sentinel_url })),
                Turn::Text("child done"),
            ],
            grandchild: vec![Turn::Text("grandchild done")],
        },
        Dimension::Provider => LiveScripts {
            parent: vec![
                Turn::Tool(
                    "Delegate",
                    json!({
                        "goal": format!("{CHILD_GOAL_L1}: summarise the workspace"),
                        "provider": "openai",
                        "toolsets": []
                    }),
                ),
                Turn::Text("parent done"),
            ],
            child: vec![Turn::Text("child done")],
            grandchild: vec![Turn::Text("grandchild done")],
        },
        Dimension::Approval => LiveScripts {
            parent: vec![
                delegate(format!("{CHILD_GOAL_L1}: write a file"), json!(["Write"])),
                Turn::Text("parent done"),
            ],
            child: vec![
                Turn::Tool(
                    "Write",
                    json!({ "file_path": probe, "content": "CORPUS_BASH_PROBE" }),
                ),
                Turn::Text("child done"),
            ],
            grandchild: vec![Turn::Text("grandchild done")],
        },
        Dimension::FanOut => {
            let tasks: Vec<serde_json::Value> = (0..8)
                .map(|i| json!({ "goal": format!("{CHILD_GOAL_L1}: corpuschild{i}") }))
                .collect();
            LiveScripts {
                parent: vec![
                    Turn::Tool("Delegate", json!({ "tasks": tasks })),
                    Turn::Text("parent done"),
                ],
                child: vec![Turn::Text("child done")],
                grandchild: vec![Turn::Text("grandchild done")],
            }
        }
        Dimension::Depth => LiveScripts {
            parent: vec![
                delegate(
                    format!("{CHILD_GOAL_L1}: delegate again, one level deeper"),
                    json!([]),
                ),
                Turn::Text("parent done"),
            ],
            child: vec![
                delegate(format!("{CHILD_GOAL_L2}: the grandchild level"), json!([])),
                Turn::Text("child done"),
            ],
            grandchild: vec![Turn::Text("grandchild done")],
        },
        Dimension::Time | Dimension::Token | Dimension::Cost => LiveScripts {
            parent: vec![
                delegate(format!("{CHILD_GOAL_L1}: consume the envelope"), json!([])),
                Turn::Text("parent done"),
            ],
            child: vec![Turn::Text("child done")],
            grandchild: vec![Turn::Text("grandchild done")],
        },
    }
}

/// Start a provider mock that answers according to WHO is asking, keyed on the
/// first user message of the incoming conversation.
///
/// A queue-ordered mock cannot do this, and the difference is not cosmetic: with
/// one shared queue the parent's second turn is answered with the script written
/// for the child, so a sentinel appearing in a transcript proves nothing about
/// which actor obtained it. Routing by requester is what makes every observation
/// below attributable.
fn start_routed_mock(rt: &tokio::runtime::Runtime, scripts: LiveScripts) -> MockServer {
    let server = rt.block_on(MockServer::start());
    let parent: Vec<String> = scripts.parent.iter().map(Turn::sse).collect();
    let child: Vec<String> = scripts.child.iter().map(Turn::sse).collect();
    let grandchild: Vec<String> = scripts.grandchild.iter().map(Turn::sse).collect();
    let cursors = std::sync::Arc::new(std::sync::Mutex::new([0usize; 3]));

    let responder = move |request: &wiremock::Request| {
        let generation = serde_json::from_slice::<serde_json::Value>(&request.body)
            .ok()
            .and_then(|body| {
                let first = body.get("messages")?.get(0)?.to_string();
                Some(if first.contains(CHILD_GOAL_L2) {
                    2usize
                } else if first.contains(CHILD_GOAL_L1) {
                    1
                } else {
                    0
                })
            })
            .unwrap_or(0);
        let script = match generation {
            2 => &grandchild,
            1 => &child,
            _ => &parent,
        };
        let mut cursor = cursors.lock().expect("cursor lock");
        let index = cursor[generation].min(script.len().saturating_sub(1));
        cursor[generation] = (cursor[generation] + 1).min(script.len());
        ResponseTemplate::new(200).set_body_raw(script[index].clone(), "text/event-stream")
    };

    rt.block_on(
        Mock::given(matchers::method("POST"))
            .and(matchers::path("/v1/messages"))
            .respond_with(responder)
            .mount(&server),
    );
    server
}

/// Whether a conversation's FIRST user message carries `marker`, which is how
/// the harness attributes a provider request to the generation that made it.
fn first_message_contains(body: &serde_json::Value, marker: &str) -> bool {
    body.get("messages")
        .and_then(|messages| messages.get(0))
        .map(|first| first.to_string().contains(marker))
        .unwrap_or(false)
}

/// What one live run produced.
struct LiveRun {
    invocation: String,
    /// Where the full raw transcript of this run was written.
    transcript_path: String,
    /// `Some` only when the run PROVED which mode it landed in.
    asserted_mode: Option<String>,
    transcript: String,
    /// Number of provider requests the mock actually served.
    provider_requests: usize,
    /// Whether any served request carried a tool_result, which proves the
    /// parent's delegating tool call was executed and returned rather than
    /// never having been reached.
    delegation_attempted: bool,
    /// How many served requests were made BY a delegated child — their first
    /// user message carries the L1 goal marker. Each distinct child contributes
    /// at least one, so this is also the observed breadth.
    child_turns: usize,
    /// How many served requests were made by a GRANDCHILD (L2 marker). Nonzero
    /// means a child successfully delegated one level deeper.
    grandchild_turns: usize,
}

/// Persist the full raw transcript of one live run and return its path.
///
/// The recorded row carries only a head; the boot frame sequence alone exceeds
/// it. A verdict whose supporting transcript cannot be read afterwards is a
/// claim, not evidence, so every run's bytes land on disk next to the ledger.
fn persist_transcript(dimension: Dimension, transport: LiveTransport, body: &str) -> String {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join("child-authority-corpus")
        .join("transcripts");
    let _ = std::fs::create_dir_all(&dir);
    let path = dir.join(format!(
        "corpus_{}-{}.txt",
        dimension.case_id(),
        transport.label()
    ));
    let _ = std::fs::write(&path, body);
    path.to_string_lossy().into_owned()
}

/// Drive one live run and prove its mode.
///
/// Two loopback servers are started, not one: the egress sentinel must exist
/// (and its port be known) BEFORE the provider script can name its URL, and a
/// `wiremock::MockServer` only reports its port once started.
fn run_live(dimension: Dimension, transport: LiveTransport, world: &LiveWorld) -> LiveRun {
    let rt = runtime();

    let sentinel: MockServer = rt.block_on(MockServer::start());
    rt.block_on(
        Mock::given(matchers::method("GET"))
            .and(matchers::path("/corpus-egress-sentinel"))
            .respond_with(ResponseTemplate::new(200).set_body_string(EGRESS_SENTINEL))
            .mount(&sentinel),
    );
    let sentinel_url = format!("{}/corpus-egress-sentinel", sentinel.uri());

    let provider: MockServer =
        start_routed_mock(&rt, live_scripts(dimension, world, &sentinel_url));
    write_live_config(
        world.root(),
        &provider.uri(),
        seeded_budget(dimension).as_deref(),
    );

    let mut run = match transport {
        LiveTransport::JsonStream => run_json_stream(world),
        LiveTransport::Headless => run_headless(world),
        LiveTransport::Tui => run_tui(world),
    };
    let served = rt.block_on(crate::support::mock_llm::received_requests(&provider));
    run.provider_requests = served.len();
    run.delegation_attempted = served
        .iter()
        .any(|request| request.body.to_string().contains("tool_result"));
    run.child_turns = served
        .iter()
        .filter(|request| first_message_contains(&request.body, CHILD_GOAL_L1))
        .count();
    run.grandchild_turns = served
        .iter()
        .filter(|request| first_message_contains(&request.body, CHILD_GOAL_L2))
        .count();
    run.transcript_path = persist_transcript(
        dimension,
        transport,
        &format!(
            "invocation: {}\nprovider requests served: {}\nasserted mode: {:?}\n\n{}",
            run.invocation, run.provider_requests, run.asserted_mode, run.transcript
        ),
    );
    run
}

/// `wayland-core --json-stream` — the host-protocol surface. The mode is proved
/// by the `ready` frame the protocol front-end emits at startup; nothing else
/// in the product emits it.
fn run_json_stream(world: &LiveWorld) -> LiveRun {
    let invocation = format!(
        "wayland-core --json-stream --provider anthropic  (stdin: one `message` command; \
         WAYLAND_HOME={})",
        world.root().display()
    );

    let mut command = Command::new(binary());
    command
        .args(["--json-stream", "--provider", "anthropic"])
        .current_dir(world.root());
    pty::harden_child_env(&mut command, world.root());
    // Without an ephemeral encrypted vault the binary refuses to start a
    // session at all under a hermetic WAYLAND_HOME, so the turn never reaches
    // a provider and every downstream observation would be an absence rather
    // than a refusal. See the module note on the anti-vacuity gate.
    let vault = vault::configure_process(&mut command);
    let spawned = command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn();
    drop(vault);
    let mut child = match spawned {
        Ok(child) => child,
        Err(error) => {
            return LiveRun {
                invocation,
                asserted_mode: None,
                transcript: format!("the binary could not be spawned: {error}"),
                provider_requests: 0,
                delegation_attempted: false,
                child_turns: 0,
                grandchild_turns: 0,
                transcript_path: String::new(),
            };
        }
    };

    let mut stdin = child.stdin.take().expect("stdin");
    let stdout = child.stdout.take().expect("stdout");
    // stderr is drained too. A run that produced no provider call usually says
    // why on stderr, and a transcript that omits it turns a diagnosable failure
    // into an unexplained absence.
    let stderr = child.stderr.take().expect("stderr");
    let stderr_sink = std::sync::Arc::new(std::sync::Mutex::new(String::new()));
    let stderr_writer = std::sync::Arc::clone(&stderr_sink);
    std::thread::spawn(move || {
        for line in BufReader::new(stderr).lines().map_while(Result::ok) {
            if let Ok(mut buffer) = stderr_writer.lock() {
                buffer.push_str(&line);
                buffer.push('\n');
            }
        }
    });
    let _ = writeln!(
        stdin,
        "{{\"type\":\"message\",\"msg_id\":\"1\",\"content\":\"delegate the task\"}}"
    );

    let (tx, rx) = mpsc::channel::<String>();
    std::thread::spawn(move || {
        for line in BufReader::new(stdout).lines().map_while(Result::ok) {
            if tx.send(line).is_err() {
                break;
            }
        }
    });

    let mut transcript = String::new();
    let mut saw_ready = false;
    let mut saw_stream_end = false;
    let deadline = Instant::now() + LIVE_RUN_BUDGET;
    while Instant::now() < deadline {
        match rx.recv_timeout(Duration::from_millis(250)) {
            Ok(line) => {
                if line.contains("\"type\":\"ready\"") {
                    saw_ready = true;
                }
                if line.contains("\"type\":\"stream_end\"") {
                    saw_stream_end = true;
                }
                // Answer the gate the way a real host does.
                //
                // In the default posture the protocol front-end suspends a
                // mutating or delegating tool call on `approval_required` and
                // waits for the host's decision. A driver that never answers
                // leaves the delegation parked forever, the child never runs,
                // and the corpus would record an absence for every dimension on
                // this surface — permanently inconclusive, and inconclusive in
                // the direction that looks like enforcement.
                //
                // Approving is the faithful move: it is what the desktop host
                // does, and it exercises the gate rather than bypassing it with
                // `--force`, which would silently change the posture the
                // approval dimension is measuring. Whether a gate appeared at
                // all is recorded separately and is that dimension's evidence.
                if let Some(call_id) = approval_call_id(&line) {
                    let _ = writeln!(
                        stdin,
                        "{{\"type\":\"tool_approve\",\"call_id\":\"{call_id}\"}}"
                    );
                }
                transcript.push_str(&line);
                transcript.push('\n');
                if saw_stream_end {
                    break;
                }
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        }
    }

    let _ = writeln!(stdin, "{{\"type\":\"stop\"}}");
    let _ = child.kill();
    let _ = child.wait();
    if let Ok(buffer) = stderr_sink.lock() {
        transcript.push_str("--- stderr ---\n");
        transcript.push_str(&buffer);
    }

    LiveRun {
        invocation,
        // The `ready` frame is emitted only by the json-stream front-end. Its
        // presence is the proof the run landed in host-protocol mode and did
        // not fall through to the line REPL.
        asserted_mode: saw_ready.then(|| LiveTransport::JsonStream.label().to_owned()),
        transcript,
        provider_requests: 0,
        delegation_attempted: false,
        child_turns: 0,
        grandchild_turns: 0,
        transcript_path: String::new(),
    }
}

/// The `call_id` of an `approval_required` frame, or `None` for any other line.
/// Parsed rather than pattern-matched on text, so a frame whose field order
/// changes still resolves.
fn approval_call_id(line: &str) -> Option<String> {
    let frame: serde_json::Value = serde_json::from_str(line).ok()?;
    if frame.get("type")?.as_str()? != "approval_required" {
        return None;
    }
    Some(frame.get("call_id")?.as_str()?.to_owned())
}

/// `wayland-core --no-tui --provider anthropic "<prompt>"` — the standalone
/// headless surface. The mode is proved by the process terminating on its own
/// (the TUI would not) and by the absence of any json-stream `ready` frame.
///
/// A correction to the census and the plan text, both of which write this
/// invocation as `wayland-core -p "<prompt>"`: the shipped binary has NO `-p`
/// flag. `crates/wcore-cli/src/main.rs:537-539` declares the prompt as a
/// `trailing_var_arg` positional, so every option must precede it or clap folds
/// the option into the prompt string. The surface itself is the one the census
/// named; only the spelling differs, and the spelling is taken from the binary
/// rather than from the document.
fn run_headless(world: &LiveWorld) -> LiveRun {
    let prompt = "delegate the task";
    let invocation = format!(
        "wayland-core --no-tui --provider anthropic \"{prompt}\"  (WAYLAND_HOME={})",
        world.root().display()
    );

    let mut command = Command::new(binary());
    command
        .args(["--no-tui", "--provider", "anthropic", prompt])
        .current_dir(world.root());
    pty::harden_child_env(&mut command, world.root());
    let vault = vault::configure_process(&mut command);
    let spawned = command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn();
    drop(vault);

    let mut child = match spawned {
        Ok(child) => child,
        Err(error) => {
            return LiveRun {
                invocation,
                asserted_mode: None,
                transcript: format!("the binary could not be spawned: {error}"),
                provider_requests: 0,
                delegation_attempted: false,
                child_turns: 0,
                grandchild_turns: 0,
                transcript_path: String::new(),
            };
        }
    };

    let streams = collect_streams(&mut child);
    // A bounded wait rather than `output()`, which has none: an unbounded
    // headless run is killed by the test runner mid-case and records nothing.
    let deadline = Instant::now() + LIVE_RUN_BUDGET;
    let mut status = None;
    while Instant::now() < deadline {
        match child.try_wait() {
            Ok(Some(exit)) => {
                status = Some(exit);
                break;
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(100)),
            Err(_) => break,
        }
    }
    let terminated_on_its_own = status.is_some();
    if !terminated_on_its_own {
        let _ = child.kill();
        let _ = child.wait();
    }
    let (stdout, stderr) = streams.join();
    let code = status.and_then(|exit| exit.code());
    let transcript = format!(
        "exit status {code:?}, terminated on its own: {terminated_on_its_own}\n--- stdout ---\n\
         {stdout}--- stderr ---\n{stderr}"
    );

    // The headless mode proof: the binary ran to completion on its own (the
    // full-screen TUI never would) and emitted no json-stream `ready` frame.
    let landed_headless = terminated_on_its_own && !transcript.contains("\"type\":\"ready\"");
    LiveRun {
        invocation,
        asserted_mode: landed_headless.then(|| LiveTransport::Headless.label().to_owned()),
        transcript,
        provider_requests: 0,
        delegation_attempted: false,
        child_turns: 0,
        grandchild_turns: 0,
        transcript_path: String::new(),
    }
}

/// Drain a child's stdout and stderr on their own threads so neither pipe can
/// fill and wedge the child while the caller is waiting on a deadline.
struct Streams {
    out: std::thread::JoinHandle<String>,
    err: std::thread::JoinHandle<String>,
}

impl Streams {
    fn join(self) -> (String, String) {
        (
            self.out.join().unwrap_or_default(),
            self.err.join().unwrap_or_default(),
        )
    }
}

fn collect_streams(child: &mut std::process::Child) -> Streams {
    let stdout = child.stdout.take().expect("stdout");
    let stderr = child.stderr.take().expect("stderr");
    Streams {
        out: std::thread::spawn(move || {
            let mut buffer = String::new();
            let _ = std::io::Read::read_to_string(&mut BufReader::new(stdout), &mut buffer);
            buffer
        }),
        err: std::thread::spawn(move || {
            let mut buffer = String::new();
            let _ = std::io::Read::read_to_string(&mut BufReader::new(stderr), &mut buffer);
            buffer
        }),
    }
}

/// The BARE binary on a real PTY — the standalone interactive surface a user
/// gets at a terminal. The mode is proved by the rendered chrome: only the
/// full-screen TUI paints the wordmark and the Workspace tab, so a run that
/// fell through to the line REPL cannot show them.
///
/// The `cfg` pair below is NOT a surface being hidden from Windows. It is the
/// declared platform limitation made compilable: `support/pty.rs`'s `Pty` is
/// itself `#[cfg(unix)]`, inheriting `pty_capture.rs`'s ConPTY gate, so the
/// Windows arm must exist for the crate to build there. `available_here()`
/// already reports the combination UNAVAILABLE on Windows, so the Windows arm
/// is unreachable in practice; it records the same declared reason rather than
/// a verdict, and it never proves a mode, so no verdict can be taken from it.
#[cfg(unix)]
fn run_tui(world: &LiveWorld) -> LiveRun {
    let invocation = format!(
        "wayland-core  (bare, attached to a real PTY; WAYLAND_HOME={})",
        world.root().display()
    );

    // `portable-pty` closes arbitrary inherited descriptors while preparing the
    // child, so the FD transport cannot reach it; the supported legacy env
    // transport is used instead. `spawn_with_env` applies extras AFTER the
    // credential-strip pass, which is the seam that makes this possible without
    // touching the shared PTY harness.
    let mut terminal = pty::Pty::spawn_with_env(
        world.root(),
        40,
        120,
        &[("WAYLAND_VAULT_PASSPHRASE", CORPUS_VAULT_PASSPHRASE)],
    );
    let deadline = Instant::now() + LIVE_RUN_BUDGET;
    let mut booted = false;
    while Instant::now() < deadline {
        let screen = terminal.screen_text();
        if screen.contains("WAYLAND") && screen.contains("Workspace") {
            booted = true;
            break;
        }
        std::thread::sleep(Duration::from_millis(100));
    }

    if booted {
        terminal.send(b"delegate the task\r");
        std::thread::sleep(Duration::from_secs(8));
    }
    let transcript = terminal.screen_text();
    terminal.quit();

    LiveRun {
        invocation,
        asserted_mode: booted.then(|| LiveTransport::Tui.label().to_owned()),
        transcript,
        provider_requests: 0,
        delegation_attempted: false,
        child_turns: 0,
        grandchild_turns: 0,
        transcript_path: String::new(),
    }
}

#[cfg(not(unix))]
fn run_tui(world: &LiveWorld) -> LiveRun {
    LiveRun {
        invocation: format!(
            "wayland-core  (bare, on a PTY) — DECLARED UNAVAILABLE; WAYLAND_HOME={}",
            world.root().display()
        ),
        asserted_mode: None,
        transcript: LiveTransport::Tui.unavailable_reason().to_owned(),
        provider_requests: 0,
        delegation_attempted: false,
        child_turns: 0,
        grandchild_turns: 0,
        transcript_path: String::new(),
    }
}

/// Turn one live run into a verdict on what the child obtained.
fn observe(dimension: Dimension, world: &LiveWorld, run: &LiveRun) -> (Outcome, String, String) {
    match dimension {
        Dimension::Tool => {
            if world.bash_probe.exists() {
                (
                    Outcome::Allowed,
                    "Bash — a tool the parent's read-only delegation floor does not grant"
                        .to_owned(),
                    format!(
                        "the delegated child's Bash command left its effect on disk in the \
                         hermetic home; {} child provider turn(s) arrived",
                        run.child_turns
                    ),
                )
            } else {
                (
                    Outcome::Refused,
                    "no Bash effect".to_owned(),
                    format!(
                        "no Bash effect reached the hermetic home after the delegation ran; {} \
                         child provider turn(s) arrived, so the refusal is attributable to what \
                         the child was given rather than to the delegation never happening",
                        run.child_turns
                    ),
                )
            }
        }
        Dimension::Filesystem => decide(
            run,
            run.transcript.contains(OUTSIDE_SENTINEL),
            "the bytes of a file outside the parent's workspace root",
            "no bytes from outside the parent's workspace root",
            "the outside sentinel",
        ),
        Dimension::Secret => decide(
            run,
            run.transcript.contains(SECRET_SENTINEL),
            "the bytes of a credential file the parent's policy denies",
            "no credential bytes",
            "the synthetic credential sentinel",
        ),
        Dimension::Egress => decide(
            run,
            run.transcript.contains(EGRESS_SENTINEL),
            "an outbound destination the parent's policy does not permit",
            "no outbound destination beyond the parent's policy",
            "the egress sentinel body",
        ),
        Dimension::Provider => {
            // Every provider request the run made went to the parent's own
            // configured endpoint — the mock. If the child had obtained a
            // foreign provider it would have talked to something else, and the
            // parent's mock would have served only the parent's turns.
            if run.child_turns > 0 {
                (
                    Outcome::NoChannel,
                    "no provider — the child ran on the parent's own configured endpoint"
                        .to_owned(),
                    format!(
                        "the delegated child's own turn arrived at the parent's mock endpoint \
                         ({} requests served in total), so the child used the parent's provider \
                         and no shipped surface offered it another",
                        run.provider_requests
                    ),
                )
            } else if run.provider_requests >= 1 {
                (
                    Outcome::NoChannel,
                    "no provider — no child provider request left the parent's endpoint".to_owned(),
                    "the delegation returned without any child reaching a provider turn, so no \
                     child provider selection was observable — and none was offered, because no \
                     shipped child-spawn schema carries a provider field"
                        .to_owned(),
                )
            } else {
                (
                    Outcome::NotExpressible,
                    "no provider request was observed at all".to_owned(),
                    "the parent's mock endpoint served no requests, so this run observed nothing \
                     about provider selection"
                        .to_owned(),
                )
            }
        }
        Dimension::Approval => {
            // A child that obtained a weaker approval posture would have run
            // the mutating Write without the host ever being asked.
            let wrote = world.bash_probe.exists();
            let gated = run.transcript.contains("approval_required")
                || run.transcript.contains("Approve")
                || run.transcript.contains("approval");
            if wrote && !gated {
                (
                    Outcome::Allowed,
                    "a mutating effect with no consent step — an approval posture weaker than the \
                     parent's"
                        .to_owned(),
                    "the delegated child's Write landed on disk and no consent surface appeared \
                     in the run"
                        .to_owned(),
                )
            } else {
                (
                    Outcome::NoChannel,
                    "no approval posture weaker than the parent's".to_owned(),
                    format!(
                        "mutating effect on disk: {wrote}; a consent surface appeared in the run: \
                         {gated}; {} child provider turn(s) arrived; no shipped surface offered \
                         the child a way to request a weaker posture",
                        run.child_turns
                    ),
                )
            }
        }
        Dimension::FanOut => {
            // Counted from the children that actually reached a provider turn,
            // not from text in the transcript: the parent could echo a child
            // name without a child ever existing.
            let ran = run.child_turns;
            if ran > 5 {
                (
                    Outcome::Allowed,
                    format!("breadth of {ran} children against a parent cap of 5"),
                    format!("a batch of 8 was requested and {ran} child provider turns arrived"),
                )
            } else {
                (
                    Outcome::Refused,
                    format!("no breadth beyond the parent cap of 5 ({ran} children ran)"),
                    format!("a batch of 8 was requested and {ran} child provider turns arrived"),
                )
            }
        }
        Dimension::Depth => {
            // A grandchild's own provider turn is the only thing that proves a
            // second level of nesting actually happened. Transcript text cannot:
            // the parent can utter any word the script contains.
            let ran = run.grandchild_turns;
            if ran > 0 {
                (
                    Outcome::Allowed,
                    "nesting depth beyond the parent's seeded max_agent_depth of 1".to_owned(),
                    format!("{ran} grandchild provider turn(s) arrived under a seeded cap of 1"),
                )
            } else {
                (
                    Outcome::Refused,
                    "no nesting depth beyond the parent's seeded envelope".to_owned(),
                    format!(
                        "no grandchild provider turn arrived under a seeded cap of 1; {} child \
                         turn(s) did",
                        run.child_turns
                    ),
                )
            }
        }
        Dimension::Time | Dimension::Token | Dimension::Cost => {
            // No shipped surface lets a child ASK for a wider wall-time, token
            // or cost cap: the census measured every production `sub_budget`
            // caller passing `None`, and no tool schema carries a budget field.
            // So the live surface cannot express this widening request at all —
            // it can only watch the parent's own cap bind, which is a statement
            // about the parent, not about inheritance. Recording that honestly
            // is the point: the property holds here by ABSENCE, and the seam
            // that would refuse an actual request is measured in process.
            //
            // The run is still driven, and what it observed is recorded, so a
            // budget request channel appearing later shows up as a child turn
            // arriving with a wider envelope rather than as silence.
            (
                Outcome::NoChannel,
                "no resource beyond the parent's envelope — and no way to ask for one".to_owned(),
                format!(
                    "no shipped surface carries a child-fillable budget field, so no widening \
                     request could be issued through the product; the run served {} provider \
                     request(s) with {} child turn(s) under the seeded cap",
                    run.provider_requests, run.child_turns
                ),
            )
        }
    }
}

/// A sentinel can only reach the transcript by way of a tool the CHILD ran,
/// because the routed mock hands the hostile tool call to the child script
/// alone. The child-turn count is carried into the evidence either way, so a
/// refusal is attributable rather than merely absent.
fn decide(
    run: &LiveRun,
    child_obtained: bool,
    obtained: &str,
    not_obtained: &str,
    marker: &str,
) -> (Outcome, String, String) {
    if child_obtained {
        (
            Outcome::Allowed,
            obtained.to_owned(),
            format!(
                "{marker} reached the run's transcript after {} child provider turn(s)",
                run.child_turns
            ),
        )
    } else {
        (
            Outcome::Refused,
            not_obtained.to_owned(),
            format!(
                "{marker} did not reach the run's transcript; {} child provider turn(s) arrived",
                run.child_turns
            ),
        )
    }
}

/// Drive one entry on one transport and package the verdict with its evidence.
fn live_probe(entry: &CorpusEntry, transport: LiveTransport) -> ProbeResult {
    if !transport.available_here() {
        return ProbeResult::new(
            Outcome::Unavailable,
            "not observed on this platform",
            transport.unavailable_reason(),
        )
        .with_live(LiveEvidence {
            invocation: "wayland-core  (bare, on a PTY) — DECLARED UNAVAILABLE on this platform"
                .to_owned(),
            asserted_mode: transport.label().to_owned(),
            observable: transport.unavailable_reason().to_owned(),
        });
    }

    let world = LiveWorld::build();
    let run = run_live(entry.dimension, transport, &world);

    let Some(asserted_mode) = run.asserted_mode.clone() else {
        // The run did not prove which mode it landed in, so no verdict from it
        // may be recorded. A piped subprocess that fell through from the TUI to
        // the line REPL would otherwise report a verdict for a surface it never
        // exercised.
        return ProbeResult::new(
            Outcome::NotExpressible,
            "no verdict — the run did not prove the mode it landed in",
            format!(
                "the {} run produced no mode proof; transcript head: {}",
                transport.label(),
                head(&run.transcript)
            ),
        )
        .with_live(LiveEvidence {
            invocation: run.invocation,
            asserted_mode: format!("{}-UNPROVEN", transport.label()),
            observable: "no mode proof; the verdict was withheld".to_owned(),
        });
    };

    // THE ANTI-VACUITY GATE, and the single most important line in this file.
    //
    // Every probe below `observe` reads a negative: no probe file, no sentinel
    // in the transcript, no grandchild completion. A negative is only evidence
    // that a restriction held if the delegation actually happened. If the run
    // never got a delegated child as far as its own provider turn, the absence
    // means nothing was attempted — not that something was refused — and
    // recording REFUSED from it would be precisely the class the Phase 20A
    // audit found 283 times: a case that looks identical to a pass from a
    // distance and proves nothing.
    //
    // The parent's own turn is request 1. A delegated child talking to the same
    // configured endpoint is request 2. Fewer than two requests means no child
    // ever ran.
    //
    // The provider dimension is the exception: its whole probe IS the request
    // count, so it interprets the count itself rather than being gated on it.
    // The signal is that the DELEGATION was attempted and returned, proved by a
    // served request carrying a tool_result. A raw request count is not enough:
    // a parent that delegates, gets an error back and then takes two more turns
    // serves three requests without a child ever existing, which is exactly the
    // shape the first instrumented run produced.
    let attempted = run.delegation_attempted;
    if !attempted && entry.dimension != Dimension::Provider {
        return ProbeResult::new(
            Outcome::NotExpressible,
            "no verdict — no delegated child reached a provider turn in this run",
            format!(
                "the {} run served {} provider request(s) and none carried a tool_result, so the \
                 delegating tool call was never executed; an absent effect from this run would \
                 mean an attempt that never happened, not a refusal; full transcript at {}; {}",
                transport.label(),
                run.provider_requests,
                run.transcript_path,
                head(&run.transcript)
            ),
        )
        .with_live(LiveEvidence {
            invocation: run.invocation,
            asserted_mode,
            observable: format!(
                "{} provider request(s) served and no tool_result among them — the delegation \
                 was never executed, so the verdict was withheld; full transcript at {}",
                run.provider_requests, run.transcript_path
            ),
        });
    }

    let (outcome, obtained, observable) = observe(entry.dimension, &world, &run);
    let observable = format!(
        "{observable} (the run served {} provider request(s); the delegating tool call executed \
         and returned; {} delegated child provider turn(s) arrived); full transcript at {}",
        run.provider_requests, run.child_turns, run.transcript_path
    );
    ProbeResult::new(
        outcome,
        obtained,
        format!("{observable}; {}", head(&run.transcript)),
    )
    .with_live(LiveEvidence {
        invocation: run.invocation,
        asserted_mode,
        observable,
    })
}

fn head(text: &str) -> String {
    let flat: String = text.chars().filter(|c| *c != '\n' && *c != '\r').collect();
    let cut: String = flat.chars().take(3000).collect();
    format!("transcript head: {cut}")
}

// ===========================================================================
// Driver 3 — standalone, live
// ===========================================================================

pub struct StandaloneLive;

impl CorpusExecutor for StandaloneLive {
    fn surface(&self) -> Surface {
        Surface::Standalone
    }

    fn mode(&self) -> Mode {
        Mode::Live
    }

    fn probe(&self, entry: &CorpusEntry) -> ProbeResult {
        // The census names one standalone live surface per dimension. A
        // restriction a user sees enforced in the TUI is proved in the TUI, not
        // inferred from a headless run.
        let transport = match entry.standalone_live_mode {
            crate::cases::StandaloneLiveMode::Headless => LiveTransport::Headless,
            crate::cases::StandaloneLiveMode::Tui => LiveTransport::Tui,
        };
        live_probe(entry, transport)
    }
}

// ===========================================================================
// Driver 4 — host protocol, live
// ===========================================================================

pub struct HostProtocolLive;

impl CorpusExecutor for HostProtocolLive {
    fn surface(&self) -> Surface {
        Surface::HostProtocol
    }

    fn mode(&self) -> Mode {
        Mode::Live
    }

    fn probe(&self, entry: &CorpusEntry) -> ProbeResult {
        live_probe(entry, LiveTransport::JsonStream)
    }
}
