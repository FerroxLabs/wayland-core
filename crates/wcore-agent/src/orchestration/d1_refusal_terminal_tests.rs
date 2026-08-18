//! D1 regression proofs: a tool that *finishes* must leave its durable
//! execution record terminal, so the turn can still be committed.
//!
//! Live Windows UAT (20A-LIVE-WINDOWS-UAT.md, defect D1) found that a refused
//! `Read` killed the whole process:
//!
//! ```text
//! > Read({"file_path":"C::\\wl-uat-work\\dir with spaces\\needle.txt"})
//!   X Refused to read C::\... : path must be absolute
//! error: Session persistence authority unavailable: invalid journal state
//!        transition: turn turn-... has nonterminal tool execution ...
//! EXIT=1
//! ```
//!
//! The decisive control in that report — an approval *denial* on the same tool
//! exits 0 — is reproduced here as [`approval_denial_control_leaves_turn_committable`].
//!
//! Every test in this file drives the real production dispatcher against real
//! production tools, then attempts the exact journal transition the live defect
//! rejected (`TurnCommitted`). The assertion is the product symptom, not an
//! implementation detail.

use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;
use wcore_tools::registry::ToolRegistry;
use wcore_types::message::ContentBlock;

use super::*;
use crate::journal_effects::{JournalEffectCoordinator, TurnEffectScope};
use crate::session_journal::{SessionEvent, SessionJournal, ToolEffectState};

const TURN: &str = "turn-d1";

fn fixture() -> (tempfile::TempDir, SessionJournal, TurnEffectScope) {
    let dir = tempfile::tempdir().expect("d1 tempdir");
    let journal = SessionJournal::open(dir.path().join("session.journal"), "session").unwrap();
    journal
        .append(SessionEvent::TurnStarted {
            turn_id: TURN.into(),
            user_message: "D1 refusal terminality proof".into(),
        })
        .unwrap();
    let scope = JournalEffectCoordinator::new(journal.clone()).for_turn(TURN);
    (dir, journal, scope)
}

fn call(id: &str, name: &str, input: Value) -> ContentBlock {
    ContentBlock::ToolUse {
        id: id.into(),
        name: name.into(),
        input,
        extra: None,
    }
}

async fn dispatch(registry: &ToolRegistry, call: &ContentBlock, scope: &TurnEffectScope) -> bool {
    let (result, ..) = execute_single_with_budget(
        registry,
        call,
        None,
        wcore_compact::CompactionLevel::Off,
        false,
        None,
        false,
        &CancellationToken::new(),
        None,
        Some(scope),
        0,
    )
    .await;
    matches!(result, ContentBlock::ToolResult { is_error: true, .. })
}

/// The live D1 symptom: after the dispatch, can the turn still be committed?
fn commit_turn(journal: &SessionJournal) -> Result<(), String> {
    journal
        .append(SessionEvent::TurnCommitted {
            turn_id: TURN.into(),
            assistant_message: "turn continues after the tool error".into(),
        })
        .map(|_| ())
        .map_err(|error| error.to_string())
}

fn only_tool_effect(journal: &SessionJournal) -> ToolEffectState {
    let state = journal.state().unwrap();
    assert_eq!(state.tools.len(), 1, "exactly one durable tool record");
    state.tools.values().next().unwrap().effect.clone()
}

/// The exact reproducer from the live UAT: a malformed absolute path refused by
/// `validate_user_path`. Sandbox-independent — plain path validation.
#[tokio::test]
async fn refused_read_leaves_turn_committable() {
    let mut registry = ToolRegistry::new();
    registry.register(Box::new(wcore_tools::read::ReadTool::new(None)));
    let (_dir, journal, scope) = fixture();

    let refused = call(
        "d1-read",
        "Read",
        json!({"file_path": "C::\\work\\needle.txt"}),
    );
    assert!(
        dispatch(&registry, &refused, &scope).await,
        "a malformed path must be refused as a tool error"
    );

    let effect = only_tool_effect(&journal);
    assert!(
        matches!(effect, ToolEffectState::Failed { .. }),
        "a refusal that touched nothing must be a terminal failure, got {effect:?}"
    );
    commit_turn(&journal).expect("the turn must still commit after a refused read");
}

/// The same door reached through `Grep`, one of the other tools the UAT
/// reported broken. Proves the defect is the dispatcher's classification of a
/// completed error, not something specific to `Read`.
#[tokio::test]
async fn failed_grep_leaves_turn_committable() {
    let mut registry = ToolRegistry::new();
    registry.register(Box::new(wcore_tools::grep::GrepTool));
    let (_dir, journal, scope) = fixture();

    let refused = call(
        "d1-grep",
        "Grep",
        json!({"pattern": "needle", "path": "../escape"}),
    );
    assert!(
        dispatch(&registry, &refused, &scope).await,
        "a traversal path must be refused as a tool error"
    );

    let effect = only_tool_effect(&journal);
    assert!(
        matches!(effect, ToolEffectState::Failed { .. }),
        "a refused grep must be a terminal failure, got {effect:?}"
    );
    commit_turn(&journal).expect("the turn must still commit after a refused grep");
}

/// Same for `Glob`, the third tool the UAT listed.
#[tokio::test]
async fn failed_glob_leaves_turn_committable() {
    let mut registry = ToolRegistry::new();
    registry.register(Box::new(wcore_tools::glob::GlobTool));
    let (_dir, journal, scope) = fixture();

    let refused = call("d1-glob", "Glob", json!({"pattern": "/absolute/**/*.rs"}));
    assert!(
        dispatch(&registry, &refused, &scope).await,
        "an absolute glob pattern must be refused as a tool error"
    );

    let effect = only_tool_effect(&journal);
    assert!(
        matches!(effect, ToolEffectState::Failed { .. }),
        "a refused glob must be a terminal failure, got {effect:?}"
    );
    commit_turn(&journal).expect("the turn must still commit after a refused glob");
}

/// The second refusal source the UAT reproduced (§1.5): the shell tool
/// refusing, or running and exiting nonzero. `BashTool` is genuinely
/// [`ToolEffectKind::Opaque`], so unlike the read family its record keeps the
/// ambiguity — but it must still reach a terminal state so the turn commits.
#[tokio::test]
async fn opaque_shell_error_leaves_turn_committable() {
    let mut registry = ToolRegistry::new();
    registry.register(Box::new(wcore_tools::bash::BashTool));
    let (_dir, journal, scope) = fixture();

    // Either the sandbox refuses (the live UAT's §1.5 refusal) or the shell
    // runs and exits 3. Both are `is_error: true` from an opaque tool, which is
    // the exact class D1 killed the session on.
    let failing = call("d1-bash", "Bash", json!({"command": "exit 3"}));
    assert!(
        dispatch(&registry, &failing, &scope).await,
        "a nonzero shell exit must be a tool error"
    );

    let effect = only_tool_effect(&journal);
    assert!(
        !matches!(
            effect,
            ToolEffectState::Prepared | ToolEffectState::Running | ToolEffectState::Unknown { .. }
        ),
        "a completed opaque dispatch must not be left nonterminal, got {effect:?}"
    );
    commit_turn(&journal).expect("the turn must still commit after an opaque tool error");
}

/// Control from the UAT report: an approval denial exits 0. This has always
/// worked and must keep working — it is the reference implementation the
/// refusal path failed to match.
#[tokio::test]
async fn approval_denial_control_leaves_turn_committable() {
    let mut registry = ToolRegistry::new();
    registry.register(Box::new(wcore_tools::read::ReadTool::new(None)));
    let (_dir, journal, scope) = fixture();

    let denied_block = ContentBlock::ToolResult {
        tool_use_id: "d1-denial".into(),
        content: "Tool execution denied: approval was not granted".into(),
        is_error: true,
    };
    let denied = record_terminal_denial(
        &registry,
        Some(&scope),
        0,
        &call(
            "d1-denial",
            "Read",
            json!({"file_path": "C::\\work\\x.txt"}),
        ),
        denied_block,
    );
    assert!(matches!(
        denied,
        ContentBlock::ToolResult { is_error: true, .. }
    ));

    assert!(matches!(
        only_tool_effect(&journal),
        ToolEffectState::NotStarted
    ));
    commit_turn(&journal).expect("a denial has always left the turn committable");
}
