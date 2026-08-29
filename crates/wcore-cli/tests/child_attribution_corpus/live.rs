//! The LIVE attribution drivers — the ones that spawn the real `wayland-core`
//! binary and read attribution off the shipped surfaces.
//!
//! ## Why in-process evidence is required and never sufficient here
//!
//! Attribution is a USER-VISIBLE property. When a nested child asks for
//! approval a human sees a prompt and answers it; when a child is cancelled a
//! human sees which work stopped; when a child's result is delivered a human
//! sees it attached to the right turn. An in-process assertion that a
//! correlation id matches proves the plumbing. Only the shipped binary proves
//! the human sees the right thing, and this codebase has already shipped a
//! version where an entire permission crate passed its own tests while no
//! consumer called it.
//!
//! ## The observable, and why it is read rather than invented
//!
//! `crates/wcore-protocol/src/events.rs` carries `SubAgentEvent` and its
//! correlated form `CorrelatedSubAgentEvent`, both serialising under the wire
//! tag `sub_agent_event`, and `crates/wcore-protocol/src/contract/spec.rs` pins
//! `parent_call_id` on that wire type with a legacy compat fixture beside it.
//! `crates/wcore-agent/src/spawn_tool.rs:377` gives every task in one `Spawn`
//! call its OWN `parent_call_id` — `spawn:{index}:{agent_label}` — and relays
//! that child's events under it. Two sibling tasks therefore produce two
//! DISTINCT `parent_call_id` values on the real wire, which is precisely the
//! observable that makes a misattribution detectable: a sibling's output landing
//! under the other sibling's key has somewhere wrong to have landed.
//!
//! The gate is real and is asserted, not assumed: `sub_agent_event` is emitted
//! only when the sink was built `with_sub_agent_traces(true)`, and
//! `crates/wcore-cli/src/main.rs:4112` is the production call that does so for
//! `--json-stream`. A run that produces no `sub_agent_event` frame at all is
//! recorded NOT-OBSERVABLE, never CORRECT.
//!
//! ## Which machinery this reuses
//!
//! The same stack `crates/wcore-eval-scenarios/src/runner.rs` drives for the
//! host-protocol surface, and `crates/wcore-eval-scenarios/src/pty_capture.rs`
//! for the interactive one. As in 21-02's corpus the in-crate siblings under
//! `crates/wcore-cli/tests/support/` are used directly: `support/pty.rs` is the
//! same `portable-pty` + `vt100` keystroke-and-rendered-screen contract as
//! `pty_capture.rs`, and `pty::write_config` + `pty::harden_child_env` provide
//! the hermetic throwaway home that `crates/wcore-eval-scenarios/src/tempenv.rs`
//! exists to provide — `tempenv` itself writes a provider identity only and has
//! no seam for the mock-LLM `base_url` these probes require, so its GUARANTEE is
//! preserved (no live result depends on the operator's config or credentials)
//! while its code is not reused. `pty_capture.rs` carries `#![cfg(unix)]` at
//! line 63 because `portable_pty`'s Windows ConPTY backend does not surface the
//! spawned binary's stdout to the master end; `support/pty.rs` inherits that
//! gate, which is why the rendered-screen surface is DECLARED unavailable on
//! Windows rather than silently skipped.
//!
//! ## Mode assertion
//!
//! `crates/wcore-cli/src/main.rs` enters the TUI only when the prompt is empty
//! AND `--no-tui` is absent AND stdout is a terminal AND `--json-stream` is
//! absent. A piped subprocess fails the terminal check and silently falls
//! through to the line REPL, so a run intended to exercise one surface can
//! quietly exercise another and report a verdict for it. Every live invocation
//! below PROVES the mode it landed in before its verdict is recorded: json-stream
//! by the `ready` frame nothing else in the product emits, and the rendered
//! screen by the full-screen chrome only the TUI paints.

use std::collections::BTreeMap;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use serde_json::json;
use tempfile::TempDir;
use wiremock::{Mock, MockServer, ResponseTemplate, matchers};

use crate::cases::{AttributionCase, LifecycleEvent};
use crate::support::owned_tree::OwnedTree;
use crate::support::{mock_llm, pty, vault};

/// Goal markers, prefixed into each sibling's delegated prompt so the provider
/// mock can answer according to WHO is asking.
///
/// A single ordered script shared by parent and siblings is answered in queue
/// order regardless of requester, so a sentinel in a transcript would prove
/// nothing about which actor obtained it. 21-02's corpus learned this the
/// expensive way — its first instrumented run reported a widening that was the
/// harness talking to itself. Routing by requester is what makes every
/// observation below attributable, which in an ATTRIBUTION corpus is not an
/// implementation detail but the whole point.
const SIBLING_A_MARKER: &str = "CORPUSATTRSIBA";
const SIBLING_B_MARKER: &str = "CORPUSATTRSIBB";

/// Each sibling's own result text. These are what must land under that
/// sibling's `parent_call_id` and under no other.
const SIBLING_A_RESULT: &str = "CORPUSATTRRESULTALPHA";
const SIBLING_B_RESULT: &str = "CORPUSATTRRESULTBETA";

/// The two sibling task names, which the wire carries as `agent_name`.
const SIBLING_A_NAME: &str = "corpus-sib-alpha";
const SIBLING_B_NAME: &str = "corpus-sib-beta";

/// The parent's closing text. Its arrival at top level is the proof the Spawn
/// tool returned, which is what bounds a live run honestly.
const PARENT_DONE: &str = "CORPUSATTRPARENTDONE";

/// Ephemeral vault passphrase for the PTY child. Not a credential: it encrypts
/// a throwaway store inside a tempdir deleted when the run ends.
const CORPUS_VAULT_PASSPHRASE: &str = "child-attribution-corpus-ephemeral-vault";

/// Wall-clock budget for ONE live run, bounded so a hung run is killed and
/// recorded as producing no verdict rather than killing the whole case. This is
/// a bound on the harness, never a loosened gate: a run that exceeds it records
/// NOT-OBSERVABLE, and nothing is ever recorded CORRECT because it ran out of
/// time.
const LIVE_RUN_BUDGET: Duration = Duration::from_secs(20);

/// The shipped binary under test. Cargo guarantees `wayland-core` is built
/// before this integration test runs.
fn binary() -> &'static str {
    env!("CARGO_BIN_EXE_wayland-core")
}

fn runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("a current-thread tokio runtime for the live drivers")
}

// ===========================================================================
// Vocabulary
// ===========================================================================

/// The attribution verdict for one (case, surface) pair, in a closed set.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Attribution {
    /// The event landed on the actor that caused it, and on no other.
    Correct,
    /// The event landed on the wrong actor. This is a red.
    Misattributed,
    /// The surface does not expose enough to distinguish correct attribution
    /// from misattribution for this event. Recorded, never asserted weakly, and
    /// never repaired by adding a production observability hook.
    NotObservable,
    /// This surface is DECLARED unavailable on this platform. Never a silent
    /// skip and never substituted with a different surface.
    Unavailable,
}

impl Attribution {
    pub const fn label(self) -> &'static str {
        match self {
            Self::Correct => "CORRECT",
            Self::Misattributed => "MISATTRIBUTED",
            Self::NotObservable => "NOT-OBSERVABLE",
            Self::Unavailable => "UNAVAILABLE",
        }
    }

    /// Whether this verdict is a statement about attribution. Only decisive
    /// verdicts participate in the cross-mode comparison; comparing a verdict
    /// against "this surface could not answer" would manufacture a divergence
    /// that is really a coverage gap.
    pub const fn is_decisive(self) -> bool {
        matches!(self, Self::Correct | Self::Misattributed)
    }
}

/// The four things a live outcome must record. A live row missing any of them
/// is not evidence and the harness fails on it.
#[derive(Debug, Clone)]
pub struct LiveEvidence {
    /// The exact invocation: binary, flags and input.
    pub invocation: String,
    /// The mode the run PROVED it landed in, read back from the process rather
    /// than assumed from the flags.
    pub asserted_mode: String,
    /// The observation that distinguished correct attribution from a
    /// misattribution — or, when it could not, what was and was not seen.
    pub observable: String,
    /// Where this run's full raw transcript was written.
    pub transcript_path: String,
}

/// One live probe's answer.
#[derive(Debug, Clone)]
pub struct LiveOutcome {
    pub transport: LiveTransport,
    pub attribution: Attribution,
    pub evidence: LiveEvidence,
}

/// Which shipped transport a live run uses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LiveTransport {
    /// `wayland-core --json-stream` — the host-protocol surface.
    JsonStream,
    /// The bare binary on a real PTY — the interactive surface a human drives.
    Tui,
}

impl LiveTransport {
    pub const fn label(self) -> &'static str {
        match self {
            Self::JsonStream => "json-stream",
            Self::Tui => "tui",
        }
    }

    /// DECLARED, not discovered. See the module header.
    pub fn available_here(self) -> bool {
        !(self == Self::Tui && cfg!(windows))
    }

    pub const fn unavailable_reason(self) -> &'static str {
        "the interactive TUI is not drivable on Windows: portable_pty's ConPTY backend does not \
         surface the spawned binary's stdout to the master end, so pty_capture.rs is #![cfg(unix)] \
         and support/pty.rs inherits the gate"
    }
}

// ===========================================================================
// The hermetic world
// ===========================================================================

struct LiveWorld {
    home: TempDir,
}

impl LiveWorld {
    fn build() -> Self {
        Self {
            home: TempDir::new().expect("hermetic home"),
        }
    }

    fn root(&self) -> &Path {
        self.home.path()
    }
}

/// One scripted assistant turn, rendered through the same SSE builders the
/// shared mock uses, so every byte still passes the real provider parser.
enum Turn {
    Text(String),
    Tool(&'static str, serde_json::Value),
}

impl Turn {
    fn sse(&self) -> String {
        match self {
            Self::Text(text) => mock_llm::text_turn_sse(text),
            Self::Tool(name, input) => mock_llm::tool_use_turn_sse(name, input),
        }
    }
}

/// The parent's script: one `Spawn` call carrying TWO sibling tasks.
///
/// `Spawn` rather than `Delegate` is deliberate and load-bearing.
/// `crates/wcore-agent/src/bootstrap.rs:2281` wires `SpawnTool` with the
/// parent's `OutputSink`, and `spawn_tool.rs:377` gives each task its own
/// `parent_call_id`; `DelegateTool` has no such relay. Only the `Spawn` path
/// puts per-sibling attribution on the wire at all, so it is the path an
/// attribution corpus must drive.
fn parent_script(case: &AttributionCase) -> Vec<Turn> {
    // The two events a human answers or watches. For those, each sibling is
    // asked to perform a mutating action, which is what raises the gate a human
    // actually sees.
    let extra = matches!(
        case.event,
        LifecycleEvent::Approval | LifecycleEvent::Cancellation
    );
    let task = |marker: &str, name: &str| {
        let prompt = if extra {
            format!("{marker}: write the probe file, then report")
        } else {
            format!("{marker}: report your result")
        };
        json!({ "name": name, "prompt": prompt })
    };
    vec![
        Turn::Tool(
            "Spawn",
            json!({
                "tasks": [
                    task(SIBLING_A_MARKER, SIBLING_A_NAME),
                    task(SIBLING_B_MARKER, SIBLING_B_NAME),
                ]
            }),
        ),
        Turn::Text(PARENT_DONE.to_owned()),
    ]
}

/// Each sibling answers with its OWN result sentinel. If the two ever land
/// under one `parent_call_id`, or under each other's, the wire misattributed.
fn sibling_scripts(case: &AttributionCase) -> (Vec<Turn>, Vec<Turn>) {
    let probe = |sentinel: &str| -> Vec<Turn> {
        match case.event {
            LifecycleEvent::Approval | LifecycleEvent::Cancellation => vec![
                Turn::Tool(
                    "Write",
                    json!({
                        "file_path": format!("corpus_attr_{sentinel}.txt"),
                        "content": sentinel,
                    }),
                ),
                Turn::Text(sentinel.to_owned()),
            ],
            _ => vec![Turn::Text(sentinel.to_owned())],
        }
    };
    (probe(SIBLING_A_RESULT), probe(SIBLING_B_RESULT))
}

/// Start a provider mock that answers according to WHICH SIBLING is asking,
/// keyed on the first user message of the incoming conversation.
fn start_routed_mock(rt: &tokio::runtime::Runtime, case: &AttributionCase) -> MockServer {
    let server = rt.block_on(MockServer::start());
    let parent: Vec<String> = parent_script(case).iter().map(Turn::sse).collect();
    let (script_a, script_b) = sibling_scripts(case);
    let sibling_a: Vec<String> = script_a.iter().map(Turn::sse).collect();
    let sibling_b: Vec<String> = script_b.iter().map(Turn::sse).collect();
    let cursors = std::sync::Arc::new(std::sync::Mutex::new([0usize; 3]));

    let responder = move |request: &wiremock::Request| {
        let who = serde_json::from_slice::<serde_json::Value>(&request.body)
            .ok()
            .and_then(|body| {
                let first = body.get("messages")?.get(0)?.to_string();
                Some(if first.contains(SIBLING_B_MARKER) {
                    2usize
                } else if first.contains(SIBLING_A_MARKER) {
                    1
                } else {
                    0
                })
            })
            .unwrap_or(0);
        let script = match who {
            2 => &sibling_b,
            1 => &sibling_a,
            _ => &parent,
        };
        let mut cursor = cursors.lock().expect("cursor lock");
        let index = cursor[who].min(script.len().saturating_sub(1));
        cursor[who] = (cursor[who] + 1).min(script.len());
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

// ===========================================================================
// One live run
// ===========================================================================

/// Everything one live run produced.
struct LiveRun {
    invocation: String,
    /// `Some` only when the run PROVED which mode it landed in.
    asserted_mode: Option<String>,
    /// Raw bytes, for the persisted transcript.
    transcript: String,
    transcript_path: String,
    /// Sub-agent text, indexed by the `parent_call_id` it arrived under. THE
    /// attribution observable.
    by_parent_call_id: BTreeMap<String, String>,
    /// The `agent_name` seen for each `parent_call_id`.
    agent_names: BTreeMap<String, String>,
    /// Every top-level `approval_required` frame's `call_id`, in arrival order.
    approval_call_ids: Vec<String>,
    /// Whether any `sub_agent_event` frame arrived at all. When false, the wire
    /// carried no per-sibling attribution and every verdict from it is
    /// NOT-OBSERVABLE rather than CORRECT.
    saw_sub_agent_event: bool,
    /// The Spawn tool's own failure output, when the siblings died rather than
    /// ran. Carried so an absent observation is diagnosable as a failure rather
    /// than reported as a bare silence.
    sibling_failure: Option<String>,
    /// Provider requests the mock actually served, and how many came from each
    /// sibling. A negative observation only means something if the siblings ran.
    provider_requests: usize,
    sibling_a_turns: usize,
    sibling_b_turns: usize,
}

impl LiveRun {
    fn empty(invocation: String, transcript: String) -> Self {
        Self {
            invocation,
            asserted_mode: None,
            transcript,
            transcript_path: String::new(),
            by_parent_call_id: BTreeMap::new(),
            agent_names: BTreeMap::new(),
            approval_call_ids: Vec::new(),
            saw_sub_agent_event: false,
            sibling_failure: None,
            provider_requests: 0,
            sibling_a_turns: 0,
            sibling_b_turns: 0,
        }
    }

    /// The `parent_call_id` values whose relayed text carries `needle`.
    fn keys_carrying(&self, needle: &str) -> Vec<String> {
        self.by_parent_call_id
            .iter()
            .filter(|(_, text)| text.contains(needle))
            .map(|(key, _)| key.clone())
            .collect()
    }
}

fn persist_transcript(case: &AttributionCase, transport: LiveTransport, body: &str) -> String {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join("child-attribution-corpus")
        .join("transcripts");
    let _ = std::fs::create_dir_all(&dir);
    let path = dir.join(format!(
        "attribution_{}-{}.txt",
        case.event.case_id(),
        transport.label()
    ));
    let _ = std::fs::write(&path, body);
    path.to_string_lossy().into_owned()
}

fn write_live_config(home: &Path, base_url: &str) {
    pty::write_config(
        home,
        "anthropic",
        Some("claude-sonnet-4-20250514"),
        Some(base_url),
    );
}

fn run_live(case: &AttributionCase, transport: LiveTransport) -> LiveRun {
    let world = LiveWorld::build();
    let rt = runtime();
    let provider = start_routed_mock(&rt, case);
    write_live_config(world.root(), &provider.uri());

    let mut run = match transport {
        LiveTransport::JsonStream => run_json_stream(&world),
        LiveTransport::Tui => run_tui(&world),
    };

    let served = rt.block_on(mock_llm::received_requests(&provider));
    run.provider_requests = served.len();
    run.sibling_a_turns = served
        .iter()
        .filter(|request| first_message_contains(&request.body, SIBLING_A_MARKER))
        .count();
    run.sibling_b_turns = served
        .iter()
        .filter(|request| first_message_contains(&request.body, SIBLING_B_MARKER))
        .count();
    run.transcript_path = persist_transcript(
        case,
        transport,
        &format!(
            "invocation: {}\nasserted mode: {:?}\nprovider requests: {}\nsibling A turns: {}\n\
             sibling B turns: {}\nsub_agent_event seen: {}\nparent_call_ids: {:?}\n\n{}",
            run.invocation,
            run.asserted_mode,
            run.provider_requests,
            run.sibling_a_turns,
            run.sibling_b_turns,
            run.saw_sub_agent_event,
            run.by_parent_call_id.keys().collect::<Vec<_>>(),
            run.transcript
        ),
    );
    run
}

fn first_message_contains(body: &serde_json::Value, marker: &str) -> bool {
    body.get("messages")
        .and_then(|messages| messages.get(0))
        .map(|first| first.to_string().contains(marker))
        .unwrap_or(false)
}

/// `wayland-core --json-stream` — the host-protocol surface. The mode is proved
/// by the `ready` frame the protocol front-end emits at startup; nothing else in
/// the product emits it.
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
    // Without an ephemeral encrypted vault the binary refuses to start a session
    // under a hermetic WAYLAND_HOME, so the turn never reaches a provider and
    // every observation would be an absence rather than a verdict.
    let guard = vault::configure_process(&mut command);
    let mut child = OwnedTree::new({
        let spawned = command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn();
        drop(guard);

        match spawned {
            Ok(child) => child,
            Err(error) => {
                return LiveRun::empty(
                    invocation,
                    format!("the binary could not be spawned: {error}"),
                );
            }
        }
    });

    let mut stdin = child.stdin.take().expect("stdin");
    let stdout = child.stdout.take().expect("stdout");
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
        "{{\"type\":\"message\",\"msg_id\":\"1\",\"content\":\"spawn the two sibling tasks\"}}"
    );

    let (tx, rx) = mpsc::channel::<String>();
    std::thread::spawn(move || {
        for line in BufReader::new(stdout).lines().map_while(Result::ok) {
            if tx.send(line).is_err() {
                break;
            }
        }
    });

    let mut run = LiveRun::empty(invocation, String::new());
    let mut saw_ready = false;
    let mut saw_parent_done = false;
    let deadline = Instant::now() + LIVE_RUN_BUDGET;
    while Instant::now() < deadline {
        match rx.recv_timeout(Duration::from_millis(250)) {
            Ok(line) => {
                ingest_frame(
                    &mut run,
                    &line,
                    &mut saw_ready,
                    &mut saw_parent_done,
                    &mut stdin,
                );
                run.transcript.push_str(&line);
                run.transcript.push('\n');
                // MEASURED, and the correction matters: `stream_end` is emitted
                // per assistant STREAM, not per turn, so the parent's very
                // first response — the one carrying the Spawn tool call —
                // already ends with one. An earlier iteration broke on it and
                // killed the process while the two siblings were still talking
                // to the provider, then recorded their absence as if the
                // topology had never existed. The run now ends when the work
                // this corpus is watching is actually finished: both siblings'
                // results have arrived, or the parent has taken its closing
                // turn, which only happens after the Spawn tool returned.
                let both_results_in = !run.keys_carrying(SIBLING_A_RESULT).is_empty()
                    && !run.keys_carrying(SIBLING_B_RESULT).is_empty();
                if both_results_in || saw_parent_done {
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
        run.transcript.push_str("--- stderr ---\n");
        run.transcript.push_str(&buffer);
    }
    // The `ready` frame is emitted only by the json-stream front-end. Its
    // presence is the proof the run landed in host-protocol mode and did not
    // fall through to the line REPL.
    run.asserted_mode = saw_ready.then(|| LiveTransport::JsonStream.label().to_owned());
    run
}

/// Parse one wire frame and fold its attribution content into the run.
///
/// Frames are PARSED rather than pattern-matched on text so a field-order change
/// still resolves, and `sub_agent_event` is read by its wire tag — which both
/// the legacy `SubAgentEvent` and the correlated form serialise under, exactly
/// as `contract/spec.rs` pins it.
fn ingest_frame(
    run: &mut LiveRun,
    line: &str,
    saw_ready: &mut bool,
    saw_parent_done: &mut bool,
    stdin: &mut std::process::ChildStdin,
) {
    let Ok(frame) = serde_json::from_str::<serde_json::Value>(line) else {
        return;
    };
    let kind = frame.get("type").and_then(serde_json::Value::as_str);
    match kind {
        Some("ready") => *saw_ready = true,
        // The parent's CLOSING text, emitted at top level rather than wrapped
        // in a sub_agent_event. Reaching it means the Spawn tool returned and
        // both siblings are done.
        Some("text_delta")
            if frame
                .get("text")
                .and_then(serde_json::Value::as_str)
                .is_some_and(|text| text.contains(PARENT_DONE)) =>
        {
            *saw_parent_done = true;
        }
        // The Spawn tool's own result. When the siblings died rather than ran,
        // this is where the product says why, and an attribution row that
        // recorded only their absence would turn a diagnosable live failure
        // into an unexplained silence.
        Some("tool_result") => {
            let failed = frame.get("status").and_then(serde_json::Value::as_str) == Some("error");
            if failed && let Some(output) = frame.get("output").and_then(serde_json::Value::as_str)
            {
                run.sibling_failure = Some(output.chars().take(400).collect());
            }
        }
        Some("sub_agent_event") => {
            let Some(parent_call_id) = frame
                .get("parent_call_id")
                .and_then(serde_json::Value::as_str)
            else {
                return;
            };
            run.saw_sub_agent_event = true;
            if let Some(agent_name) = frame.get("agent_name").and_then(serde_json::Value::as_str) {
                run.agent_names
                    .entry(parent_call_id.to_owned())
                    .or_insert_with(|| agent_name.to_owned());
            }
            let inner = frame
                .get("inner")
                .map(std::string::ToString::to_string)
                .unwrap_or_default();
            let bucket = run
                .by_parent_call_id
                .entry(parent_call_id.to_owned())
                .or_default();
            bucket.push_str(&inner);
            bucket.push('\n');
        }
        Some("approval_required") => {
            if let Some(call_id) = frame.get("call_id").and_then(serde_json::Value::as_str) {
                run.approval_call_ids.push(call_id.to_owned());
                // Answer the gate the way a real host does. A driver that never
                // answers leaves the delegation parked forever and every
                // downstream observation becomes an absence — inconclusive in
                // the direction that looks like correctness.
                let _ = writeln!(
                    stdin,
                    "{{\"type\":\"tool_approve\",\"call_id\":\"{call_id}\"}}"
                );
            }
        }
        _ => {}
    }
}

/// The BARE binary on a real PTY — the surface a human actually answers an
/// approval on and actually watches work stop on. The mode is proved by the
/// rendered chrome: only the full-screen TUI paints the wordmark and the
/// Workspace tab, so a run that fell through to the line REPL cannot show them.
///
/// The `cfg` pair is the DECLARED platform limitation made compilable, not a
/// surface hidden from Windows: `support/pty.rs`'s `Pty` is itself
/// `#[cfg(unix)]`, inheriting `pty_capture.rs`'s ConPTY gate, so the Windows arm
/// must exist for the crate to build there. `available_here()` already reports
/// the surface UNAVAILABLE on Windows, the Windows arm never proves a mode, and
/// no verdict can be taken from it.
#[cfg(unix)]
fn run_tui(world: &LiveWorld) -> LiveRun {
    let invocation = format!(
        "wayland-core  (bare, attached to a real PTY; WAYLAND_HOME={})",
        world.root().display()
    );
    let mut terminal = pty::Pty::spawn_with_env(
        world.root(),
        40,
        140,
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
        terminal.send(b"spawn the two sibling tasks\r");
        std::thread::sleep(Duration::from_secs(10));
    }
    let screen = terminal.screen_text();
    terminal.quit();

    let mut run = LiveRun::empty(invocation, screen);
    run.asserted_mode = booted.then(|| LiveTransport::Tui.label().to_owned());
    run
}

#[cfg(not(unix))]
fn run_tui(world: &LiveWorld) -> LiveRun {
    LiveRun::empty(
        format!(
            "wayland-core  (bare, on a PTY) — DECLARED UNAVAILABLE; WAYLAND_HOME={}",
            world.root().display()
        ),
        format!(
            "{}; the ephemeral vault passphrase this surface supplies elsewhere ({} bytes) is \
             never reached on this platform",
            LiveTransport::Tui.unavailable_reason(),
            CORPUS_VAULT_PASSPHRASE.len()
        ),
    )
}

// ===========================================================================
// Turning one run into an attribution verdict
// ===========================================================================

/// Read the wire's per-sibling attribution.
///
/// The question every event asks of this surface is the same: did each
/// sibling's own observable land under that sibling's `parent_call_id`, and
/// under no other? The events differ only in whether the surface carries an
/// observable for them at all — and where it does not, the answer is
/// NOT-OBSERVABLE with what was and was not seen, never CORRECT.
fn observe_wire(case: &AttributionCase, run: &LiveRun) -> (Attribution, String) {
    let context = format!(
        "the run served {} provider request(s) ({} from sibling A, {} from sibling B) and \
         produced {} distinct parent_call_id(s) {:?} with agent names {:?}",
        run.provider_requests,
        run.sibling_a_turns,
        run.sibling_b_turns,
        run.by_parent_call_id.len(),
        run.by_parent_call_id.keys().collect::<Vec<_>>(),
        run.agent_names.values().collect::<Vec<_>>()
    );

    if !run.saw_sub_agent_event {
        return (
            Attribution::NotObservable,
            format!(
                "no sub_agent_event frame reached the wire, so this run carried no per-sibling \
                 attribution to read at all; {context}"
            ),
        );
    }

    // The universal precondition: two siblings must have produced two DISTINCT
    // keys. One key for two siblings is a misattribution by collapse, and it is
    // exactly the failure a single-child corpus could never see.
    let a_keys = run.keys_carrying(SIBLING_A_RESULT);
    let b_keys = run.keys_carrying(SIBLING_B_RESULT);

    match case.event {
        // Result delivery is the one event this surface carries directly: each
        // sibling's own result text arrives under its own parent_call_id.
        LifecycleEvent::Delivery => {
            if a_keys.is_empty() || b_keys.is_empty() {
                return (
                    Attribution::NotObservable,
                    format!(
                        "only one sibling's result reached the wire (A under {a_keys:?}, B under \
                         {b_keys:?}), so there was no second place a misattribution could have \
                         landed and no verdict may be taken; {context}"
                    ),
                );
            }
            if a_keys.len() > 1 || b_keys.len() > 1 || a_keys == b_keys {
                return (
                    Attribution::Misattributed,
                    format!(
                        "a sibling's result reached more than one parent_call_id, or both \
                         siblings' results reached the same one: A under {a_keys:?}, B under \
                         {b_keys:?}; {context}"
                    ),
                );
            }
            (
                Attribution::Correct,
                format!(
                    "sibling A's result reached only {a_keys:?} and sibling B's only {b_keys:?}, \
                     two distinct parent_call_ids, so neither result landed on the other \
                     sibling's key; {context}"
                ),
            )
        }
        // Approval: the wire's `approval_required` frame carries `call_id` and
        // `correlation_id` and NO sibling identity, and a sub-agent's tool calls
        // are not relayed (`ChannelSink::emit_tool_call` is a deliberate no-op),
        // so whether a raised approval belongs to sibling A or sibling B is
        // read out of the frames themselves rather than assumed.
        LifecycleEvent::Approval => {
            let attributable = run
                .approval_call_ids
                .iter()
                .filter(|call_id| {
                    run.by_parent_call_id
                        .keys()
                        .any(|key| call_id.contains(key.as_str()))
                })
                .count();
            if run.approval_call_ids.is_empty() {
                return (
                    Attribution::NotObservable,
                    format!(
                        "no approval_required frame reached the wire during a run in which both \
                         siblings were asked to mutate, so there was no approval to attribute; \
                         {context}"
                    ),
                );
            }
            if attributable == 0 {
                return (
                    Attribution::NotObservable,
                    format!(
                        "{} approval_required frame(s) reached the wire ({:?}) and none carries \
                         any field tying it to the sibling that raised it, so correct attribution \
                         cannot be distinguished from misattribution on this surface; {context}",
                        run.approval_call_ids.len(),
                        run.approval_call_ids
                    ),
                );
            }
            (
                Attribution::Correct,
                format!(
                    "{attributable} of {} approval_required frame(s) name the sibling that \
                     raised them ({:?}); {context}",
                    run.approval_call_ids.len(),
                    run.approval_call_ids
                ),
            )
        }
        // Cancellation, reservation, refund and escalation: the host protocol
        // carries no per-child command or per-child counter for any of them.
        // `ProtocolCommand` has `Stop` (whole-turn) and no per-child variant;
        // `BudgetExceeded` carries reason/observed/limit and no actor; and
        // `ChannelSink` relays only text, thinking, stream lifecycle, error and
        // info. What the surface CAN show is that the two siblings remained
        // separable at all, which is recorded — but it is not the event's own
        // attribution and is not reported as if it were.
        LifecycleEvent::Cancellation
        | LifecycleEvent::Reservation
        | LifecycleEvent::Refund
        | LifecycleEvent::Escalation => {
            let separable = a_keys.len() == 1 && b_keys.len() == 1 && a_keys != b_keys;
            (
                Attribution::NotObservable,
                format!(
                    "the host protocol exposes no per-child observable for {}: ProtocolCommand \
                     carries only a whole-turn Stop, BudgetExceeded carries reason/observed/limit \
                     and no actor, and ChannelSink relays only text, thinking, stream lifecycle, \
                     error and info. What this run DID show is that the two siblings stayed \
                     separable on the wire: {separable}. That is not this event's attribution and \
                     is not recorded as if it were; {context}",
                    case.event.requirement_name()
                ),
            )
        }
    }
}

/// Read the rendered screen's per-sibling attribution — the surface a human
/// actually uses for approval and cancellation.
fn observe_screen(case: &AttributionCase, run: &LiveRun) -> (Attribution, String) {
    let screen = &run.transcript;
    let names_a = screen.contains(SIBLING_A_NAME);
    let names_b = screen.contains(SIBLING_B_NAME);
    let context = format!(
        "the rendered screen names sibling A: {names_a}, sibling B: {names_b}; the run served {} \
         provider request(s) ({} from sibling A, {} from sibling B)",
        run.provider_requests, run.sibling_a_turns, run.sibling_b_turns
    );
    if !names_a && !names_b {
        return (
            Attribution::NotObservable,
            format!(
                "the rendered screen names neither sibling, so a human looking at it could not \
                 tell which nested actor a {} belongs to; {context}",
                case.event.requirement_name()
            ),
        );
    }
    if names_a != names_b {
        return (
            Attribution::Misattributed,
            format!(
                "the rendered screen names exactly one of two running siblings, so a human \
                 answering or watching cannot tell the two apart and the unnamed sibling's \
                 activity is presented as if it did not exist; {context}"
            ),
        );
    }
    (
        Attribution::Correct,
        format!(
            "the rendered screen names both siblings distinctly, so a human can tell which nested \
             actor a {} belongs to; {context}",
            case.event.requirement_name()
        ),
    )
}

/// Drive one case on one live transport and package the verdict with its
/// evidence.
pub fn live_probe(case: &AttributionCase, transport: LiveTransport) -> LiveOutcome {
    if !transport.available_here() {
        return LiveOutcome {
            transport,
            attribution: Attribution::Unavailable,
            evidence: LiveEvidence {
                invocation: "wayland-core  (bare, on a PTY) — DECLARED UNAVAILABLE on this \
                             platform"
                    .to_owned(),
                asserted_mode: transport.label().to_owned(),
                observable: transport.unavailable_reason().to_owned(),
                transcript_path: String::new(),
            },
        };
    }

    let run = run_live(case, transport);

    let Some(asserted_mode) = run.asserted_mode.clone() else {
        // No verdict may be recorded from a run that never proved which mode it
        // landed in. A piped subprocess that fell through from the TUI to the
        // line REPL would otherwise report a verdict for a surface it never
        // exercised.
        return LiveOutcome {
            transport,
            attribution: Attribution::NotObservable,
            evidence: LiveEvidence {
                invocation: run.invocation.clone(),
                asserted_mode: format!("{}-UNPROVEN", transport.label()),
                observable: format!(
                    "the {} run produced no mode proof, so its verdict was withheld; full \
                     transcript at {}",
                    transport.label(),
                    run.transcript_path
                ),
                transcript_path: run.transcript_path,
            },
        };
    };

    // THE ANTI-VACUITY GATE. Every observation below reads whether an event
    // landed on the right actor. If the two siblings never ran, "it did not land
    // on the wrong sibling" means nothing was attempted rather than that
    // attribution held — the failure class the Phase 20A audit found 283 times,
    // which looks identical to a pass from a distance.
    let both_siblings_ran = run.sibling_a_turns > 0 && run.sibling_b_turns > 0;
    if !both_siblings_ran {
        return LiveOutcome {
            transport,
            attribution: Attribution::NotObservable,
            evidence: LiveEvidence {
                invocation: run.invocation.clone(),
                asserted_mode,
                observable: format!(
                    "sibling A took {} provider turn(s) and sibling B took {}, so the two-sibling \
                     topology this case requires never existed in the run and no attribution \
                     verdict may be taken from it; {} provider request(s) were served; the Spawn \
                     tool reported: {}; full transcript at {}",
                    run.sibling_a_turns,
                    run.sibling_b_turns,
                    run.provider_requests,
                    run.sibling_failure
                        .clone()
                        .unwrap_or_else(|| "no tool failure was reported".to_owned()),
                    run.transcript_path
                ),
                transcript_path: run.transcript_path,
            },
        };
    }

    let (attribution, observable) = match transport {
        LiveTransport::JsonStream => observe_wire(case, &run),
        LiveTransport::Tui => observe_screen(case, &run),
    };

    LiveOutcome {
        transport,
        attribution,
        evidence: LiveEvidence {
            invocation: run.invocation.clone(),
            asserted_mode,
            observable: format!("{observable}; full transcript at {}", run.transcript_path),
            transcript_path: run.transcript_path,
        },
    }
}

/// Which live transports a case is driven on. Every case runs on the
/// host-protocol wire; the two events a human answers or watches also run on the
/// rendered screen, because a wire-level assertion does not prove the person saw
/// the right thing.
pub fn transports_for(case: &AttributionCase) -> Vec<LiveTransport> {
    match case.human_visible_surface {
        crate::cases::HumanVisibleSurface::Wire => vec![LiveTransport::JsonStream],
        crate::cases::HumanVisibleSurface::RenderedScreen => {
            vec![LiveTransport::JsonStream, LiveTransport::Tui]
        }
    }
}
