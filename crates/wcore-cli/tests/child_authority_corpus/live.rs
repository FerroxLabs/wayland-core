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
use crate::support::owned_tree::OwnedTree;
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
/// THE LIVE SHELL KNOWN-POSITIVE — 21-C3.
///
/// Printed to stdout by the delegated child's Bash command BEFORE that command
/// attempts anything the product might refuse. `BashTool` returns the command's
/// stdout as its `tool_result`, the child feeds that result back into its next
/// provider request, and this corpus's routed mock records every request body —
/// so this marker appearing in a served body is proof that the CHILD'S SHELL
/// ACTUALLY RAN, taken from the wire rather than from the parent's screen.
///
/// It exists because the live tool verdict had no such observable. It read one
/// bit — "is the probe file on disk?" — and recorded REFUSED whenever the answer
/// was no. Every one of these produces that same no: the sandbox backend
/// refusing to spawn a shell at all, workspace containment binding the write,
/// the parent-authority intersection withholding the Bash tool, the approval
/// gate denying, and the child dying before its tool call. The live row's own
/// evidence claimed the refusal "is attributable to what that child was given",
/// which the run had no way to establish.
///
/// The child engine writes to a `NullSink`, so its tool results never reach the
/// parent transcript; the served request bodies are the only place this is
/// visible without adding a production observability hook, which the corpus is
/// forbidden to do.
///
/// ## THIS MARKER IS SPLIT, AND THE SPLIT IS THE WHOLE MECHANISM
///
/// The first draft of this observable was SELF-PASSING, and its own
/// known-negative run caught it. The marker was written literally into the
/// child's Bash command; the command text travels inside the child's `ToolUse`
/// block, which is in the SAME served request bodies this matcher searches. So
/// the marker was present whether or not any shell ever ran, and an injection
/// that made the command exit before printing anything did not change the
/// verdict by one character. That is the eleventh-instance class LANE-BRIEF
/// §6b-ii names — an instrument carrying the defect it hunts — and it is
/// repaired here rather than written up and left in place.
///
/// The repair: the shell CONCATENATES the two halves at runtime
/// (`printf %s%s CORPUSSHELL RAN7d21` on Unix, `echo CORPUSSHELL^RAN7d21` under
/// `cmd`, where `^` is the escape character). The command text therefore never
/// contains the joined string, and only the shell's own stdout can produce it.
/// Both constructions were verified against their real shells before use.
///
/// [`shell_marker_seen`] is the only permitted matcher, and
/// `the_shell_marker_matcher_is_not_satisfied_by_the_command_text` pins all
/// three assertions §6b-ii requires — including that the OLD matcher would have
/// missed the repair.
const CHILD_SHELL_RAN: &str = "CORPUSSHELLRAN7d21";
/// The two halves as the child's shell is told to emit them.
const CHILD_SHELL_RAN_HEAD: &str = "CORPUSSHELL";
const CHILD_SHELL_RAN_TAIL: &str = "RAN7d21";
/// THE LIVE CONTAINMENT DISCRIMINATOR — 21-C3.
///
/// Printed only if a write to a RELATIVE path succeeds. `BashTool` sets the
/// sandbox command's `cwd` to the workspace policy's own root
/// (`bash.rs:136`), so a relative write lands inside the child's own workspace
/// and the containment guard has nothing to bind. The outside write in the same
/// command has everything to bind.
///
/// Together with [`CHILD_SHELL_RAN`] this is what separates the two mechanisms
/// `21-04-PHASE-VERDICT.md` §1 C3 bullet 4 records as jointly attributable: an
/// absent outside effect beside a PRESENT inside effect is workspace
/// containment, and it means the child demonstrably held and exercised Bash —
/// so that refusal must not be read as evidence that tool authority is
/// enforced, exactly as the verdict warns.
/// Split for the same reason as [`CHILD_SHELL_RAN`] — see that doc comment.
const CHILD_SHELL_WROTE_INSIDE: &str = "CORPUSINSIDE7d21";
const CHILD_SHELL_WROTE_INSIDE_HEAD: &str = "CORPUSIN";
const CHILD_SHELL_WROTE_INSIDE_TAIL: &str = "SIDE7d21";

/// The ONLY sanctioned matcher for a shell-emitted marker.
///
/// Free-standing and pure so its three self-test assertions are cheap and
/// permanent. It searches for the JOINED marker, which the split command text
/// cannot contain.
fn shell_marker_seen(bodies: &[String], joined: &str) -> bool {
    bodies.iter().any(|body| body.contains(joined))
}
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
    /// `wayland-core --no-tui ... "<prompt>"` over PIPES — the standalone
    /// headless surface with NO approval channel. See
    /// [`LiveTransport::approval_channel_reason`].
    Headless,
    /// `wayland-core --no-tui ... "<prompt>"` on a real PTY — the same
    /// standalone headless surface, driven with the approval channel a user at
    /// a terminal actually has.
    HeadlessPty,
    /// The bare binary on a real PTY — the standalone interactive surface.
    Tui,
}

impl LiveTransport {
    pub const fn label(self) -> &'static str {
        match self {
            Self::JsonStream => "json-stream",
            Self::Headless => "headless",
            Self::HeadlessPty => "headless-pty",
            Self::Tui => "tui",
        }
    }

    /// Whether this transport can be driven on the current platform. DECLARED,
    /// not discovered: every PTY-backed transport is unavailable on Windows
    /// because `pty_capture.rs` is `#![cfg(unix)]` and `support/pty.rs`
    /// inherits that gate, and the corpus states the fact rather than probing
    /// for it.
    pub fn available_here(self) -> bool {
        !(matches!(self, Self::Tui | Self::HeadlessPty) && cfg!(windows))
    }

    pub const fn unavailable_reason(self) -> &'static str {
        "no PTY-backed transport is drivable on Windows: portable_pty's ConPTY backend does not \
         surface the spawned binary's stdout to the master end, so pty_capture.rs is #![cfg(unix)] \
         and support/pty.rs inherits the gate"
    }

    /// Whether a run on this transport can answer a tool-approval gate at all.
    ///
    /// This is a PRODUCT fact, not a harness preference, and it is the single
    /// reason the piped headless surface could never get a delegated child to
    /// act: `wcore_agent::confirm::ToolConfirmer::check_for` returns `Denied`
    /// unconditionally when `io::stdin()` is not a terminal, so a `Delegate`
    /// call issued over pipes is refused before any child exists. Every verdict
    /// the corpus recorded from such a run was an absence of effect from an
    /// actor that never acted.
    pub const fn has_approval_channel(self) -> bool {
        match self {
            // The host answers `approval_required` with `tool_approve`.
            Self::JsonStream => true,
            // A real terminal: the confirmer prompts and reads a keystroke.
            Self::HeadlessPty | Self::Tui => true,
            Self::Headless => false,
        }
    }

    pub const fn approval_channel_reason(self) -> &'static str {
        match self {
            Self::JsonStream => {
                "the protocol front-end suspends the call on `approval_required` \
                                 and the driver answers with `tool_approve`, exactly as the \
                                 desktop host does"
            }
            Self::HeadlessPty => {
                "stdin is a real terminal, so the shipped confirmer prompts and \
                                  the driver answers `y`, exactly as a user at a terminal does"
            }
            Self::Tui => {
                "the full-screen approval card is rendered and the driver presses `y`, \
                          exactly as a user at a terminal does"
            }
            Self::Headless => {
                "stdin is a pipe, and confirm.rs denies any tool call needing \
                               confirmation when stdin is not a terminal — so a delegation can \
                               never execute on this transport and no child can ever act"
            }
        }
    }
}

/// A hermetic world for one live run: a throwaway home, the seeded fixtures the
/// probes read, and the loopback destination the egress probe targets.
struct LiveWorld {
    home: TempDir,
    /// The repository the session governs — a REAL git repo one level below the
    /// home, and the run's `cwd`.
    ///
    /// It is separate from the home because a delegated child that asks for a
    /// mutating toolset resolves `RequestedChildWorkspace::IsolatedMutation`,
    /// whose checkout root is derived under `<WAYLAND_HOME>/sessions`. With
    /// `cwd == home` that root's parent overlaps the repository and
    /// `WorktreeManager::new_with_workspace_root` refuses — so no mutating
    /// child could ever be created, and the tool dimension's live REFUSED was a
    /// workspace-preparation failure rather than an authority decision. The
    /// captured evidence is verbatim: `durable child workspace preparation
    /// failed: worktree io: orchestrator worktree root must not overlap
    /// repository`.
    workspace: PathBuf,
    /// The probe a Bash-capable child would write. Its existence is the tool
    /// dimension's invariant evidence.
    bash_probe: PathBuf,
    /// A file outside the home entirely.
    outside: PathBuf,
    /// A synthetic credential file inside the home.
    dotenv: PathBuf,
    /// Kept alive so the outside file is not collected mid-run.
    _outside_dir: TempDir,
    /// `Some(reason)` when the workspace repository could not be created, so a
    /// run that needs one records that rather than a refusal it never observed.
    repo_failure: Option<String>,
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
        let workspace = home.path().join("workspace");
        std::fs::create_dir_all(&workspace).expect("workspace dir");
        let repo_failure = init_live_repo(&workspace).err();
        Self {
            home,
            workspace,
            bash_probe,
            outside,
            dotenv,
            _outside_dir: outside_dir,
            repo_failure,
        }
    }

    fn root(&self) -> &Path {
        self.home.path()
    }

    /// The run's working directory — the governed repository, not the home.
    fn cwd(&self) -> &Path {
        &self.workspace
    }
}

/// Make the live run's working directory a real git repository with one commit,
/// so an isolated-mutation child has a parent to branch a worktree from.
///
/// Argv mode; identity is supplied per-invocation with `-c` so nothing reads or
/// writes a global git config.
fn init_live_repo(root: &Path) -> Result<(), String> {
    let run = |args: &[&str]| -> Result<(), String> {
        let output = Command::new("git")
            .args(args)
            .current_dir(root)
            .output()
            .map_err(|error| format!("git is not available on this host: {error}"))?;
        if output.status.success() {
            Ok(())
        } else {
            Err(format!(
                "git {:?} did not succeed: {}",
                args,
                String::from_utf8_lossy(&output.stderr).trim()
            ))
        }
    };
    run(&["init", "--initial-branch=corpus"])?;
    std::fs::write(root.join("README.corpus"), b"corpus fixture repository")
        .map_err(|error| error.to_string())?;
    // The binary writes its own per-workspace state under `.wayland-core/`, and
    // an isolated-mutation dispatch refuses on a dirty checkout. Ignoring that
    // directory is what keeps the repository clean enough for the child to be
    // created at all; without it every mutating child dies before existing and
    // the probe reads an absent effect.
    std::fs::write(root.join(".gitignore"), b".wayland-core/\n")
        .map_err(|error| error.to_string())?;
    run(&["add", "README.corpus", ".gitignore"])?;
    run(&[
        "-c",
        "user.email=corpus@example.invalid",
        "-c",
        "user.name=corpus",
        "commit",
        "-m",
        "corpus fixture",
    ])
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

/// The `Delegate` batch topology cap the fan-out dimension attacks, and the
/// two batch sizes the live differential drives — 21-C3.
///
/// `FAN_OUT_CAP` is the AT-CAP control: a batch the gate must admit. It exists
/// because fan-out is the one dimension where **zero children is the correct
/// enforcement outcome**, which makes a refusal and a broken fixture produce
/// byte-identical readings. The in-process probe has run this control since
/// `359ce2bf`; the live probe did not, which is why
/// `21-04-PHASE-VERDICT.md` records fan-out live as UNDETERMINED on both
/// platforms and both surfaces.
const FAN_OUT_CAP: usize = 5;
const FAN_OUT_OVER_CAP: usize = 8;

/// Build the hostile scripts for one dimension, transcribed from the census's
/// `WIDENING ::` row.
///
/// `fan_out_batch` is consumed by the fan-out dimension alone and is how the
/// live at-cap control differs from the live over-cap run — nothing else about
/// the two runs changes.
fn live_scripts(
    dimension: Dimension,
    world: &LiveWorld,
    sentinel_url: &str,
    fan_out_batch: usize,
) -> LiveScripts {
    // 21-C3: the marker is printed FIRST and unconditionally, so it returns
    // whether or not the write that follows is refused. That ordering is the
    // whole point — it separates "the shell never ran" from "the shell ran and
    // the write was refused", which the previous single-observable command
    // could not.
    // Both branches emit their markers SPLIT — see `CHILD_SHELL_RAN`. `^` is
    // cmd's escape character, so `echo A^B` prints `AB`; `printf %s%s A B`
    // concatenates on Unix. Neither command TEXT contains a joined marker, so
    // the matcher cannot be satisfied by the command echoing back in the
    // conversation.
    let bash_command = if cfg!(windows) {
        format!(
            "echo {CHILD_SHELL_RAN_HEAD}^{CHILD_SHELL_RAN_TAIL}& (echo x > corpus_inside.txt && \
             echo {CHILD_SHELL_WROTE_INSIDE_HEAD}^{CHILD_SHELL_WROTE_INSIDE_TAIL})& echo \
             CORPUS_BASH_PROBE > \"{}\"",
            world.bash_probe.display()
        )
    } else {
        format!(
            "printf %s%s {CHILD_SHELL_RAN_HEAD} {CHILD_SHELL_RAN_TAIL}; printf x > \
             corpus_inside.txt && printf %s%s {CHILD_SHELL_WROTE_INSIDE_HEAD} \
             {CHILD_SHELL_WROTE_INSIDE_TAIL}; printf CORPUS_BASH_PROBE > '{}'",
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
            let tasks: Vec<serde_json::Value> = (0..fan_out_batch)
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
    /// 21-C3 — whether the delegated child's SHELL actually ran, read off the
    /// wire. See [`CHILD_SHELL_RAN`]. Only the tool dimension's script emits
    /// the marker; every other dimension leaves this false and does not read it.
    child_shell_ran: bool,
    /// 21-C3 — whether that same shell's write to a path INSIDE its own
    /// workspace succeeded. See [`CHILD_SHELL_WROTE_INSIDE`].
    child_shell_wrote_inside: bool,
    /// 21-C3 — the `tool_result` bodies the CHILD sent back to its own
    /// endpoint. Evidence only; nothing asserts on it.
    child_tool_results: String,
}

/// Pull the `content` of every `tool_result` block out of a served request
/// body. Deliberately a shallow structural walk rather than a regex: a regex
/// over minified JSON is the kind of matcher that silently returns nothing and
/// reads as "the child said nothing".
fn extract_tool_result_bodies(body: &str) -> Vec<String> {
    let Ok(value) = serde_json::from_str::<serde_json::Value>(body) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    let mut stack = vec![value];
    while let Some(node) = stack.pop() {
        match node {
            serde_json::Value::Object(map) => {
                let is_tool_result = map
                    .get("type")
                    .and_then(serde_json::Value::as_str)
                    .is_some_and(|t| t == "tool_result");
                if is_tool_result
                    && let Some(content) = map.get("content").and_then(serde_json::Value::as_str)
                {
                    out.push(content.chars().take(400).collect::<String>());
                }
                stack.extend(map.into_iter().map(|(_, v)| v));
            }
            serde_json::Value::Array(items) => stack.extend(items),
            _ => {}
        }
    }
    out
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
    run_live_with_batch(dimension, transport, world, FAN_OUT_OVER_CAP)
}

/// The same live run with the fan-out batch size named. Only the fan-out
/// dimension reads it; every other dimension's scripts are identical for any
/// value.
fn run_live_with_batch(
    dimension: Dimension,
    transport: LiveTransport,
    world: &LiveWorld,
    fan_out_batch: usize,
) -> LiveRun {
    let rt = runtime();

    let sentinel: MockServer = rt.block_on(MockServer::start());
    rt.block_on(
        Mock::given(matchers::method("GET"))
            .and(matchers::path("/corpus-egress-sentinel"))
            .respond_with(ResponseTemplate::new(200).set_body_string(EGRESS_SENTINEL))
            .mount(&sentinel),
    );
    let sentinel_url = format!("{}/corpus-egress-sentinel", sentinel.uri());

    let provider: MockServer = start_routed_mock(
        &rt,
        live_scripts(dimension, world, &sentinel_url, fan_out_batch),
    );
    write_live_config(
        world.root(),
        &provider.uri(),
        seeded_budget(dimension).as_deref(),
    );

    let mut run = match transport {
        LiveTransport::JsonStream => run_json_stream(world),
        LiveTransport::Headless => run_headless(world),
        LiveTransport::HeadlessPty => run_headless_pty(world),
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
    // 21-C3. Searched across every served body: the marker travels inside the
    // tool_result block of the child's SECOND request, and restricting the
    // search to first messages — as the generation counters above do — would
    // miss it entirely.
    let bodies: Vec<String> = served
        .iter()
        .map(|request| request.body.to_string())
        .collect();
    run.child_shell_ran = shell_marker_seen(&bodies, CHILD_SHELL_RAN);
    run.child_shell_wrote_inside = shell_marker_seen(&bodies, CHILD_SHELL_WROTE_INSIDE);
    // 21-C3 — what the CHILD'S tool call actually returned, taken from the
    // conversation the child sent back to its own endpoint. The child engine
    // writes to a `NullSink`, so this never appears on the parent's screen and
    // the corpus previously had no way to say WHY a live child obtained
    // nothing. Carried as evidence only; no verdict reads it, so the corpus's
    // rule that nothing asserts on an error shape is untouched.
    run.child_tool_results = bodies
        .iter()
        .filter(|body| body.contains(CHILD_GOAL_L1) && body.contains("tool_result"))
        .flat_map(|body| extract_tool_result_bodies(body))
        .collect::<Vec<_>>()
        .join(" | ");
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
        .current_dir(world.cwd());
    pty::harden_child_env(&mut command, world.root());
    // Without an ephemeral encrypted vault the binary refuses to start a
    // session at all under a hermetic WAYLAND_HOME, so the turn never reaches
    // a provider and every downstream observation would be an absence rather
    // than a refusal. See the module note on the anti-vacuity gate.
    let vault = vault::configure_process(&mut command);
    let mut child = OwnedTree::new({
        let spawned = command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn();
        drop(vault);
        match spawned {
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
                    child_shell_ran: false,
                    child_shell_wrote_inside: false,
                    child_tool_results: String::new(),
                    transcript_path: String::new(),
                };
            }
        }
    });

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
        child_shell_ran: false,
        child_shell_wrote_inside: false,
        child_tool_results: String::new(),
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
        .current_dir(world.cwd());
    pty::harden_child_env(&mut command, world.root());
    let vault = vault::configure_process(&mut command);
    let mut child = OwnedTree::new({
        let spawned = command
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn();
        drop(vault);

        match spawned {
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
                    child_shell_ran: false,
                    child_shell_wrote_inside: false,
                    child_tool_results: String::new(),
                    transcript_path: String::new(),
                };
            }
        }
    });

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
        child_shell_ran: false,
        child_shell_wrote_inside: false,
        child_tool_results: String::new(),
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

/// `wayland-core --no-tui --provider anthropic "<prompt>"` on a REAL PTY — the
/// same standalone headless surface as [`run_headless`], driven with the
/// approval channel a user at a terminal actually has.
///
/// This transport exists because of a product fact the piped variant hid for
/// two plans. `wcore_agent::confirm::ToolConfirmer::check_for` returns `Denied`
/// unconditionally when `io::stdin()` is not a terminal — a deliberate
/// fail-closed rule, since a blocking `read_line` on a pipe that never reaches
/// EOF would wedge the turn. The consequence is that on the piped headless
/// transport the parent's `Delegate` call is refused before any child exists:
/// every captured run shows `X Tool execution denied by user` and zero child
/// provider turns. Every REFUSED verdict recorded from such a run was an
/// absence of effect from an actor that never acted.
///
/// Answering `y` here is the faithful move, not a bypass: it is exactly what a
/// user at a terminal does, it exercises the gate rather than skipping it (as
/// `--force` would, which would silently change the posture the approval
/// dimension measures), and it is the same choice the json-stream driver
/// already makes when it answers `approval_required` with `tool_approve`.
///
/// The mode proof is three-legged: the process terminated on its own (the
/// full-screen TUI never would), the screen carries no json-stream `ready`
/// frame, and it carries none of the TUI chrome — so a run that fell through
/// into either neighbouring surface cannot report a verdict for this one.
#[cfg(unix)]
fn run_headless_pty(world: &LiveWorld) -> LiveRun {
    let prompt = "delegate the task";
    let invocation = format!(
        "wayland-core --no-tui --provider anthropic \"{prompt}\"  (attached to a real PTY; \
         WAYLAND_HOME={})",
        world.root().display()
    );

    let mut terminal = pty::Pty::spawn_with_args_env(
        world.root(),
        world.cwd(),
        40,
        120,
        &["--no-tui", "--provider", "anthropic", prompt],
        &[("WAYLAND_VAULT_PASSPHRASE", CORPUS_VAULT_PASSPHRASE)],
    );

    let approvals = answer_approval_prompts(
        &mut terminal,
        |screen| screen.contains("Allow?") || screen.contains("[y]es"),
        LIVE_RUN_BUDGET,
        true,
    );
    let exited = terminal.wait_for_exit(Duration::from_secs(3)).is_some();
    let transcript = format!(
        "terminated on its own: {exited}; approval prompts answered: {approvals}\n--- screen ---\n{}",
        terminal.screen_text()
    );

    // The headless mode proof. All three legs must hold: the run ended by
    // itself, it is not the json-stream front-end, and it is not the TUI.
    let landed_headless = exited
        && !transcript.contains("\"type\":\"ready\"")
        && !(transcript.contains("WAYLAND") && transcript.contains("Workspace"));

    LiveRun {
        invocation,
        asserted_mode: landed_headless.then(|| LiveTransport::HeadlessPty.label().to_owned()),
        transcript,
        provider_requests: 0,
        delegation_attempted: false,
        child_turns: 0,
        grandchild_turns: 0,
        child_shell_ran: false,
        child_shell_wrote_inside: false,
        child_tool_results: String::new(),
        transcript_path: String::new(),
    }
}

#[cfg(not(unix))]
fn run_headless_pty(world: &LiveWorld) -> LiveRun {
    LiveRun {
        invocation: format!(
            "wayland-core --no-tui (on a PTY) — DECLARED UNAVAILABLE; WAYLAND_HOME={}",
            world.root().display()
        ),
        asserted_mode: None,
        transcript: LiveTransport::HeadlessPty.unavailable_reason().to_owned(),
        provider_requests: 0,
        delegation_attempted: false,
        child_turns: 0,
        grandchild_turns: 0,
        child_shell_ran: false,
        child_shell_wrote_inside: false,
        child_tool_results: String::new(),
        transcript_path: String::new(),
    }
}

/// Answer every approval gate that appears on a PTY-backed run, the way a user
/// at a terminal does, and report how many were answered.
///
/// Bounded on both axes — a wall-clock deadline and a cap on answers — so a
/// run that somehow re-prompts forever is killed by the harness budget rather
/// than typing into it indefinitely. A run in which NO gate ever appeared
/// answers zero and is recorded as such; the count is carried into the
/// evidence so "the gate was answered" and "no gate appeared" can never be
/// confused for one another.
#[cfg(unix)]
fn answer_approval_prompts(
    terminal: &mut pty::Pty,
    pending: impl Fn(&str) -> bool,
    budget: Duration,
    stop_on_exit: bool,
) -> usize {
    const MAX_ANSWERS: usize = 8;
    let deadline = Instant::now() + budget;
    let mut answered = 0usize;
    let mut last_answer: Option<Instant> = None;
    while Instant::now() < deadline && answered < MAX_ANSWERS {
        let screen = terminal.screen_text();
        let settled = last_answer.is_none_or(|at| at.elapsed() >= Duration::from_millis(600));
        if pending(&screen) && settled {
            terminal.send(b"y\r");
            last_answer = Some(Instant::now());
            answered += 1;
        }
        if terminal
            .wait_for_exit(Duration::from_millis(120))
            .is_some_and(|_| stop_on_exit)
        {
            break;
        }
    }
    answered
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
    let mut terminal = pty::Pty::spawn_with_args_env(
        world.root(),
        world.cwd(),
        40,
        120,
        &[] as &[&str],
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

    let mut approvals = 0usize;
    if booted {
        terminal.send(b"delegate the task\r");
        // Answer the approval card the way a user does. Without this the
        // delegation sits parked on `Awaiting your approval: Delegate` for the
        // whole run, no child is ever created, and every downstream observation
        // is an absence rather than a refusal — which is exactly the vacuity
        // the piped headless transport suffered from.
        approvals = answer_approval_prompts(
            &mut terminal,
            |screen| screen.contains("Awaiting your approval") || screen.contains("approve"),
            Duration::from_secs(9),
            false,
        );
        std::thread::sleep(Duration::from_secs(3));
    }
    let transcript = format!(
        "approval prompts answered: {approvals}\n--- screen ---\n{}",
        terminal.screen_text()
    );
    terminal.quit();

    LiveRun {
        invocation,
        asserted_mode: booted.then(|| LiveTransport::Tui.label().to_owned()),
        transcript,
        provider_requests: 0,
        delegation_attempted: false,
        child_turns: 0,
        grandchild_turns: 0,
        child_shell_ran: false,
        child_shell_wrote_inside: false,
        child_tool_results: String::new(),
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
        transcript: format!(
            "{}; the ephemeral vault passphrase this transport supplies elsewhere ({} bytes) is \
             never reached on this platform",
            LiveTransport::Tui.unavailable_reason(),
            CORPUS_VAULT_PASSPHRASE.len()
        ),
        provider_requests: 0,
        delegation_attempted: false,
        child_turns: 0,
        grandchild_turns: 0,
        child_shell_ran: false,
        child_shell_wrote_inside: false,
        child_tool_results: String::new(),
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
            } else if run.child_shell_ran {
                // The refusal is real. Which mechanism produced it is now
                // measured rather than jointly attributed.
                let mechanism = if run.child_shell_wrote_inside {
                    "ATTRIBUTED TO WORKSPACE CONTAINMENT, NOT TOOL AUTHORITY. The same shell \
                     command's write to a RELATIVE path — inside the child's own workspace, where \
                     containment has nothing to bind — succeeded and returned its marker on the \
                     wire. So the child demonstrably HELD and exercised Bash, and only the \
                     out-of-workspace destination was refused. This row must not be read as \
                     evidence that the tool dimension is enforced"
                } else {
                    "NOT SEPARABLE ON THIS RUN. The shell ran, but its write to a relative path \
                     inside the child's own workspace also produced nothing, so the outside \
                     refusal is attributable to workspace containment OR to something that \
                     refused every write this shell attempted. Recorded as it stands rather than \
                     assigned to one mechanism"
                };
                (
                    Outcome::Refused,
                    "no Bash effect".to_owned(),
                    format!(
                        "the delegated child's SHELL RAN — its stdout marker returned on the wire \
                         in a served provider request — and its write still produced no effect in \
                         the hermetic home. {} delegated child provider turn(s) arrived. \
                         {mechanism}",
                        run.child_turns
                    ),
                )
            } else {
                // THE LIVE KNOWN-POSITIVE, FAILING. Without this arm the row
                // recorded REFUSED and asserted the refusal was "attributable
                // to what that child was given" from a run in which the child's
                // shell may never have started at all.
                (
                    Outcome::NotExpressible,
                    "no verdict — the delegated child's shell never ran".to_owned(),
                    format!(
                        "no Bash effect reached the hermetic home AND the child's stdout marker \
                         never returned on the wire, so the shell did not run. An absent effect \
                         from a shell that never started says nothing about tool authority, \
                         workspace containment or the approval gate, and is not recorded as a \
                         refusal. {} delegated child provider turn(s) arrived. What the child's \
                         own tool call returned: {}",
                        run.child_turns,
                        if run.child_tool_results.is_empty() {
                            "nothing — the child sent no tool_result back to its own endpoint"
                        } else {
                            &run.child_tool_results
                        }
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
            } else {
                // F-V2: a run in which no child ever took a provider turn
                // observed NOTHING about which provider a child would have
                // used. This branch used to record NO-CHANNEL — a decisive
                // verdict — from exactly that run. The absence of a foreign
                // provider request is not evidence when there was no actor to
                // make one.
                (
                    Outcome::NotExpressible,
                    "no verdict — no delegated child took a provider turn in this run".to_owned(),
                    format!(
                        "the parent's mock endpoint served {} request(s) and 0 of them were made \
                         by a delegated child, so nothing was observed about a child's provider \
                         selection; the structural absence of a provider field on the shipped \
                         child-spawn schemas is measured in process, not here",
                        run.provider_requests
                    ),
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
            if ran > FAN_OUT_CAP {
                (
                    Outcome::Allowed,
                    format!("breadth of {ran} children against a parent cap of {FAN_OUT_CAP}"),
                    format!(
                        "a batch of {FAN_OUT_OVER_CAP} was requested and {ran} child provider \
                         turns arrived"
                    ),
                )
            } else {
                (
                    Outcome::Refused,
                    format!(
                        "no breadth beyond the parent cap of {FAN_OUT_CAP} ({ran} children ran)"
                    ),
                    format!(
                        "a batch of {FAN_OUT_OVER_CAP} was requested and {ran} child provider \
                         turns arrived"
                    ),
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
    // A dimension that needs a MUTATING child needs the governed repository to
    // exist: `RequestedChildWorkspace::IsolatedMutation` allocates a git
    // worktree of it. Without one the child dies in workspace preparation, and
    // the probe would read an absent effect from a child that never existed —
    // which is what the tool dimension recorded at both prior SHAs.
    if let Some(reason) = &world.repo_failure
        && entry.dimension == Dimension::Tool
    {
        return ProbeResult::new(
            Outcome::NotExpressible,
            "no verdict — the governed repository could not be created",
            format!(
                "this dimension requires an isolated-mutation child, whose workspace is a git \
                    worktree of the run's repository, and the repository could not be created on \
                    this host: {reason}"
            ),
        )
        .with_live(LiveEvidence {
            invocation: format!("wayland-core ({}) — NOT DRIVEN", transport.label()),
            asserted_mode: transport.label().to_owned(),
            observable: format!("the governed repository could not be created: {reason}"),
        });
    }
    // THE LIVE AT-CAP CONTROL — 21-C3, closing `21-04-PHASE-VERDICT.md` §1 C3
    // bullet 2 ("fan-out is undetermined live, on both platforms and both
    // surfaces").
    //
    // Fan-out is the only dimension whose CORRECT enforcement outcome is zero
    // children, so the shared anti-vacuity gate below — which withholds every
    // verdict taken from a run with no child turn — cannot tell a bound cap
    // from a fixture that could not launch anything. At `359ce2bf` the live
    // over-cap run produced 0 child turns, the gate fired, and the verdict was
    // withheld on both platforms and both surfaces. That withholding was
    // correct given what the probe could see; it is the probe that was short a
    // control, and the in-process sibling has had one since the same SHA.
    //
    // So: drive an AT-CAP batch first through the identical transport, world
    // and script shape. If it admits at least one child, the breadth seam is
    // live in this configuration and a subsequent over-cap run producing zero
    // children is a refusal rather than an absence. If it admits none, nothing
    // is claimed.
    let fan_out_control = (entry.dimension == Dimension::FanOut)
        .then(|| run_live_with_batch(entry.dimension, transport, &world, FAN_OUT_CAP));
    if let Some(control) = &fan_out_control
        && control.child_turns == 0
    {
        return ProbeResult::new(
            Outcome::NotExpressible,
            "no verdict — the breadth seam admitted no child even at the cap",
            format!(
                "the AT-CAP live control requested a batch of {FAN_OUT_CAP} through the same \
                 transport and produced {} delegated child provider turn(s) from {} served \
                 request(s), so this configuration cannot launch a child at all and an over-cap \
                 batch producing zero would prove nothing about the cap. Control transcript at \
                 {}; {}",
                control.child_turns,
                control.provider_requests,
                control.transcript_path,
                head(&control.transcript)
            ),
        )
        .with_live(LiveEvidence {
            invocation: control.invocation.clone(),
            asserted_mode: control
                .asserted_mode
                .clone()
                .unwrap_or_else(|| format!("{}-UNPROVEN", transport.label())),
            observable: format!(
                "at-cap control admitted 0 children; the over-cap verdict was withheld. Control \
                 transcript at {}",
                control.transcript_path
            ),
        });
    }

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

    // THE ANTI-VACUITY GATE, and the single most important lines in this file.
    //
    // Every probe below `observe` reads a negative: no probe file, no sentinel
    // in the transcript, no grandchild completion. A negative is only evidence
    // that a restriction held if the CHILD ACTUALLY ACTED. If no delegated
    // child ever reached its own provider turn, the absence means nothing was
    // attempted — not that something was refused — and recording REFUSED from
    // it would be precisely the class the Phase 20A audit found 283 times: a
    // case that looks identical to a pass from a distance and proves nothing.
    //
    // FINDING F-V2 (Phase 21 verification, 2026-07-26): this gate used to key
    // on `delegation_attempted` — a served request carrying a `tool_result`.
    // That proves the DELEGATING CALL RETURNED. It does not prove the child
    // acted, and the two came apart on every piped-headless run: the confirmer
    // denied the `Delegate` call (`X Tool execution denied by user`), the
    // denial came back as a `tool_result`, the gate passed, and twelve decisive
    // REFUSED verdicts were recorded across two platforms from runs with zero
    // child provider turns. The precondition is now keyed on evidence the CHILD
    // produced: a provider request whose FIRST user message carries this run's
    // L1 goal marker, which only a delegated child's own conversation can
    // carry. `delegation_attempted` is still recorded, because "the delegation
    // never executed" and "the delegation executed but the child never reached
    // a provider turn" are different facts and neither should be readable as
    // the other.
    //
    // The provider dimension is the exception: its whole probe IS the request
    // accounting, so it interprets the counts itself rather than being gated on
    // them — and it now withholds a verdict on the same condition (see
    // `observe`), rather than reporting NO-CHANNEL from a run with no child.
    // 21-C3: fan-out joins the provider dimension as an exception, and for the
    // mirror-image reason. Provider is exempt because its probe IS the request
    // accounting; fan-out is exempt because zero children is its CORRECT
    // enforcement outcome — but only once the at-cap control above has proved
    // the seam live in this exact configuration. Without that control the
    // exemption would be the fail-open the gate exists to prevent, so the two
    // are written as one condition and cannot be separated by a later edit.
    let fan_out_seam_proved_live = fan_out_control
        .as_ref()
        .is_some_and(|control| control.child_turns > 0);
    let child_acted = run.child_turns > 0;
    if !child_acted && entry.dimension != Dimension::Provider && !fan_out_seam_proved_live {
        let cause = if run.delegation_attempted {
            "the delegating tool call executed and returned, but no delegated child reached a \
             provider turn"
        } else {
            "no served request carried a tool_result, so the delegating tool call was never \
             executed"
        };
        return ProbeResult::new(
            Outcome::NotExpressible,
            "no verdict — no delegated child took a provider turn in this run",
            format!(
                "the {} run served {} provider request(s) and 0 of them were made by a delegated \
                 child; {cause}; an absent effect from this run would mean an attempt that never \
                 happened, not a refusal. Approval channel on this transport: {}. Full transcript \
                 at {}; {}",
                transport.label(),
                run.provider_requests,
                transport.approval_channel_reason(),
                run.transcript_path,
                head(&run.transcript)
            ),
        )
        .with_live(LiveEvidence {
            invocation: run.invocation,
            asserted_mode,
            observable: format!(
                "{} provider request(s) served, 0 by a delegated child — {cause}, so the verdict \
                 was withheld; full transcript at {}",
                run.provider_requests, run.transcript_path
            ),
        });
    }

    let (outcome, obtained, observable) = observe(entry.dimension, &world, &run);
    // The control's numbers travel with the verdict, so a reader can check the
    // differential rather than take the refusal on trust.
    let control_note = match &fan_out_control {
        Some(control) => format!(
            "; AT-CAP LIVE CONTROL: a batch of {FAN_OUT_CAP} admitted {} delegated child provider \
             turn(s) from {} served request(s), so the breadth seam is live in this \
             configuration and the over-cap result below is a refusal rather than an absence \
             (control transcript at {})",
            control.child_turns, control.provider_requests, control.transcript_path
        ),
        None => String::new(),
    };
    let observable = format!(
        "{observable} (the run served {} provider request(s); the delegating tool call executed \
         and returned; {} delegated child provider turn(s) arrived){control_note}; full \
         transcript at {}",
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
        //
        // F-V2: for the headless surface the corpus drives the PTY-backed
        // variant wherever the platform permits it. This is the SAME shipped
        // surface and the SAME invocation — `wayland-core --no-tui --provider
        // anthropic "<prompt>"` — differing only in whether the process is
        // attached to a terminal. That difference is decisive rather than
        // cosmetic: `confirm.rs` denies every tool call needing confirmation
        // when stdin is not a terminal, so over pipes the delegation cannot
        // execute and no child can ever act. Where no PTY is available (Windows)
        // the piped variant is driven and its verdict is withheld by the
        // anti-vacuity gate, which is the honest record of a surface this
        // harness cannot drive to an actor on that platform.
        let transport = match entry.standalone_live_mode {
            crate::cases::StandaloneLiveMode::Headless => {
                if LiveTransport::HeadlessPty.available_here() {
                    LiveTransport::HeadlessPty
                } else {
                    LiveTransport::Headless
                }
            }
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

// ===========================================================================
// SELF-TEST FOR THE SHELL-MARKER MATCHER — 21-C3, LANE-BRIEF §6b-ii
//
// The first draft of the live shell observable was self-passing: the marker was
// written literally into the child's Bash command, the command text travels in
// the same served request bodies the matcher searches, and an injection that
// stopped the shell from producing ANY output left the verdict unchanged. The
// defect was found by that lane's own known-negative run and is repaired here
// rather than documented and carried.
//
// §6b-ii requires three assertions, not two, and the third is the only one that
// proves the repair does anything — without it a self-test passes on the broken
// instrument too.
// ===========================================================================

/// A served request body as it looks when the child's tool call is echoed back
/// but the shell produced NOTHING — the exact shape the injection created.
fn body_with_command_text_only() -> String {
    format!(
        r#"{{"messages":[{{"role":"assistant","content":[{{"type":"tool_use","name":"Bash","input":{{"command":"printf %s%s {CHILD_SHELL_RAN_HEAD} {CHILD_SHELL_RAN_TAIL}; printf x > corpus_inside.txt"}}}}]}}]}}"#
    )
}

/// The same body after a shell actually ran and returned its stdout.
fn body_with_shell_output() -> String {
    let mut body = body_with_command_text_only();
    body.push_str(&format!(
        r#",{{"role":"user","content":[{{"type":"tool_result","content":"{CHILD_SHELL_RAN}"}}]}}"#
    ));
    body
}

/// The matcher as it was FIRST written — searching for the two halves the
/// command text carries rather than for the joined string the shell emits.
fn the_old_broken_matcher(bodies: &[String]) -> bool {
    bodies
        .iter()
        .any(|body| body.contains(CHILD_SHELL_RAN_HEAD) && body.contains(CHILD_SHELL_RAN_TAIL))
}

#[test]
fn the_shell_marker_matcher_is_not_satisfied_by_the_command_text() {
    let ran = vec![body_with_shell_output()];
    let never_ran = vec![body_with_command_text_only()];

    // 1. KNOWN-POSITIVE — a shell that ran is detected.
    assert!(
        shell_marker_seen(&ran, CHILD_SHELL_RAN),
        "the matcher failed on a body carrying the shell's own joined stdout marker, so a real \
         shell run would be recorded as 'the shell never ran'"
    );

    // 2. KNOWN-NEGATIVE — the command text alone is NOT detected. This is the
    //    defect: the body below is what the product produces when the child's
    //    Bash call is dispatched and its command exits without printing.
    assert!(
        !shell_marker_seen(&never_ran, CHILD_SHELL_RAN),
        "the matcher was satisfied by the command text alone. The live tool verdict would then \
         report 'the delegated child's SHELL RAN' for a shell that produced nothing, which is the \
         self-passing shape this corpus exists to eliminate."
    );

    // 3. THE ASSERTION THAT PROVES THE REPAIR DOES SOMETHING — §6b-ii. Without
    //    it, assertions 1 and 2 both pass on an instrument that never had the
    //    defect, and the self-test proves nothing about the fix.
    assert!(
        the_old_broken_matcher(&never_ran),
        "the old matcher did NOT match the command-text-only body, so this self-test is not \
         pinning the defect it claims to pin and the split-marker repair is unmotivated"
    );
}

#[test]
fn the_shell_command_text_never_contains_a_joined_marker() {
    // The other half of the repair, checked against the string the product is
    // actually handed rather than against a transcription of it. If someone
    // rejoins the halves in `live_scripts`, this fails before any run does.
    let world = LiveWorld::build();
    let scripts = live_scripts(Dimension::Tool, &world, "http://127.0.0.1:1/x", FAN_OUT_CAP);
    let command = scripts
        .child
        .iter()
        .map(Turn::sse)
        .collect::<Vec<_>>()
        .join(" ");
    assert!(
        command.contains(CHILD_SHELL_RAN_HEAD) && command.contains(CHILD_SHELL_RAN_TAIL),
        "the child's script no longer carries the shell marker halves at all, so the live \
         known-positive cannot fire: {command}"
    );
    for joined in [CHILD_SHELL_RAN, CHILD_SHELL_WROTE_INSIDE] {
        assert!(
            !command.contains(joined),
            "the child's Bash command text contains the JOINED marker {joined:?}. The command is \
             echoed back inside the served request bodies the matcher searches, so the observable \
             would report a shell that never ran as having run."
        );
    }
}
