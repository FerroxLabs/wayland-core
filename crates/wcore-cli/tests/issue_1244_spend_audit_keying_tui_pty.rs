//! FerroxLabs/wayland#1244 — the spend-audit key survives an in-session model
//! switch, driven end to end on a real terminal.
//!
//! ## What #1203 fixed, and why that is not what #1244 asks for
//!
//! Both `AgentEngine` constructors used to hand `install_spend_guard` a fresh
//! `uuid::Uuid::new_v4()`. That is not an identity: it is minted per engine
//! construction, never persisted, never restored on resume — and
//! `rebind_provider`, the one call site that got it right, then swapped it for
//! the real `budget_session_id()` mid-session. So a single `/model` switch
//! split ONE conversation into TWO unrelated keys in
//! `~/.wayland/budget/spend-audit.jsonl`, and the durable audit trail #174 c2
//! exists to provide could not answer "what did this session cost".
//!
//! #1203 replaced the placeholder with `UNBOUND_BUDGET_SESSION_ID`, the same
//! value the authority chain falls back to. #1244 asks for that to be shown
//! HAPPENING: a PTY-driven run of the shipped `wayland-core` TUI that performs
//! an in-session model switch reaching `rebind_provider`, with the audit
//! records written before and after the switch sharing one `session_id`.
//!
//! A unit test on `install_spend_guard` cannot answer that. It proves the
//! constructor is handed a string; it cannot prove the TUI's `/model` arm
//! reaches `rebind_provider` at all, nor that the record the engine writes at
//! task end carries the id the session is really keyed by. Both of those are
//! the whole claim, and both live in the chain from keystroke to jsonl line.
//!
//! ## The chain this drives
//!
//! keystrokes → composer → `SurfaceAction::Command` → the `/model <id>` arm
//! (`tui/surfaces/mod.rs`) → `TuiEngine::rebind_with_config` →
//! `AgentEngine::rebind_provider` → `install_spend_guard` → `SpendGuard` →
//! `JsonlSpendAuditSink` → `<WAYLAND_HOME>/budget/spend-audit.jsonl`.
//!
//! The provider is a local `MockLlm` speaking the Anthropic SSE wire format,
//! so no network is reached and no credential is needed. The api key in the
//! seeded config is not a credential.
//!
//! ## Why `#![cfg(unix)]`
//!
//! Same as every other PTY smoke in this crate: `portable_pty`'s ConPTY
//! backend on a headless Windows runner does not surface the child's stdout to
//! the master end, so the vt100 grid stays empty and every wait times out.
//! **The Windows and macOS terminal legs of this criterion are NOT measured
//! here.**

#![cfg(unix)]

use std::time::Duration;

use tempfile::TempDir;

#[path = "support/mod.rs"]
mod support;

use support::mock_llm::{MockLlm, received_requests};
use support::pty::{Pty, write_config};

/// The model the session BOOTS on, and the one it is switched TO. Two ids the
/// mock will answer for either way, so the switch is the only variable.
const MODEL_A: &str = "claude-sonnet-4-20250514";
const MODEL_B: &str = "claude-3-5-haiku-20241022";

/// Answer tokens, one per turn, so each `wait_for` is anchored on the turn it
/// is actually waiting for rather than on "something appeared".
const ANSWER_1: &str = "WAYLAND1244ANSWERONE";
const ANSWER_2: &str = "WAYLAND1244ANSWERTWO";

/// One completed run: two turns with a `/model` switch between them.
struct Run {
    /// Every record in `<home>/budget/spend-audit.jsonl`, in file order.
    records: Vec<serde_json::Value>,
    /// The raw jsonl lines, so a failure can QUOTE them rather than describe
    /// them — c1 and c3 both ask for the lines.
    lines: Vec<String>,
    /// The `model` field of every request that actually reached the provider.
    models: Vec<String>,
    /// Session ids found under the configured `[session] directory`.
    session_dir_ids: Vec<String>,
    /// The last screen, for a failure report.
    screen: String,
}

/// Boot the shipped TUI against a local mock, send a prompt, switch the model
/// in-session, send a second prompt, and collect everything the assertions
/// below need.
fn drive_a_model_switch() -> Run {
    let home = TempDir::new().expect("tempdir");
    let rt = tokio::runtime::Runtime::new().expect("tokio runtime");
    let server = rt.block_on(MockLlm::new().text(ANSWER_1).text(ANSWER_2).start());
    write_config(home.path(), "anthropic", Some(MODEL_A), Some(&server.uri()));

    let mut pty = Pty::spawn_with_env(home.path(), 40, 200, &[] as &[(&str, &str)]);
    pty.wait_for(
        |s| s.contains("WAYLAND") && s.contains("Workspace"),
        Duration::from_secs(60),
        "TUI to render the chrome wordmark and Workspace tab",
    );

    pty.send(b"first prompt\r");
    pty.wait_for(
        |s| s.contains(ANSWER_1),
        Duration::from_secs(60),
        "the first turn's answer to reach the screen",
    );

    // THE SWITCH. `/model <id>` switches the model live within the current
    // provider — the arm that routes through `rebind_with_config` and so
    // through `rebind_provider`. Bare `/model` would open the picker overlay
    // instead, which needs arrow keys and proves the same thing more slowly.
    pty.send(format!("/model {MODEL_B}\r").as_bytes());
    pty.wait_for(
        |s| s.contains(MODEL_B),
        Duration::from_secs(60),
        "the TUI to acknowledge the model switch",
    );

    pty.send(b"second prompt\r");
    pty.wait_for(
        |s| s.contains(ANSWER_2),
        Duration::from_secs(60),
        "the second turn's answer to reach the screen",
    );

    let screen = pty.screen_text();
    pty.quit();

    let models = rt
        .block_on(received_requests(&server))
        .iter()
        .filter_map(|r| r.model().map(str::to_owned))
        .collect::<Vec<_>>();

    let audit = home.path().join("budget").join("spend-audit.jsonl");
    let raw = std::fs::read_to_string(&audit).unwrap_or_else(|e| {
        panic!(
            "no spend audit at {} ({e}); the run wrote no record at all, so nothing \
             below can be graded.\n--- last screen ---\n{screen}\n--- end ---",
            audit.display()
        )
    });
    let lines: Vec<String> = raw
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .map(str::to_owned)
        .collect();
    let records = lines
        .iter()
        .map(|l| serde_json::from_str::<serde_json::Value>(l).expect("audit line is JSON"))
        .collect();

    // `[session] directory` defaults to `<WAYLAND_HOME>/sessions`; the id is
    // the file stem of each journal entry there.
    let mut session_dir_ids = Vec::new();
    if let Ok(entries) = std::fs::read_dir(home.path().join("sessions")) {
        for e in entries.flatten() {
            let p = e.path();
            if let Some(stem) = p.file_stem().and_then(|s| s.to_str()) {
                session_dir_ids.push(stem.to_owned());
            }
        }
    }
    session_dir_ids.sort();

    Run {
        records,
        lines,
        models,
        session_dir_ids,
        screen,
    }
}

/// The `session_id` of every record.
///
/// The sink writes an ENVELOPE — `{"kind":"task_spend_audit","payload":{…}}` —
/// so the field is one level down. Read here rather than in each test, and
/// PANICKING when it is absent rather than substituting a placeholder: the
/// first run of this file did substitute one, every record then carried the
/// same `"<missing>"`, and `all ids are equal` passed while measuring nothing.
/// A missing key is a broken instrument, not a passing property.
fn session_ids(run: &Run) -> Vec<String> {
    run.records
        .iter()
        .map(|r| {
            r.get("payload")
                .and_then(|p| p.get("session_id"))
                .and_then(|v| v.as_str())
                .unwrap_or_else(|| {
                    panic!(
                        "an audit record carries no payload.session_id, so nothing in this \
                         file can be graded. Lines:\n{}",
                        run.lines.join("\n")
                    )
                })
                .to_owned()
        })
        .collect()
}

/// The provider models each record's dispatches actually ran on.
///
/// The audit trail's own account of the switch, independent of what the mock
/// server saw. Two instruments for one fact: a run where `/model` silently did
/// nothing has to defeat both.
fn dispatch_models(record: &serde_json::Value) -> Vec<String> {
    record
        .get("payload")
        .and_then(|p| p.get("dispatches"))
        .and_then(|d| d.as_array())
        .map(|d| {
            d.iter()
                .filter_map(|x| x.get("model").and_then(|m| m.as_str()).map(str::to_owned))
                .collect()
        })
        .unwrap_or_default()
}

fn looks_like_a_uuid(s: &str) -> bool {
    let parts: Vec<&str> = s.split('-').collect();
    parts.len() == 5
        && [8, 4, 4, 4, 12]
            == [
                parts[0].len(),
                parts[1].len(),
                parts[2].len(),
                parts[3].len(),
                parts[4].len(),
            ]
        && s.chars().all(|c| c.is_ascii_hexdigit() || c == '-')
}

/// c1 — the records written BEFORE and AFTER the switch share one `session_id`,
/// and the switch really happened.
///
/// c3's red arm is this same test against a build whose `install_spend_guard`
/// call sites take `uuid::Uuid::new_v4()`: the two lines then carry different
/// keys and the assertion below prints both.
#[test]
fn the_spend_audit_key_survives_an_in_session_model_switch() {
    let run = drive_a_model_switch();

    // The switch is not assumed. A run where `/model` never reached the
    // provider swap would have both turns on MODEL_A, and every assertion
    // below would pass vacuously.
    assert!(
        run.models.iter().any(|m| m == MODEL_A),
        "no request went out on the boot model {MODEL_A}; models seen: {:?}\n\
         --- last screen ---\n{}\n--- end ---",
        run.models,
        run.screen
    );
    assert!(
        run.models.iter().any(|m| m == MODEL_B),
        "no request went out on the switched-to model {MODEL_B}, so `/model` never \
         reached `rebind_provider` and this run proves nothing about the key \
         surviving a switch; models seen: {:?}\n--- last screen ---\n{}\n--- end ---",
        run.models,
        run.screen
    );

    assert!(
        run.records.len() >= 2,
        "expected an audit record on each side of the switch, got {}:\n{}\n\
         --- last screen ---\n{}\n--- end ---",
        run.records.len(),
        run.lines.join("\n"),
        run.screen
    );

    // The audit trail's OWN account of the switch: the first record's
    // dispatches ran on the boot model and the last record's on the switched-to
    // one. Without this the equality below is satisfied by a run where nothing
    // was ever switched.
    let first_models = dispatch_models(&run.records[0]);
    let last_models = dispatch_models(run.records.last().expect("records is non-empty"));
    assert!(
        first_models.iter().any(|m| m == MODEL_A),
        "the first audit record's dispatches did not run on {MODEL_A}: {first_models:?}\n{}",
        run.lines.join("\n")
    );
    assert!(
        last_models.iter().any(|m| m == MODEL_B),
        "the last audit record's dispatches did not run on {MODEL_B}, so the two records \
         do not straddle a model switch: {last_models:?}\n{}",
        run.lines.join("\n")
    );

    let ids = session_ids(&run);
    let first = &ids[0];
    assert!(
        !first.is_empty() && first != "session-unknown",
        "the run is keyed {first:?}, which is not a session identity — so `all ids are \
         equal` below would hold while measuring nothing:\n{}",
        run.lines.join("\n")
    );
    assert!(
        ids.iter().all(|id| id == first),
        "the model switch split one conversation into {} distinct audit keys \
         ({ids:?}) — #1244 c1. The lines:\n{}",
        ids.iter().collect::<std::collections::BTreeSet<_>>().len(),
        run.lines.join("\n")
    );
}

/// c2 — that one `session_id` is the session's REAL id: it equals an id under
/// the configured `[session] directory`, and it is neither a uuid nor the
/// `session-unknown` placeholder.
///
/// Both negative halves matter. A run keyed `session-unknown` throughout would
/// satisfy c1 perfectly while carrying no identity at all, and a run keyed by a
/// uuid is the pre-#1203 placeholder that started this.
#[test]
fn the_audit_key_is_the_session_id_and_not_a_placeholder() {
    let run = drive_a_model_switch();
    let ids = session_ids(&run);
    assert!(!ids.is_empty(), "no audit records:\n{}", run.screen);
    let key = &ids[0];

    assert_ne!(
        key,
        "session-unknown",
        "the whole run is keyed by the unbound placeholder, so the audit trail \
         names no session. Lines:\n{}",
        run.lines.join("\n")
    );
    assert!(
        !looks_like_a_uuid(key),
        "the audit key {key:?} is a bare uuid — the pre-#1203 placeholder, which is \
         minted per engine construction and never persisted. Lines:\n{}",
        run.lines.join("\n")
    );
    assert!(
        run.session_dir_ids.iter().any(|s| s == key),
        "the audit key {key:?} is not among the session ids under [session] \
         directory ({:?}), so the audit trail cannot be joined to the session it \
         bills. Lines:\n{}",
        run.session_dir_ids,
        run.lines.join("\n")
    );
}
