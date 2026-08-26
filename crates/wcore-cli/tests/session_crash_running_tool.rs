//! A crash while a tool was RUNNING must not brick the session.
//!
//! The conformance run reproduced this from outside the product: SIGKILL an
//! agent while a `Bash` tool is in flight, and every remedy the engine's own
//! refusal names — "resume, reconcile, or cancel it" — is refused. `reconcile
//! --resolve` answers that the tool execution "is in state Running — this
//! state has no operator-writable receipt", and `cancel` then exits 5 with
//! "outstanding reconcile item(s)", so `--continue` never stops refusing.
//!
//! Layer 1 of the harness split used by `session_operator_lifecycle.rs`: spawn
//! the real `wayland-core` binary and grade its exit codes and STDOUT tokens,
//! plus the journal re-read by a fresh process. Nothing here reads the agent's
//! own account of itself.

use std::path::Path;
use std::process::{Command, Output};

fn binary() -> &'static str {
    env!("CARGO_BIN_EXE_wayland-core")
}

fn run(args: &[&str], home: &Path) -> Output {
    Command::new(binary())
        .args(args)
        .current_dir(home)
        .env("HOME", home)
        .env("WAYLAND_HOME", home)
        .env_remove("API_KEY")
        .env_remove("ANTHROPIC_API_KEY")
        .env_remove("OPENAI_API_KEY")
        .output()
        .unwrap_or_else(|e| panic!("failed to spawn {} {args:?}: {e}", binary()))
}

fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn code(output: &Output) -> i32 {
    output.status.code().unwrap_or(-1)
}

/// Build the exact durable state a mid-tool crash leaves: a finished-but-
/// unconsumed pre-tool hook phase, a RUNNING tool execution, and a prepared
/// post-tool hook phase — written with the same journal leases the engine
/// uses, then abandoned without any terminal transition.
///
/// Returns the session directory, the session id and the tool execution id.
fn crashed_mid_tool_session(home: &Path) -> (std::path::PathBuf, String, String) {
    crashed_mid_tool_session_with(
        home,
        "Bash",
        wcore_types::tool::ToolEffectContract::default(),
    )
}

/// As above, for a named tool with a named effect contract. The contract is
/// what decides whether the product can settle the interruption itself, so a
/// test about that distinction must be able to set it.
fn crashed_mid_tool_session_with(
    home: &Path,
    tool_name: &str,
    contract: wcore_types::tool::ToolEffectContract,
) -> (std::path::PathBuf, String, String) {
    use wcore_agent::journal_effects::JournalEffectCoordinator;
    use wcore_agent::session::SessionManager;
    use wcore_agent::session_journal::{
        HookManifestSlot, HookSlotReceipt, HookSlotSource, HookSlotTerminalStatus, SessionEvent,
        ToolHookPhase, state_payload_digest,
    };

    let sessions = home.join("sessions");
    std::fs::create_dir_all(&sessions).unwrap();
    let manager = SessionManager::new(sessions.clone(), 50);

    // Everything holding the journal writer lease lives in this block. The
    // block ending IS the crash.
    let mut active = manager
        .create_for_run("anthropic", "test-model", "/tmp", None)
        .unwrap();
    let id = active.session.id.clone();
    active
        .session
        .messages
        .push(wcore_types::message::Message::new(
            wcore_types::message::Role::User,
            vec![wcore_types::message::ContentBlock::Text {
                text: "run the job".to_owned(),
            }],
        ));
    manager.persist_first_message(&active.session).unwrap();
    active
        .journal
        .append(SessionEvent::TurnStarted {
            turn_id: "turn-1".to_owned(),
            user_message: "run the job".to_owned(),
        })
        .unwrap();

    let coordinator = JournalEffectCoordinator::new(active.journal.clone());
    let scope = coordinator.for_turn("turn-1");

    // One built-in Rust hook slot, named exactly as the orchestrator names it
    // (`{source}-{ordinal}-{descriptor_digest}`) — a default install already
    // has one, which is why the crashed session in the conformance run carried
    // hook phases at all.
    let descriptor_digest =
        state_payload_digest(&serde_json::json!({ "kind": "rust", "name": "builtin" })).unwrap();
    let slots = vec![HookManifestSlot {
        ordinal: 0,
        slot_id: format!("rust-0-{descriptor_digest}"),
        source: HookSlotSource::Rust,
        descriptor_digest: descriptor_digest.clone(),
    }];
    let receipts = vec![HookSlotReceipt {
        ordinal: 0,
        slot_id: slots[0].slot_id.clone(),
        descriptor_digest,
        status: HookSlotTerminalStatus::Completed,
    }];
    let manifest_digest = state_payload_digest(&serde_json::to_value(&slots).unwrap()).unwrap();
    let receipts_digest = state_payload_digest(&serde_json::to_value(&receipts).unwrap()).unwrap();
    let tool_input = serde_json::json!({ "command": "sleep 60" });
    let input_digest = state_payload_digest(&tool_input).unwrap();
    let authority_digest = state_payload_digest(&serde_json::json!({ "hooks": [] })).unwrap();
    let outcome_digest = state_payload_digest(&serde_json::json!({ "decision": "allow" })).unwrap();

    // 1. the pre-tool hook phase finished but was never consumed — a recovery
    //    checkpoint is what consumes one, and the crash landed before any was
    //    recorded.
    let pre = scope
        .prepare_hook_phase(
            "call-1",
            0,
            ToolHookPhase::PreToolUse,
            None,
            input_digest.clone(),
            authority_digest.clone(),
            manifest_digest.clone(),
            slots.clone(),
        )
        .unwrap();
    let pre_hook_phase_id = pre.id().to_owned();
    pre.start(None)
        .unwrap()
        .finish(
            Some(input_digest.clone()),
            outcome_digest,
            receipts_digest,
            receipts,
        )
        .unwrap();

    // 2. the tool is RUNNING — the journal's last word on it.
    let prepared = scope
        .prepare_tool_after_hook(
            "call-1",
            0,
            tool_name,
            tool_input.clone(),
            tool_input,
            contract,
            pre_hook_phase_id,
        )
        .unwrap();
    let tool_execution_id = prepared.id().to_owned();

    // 3. the post-tool hook phase is prepared before the tool may start (the
    //    reducer refuses `ToolExecutionStarted` without one) and will never
    //    start itself.
    let post = scope
        .prepare_hook_phase(
            "call-1",
            0,
            ToolHookPhase::PostToolUse,
            Some(tool_execution_id.clone()),
            input_digest,
            authority_digest,
            manifest_digest,
            slots,
        )
        .unwrap();
    let running = prepared.start().unwrap();

    drop(post);
    drop(running);
    drop(scope);
    drop(coordinator);
    drop(active);

    (sessions, id, tool_execution_id)
}

#[test]
fn a_crash_while_a_tool_is_running_is_recoverable_by_the_named_remedies() {
    use wcore_agent::session_journal::SessionJournal;

    let home = tempfile::tempdir().unwrap();
    let (sessions, id, tool_execution_id) = crashed_mid_tool_session(home.path());
    let dir = sessions.to_str().unwrap();

    let before = run(&["session", "--dir", dir, "show", &id], home.path());
    assert_eq!(code(&before), 0);
    let shown = stdout(&before);
    assert!(
        shown.contains("interrupted=1") && shown.contains("reason=Running"),
        "the fixture must present one interrupted turn holding a RUNNING tool; got:\n{shown}"
    );

    // REMEDY the engine's refusal names: reconcile the interrupted effect.
    let resolved = run(
        &[
            "session",
            "--dir",
            dir,
            "reconcile",
            &id,
            "--resolve",
            &tool_execution_id,
            "--as-outcome",
            "not-started",
        ],
        home.path(),
    );
    assert_eq!(
        code(&resolved),
        0,
        "reconcile must be able to dispose of a tool the crash left RUNNING; stderr: {}",
        String::from_utf8_lossy(&resolved.stderr)
    );

    // REMEDY the engine's refusal names: cancel the interrupted turn.
    let cancelled = run(&["session", "--dir", dir, "cancel", &id], home.path());
    assert_eq!(
        code(&cancelled),
        0,
        "cancel must succeed once every effect has an operator disposition; stderr: {}",
        String::from_utf8_lossy(&cancelled.stderr)
    );

    // External truth, read by a fresh process: nothing is left interrupted, so
    // the engine's `--continue` refusal no longer applies.
    let state = SessionJournal::recovered_state(sessions.join(format!("{id}.journal"))).unwrap();
    assert!(
        state.turns.values().all(|turn| turn.completion.is_some()),
        "no turn may remain without a terminal completion after the documented path"
    );
    let after = run(&["session", "--dir", dir, "show", &id], home.path());
    assert!(
        stdout(&after).contains("interrupted=0"),
        "a recovered session must not be presented as interrupted again; got:\n{}",
        stdout(&after)
    );
}

/// The operator must still be told what happened to a real side effect before
/// the turn can be declared over. `cancel` disposing of engine bookkeeping is
/// not a licence to bury an unreconciled tool.
#[test]
fn cancel_still_refuses_while_a_tool_effect_has_no_operator_disposition() {
    let home = tempfile::tempdir().unwrap();
    let (sessions, id, _tool_execution_id) = crashed_mid_tool_session(home.path());
    let dir = sessions.to_str().unwrap();

    let cancelled = run(&["session", "--dir", dir, "cancel", &id], home.path());
    assert_eq!(
        code(&cancelled),
        5,
        "cancel must exit 5 while the crashed tool effect is unreconciled; stdout: {} stderr: {}",
        stdout(&cancelled),
        String::from_utf8_lossy(&cancelled.stderr)
    );
    assert!(
        String::from_utf8_lossy(&cancelled.stderr).contains("outstanding reconcile item"),
        "the refusal must name the outstanding item; stderr: {}",
        String::from_utf8_lossy(&cancelled.stderr)
    );
}

/// The refusal must name the way out, with the real identifier substituted.
///
/// Reproduced before this test existed: `session cancel` exited 5 with ZERO
/// bytes on stdout and one stderr line counting items, and the user had no way
/// to learn which command clears them. The product already has the shape this
/// should take — the missing-API-key error names the exact command, the
/// one-off flag and the env vars.
#[test]
fn the_cancel_refusal_prints_the_exact_command_that_clears_it() {
    let home = tempfile::tempdir().unwrap();
    let (sessions, id, tool_execution_id) = crashed_mid_tool_session(home.path());
    let dir = sessions.to_str().unwrap();

    let cancelled = run(&["session", "--dir", dir, "cancel", &id], home.path());
    assert_eq!(code(&cancelled), 5);
    let out = stdout(&cancelled);

    assert!(
        !out.is_empty(),
        "a refusal that prints nothing on stdout cannot be read by the `> log` \
         redirect this surface documents"
    );
    // The remedy, exact and copy-pasteable, carrying the REAL uuid.
    for outcome in ["not-started", "succeeded", "failed"] {
        let expected = format!(
            "wayland-core session reconcile {id} --resolve {tool_execution_id} \
             --as-outcome {outcome}"
        );
        assert!(
            out.contains(&expected),
            "the refusal must print `{expected}`; got:\n{out}"
        );
    }
    // And what each one COMMITS the session to, because the question is one
    // the operator can only guess at.
    assert!(
        out.contains("no output is invented") && out.contains("the work never began"),
        "each disposition must state its consequence; got:\n{out}"
    );
    assert!(
        out.contains(&format!("wayland-core session cancel {id}")),
        "the refusal must name the command to re-run afterwards; got:\n{out}"
    );
    // Unchanged contract: the classifier line stays on stderr and the exit
    // code stays 5.
    assert!(
        String::from_utf8_lossy(&cancelled.stderr).contains("outstanding reconcile item"),
        "stderr: {}",
        String::from_utf8_lossy(&cancelled.stderr)
    );
}

/// `--as-outcome` used to DEFAULT to `not-started`, which answered "did the
/// effect land?" on the operator's behalf and wrote the answer durably. For a
/// tool the journal cannot settle, omitting it must refuse — and the refusal
/// must be the thing that teaches the choice.
#[test]
fn reconcile_refuses_to_guess_for_a_tool_whose_effect_it_cannot_know() {
    let home = tempfile::tempdir().unwrap();
    let (sessions, id, tool_execution_id) = crashed_mid_tool_session(home.path());
    let dir = sessions.to_str().unwrap();

    let guessed = run(
        &[
            "session",
            "--dir",
            dir,
            "reconcile",
            &id,
            "--resolve",
            &tool_execution_id,
        ],
        home.path(),
    );
    assert_eq!(
        code(&guessed),
        4,
        "an unanswerable disposition must be refused, not defaulted; stdout: {} stderr: {}",
        stdout(&guessed),
        String::from_utf8_lossy(&guessed.stderr)
    );
    let refusal = String::from_utf8_lossy(&guessed.stderr).into_owned();
    for outcome in ["not-started", "succeeded", "failed"] {
        assert!(
            refusal.contains(outcome),
            "the refusal must explain `{outcome}`; got:\n{refusal}"
        );
    }
    assert!(
        refusal.contains("only you can say what happened"),
        "the refusal must say plainly that the product does not know; got:\n{refusal}"
    );

    // The durable state is untouched: refusing wrote nothing.
    let after = run(&["session", "--dir", dir, "show", &id], home.path());
    assert!(
        stdout(&after).contains("interrupted=1"),
        "a refusal must not have changed the session; got:\n{}",
        stdout(&after)
    );
}

/// The other half: where the journal DOES know, it must not ask.
///
/// A tool that declares `RepeatSafe` cannot have created an external effect,
/// so there is nothing for an operator to have an opinion about — and the
/// four-command recovery collapses to one.
#[test]
fn a_repeat_safe_tool_is_settled_by_cancel_alone_in_one_command() {
    use wcore_agent::session_journal::SessionJournal;

    let home = tempfile::tempdir().unwrap();
    let (sessions, id, tool_execution_id) = crashed_mid_tool_session_with(
        home.path(),
        "Grep",
        // Exactly what the real `Grep` declares: the repeat-safe kind AND the
        // registered reconciler that certifies it. The kind alone buys
        // nothing — see `determined_disposition`.
        wcore_types::tool::repeat_safe_contract(wcore_types::tool::READ_ONLY_FILESYSTEM_RECONCILER),
    );
    let dir = sessions.to_str().unwrap();

    // ONE command. No reconcile list, no --resolve, no --as-outcome.
    let cancelled = run(&["session", "--dir", dir, "cancel", &id], home.path());
    assert_eq!(
        code(&cancelled),
        0,
        "a repeat-safe effect needs no human judgement; stdout: {} stderr: {}",
        stdout(&cancelled),
        String::from_utf8_lossy(&cancelled.stderr)
    );
    let out = stdout(&cancelled);
    assert!(
        out.contains(&format!(
            "F23_SESSION=cancel_auto_resolved id={id} kind=tool_execution \
             ref={tool_execution_id} tool=Grep determined_by=repeat_safe_effect_contract"
        )),
        "every receipt written on the operator's behalf must be reported; got:\n{out}"
    );
    assert!(
        out.contains("auto_resolved=1"),
        "the summary must count it; got:\n{out}"
    );

    // External truth, read by a fresh process.
    let state = SessionJournal::recovered_state(sessions.join(format!("{id}.journal"))).unwrap();
    assert!(
        state.turns.values().all(|turn| turn.completion.is_some()),
        "no turn may remain interrupted after the one-command path"
    );
    assert!(
        stdout(&run(&["session", "--dir", dir, "show", &id], home.path()))
            .contains("interrupted=0")
    );
}

/// The gate on the line above: it is the reconciler NAME that buys the
/// automatic receipt, not the repeat-safe kind.
///
/// Same crash, same kind, one character different in the identifier — and the
/// product must go back to asking. Without this the two arms are
/// indistinguishable, because both use a name the build happens to know, and
/// `determined_disposition` could check nothing at all and still pass. Then
/// any tool — a plugin, an MCP proxy from a server that made the name up —
/// could have `session cancel` write "no external effect is possible" into the
/// journal on its behalf.
#[test]
fn a_repeat_safe_kind_with_a_reconciler_this_build_does_not_know_is_still_asked_about() {
    let registered = wcore_types::tool::READ_ONLY_FILESYSTEM_RECONCILER;
    assert!(
        wcore_types::tool::repeat_safe_reconciler_is_registered(registered),
        "positive control: the name the arm above uses IS registered"
    );
    let invented = format!("{registered}-but-invented");
    assert!(!wcore_types::tool::repeat_safe_reconciler_is_registered(
        &invented
    ));

    let home = tempfile::tempdir().unwrap();
    let (sessions, id, tool_execution_id) = crashed_mid_tool_session_with(
        home.path(),
        "Grep",
        wcore_types::tool::ToolEffectContract {
            kind: wcore_types::tool::ToolEffectKind::RepeatSafe,
            reconciler: Some(invented),
        },
    );
    let dir = sessions.to_str().unwrap();

    let cancelled = run(&["session", "--dir", dir, "cancel", &id], home.path());
    assert_eq!(
        code(&cancelled),
        5,
        "an unrecognised reconciler must leave the item outstanding; stdout: {} stderr: {}",
        stdout(&cancelled),
        String::from_utf8_lossy(&cancelled.stderr)
    );
    assert!(
        String::from_utf8_lossy(&cancelled.stderr).contains("outstanding reconcile item"),
        "stderr: {}",
        String::from_utf8_lossy(&cancelled.stderr)
    );
    assert!(
        !stdout(&cancelled).contains("cancel_auto_resolved"),
        "nothing may be resolved on the operator's behalf here; got:\n{}",
        stdout(&cancelled)
    );

    // And the manual verb refuses just the same rather than defaulting.
    let guessed = run(
        &[
            "session",
            "--dir",
            dir,
            "reconcile",
            &id,
            "--resolve",
            &tool_execution_id,
        ],
        home.path(),
    );
    assert_eq!(
        code(&guessed),
        4,
        "an unrecognised reconciler is not an answer; stdout: {} stderr: {}",
        stdout(&guessed),
        String::from_utf8_lossy(&guessed.stderr)
    );
    assert!(
        String::from_utf8_lossy(&guessed.stderr).contains("only you can say what happened"),
        "stderr: {}",
        String::from_utf8_lossy(&guessed.stderr)
    );

    // The refusal wrote nothing.
    assert!(
        stdout(&run(&["session", "--dir", dir, "show", &id], home.path()))
            .contains("interrupted=1")
    );
}
