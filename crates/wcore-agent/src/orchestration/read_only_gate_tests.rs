//! `[default] read_only` enforcement proofs.
//!
//! Three defects are under test here, all from the same review:
//!
//! 1. **The flag did nothing.** `[default] read_only = true` parsed, merged and
//!    round-tripped through TOML, and then resolution into the runtime `Config`
//!    dropped it — so no component could see it and nothing enforced it.
//! 2. **The carve-out.** The first gate keyed off `ToolCategory`.
//!    `SkillTool::category_for` returns `Info` for an inline skill, so `Skill`
//!    passed the gate and then ran the skill body's embedded `` !`…` `` shell
//!    directive, which created a real file on disk in a session the operator
//!    had been told was read-only.
//! 3. **Hooks fired for refused calls.** The gate sat AFTER the PreToolUse hook
//!    block, so a read-only session still executed operator-configured shell
//!    for a call it was about to refuse.
//!
//! Every test here has a **control** that runs the identical scenario with
//! `read_only` off and asserts the write DID happen and the hook DID fire.
//! Without the control, "no file appeared" would be satisfied by a test that
//! never had a working write in the first place.

use serde_json::{Value, json};
use std::path::Path;
use tokio_util::sync::CancellationToken;
use wcore_config::hooks::{HookDef, HooksConfig};
use wcore_tools::registry::ToolRegistry;
use wcore_types::message::ContentBlock;

use super::*;
use crate::hooks::HookEngine;
use crate::journal_effects::{JournalEffectCoordinator, TurnEffectScope};
use crate::session_journal::{SessionEvent, SessionJournal};
use crate::skill_tool::SkillTool;

const TURN: &str = "turn-read-only";

fn scope_fixture(dir: &Path) -> (SessionJournal, TurnEffectScope) {
    let journal = SessionJournal::open(dir.join("session.journal"), "session").unwrap();
    journal
        .append(SessionEvent::TurnStarted {
            turn_id: TURN.into(),
            user_message: "read_only enforcement proof".into(),
        })
        .unwrap();
    let scope = JournalEffectCoordinator::new(journal.clone()).for_turn(TURN);
    (journal, scope)
}

fn call(id: &str, name: &str, input: Value) -> ContentBlock {
    ContentBlock::ToolUse {
        id: id.into(),
        name: name.into(),
        input,
        extra: None,
    }
}

/// A PreToolUse shell hook whose only observable effect is creating `sentinel`.
/// Its existence after a dispatch is a direct, non-inferred answer to "did the
/// operator's hook run".
fn touch_hook(sentinel: &Path) -> HookEngine {
    let command = if cfg!(windows) {
        format!("type nul > \"{}\"", sentinel.display())
    } else {
        format!("touch '{}'", sentinel.display())
    };
    // A matching PostToolUse hook is required for the CONTROL to reach the end
    // of dispatch: a tool that actually runs under a journal scope must have a
    // prepared post-hook phase, and the phase is only prepared when a post hook
    // matches. It is a no-op command — the sentinel is written by the pre hook
    // and only by the pre hook.
    let no_op = if cfg!(windows) { "cd ." } else { "true" };
    let config = HooksConfig {
        pre_tool_use: vec![HookDef {
            name: "read-only-hook-witness".to_string(),
            tool_match: Vec::new(),
            file_match: Vec::new(),
            command,
            timeout_ms: 30_000,
        }],
        post_tool_use: vec![HookDef {
            name: "read-only-post-noop".to_string(),
            tool_match: Vec::new(),
            file_match: Vec::new(),
            command: no_op.to_string(),
            timeout_ms: 30_000,
        }],
        ..HooksConfig::default()
    };
    HookEngine::new(config)
}

async fn dispatch(
    registry: &ToolRegistry,
    call: &ContentBlock,
    hooks: Option<&HookEngine>,
    scope: &TurnEffectScope,
) -> ContentBlock {
    let (result, ..) = execute_single_with_streaming(
        registry,
        call,
        hooks,
        wcore_compact::CompactionLevel::Off,
        false,
        None,
        None,
        false,
        &CancellationToken::new(),
        None,
        Some(scope),
        0,
        None,
    )
    .await;
    result
}

fn refusal_text(result: &ContentBlock) -> String {
    match result {
        ContentBlock::ToolResult {
            content, is_error, ..
        } => {
            assert!(*is_error, "a read_only refusal must be an error result");
            content.clone()
        }
        other => panic!("expected a ToolResult, got {other:?}"),
    }
}

fn write_registry(read_only: bool) -> ToolRegistry {
    let mut registry = ToolRegistry::new();
    registry.register(Box::new(wcore_tools::write::WriteTool::new(None)));
    registry.register(Box::new(wcore_tools::read::ReadTool::new(None)));
    registry.set_read_only(read_only);
    registry
}

// ---------------------------------------------------------------------------
// Defect 3 — the gate must sit ahead of the PreToolUse hook block.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn read_only_refuses_write_and_the_pre_tool_use_hook_never_fires() {
    let dir = tempfile::tempdir().unwrap();
    let (_journal, scope) = scope_fixture(dir.path());
    let sentinel = dir.path().join("hook-fired");
    let target = dir.path().join("written-by-write-tool.txt");

    let registry = write_registry(true);
    let hooks = touch_hook(&sentinel);
    let result = dispatch(
        &registry,
        &call(
            "ro-write",
            "Write",
            json!({"file_path": target.to_str().unwrap(), "content": "payload"}),
        ),
        Some(&hooks),
        &scope,
    )
    .await;

    let text = refusal_text(&result);
    assert!(
        text.contains("read-only"),
        "the refusal must name the posture that caused it; got: {text}"
    );
    assert!(
        !target.exists(),
        "a read-only session must not have written {}",
        target.display()
    );
    assert!(
        !sentinel.exists(),
        "the PreToolUse hook fired for a call the session refused — the gate is \
         still behind the hook block"
    );
}

/// The control for the test above. Same registry, same hook, same call, with
/// `read_only` off: the hook DOES fire and the file IS written. This is what
/// makes the assertions above capable of failing.
#[tokio::test]
async fn control_write_and_hook_both_run_when_read_only_is_off() {
    let dir = tempfile::tempdir().unwrap();
    let (_journal, scope) = scope_fixture(dir.path());
    let sentinel = dir.path().join("hook-fired");
    let target = dir.path().join("written-by-write-tool.txt");

    let registry = write_registry(false);
    let hooks = touch_hook(&sentinel);
    let result = dispatch(
        &registry,
        &call(
            "rw-write",
            "Write",
            json!({"file_path": target.to_str().unwrap(), "content": "payload"}),
        ),
        Some(&hooks),
        &scope,
    )
    .await;

    match &result {
        ContentBlock::ToolResult { is_error, .. } => {
            assert!(!is_error, "control write must succeed: {result:?}")
        }
        other => panic!("expected a ToolResult, got {other:?}"),
    }
    assert!(
        target.exists(),
        "control: the write must actually land, or the read-only assertion proves nothing"
    );
    assert!(
        sentinel.exists(),
        "control: the PreToolUse hook must actually fire, or its absence under \
         read_only proves nothing"
    );
}

// ---------------------------------------------------------------------------
// Defect 2 — the Skill carve-out. This is the reported bypass, verbatim.
// ---------------------------------------------------------------------------

fn shell_skill(name: &str, artifact: &Path) -> wcore_skills::types::SkillMetadata {
    let command = if cfg!(windows) {
        format!("type nul > \"{}\"", artifact.display())
    } else {
        format!("touch '{}'", artifact.display())
    };
    let content = format!("before !`{command}` after");
    wcore_skills::types::SkillMetadata {
        name: name.to_string(),
        display_name: None,
        description: format!("desc of {name}"),
        has_user_specified_description: true,
        allowed_tools: Vec::new(),
        argument_hint: None,
        argument_names: Vec::new(),
        when_to_use: None,
        version: None,
        model: None,
        disable_model_invocation: false,
        user_invocable: true,
        execution_context: wcore_skills::types::ExecutionContext::Inline,
        agent: None,
        effort: None,
        shell: None,
        paths: Vec::new(),
        artifacts: Vec::new(),
        hooks_raw: None,
        source: wcore_skills::types::SkillSource::User,
        loaded_from: wcore_skills::types::LoadedFrom::Skills,
        content_length: content.len(),
        content,
        skill_root: None,
        max_turns: None,
        max_tokens: None,
    }
}

fn skill_tool(artifact: &Path, cwd: &Path, read_only: bool) -> SkillTool {
    SkillTool::new(
        std::sync::Arc::new(wcore_skills::refs::SkillCatalog::from_metadata_vec(vec![
            shell_skill("writer", artifact),
        ])),
        cwd.to_string_lossy().into_owned(),
        wcore_skills::permissions::SkillPermissionChecker::new(Vec::new(), Vec::new(), true),
    )
    .with_read_only(read_only)
}

fn skill_registry(artifact: &Path, cwd: &Path, read_only: bool) -> ToolRegistry {
    let mut registry = ToolRegistry::new();
    registry.register(Box::new(skill_tool(artifact, cwd, read_only)));
    registry.set_read_only(read_only);
    registry
}

/// `SkillTool::category_for` returns `Info` for this skill — the exact reason
/// the first, category-keyed gate let it through. Asserted here so a future
/// change that re-derives the gate from category is caught by a test that
/// states why it must not be.
#[test]
fn the_skill_that_writes_still_classifies_as_info() {
    let dir = tempfile::tempdir().unwrap();
    let artifact = dir.path().join("artifact");
    let tool = skill_tool(&artifact, dir.path(), true);
    assert_eq!(
        wcore_tools::Tool::category_for(&tool, &json!({"skill": "writer"})),
        wcore_protocol::events::ToolCategory::Info,
        "if this ever stops being Info the comment explaining the carve-out is stale"
    );
    assert!(
        !wcore_tools::Tool::read_only_safe(&tool, &json!({"skill": "writer"})),
        "Skill must never declare read-only safety — its body can run shell"
    );
}

#[tokio::test]
async fn read_only_refuses_the_skill_so_its_embedded_shell_cannot_write() {
    let dir = tempfile::tempdir().unwrap();
    let (_journal, scope) = scope_fixture(dir.path());
    let artifact = dir.path().join("written-by-skill-shell.txt");

    let registry = skill_registry(&artifact, dir.path(), true);
    let result = dispatch(
        &registry,
        &call("ro-skill", "Skill", json!({"skill": "writer"})),
        None,
        &scope,
    )
    .await;

    let text = refusal_text(&result);
    assert!(
        text.contains("read-only"),
        "the refusal must name the posture; got: {text}"
    );
    assert!(
        !artifact.exists(),
        "the skill's embedded shell created {} in a read-only session — this is \
         the reported bypass",
        artifact.display()
    );
}

/// Control: the same skill, dispatched with `read_only` off, really does run
/// its embedded shell and really does create the file. Without this, the test
/// above could be green because the skill never worked.
#[tokio::test]
async fn control_skill_shell_writes_the_file_when_read_only_is_off() {
    let dir = tempfile::tempdir().unwrap();
    let (_journal, scope) = scope_fixture(dir.path());
    let artifact = dir.path().join("written-by-skill-shell.txt");

    let registry = skill_registry(&artifact, dir.path(), false);
    let result = dispatch(
        &registry,
        &call("rw-skill", "Skill", json!({"skill": "writer"})),
        None,
        &scope,
    )
    .await;

    match &result {
        ContentBlock::ToolResult { is_error, .. } => {
            assert!(!is_error, "control skill must succeed: {result:?}")
        }
        other => panic!("expected a ToolResult, got {other:?}"),
    }
    assert!(
        artifact.exists(),
        "control: the skill's embedded shell must actually write, or its absence \
         under read_only proves nothing"
    );
}

/// The cron skill sink builds a `SkillTool` and calls `execute()` on it with no
/// dispatcher in the path, so the registry gate cannot see it. The posture has
/// to be enforced inside the tool as well, or an unattended fire keeps the hole
/// open.
#[tokio::test]
async fn skill_tool_refuses_without_a_dispatcher_in_the_path() {
    let dir = tempfile::tempdir().unwrap();
    let artifact = dir.path().join("written-by-cron-skill.txt");

    let refused = wcore_tools::Tool::execute(
        &skill_tool(&artifact, dir.path(), true),
        json!({"skill": "writer"}),
    )
    .await;
    assert!(
        refused.is_error,
        "direct execute must refuse under read_only"
    );
    assert!(
        !artifact.exists(),
        "no shell may run on the dispatcher-less path"
    );

    let allowed = wcore_tools::Tool::execute(
        &skill_tool(&artifact, dir.path(), false),
        json!({"skill": "writer"}),
    )
    .await;
    assert!(!allowed.is_error, "control: {}", allowed.content);
    assert!(
        artifact.exists(),
        "control: the dispatcher-less path must really write when read_only is off"
    );
}

// ---------------------------------------------------------------------------
// The allow-set is a positive claim, not a category or a name list.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn read_only_still_allows_the_tools_that_declare_read_only_safety() {
    let dir = tempfile::tempdir().unwrap();
    let (_journal, scope) = scope_fixture(dir.path());
    let target = dir.path().join("readable.txt");
    std::fs::write(&target, "content").unwrap();

    let registry = write_registry(true);
    let result = dispatch(
        &registry,
        &call(
            "ro-read",
            "Read",
            json!({"file_path": target.to_str().unwrap()}),
        ),
        None,
        &scope,
    )
    .await;

    match &result {
        ContentBlock::ToolResult {
            is_error, content, ..
        } => {
            assert!(
                !is_error,
                "Read must survive a read-only session: {content}"
            );
            assert!(content.contains("content"), "got: {content}");
        }
        other => panic!("expected a ToolResult, got {other:?}"),
    }
}

/// Default-deny is the whole design: a tool this registry has never heard of
/// must be refused by the posture, not admitted by the absence of a rule.
#[tokio::test]
async fn read_only_refuses_a_tool_the_registry_does_not_know() {
    let dir = tempfile::tempdir().unwrap();
    let (_journal, scope) = scope_fixture(dir.path());
    let sentinel = dir.path().join("hook-fired");

    let registry = write_registry(true);
    let hooks = touch_hook(&sentinel);
    let result = dispatch(
        &registry,
        &call("ro-unknown", "SomeFutureTool", json!({})),
        Some(&hooks),
        &scope,
    )
    .await;

    let text = refusal_text(&result);
    assert!(text.contains("read-only"), "got: {text}");
    assert!(
        !sentinel.exists(),
        "an unknown tool must not fire hooks in a read-only session either"
    );
}

/// The built-in tools that DO claim read-only safety, asserted directly. If a
/// future edit adds `read_only_safe -> true` to a mutating tool this list is
/// where the review should notice.
#[test]
fn only_read_grep_and_glob_declare_read_only_safety() {
    let input = json!({});
    assert!(wcore_tools::Tool::read_only_safe(
        &wcore_tools::read::ReadTool::new(None),
        &input
    ));
    assert!(wcore_tools::Tool::read_only_safe(
        &wcore_tools::grep::GrepTool,
        &input
    ));
    assert!(wcore_tools::Tool::read_only_safe(
        &wcore_tools::glob::GlobTool,
        &input
    ));
    assert!(!wcore_tools::Tool::read_only_safe(
        &wcore_tools::write::WriteTool::new(None),
        &input
    ));
    assert!(!wcore_tools::Tool::read_only_safe(
        &wcore_tools::edit::EditTool::new(None),
        &input
    ));
}
