//! wayland#1195 — pin what each session approval mode actually gates, as a HOST
//! sees it on the `--json-stream` wire.
//!
//! # Why this file exists
//!
//! Desktop's workflow and cron builders inherit `yoloMode: true`, which becomes
//! `--auto-approve` and puts the engine in `force`. The proposed fix is for
//! those unattended sessions to select `default` or `auto_edit` instead. That
//! choice is only safe if the engine's behaviour under each mode is KNOWN, and
//! it was not: nobody had measured which tool classes produce
//! `approval_required` under which mode.
//!
//! There is a unit test of the predicate already
//! (`wcore_agent::confirm::typed_policy_confirmation_matrix_is_fail_closed`),
//! and it is not enough. It grades `ToolConfirmer::requires_confirmation_for`,
//! which is the INTERACTIVE-terminal gate. The json-stream path a host drives
//! never calls it: there the parked-ness decision is
//! `orchestration::execute_tool_calls_with_approval`'s `needs_approval`, and
//! the host-visible frame is synthesized separately by
//! `main.rs::GatingProtocolWriter`. Two predicates, one wire. They disagreed —
//! see `force_still_surfaces_a_question_to_the_host` below — and only a test
//! that reads real stdout could see it.
//!
//! So this file spawns the REAL binary, speaks the real protocol at it (mode is
//! applied exactly as Desktop applies it: `WAYLAND_ALLOW_WIRE_FORCE=1` plus a
//! `set_mode` command), and records, per tool, whether the host saw
//! `approval_required` (GATED) or `call_announced` (AUTO — the engine ran it
//! without asking).
//!
//! # Hermetic by construction
//!
//! Mirrors `acp_gate_d012.rs`: every child points `WAYLAND_HOME` + `HOME` at a
//! throwaway tempdir and strips the full provider-credential env set, and the
//! mock provider scripts every tool call, so no real provider is contacted.

use std::io::{BufRead, BufReader, Write};
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

#[path = "support/mod.rs"]
mod support;
use support::mock_llm::MockLlm;
use support::owned_tree::OwnedTree;
use tempfile::TempDir;

/// The provider-credential env-var set every spawned child must NOT inherit.
/// Mirrors `acp_gate_d012.rs::STRIPPED_PROVIDER_ENV`.
const STRIPPED_PROVIDER_ENV: &[&str] = &[
    "API_KEY",
    "ANTHROPIC_API_KEY",
    "OPENAI_API_KEY",
    "GEMINI_API_KEY",
    "GOOGLE_API_KEY",
    "OPENROUTER_API_KEY",
    "DEEPSEEK_API_KEY",
    "GROQ_API_KEY",
    "AWS_ACCESS_KEY_ID",
    "AWS_SECRET_ACCESS_KEY",
    "AWS_SESSION_TOKEN",
    "AWS_PROFILE",
    "AWS_REGION",
    "AWS_DEFAULT_REGION",
    "VERTEX_PROJECT",
    "VERTEX_LOCATION",
    "GOOGLE_APPLICATION_CREDENTIALS",
];

/// What the host saw for one tool call.
#[derive(Debug, PartialEq, Eq)]
enum Seen {
    /// `call_announced` — the engine auto-ran the call without asking.
    Auto,
    /// `approval_required` — the host was given a decision, carrying this
    /// `reason` (the tool's `ToolCategory`).
    Gated(String),
}

/// One measured cell, keyed by the tool name so a reordered transcript cannot
/// silently line up against the wrong expectation.
#[derive(Debug)]
struct Cell {
    tool: String,
    seen: Seen,
    /// The `escalation.kind` on the `tool_request`, when the engine raised one.
    escalation: Option<String>,
}

fn write_config(home: &Path, base_url: &str) {
    let toml = format!(
        "[default]\nprovider = \"anthropic\"\nmodel = \"claude-sonnet-4-20250514\"\n\
         \n[providers.anthropic]\napi_key = \"sk-ant-harness-not-real-key-0000000000\"\n\
         base_url = \"{base_url}\"\n"
    );
    std::fs::write(home.join("config.toml"), toml).expect("write config.toml");
}

/// Drive one `--json-stream` session in `mode` and script exactly ONE tool
/// call at it, denying the gate if one is raised.
///
/// One process per cell on purpose. Scripting several calls into a single
/// session was tried first and is not a sound instrument: a denied call
/// perturbs the turns after it, so a short transcript could be read as "that
/// tool auto-ran" when it means "that turn never happened". One call per
/// process makes every cell independent, and a cell that produced no frame at
/// all is reported as such rather than inferred away.
fn drive(mode: &str, tool: &str, input: serde_json::Value) -> Option<Cell> {
    let home = TempDir::new().expect("tempdir");
    std::fs::write(home.path().join("seed.txt"), "seedline\n").expect("seed file");

    let rt = tokio::runtime::Runtime::new().expect("tokio runtime");
    let mock = MockLlm::new().tool_use(tool, input).text("MATRIX_DONE");
    let server = rt.block_on(mock.start());
    write_config(home.path(), &server.uri());

    let mut cmd = Command::new(env!("CARGO_BIN_EXE_wayland-core"));
    cmd.args(["--json-stream", "--provider", "anthropic"])
        .current_dir(home.path())
        .env("WAYLAND_HOME", home.path())
        .env("HOME", home.path())
        .env("TERM", "dumb")
        // The local-operator opt-in (GHSA-8r7g-7556-hj3j). Desktop sets it, so
        // a wire `set_mode` to an auto-approving mode is honoured rather than
        // refused; without it every arm would silently stay on `default` and
        // this whole matrix would read as one mode measured three times.
        .env("WAYLAND_ALLOW_WIRE_FORCE", "1");
    for key in STRIPPED_PROVIDER_ENV {
        cmd.env_remove(key);
    }
    let vault = support::vault::configure_process(&mut cmd);
    let mut child = OwnedTree::new(
        cmd.stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("spawn --json-stream"),
    );
    drop(vault);

    let mut stdin = child.stdin.take().expect("stdin");
    let stdout = child.stdout.take().expect("stdout");
    let (tx, rx) = std::sync::mpsc::channel::<String>();
    std::thread::spawn(move || {
        for line in BufReader::new(stdout).lines().map_while(Result::ok) {
            if tx.send(line).is_err() {
                break;
            }
        }
    });

    writeln!(stdin, "{{\"type\":\"set_mode\",\"mode\":\"{mode}\"}}").expect("set_mode");
    stdin.flush().ok();

    // Wait for the mode to be APPLIED before sending the prompt. Racing the
    // message against the command would measure whatever mode happened to win.
    let applied = Instant::now() + Duration::from_secs(20);
    let mut mode_applied = false;
    while Instant::now() < applied {
        let Ok(line) = rx.recv_timeout(Duration::from_millis(200)) else {
            continue;
        };
        let Ok(v) = serde_json::from_str::<serde_json::Value>(&line) else {
            continue;
        };
        if v["type"] == "set_mode_refused" {
            panic!("the wire set_mode to {mode} was REFUSED: {v}");
        }
        if v["type"] == "execution_policy" && v["reason"] == "mode_change" {
            mode_applied = true;
            break;
        }
        // `default` is the launch mode, so setting it publishes no change.
        if mode == "default" && v["type"] == "execution_policy" && v["reason"] == "launch" {
            mode_applied = true;
            break;
        }
    }
    assert!(
        mode_applied,
        "no execution_policy frame confirmed mode {mode} before the prompt was sent"
    );

    writeln!(
        stdin,
        "{{\"type\":\"message\",\"msg_id\":\"1\",\"content\":\"run the probes\"}}"
    )
    .expect("message");
    stdin.flush().ok();

    let mut cell: Option<Cell> = None;
    let mut pending: std::collections::HashMap<String, (String, Option<String>)> =
        std::collections::HashMap::new();
    let deadline = Instant::now() + Duration::from_secs(60);
    while Instant::now() < deadline && cell.is_none() {
        let Ok(line) = rx.recv_timeout(Duration::from_millis(250)) else {
            continue;
        };
        let Ok(v) = serde_json::from_str::<serde_json::Value>(&line) else {
            continue;
        };
        match v["type"].as_str().unwrap_or("") {
            "tool_request" => {
                let call_id = v["call_id"].as_str().unwrap_or_default().to_string();
                let name = v["tool"]["name"].as_str().unwrap_or("?").to_string();
                let escalation = v["tool"]["escalation"]["kind"].as_str().map(str::to_string);
                pending.insert(call_id, (name, escalation));
            }
            "call_announced" => {
                cell = Some(Cell {
                    tool: v["tool"]["name"].as_str().unwrap_or("?").to_string(),
                    seen: Seen::Auto,
                    escalation: None,
                });
            }
            "approval_required" => {
                let call_id = v["call_id"].as_str().unwrap_or_default().to_string();
                let reason = v["reason"].as_str().unwrap_or("?").to_string();
                let (name, escalation) = pending
                    .remove(&call_id)
                    .unwrap_or_else(|| ("?".to_string(), None));
                cell = Some(Cell {
                    tool: name,
                    seen: Seen::Gated(reason),
                    escalation,
                });
                // Deny rather than approve: the measurement is the GATE, and a
                // denial keeps the turn moving without running the tool.
                let _ = writeln!(
                    stdin,
                    "{{\"type\":\"tool_deny\",\"call_id\":\"{call_id}\",\"reason\":\"matrix probe\"}}"
                );
                let _ = stdin.flush();
            }
            _ => {}
        }
    }

    let _ = writeln!(stdin, "{{\"type\":\"stop\"}}");
    let _ = child.kill();
    let _ = child.wait();
    cell
}

/// Drive one cell and insist the host saw SOMETHING for it. A cell that
/// produced neither `call_announced` nor `approval_required` is a parked engine
/// nobody was told about -- the wayland#1195 defect -- so it must never read as
/// a pass.
fn measure(mode: &str, tool: &str, input: serde_json::Value) -> Cell {
    drive(mode, tool, input).unwrap_or_else(|| {
        panic!(
            "{mode}/{tool}: the host saw neither call_announced nor \
             approval_required. The engine either never dispatched the call or \
             is parked on a gate it never surfaced."
        )
    })
}

/// One representative call per class the matrix has to answer for.
///
/// `Read`/`WebFetch` are on the shipped `[tools] allow_list`
/// (`wcore-config::default_allow_list`) and `todo` is not, which is why both an
/// allow-listed and a non-allow-listed Info tool are here: the allow-list
/// bypasses the gate in EVERY mode, so measuring only `Read` would report
/// `default` as far more permissive than the mode itself is, and measuring only
/// `todo` would hide that the shipped default lets an unattended session read
/// files and fetch URLs without ever asking.
fn class_probes(home_seed: &str) -> Vec<(&'static str, serde_json::Value)> {
    vec![
        // Info, allow-listed.
        ("Read", serde_json::json!({ "file_path": home_seed })),
        // Info, NOT allow-listed.
        ("todo", serde_json::json!({ "action": "list" })),
        // Edit.
        (
            "Write",
            serde_json::json!({ "file_path": "matrix_out.txt", "content": "m" }),
        ),
        // Exec.
        ("Bash", serde_json::json!({ "command": "echo matrix" })),
        // Mcp, allow-listed.
        (
            "WebFetch",
            serde_json::json!({ "url": "http://127.0.0.1:1/none" }),
        ),
    ]
}

/// A path relative to the session cwd, which every arm sets to its own tempdir.
const SEED: &str = "seed.txt";

/// `default` — the strict mode. Everything the mode governs gates. What it does
/// NOT gate is the shipped `allow_list`, and that is the finding: on a stock
/// install, `default` still lets a session read files and fetch URLs unasked.
#[test]
fn default_gates_everything_the_mode_governs() {
    let probes = class_probes(SEED);
    let cells: Vec<Cell> = probes
        .into_iter()
        .map(|(tool, input)| measure("default", tool, input))
        .collect();

    assert_eq!(seen_for(&cells, "todo"), &Seen::Gated("info".into()));
    assert_eq!(seen_for(&cells, "Write"), &Seen::Gated("edit".into()));
    assert_eq!(seen_for(&cells, "Bash"), &Seen::Gated("exec".into()));

    // The allow-list bypass, asserted rather than assumed. It is the reason
    // `default` is not "asks about everything", and a change to
    // `default_allow_list` that silently widened it must redden here.
    assert_eq!(
        seen_for(&cells, "Read"),
        &Seen::Auto,
        "Read is on the shipped allow_list, so `default` does not gate it"
    );
    assert_eq!(
        seen_for(&cells, "WebFetch"),
        &Seen::Auto,
        "WebFetch is on the shipped allow_list, so NO mode gates network egress \
         through it — this is the posture an unattended run inherits"
    );
}

/// `auto_edit` — the middle ground. It moves EXACTLY the built-in `Write`/`Edit`
/// pair from gated to auto; every other class is unchanged from `default`.
///
/// The `todo` row is the one that matters for wayland#1195: core gates it by
/// NAME (`AutoEdit` auto-approves only `Write`/`Edit`, never a category), and
/// the frame it gates with carries `reason: "info"` — which is the category a
/// category-matching host then auto-approves.
#[test]
fn auto_edit_moves_only_the_builtin_write_edit_pair() {
    let probes = class_probes(SEED);
    let cells: Vec<Cell> = probes
        .into_iter()
        .map(|(tool, input)| measure("auto_edit", tool, input))
        .collect();

    assert_eq!(
        seen_for(&cells, "Write"),
        &Seen::Auto,
        "auto_edit's whole purpose is that a file write proceeds"
    );
    assert_eq!(
        seen_for(&cells, "Bash"),
        &Seen::Gated("exec".into()),
        "auto_edit must not widen to exec"
    );
    assert_eq!(
        seen_for(&cells, "todo"),
        &Seen::Gated("info".into()),
        "auto_edit is name-scoped, not category-scoped: an Info-category tool \
         that is not Write/Edit still gates, and it gates as `info`"
    );
    assert_eq!(seen_for(&cells, "Read"), &Seen::Auto);
    assert_eq!(seen_for(&cells, "WebFetch"), &Seen::Auto);
}

/// `force` (`yolo` on the wire) — nothing the mode governs gates. This is the
/// posture Desktop's workflow and cron builders inherit today.
#[test]
fn force_gates_nothing_the_mode_governs() {
    let probes = class_probes(SEED);
    let cells: Vec<Cell> = probes
        .into_iter()
        .map(|(tool, input)| measure("force", tool, input))
        .collect();
    for tool in ["Read", "todo", "Write", "Bash", "WebFetch"] {
        assert_eq!(
            seen_for(&cells, tool),
            &Seen::Auto,
            "under force nothing may gate, and {tool} did"
        );
    }
}

/// Look one tool up in a measured set of cells.
fn seen_for<'a>(cells: &'a [Cell], tool: &str) -> &'a Seen {
    &cells
        .iter()
        .find(|c| c.tool == tool)
        .unwrap_or_else(|| panic!("no cell for {tool}; measured: {cells:#?}"))
        .seen
}

/// #1099 wiring, pinned here because it is the one escalation that crosses
/// every category: a read outside the workspace gates even though `Read` is
/// allow-listed and its category is `info`, and the `tool_request` carries the
/// `path_boundary` escalation a host needs to render the folder-grant card.
///
/// Under `force` the classifier is suppressed at
/// `orchestration/mod.rs` (`if globally_approved || recovered_approval`), so no
/// card is raised at all — asserted as the negative leg so the asymmetry is a
/// recorded decision rather than an accident.
#[test]
fn a_read_outside_the_workspace_escalates_in_every_mode_except_force() {
    // A readable path outside the session workspace, BUILT here rather than
    // borrowed from the OS.
    //
    // core#409 c1 — the probe used to be the literal `/etc/hostname`, and on
    // Windows that measured nothing. `Path::new("/etc/hostname").is_absolute()`
    // is FALSE there (a root with no drive prefix), so `read_path_boundary`
    // takes its relative branch and resolves the probe against the workspace
    // root, which on Windows means the drive prefix plus `etc\hostname` —
    // measured on a real host as `\\?\F:\etc\hostname`. Neither that file nor its
    // containing
    // folder exists, so `grantable_read_root`'s `canonicalize` fails and the
    // classifier declines the card, exactly as it is documented to: it never
    // offers a folder grant it could not actually mint. `Read` then fell
    // through to the shipped allow_list and auto-ran, and this assertion read
    // `Auto`. The PRODUCT was right; the fixture's notion of "outside" was
    // Unix-only.
    //
    // Making the folder ourselves is what the rest of this file already does
    // for the workspace side, and it makes "outside the workspace" true by
    // construction on every platform instead of by borrowing a system path.
    let outside_dir = TempDir::new().expect("outside tempdir");
    let outside_root = outside_dir.path().join("readable");
    std::fs::create_dir(&outside_root).expect("outside root");
    let outside_file = outside_root.join("note.txt");
    std::fs::write(&outside_file, b"outside\n").expect("outside file");
    let outside = serde_json::json!({
        "file_path": outside_file.to_str().expect("utf-8 outside path"),
    });

    for mode in ["default", "auto_edit"] {
        let cell = measure(mode, "Read", outside.clone());
        assert_eq!(
            cell.seen,
            Seen::Gated("info".into()),
            "{mode}: a boundary read must gate even though Read is allow-listed"
        );
        assert_eq!(
            cell.escalation.as_deref(),
            Some("path_boundary"),
            "{mode}: the host cannot render a folder grant it was never sent"
        );
    }

    let cell = measure("force", "Read", outside);
    assert_eq!(
        cell.seen,
        Seen::Auto,
        "force suppresses the boundary classifier; the read is auto-run and \
         then refused by the workspace policy instead"
    );
}

/// wayland#1195 REGRESSION — `AskUserQuestion` parks the engine in EVERY
/// posture, `force` included (`orchestration`'s `needs_approval` names it
/// unconditionally, and `wcore_agent::confirm` has the matching
/// `ask_user_question_always_requires_a_host_response`). The host must
/// therefore be SHOWN that park.
///
/// It was not. `GatingProtocolWriter` re-derived parked-ness from the approval
/// POSTURE, which says "auto-approved" under force, and suppressed the gate
/// frame — so the measured wire carried `tool_request` and then nothing for the
/// life of the turn. A host that keys on `approval_required` (the D012 gate
/// frame, and what `acp_engine`'s relay projects to ACP clients) waits forever.
/// The unit test above it passed throughout: it grades the predicate, not the
/// wire.
#[test]
fn force_still_surfaces_a_question_to_the_host() {
    let cell = measure(
        "force",
        "AskUserQuestion",
        serde_json::json!({ "question": "pick one", "options": ["a", "b"] }),
    );
    assert_eq!(
        cell.seen,
        Seen::Gated("info".into()),
        "AskUserQuestion needs an ANSWER, which no approval mode can supply, so \
         the gate frame must reach the host in force too"
    );
}
