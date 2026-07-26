//! F23-02 — Success Criterion 2 verbs, driven against the compiled binary.
//!
//! Layer 1 of the established harness split (see `harness_cli_surface.rs`):
//! spawn the real `wayland-core` binary as a subprocess, assert on its exit
//! code and its STDOUT tokens. Nothing here calls a provider, so no API key is
//! needed — which is itself part of the contract, because a first-run user must
//! be able to list and search sessions.
//!
//! Two cases are hostile and are the reason this file exists rather than being
//! folded into the unit tests:
//!
//! * `rewind_refuses_a_destination_outside_the_workspace_root` hand-authors a
//!   `meta.json` naming a path outside the root. No legitimate capture produces
//!   one, so the fixture must be built by hand. Before Phase 23B the store
//!   silently *skipped* such an entry and applied the rest — a partial rewind
//!   with no operator signal.
//! * `cancel_makes_a_crash_interrupted_session_resumable` reproduces live
//!   Windows UAT defect D2 through the shipped surface.

use std::path::Path;
use std::process::{Command, Output};

use tempfile::TempDir;

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

/// A session directory holding one persisted session, built with the same
/// `SessionManager` the product uses so the fixture is a real session file
/// rather than a hand-authored struct.
struct Fixture {
    home: TempDir,
    sessions: std::path::PathBuf,
    id: String,
}

fn fixture(text: &str) -> Fixture {
    use wcore_agent::session::SessionManager;
    use wcore_types::message::{ContentBlock, Message, Role};

    let home = tempfile::tempdir().expect("tempdir");
    let sessions = home.path().join("sessions");
    std::fs::create_dir_all(&sessions).expect("session dir");
    let manager = SessionManager::new(sessions.clone(), 50);
    let mut session = manager
        .create("anthropic", "test-model", "/tmp", None)
        .expect("create");
    session.messages.push(Message::new(
        Role::User,
        vec![ContentBlock::Text {
            text: text.to_owned(),
        }],
    ));
    manager.persist_first_message(&session).expect("persist");
    manager.save(&session).expect("save");
    Fixture {
        id: session.id.clone(),
        home,
        sessions,
    }
}

impl Fixture {
    fn dir(&self) -> &str {
        self.sessions.to_str().expect("utf-8 session dir")
    }
    fn home(&self) -> &Path {
        self.home.path()
    }
}

#[test]
fn the_session_subcommand_is_reachable_on_the_shipped_binary() {
    let home = tempfile::tempdir().unwrap();
    let output = run(&["session", "--help"], home.path());
    assert_eq!(
        code(&output),
        0,
        "`session --help` must succeed; stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let help = stdout(&output);
    for verb in [
        "list",
        "search",
        "show",
        "checkpoint",
        "rewind",
        "retry",
        "fork",
        "export",
        "retain",
        "reconcile",
        "cancel",
    ] {
        assert!(
            help.contains(verb),
            "`session --help` must advertise the `{verb}` verb; got:\n{help}"
        );
    }
}

#[test]
fn list_and_search_work_without_a_provider_api_key() {
    let f = fixture("the aardvark ate the mango");

    let listed = run(&["session", "--dir", f.dir(), "list"], f.home());
    assert_eq!(code(&listed), 0);
    let out = stdout(&listed);
    assert!(
        out.contains(&format!("F23_SESSION=list id={}", f.id)),
        "list must print the session id to STDOUT; got:\n{out}"
    );
    assert!(out.contains("F23_SESSION=list_total count=1"));

    let hit = run(
        &["session", "--dir", f.dir(), "search", "aardvark"],
        f.home(),
    );
    assert_eq!(code(&hit), 0);
    assert!(stdout(&hit).contains(&format!("F23_SESSION=search id={}", f.id)));

    let miss = run(
        &["session", "--dir", f.dir(), "search", "zzz-absent-term"],
        f.home(),
    );
    assert_eq!(
        code(&miss),
        0,
        "a term matching nothing is a successful empty result"
    );
    let miss_out = stdout(&miss);
    assert!(
        miss_out.contains("count=0"),
        "the total line must still be emitted so a driver can tell 'found nothing' from 'did not run'; got:\n{miss_out}"
    );
}

#[test]
fn show_of_an_absent_session_exits_with_the_not_found_code() {
    let home = tempfile::tempdir().unwrap();
    let dir = home.path().join("sessions");
    std::fs::create_dir_all(&dir).unwrap();
    let output = run(
        &[
            "session",
            "--dir",
            dir.to_str().unwrap(),
            "show",
            "no-such-session",
        ],
        home.path(),
    );
    assert_eq!(
        code(&output),
        3,
        "not-found must map to exit 3, distinctly from generic failure"
    );
}

#[test]
fn fork_reports_the_parent_as_unchanged_and_the_child_carries_lineage() {
    let f = fixture("parent content");
    let forked = run(&["session", "--dir", f.dir(), "fork", &f.id], f.home());
    assert_eq!(code(&forked), 0);
    let out = stdout(&forked);
    assert!(
        out.contains("parent_unchanged=true"),
        "fork must leave the parent's bytes untouched; got:\n{out}"
    );

    let child_id = out
        .split_whitespace()
        .find_map(|token| token.strip_prefix("child="))
        .expect("fork must print the child id")
        .to_owned();
    let shown = run(&["session", "--dir", f.dir(), "show", &child_id], f.home());
    assert_eq!(code(&shown), 0);
    assert!(
        stdout(&shown).contains(&format!("parent={}", f.id)),
        "the child must record its lineage parent"
    );
}

#[test]
fn export_omits_a_run_time_nonce_planted_in_the_session() {
    // Generated here, at run time: no shape-matching filter could target it.
    let nonce = format!("nonce{}", uuid::Uuid::new_v4().simple());
    let f = fixture(&format!("secret {nonce} end"));

    // Prove the nonce is genuinely in the stored session first. An absence
    // that was always absent proves nothing.
    let stored: String = std::fs::read_dir(&f.sessions)
        .unwrap()
        .filter_map(|e| e.ok())
        .filter(|e| e.path().extension().is_some_and(|x| x == "json"))
        .map(|e| std::fs::read_to_string(e.path()).unwrap_or_default())
        .collect();
    assert!(
        stored.contains(&nonce),
        "the fixture must contain the nonce before export is tested"
    );

    let out_path = f.home().join("export.json");
    let exported = run(
        &[
            "session",
            "--dir",
            f.dir(),
            "export",
            &f.id,
            "--out",
            out_path.to_str().unwrap(),
        ],
        f.home(),
    );
    assert_eq!(code(&exported), 0);
    let bytes = std::fs::read_to_string(&out_path).expect("export file");
    assert!(
        !bytes.contains(&nonce),
        "the export envelope must not carry session free text"
    );
    assert!(
        bytes.contains("\"source_session_id\""),
        "the envelope must carry provenance"
    );
}

#[test]
fn retain_reports_an_expired_bound_without_deleting_the_session() {
    let f = fixture("hello");
    let future = run(
        &["session", "--dir", f.dir(), "retain", &f.id, "--days", "7"],
        f.home(),
    );
    assert_eq!(code(&future), 0);
    assert!(stdout(&future).contains("retained"));

    let past = run(
        &["session", "--dir", f.dir(), "retain", &f.id, "--days", "-7"],
        f.home(),
    );
    assert_eq!(code(&past), 0);
    assert!(
        stdout(&past).contains("expired"),
        "a bound in the past must report expired"
    );

    let still_there = run(&["session", "--dir", f.dir(), "show", &f.id], f.home());
    assert_eq!(
        code(&still_there),
        0,
        "an expired session is reported, never silently deleted"
    );
}

#[test]
fn checkpoint_then_rewind_restores_bytes_and_removes_a_later_file() {
    let f = fixture("hello");
    let workspace = f.home().join("ws");
    std::fs::create_dir_all(&workspace).unwrap();
    let tracked = workspace.join("tracked.txt");
    let later = workspace.join("created-after.txt");
    std::fs::write(&tracked, b"original bytes\n").unwrap();

    let captured = run(
        &[
            "session",
            "--dir",
            f.dir(),
            "--workspace",
            workspace.to_str().unwrap(),
            "checkpoint",
            tracked.to_str().unwrap(),
            later.to_str().unwrap(),
        ],
        f.home(),
    );
    assert_eq!(
        code(&captured),
        0,
        "checkpoint failed: {}",
        String::from_utf8_lossy(&captured.stderr)
    );
    let checkpoint_id = stdout(&captured)
        .split_whitespace()
        .find_map(|t| t.strip_prefix("id="))
        .expect("checkpoint must print its id")
        .to_owned();

    // Mutate the tracked file and create a file that did not exist at capture.
    std::fs::write(&tracked, b"mutated bytes\n").unwrap();
    std::fs::write(&later, b"should be gone after rewind\n").unwrap();

    let restored = run(
        &[
            "session",
            "--dir",
            f.dir(),
            "--workspace",
            workspace.to_str().unwrap(),
            "rewind",
            &checkpoint_id,
        ],
        f.home(),
    );
    assert_eq!(
        code(&restored),
        0,
        "rewind failed: {}",
        String::from_utf8_lossy(&restored.stderr)
    );
    assert_eq!(
        std::fs::read(&tracked).unwrap(),
        b"original bytes\n",
        "rewind must restore byte-identical content"
    );
    assert!(
        !later.exists(),
        "a file created after the checkpoint must be gone after the restore"
    );
}

#[test]
fn rewind_refuses_a_destination_outside_the_workspace_root() {
    let f = fixture("hello");
    let workspace = f.home().join("ws");
    std::fs::create_dir_all(&workspace).unwrap();
    let inside = workspace.join("inside.txt");
    std::fs::write(&inside, b"inside original\n").unwrap();

    // Take a legitimate checkpoint so the blob layout is real, then poison the
    // recorded destination by hand — no legitimate capture produces one.
    let captured = run(
        &[
            "session",
            "--dir",
            f.dir(),
            "--workspace",
            workspace.to_str().unwrap(),
            "checkpoint",
            inside.to_str().unwrap(),
        ],
        f.home(),
    );
    assert_eq!(code(&captured), 0);
    let checkpoint_id = stdout(&captured)
        .split_whitespace()
        .find_map(|t| t.strip_prefix("id="))
        .expect("checkpoint id")
        .to_owned();

    let victim = f.home().join("outside-the-root.txt");
    std::fs::write(&victim, b"must not be overwritten\n").unwrap();

    let meta_path = f
        .sessions
        .join("checkpoints")
        .join(&checkpoint_id)
        .join("meta.json");
    let raw = std::fs::read_to_string(&meta_path).expect("meta.json");
    let poisoned = raw.replace(
        &inside.to_string_lossy().replace('\\', "\\\\").to_string(),
        &victim.to_string_lossy().replace('\\', "\\\\").to_string(),
    );
    assert_ne!(raw, poisoned, "the hostile fixture must actually differ");
    std::fs::write(&meta_path, poisoned).unwrap();

    // Also mutate the in-root file so a successful restore would be visible.
    std::fs::write(&inside, b"mutated\n").unwrap();

    let refused = run(
        &[
            "session",
            "--dir",
            f.dir(),
            "--workspace",
            workspace.to_str().unwrap(),
            "rewind",
            &checkpoint_id,
        ],
        f.home(),
    );
    assert_eq!(
        code(&refused),
        4,
        "an escaping destination is an authority refusal (exit 4); stderr: {}",
        String::from_utf8_lossy(&refused.stderr)
    );
    assert_eq!(
        std::fs::read(&victim).unwrap(),
        b"must not be overwritten\n",
        "the refused restore must write nothing outside the root"
    );
}

#[test]
fn cancel_makes_a_crash_interrupted_session_resumable() {
    use wcore_agent::session::SessionManager;
    use wcore_agent::session_journal::{SessionEvent, SessionJournal};

    let home = tempfile::tempdir().unwrap();
    let sessions = home.path().join("sessions");
    std::fs::create_dir_all(&sessions).unwrap();
    let manager = SessionManager::new(sessions.clone(), 50);
    let mut active = manager
        .create_for_run("anthropic", "test-model", "/tmp", None)
        .unwrap();
    let id = active.session.id.clone();
    // A real interrupted session has been written to disk: the engine persists
    // the first user message before any provider call (F-030). Without this the
    // session has a journal but no index entry, which is a different — and
    // rarer — shape than the one defect D2 describes.
    active
        .session
        .messages
        .push(wcore_types::message::Message::new(
            wcore_types::message::Role::User,
            vec![wcore_types::message::ContentBlock::Text {
                text: "do a thing".to_owned(),
            }],
        ));
    manager.persist_first_message(&active.session).unwrap();
    active
        .journal
        .append(SessionEvent::TurnStarted {
            turn_id: "turn-1".to_owned(),
            user_message: "do a thing".to_owned(),
        })
        .unwrap();
    // Drop the writer lease without a terminal transition — exactly what a
    // crash leaves behind, and exactly the state that makes `--continue` refuse
    // with "resume, reconcile, or cancel" (engine.rs:6059).
    drop(active);

    let dir = sessions.to_str().unwrap();

    let before = run(&["session", "--dir", dir, "show", &id], home.path());
    assert_eq!(code(&before), 0);
    assert!(
        stdout(&before).contains("interrupted=1"),
        "the fixture must genuinely present one interrupted turn; got:\n{}",
        stdout(&before)
    );

    let cancelled = run(&["session", "--dir", dir, "cancel", &id], home.path());
    assert_eq!(
        code(&cancelled),
        0,
        "cancel failed: {}",
        String::from_utf8_lossy(&cancelled.stderr)
    );
    assert!(stdout(&cancelled).contains("F23_SESSION=cancel_turn"));
    assert!(stdout(&cancelled).contains("cancelled=1"));

    // The disposition must survive a fresh process reading the journal again.
    let after = run(&["session", "--dir", dir, "show", &id], home.path());
    assert!(
        stdout(&after).contains("interrupted=0"),
        "a cancelled turn must not be presented as interrupted again; got:\n{}",
        stdout(&after)
    );

    // And the recovery planner must now agree the session is Ready, which is
    // what makes `--continue` stop refusing.
    let state = SessionJournal::recovered_state(sessions.join(format!("{id}.journal"))).unwrap();
    assert!(
        state.turns.values().all(|t| t.completion.is_some()),
        "no turn may remain without a terminal completion after cancel"
    );
}

#[test]
fn reconcile_lists_nothing_for_a_clean_session_and_exits_zero() {
    let f = fixture("hello");
    let output = run(&["session", "--dir", f.dir(), "reconcile", &f.id], f.home());
    assert_eq!(code(&output), 0);
    assert!(stdout(&output).contains("outstanding=0"));
}

#[test]
fn retry_of_an_unknown_turn_exits_not_found() {
    let f = fixture("hello");
    let output = run(
        &["session", "--dir", f.dir(), "retry", &f.id, "turn-absent"],
        f.home(),
    );
    assert_eq!(code(&output), 3);
}

#[test]
fn a_corrupt_session_file_exits_non_zero_naming_the_file() {
    let f = fixture("hello");
    let path = std::fs::read_dir(&f.sessions)
        .unwrap()
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .find(|p| {
            p.extension().is_some_and(|x| x == "json")
                && p.file_name().is_some_and(|n| n != "index.json")
        })
        .expect("session file");
    std::fs::write(&path, b"{ not json").unwrap();

    let output = run(&["session", "--dir", f.dir(), "show", &f.id], f.home());
    assert_ne!(code(&output), 0, "a corrupt session must not exit zero");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains(path.file_name().unwrap().to_str().unwrap()),
        "the error must name the offending file; got: {stderr}"
    );
}
