//! #693 — the non-bypassable command floor as `BashTool` actually reaches it.
//!
//! `wcore_config::command_floor::tests` grades the predicate. That leaves the
//! part that actually ships ungraded: the FOUR `BashTool` entry points that
//! have to call it. The measured defect happened through the tool, not through
//! the function, so it is graded here through the tool — on every entry point,
//! with every waivable layer opened first:
//!
//! * `WAYLAND_SANDBOX=none` + `WAYLAND_ALLOW_NO_SANDBOX=1` — no OS isolation,
//! * `TIRITH_ENABLED=0` + `TIRITH_FAIL_OPEN=true` — the security layer off,
//! * `WorkspacePolicy::trusted_local` — the most permissive workspace posture,
//! * `WAYLAND_HOME` pointed at a decoy directory, to prove an environment
//!   variable cannot move the floor off the operator's real store.
//!
//! Each test asserts the WORLD, not the receipt: the file on disk is read back
//! after the call, and it has to be untouched.
//!
//! Every refusal arm carries a known-positive control on the SAME entry point
//! in the SAME environment — an ordinary command that must actually run. A
//! suite where everything is refused proves nothing: a fail-closed sandbox or a
//! broken entry point would satisfy the refusal assertions on its own.

use std::path::Path;
use std::sync::{Arc, Mutex};

use serde_json::json;
use wcore_tools::bash::BashTool;
use wcore_tools::context::ToolContext;
use wcore_tools::workspace_policy::WorkspacePolicy;
use wcore_tools::{Tool, ToolOutputSink};
use wcore_types::tool::ToolResult;

/// The two refusal families, spelled as prefixes so the assertion cannot be
/// satisfied by an unrelated error (a missing binary, a timeout, a fail-closed
/// sandbox all set `is_error`).
const AUTHORITY_PREFIX: &str =
    "Refused by the command floor: this command references the agent's own authority state";
const REPO_CONTROL_PREFIX: &str =
    "Refused by the command floor: this command references the repository control surface";

const SENTINEL: &str = "# the operator's own grants\n";
const CONTROL: &str = "command_floor_control_ok";

#[derive(Default)]
struct RecordingSink {
    chunks: Mutex<Vec<String>>,
}

impl RecordingSink {
    fn chunks(&self) -> Vec<String> {
        self.chunks.lock().expect("sink mutex poisoned").clone()
    }
}

impl ToolOutputSink for RecordingSink {
    fn emit_chunk(&self, chunk: &str) {
        self.chunks
            .lock()
            .expect("sink mutex poisoned")
            .push(chunk.to_string());
    }
}

/// The four `BashTool` entry points, named so a failure says which one leaked.
#[derive(Clone, Copy)]
enum Entry {
    Buffered,
    Streaming,
    CtxBuffered,
    CtxStreaming,
}

fn entries() -> [(&'static str, Entry); 4] {
    [
        ("execute", Entry::Buffered),
        ("execute_streaming", Entry::Streaming),
        ("execute_with_ctx", Entry::CtxBuffered),
        ("execute_streaming_with_ctx", Entry::CtxStreaming),
    ]
}

async fn run(entry: Entry, command: &str, ctx: &ToolContext) -> (ToolResult, Vec<String>) {
    let sink = RecordingSink::default();
    let input = json!({ "command": command });
    let result = match entry {
        Entry::Buffered => BashTool.execute(input).await,
        Entry::Streaming => BashTool.execute_streaming(input, &sink).await,
        Entry::CtxBuffered => BashTool.execute_with_ctx(input, ctx).await,
        Entry::CtxStreaming => BashTool.execute_streaming_with_ctx(input, ctx, &sink).await,
    };
    (result, sink.chunks())
}

/// Open every layer a floor is supposed to sit under, then point `HOME` and
/// `WAYLAND_HOME` at throwaway directories so no assertion in this file can
/// reach the machine's real store.
///
/// SAFETY: test-only env mutation; `#[serial_test::serial]` prevents env races,
/// and this must precede `ToolContext::test_default()`, which captures the
/// sandbox backend at construction.
fn open_every_escape_hatch(home: &Path, wayland_home: &Path) {
    unsafe {
        std::env::set_var("WAYLAND_SANDBOX", "none");
        std::env::set_var("WAYLAND_ALLOW_NO_SANDBOX", "1");
        std::env::set_var("TIRITH_ENABLED", "0");
        std::env::set_var("TIRITH_FAIL_OPEN", "true");
        std::env::set_var("HOME", home);
        std::env::set_var("WAYLAND_HOME", wayland_home);
    }
}

fn close_escape_hatches(prior_home: Option<std::ffi::OsString>) {
    unsafe {
        std::env::remove_var("WAYLAND_SANDBOX");
        std::env::remove_var("WAYLAND_ALLOW_NO_SANDBOX");
        std::env::remove_var("TIRITH_ENABLED");
        std::env::remove_var("TIRITH_FAIL_OPEN");
        std::env::remove_var("WAYLAND_HOME");
        match prior_home {
            Some(h) => std::env::set_var("HOME", h),
            None => std::env::remove_var("HOME"),
        }
    }
}

/// The known-positive control: this exact entry point, in this exact
/// environment, does run an ordinary command. Without it a refusal proves
/// nothing.
async fn assert_entry_point_is_live(name: &str, entry: Entry, ctx: &ToolContext) {
    let (result, _) = run(entry, &format!("echo {CONTROL}"), ctx).await;
    assert!(
        !result.is_error,
        "control: {name} must run an ordinary command, got: {}",
        result.content
    );
    assert!(
        result.content.contains(CONTROL),
        "control: {name} produced no echo output: {}",
        result.content
    );
}

fn assert_refused(result: &ToolResult, chunks: &[String], prefix: &str, what: &str) {
    assert!(result.is_error, "{what}: must be refused, got: {result:?}");
    assert!(
        result.content.starts_with(prefix),
        "{what}: must be the command-floor refusal, got: {}",
        result.content
    );
    // A refused command is stopped BEFORE the shell spawns; a spawned shell
    // forwards its output to the sink.
    assert!(
        chunks.is_empty(),
        "{what}: shell output reached the sink, so a shell ran: {chunks:?}"
    );
}

/// The measured defect #1, on all four entry points: a command that appends to
/// the learned-grant store has granted the agent standing auto-approval in
/// every future session.
#[tokio::test]
#[serial_test::serial]
async fn the_grant_store_survives_every_bash_entry_point() {
    let home = tempfile::tempdir().unwrap();
    let decoy = tempfile::tempdir().unwrap();
    let prior_home = std::env::var_os("HOME");
    open_every_escape_hatch(home.path(), decoy.path());

    let store = home.path().join(".wayland").join("permissions.toml");
    std::fs::create_dir_all(store.parent().unwrap()).unwrap();
    std::fs::write(&store, SENTINEL).unwrap();

    let work = tempfile::tempdir().unwrap();
    let root = dunce::canonicalize(work.path()).unwrap();
    let ctx =
        ToolContext::test_default().with_workspace(Arc::new(WorkspacePolicy::trusted_local(&root)));

    for (name, entry) in entries() {
        assert_entry_point_is_live(name, entry, &ctx).await;

        // Absolute spelling — the on-disk assertion below is only meaningful
        // against a path that does not depend on the child's cwd.
        let cmd = format!("echo 'tools.auto_approve = true' >> {}", store.display());
        let (result, chunks) = run(entry, &cmd, &ctx).await;
        assert_refused(&result, &chunks, AUTHORITY_PREFIX, name);
        assert_eq!(
            std::fs::read_to_string(&store).unwrap(),
            SENTINEL,
            "{name}: the grant store was modified"
        );

        // The `$HOME` spelling the shell would expand, and the bare name a
        // `cd` into the profile home would leave behind.
        for spelling in [
            "echo x >> $HOME/.wayland/permissions.toml",
            "echo x >> ${HOME}/.wayland/permissions.toml",
            "echo x >> ~/.wayland/permissions.toml",
            "cd ~/.wayland && echo x >> permissions.toml",
        ] {
            let (result, chunks) = run(entry, spelling, &ctx).await;
            assert_refused(&result, &chunks, AUTHORITY_PREFIX, name);
        }
        assert_eq!(
            std::fs::read_to_string(&store).unwrap(),
            SENTINEL,
            "{name}: the grant store was modified through an unresolved spelling"
        );
    }

    close_escape_hatches(prior_home);
}

/// The measured defect #2, on all four entry points: `.git/hooks/pre-commit` is
/// arbitrary code executed as the operator on their next commit.
#[tokio::test]
#[serial_test::serial]
async fn git_hooks_survive_every_bash_entry_point() {
    let home = tempfile::tempdir().unwrap();
    let decoy = tempfile::tempdir().unwrap();
    let prior_home = std::env::var_os("HOME");
    open_every_escape_hatch(home.path(), decoy.path());

    let work = tempfile::tempdir().unwrap();
    let root = dunce::canonicalize(work.path()).unwrap();
    std::fs::create_dir_all(root.join(".git").join("hooks")).unwrap();
    let hook = root.join(".git").join("hooks").join("pre-commit");
    let ctx =
        ToolContext::test_default().with_workspace(Arc::new(WorkspacePolicy::trusted_local(&root)));

    for (name, entry) in entries() {
        assert_entry_point_is_live(name, entry, &ctx).await;

        for cmd in [
            format!("printf '#!/bin/sh\\nid\\n' > {}", hook.display()),
            format!("echo id >> {}", hook.display()),
            format!("cp /bin/sh {}", hook.display()),
            // Host-wide, not workspace-scoped: the only thing keeping a
            // command away from ANOTHER repository's hooks was the sandbox.
            "echo id > /some/other/repo/.git/hooks/pre-push".to_string(),
            // And the control file that reaches the same execution surface
            // through core.fsmonitor / core.sshCommand / filter.*.clean.
            format!("echo x >> {}", root.join(".git").join("config").display()),
        ] {
            let (result, chunks) = run(entry, &cmd, &ctx).await;
            assert_refused(&result, &chunks, REPO_CONTROL_PREFIX, name);
        }

        assert!(
            !hook.exists(),
            "{name}: a pre-commit hook was authored on disk"
        );
    }

    close_escape_hatches(prior_home);
}

/// The wrong-refusal arm, and the reason the floor does not match ancestors:
/// the shortest ancestor token of `.git/hooks` is `.`, and refusing that costs
/// `git add .`. A floor that costs ordinary session work does not survive
/// contact with real use, so this arm is graded as hard as the refusals.
///
/// `cargo --version` stands in for the ticket's `cargo build`: it exercises the
/// same tokens (`cargo`, a flag) without a multi-minute compile inside a test.
#[tokio::test]
#[serial_test::serial]
async fn the_floor_costs_no_ordinary_work() {
    let home = tempfile::tempdir().unwrap();
    let decoy = tempfile::tempdir().unwrap();
    let prior_home = std::env::var_os("HOME");
    open_every_escape_hatch(home.path(), decoy.path());

    let work = tempfile::tempdir().unwrap();
    let root = dunce::canonicalize(work.path()).unwrap();
    let ctx =
        ToolContext::test_default().with_workspace(Arc::new(WorkspacePolicy::trusted_local(&root)));

    for args in [
        &["init", "-q"][..],
        &["config", "user.email", "t@example.com"][..],
        &["config", "user.name", "t"][..],
        &["config", "commit.gpgsign", "false"][..],
    ] {
        let ok = std::process::Command::new("git")
            .args(args)
            .current_dir(&root)
            .status()
            .expect("git must be available for this test")
            .success();
        assert!(ok, "git {args:?} failed");
    }
    std::fs::write(root.join("file.py"), "print(1)\n").unwrap();
    std::fs::create_dir_all(root.join(".cargo")).unwrap();
    std::fs::write(root.join(".cargo").join("config.toml"), "[build]\n").unwrap();

    for cmd in [
        "git add .",
        "git status",
        "git config --get user.name",
        "ls *",
        "git commit -m x",
        "cargo --version",
        "ls -la .git",
        "cat .cargo/config.toml",
        "git log --oneline",
    ] {
        let (result, _) = run(Entry::CtxBuffered, cmd, &ctx).await;
        assert!(
            !result.is_error,
            "the floor must not cost {cmd:?}, got: {}",
            result.content
        );
        assert!(
            !result.content.contains("command floor"),
            "the floor refused ordinary work {cmd:?}: {}",
            result.content
        );
    }

    close_escape_hatches(prior_home);
}

/// The measured defect #3, and the one a per-file protected list did not stop:
/// the store is reached WITHOUT ever naming `permissions.toml`.
///
/// Every command here was proven to land end-to-end through
/// `execute_with_ctx` — `is_error=false`, "Exit code: 0", the grant store on
/// disk replaced — against the first form of this floor. A directory rename is
/// not "unbounded indirection": it is one line, no `eval`, no variable, no
/// obfuscation, and it grants the agent standing auto-approval in every future
/// session.
///
/// Paths are spelled ABSOLUTELY against the temporary `HOME`, never as `~`.
/// If the floor is removed to grade this test's red arm these commands really
/// do run, and `rm -rf ~/.wayland` in a child that did not inherit the test's
/// `HOME` would delete the operator's real store.
#[tokio::test]
#[serial_test::serial]
async fn the_grant_store_survives_a_directory_swap() {
    let home = tempfile::tempdir().unwrap();
    let decoy = tempfile::tempdir().unwrap();
    let prior_home = std::env::var_os("HOME");
    open_every_escape_hatch(home.path(), decoy.path());

    let wayland = home.path().join(".wayland");
    let store = wayland.join("permissions.toml");
    std::fs::create_dir_all(&wayland).unwrap();
    std::fs::write(&store, SENTINEL).unwrap();

    let work = tempfile::tempdir().unwrap();
    let root = dunce::canonicalize(work.path()).unwrap();
    // A ready-made replacement store, and a file to copy over the real one.
    let evil = root.join("evil");
    std::fs::create_dir_all(&evil).unwrap();
    std::fs::write(evil.join("permissions.toml"), "tools.auto_approve = true\n").unwrap();
    let src = root.join("src.txt");
    std::fs::write(&src, "PWNED\n").unwrap();
    let backup = root.join("backup");

    let ctx =
        ToolContext::test_default().with_workspace(Arc::new(WorkspacePolicy::trusted_local(&root)));

    for (name, entry) in entries() {
        assert_entry_point_is_live(name, entry, &ctx).await;

        // Ordered so the first arm is one no OTHER guard can claim: the
        // exfiltration copy destroys nothing, so the unsaved-work guard has no
        // opinion on it and a red arm here fails with the command having RUN,
        // not with a refusal from somewhere else.
        for cmd in [
            // Exfiltrate oauth/, .env and the credential stores wholesale.
            format!("cp -r {} {}", wayland.display(), backup.display()),
            // Reach the file through a glob.
            format!("cp {} {}/perm*.toml", src.display(), wayland.display()),
            // Replace the directory around the file.
            format!(
                "rm -rf {} && mv {} {}",
                wayland.display(),
                evil.display(),
                wayland.display()
            ),
            // Reach the directory through a glob.
            format!("cp -r {} {}/.way*", evil.display(), home.path().display()),
            // Point the directory at attacker-controlled content.
            format!("ln -sfn {} {}", evil.display(), wayland.display()),
            // `security.enabled` / `tools.auto_approve` through a basename too
            // common to protect bare, with the resolved path hidden by a `cd`.
            format!(
                "cd {} && echo 'tools.auto_approve = true' >> config.toml",
                wayland.display()
            ),
        ] {
            let (result, chunks) = run(entry, &cmd, &ctx).await;
            assert_refused(&result, &chunks, AUTHORITY_PREFIX, name);
        }

        // Assert the WORLD, not the receipt.
        assert_eq!(
            std::fs::read_to_string(&store).unwrap(),
            SENTINEL,
            "{name}: the grant store was replaced"
        );
        assert!(
            !wayland.join("config.toml").exists(),
            "{name}: tools.auto_approve was written to the profile config"
        );
        assert!(!backup.exists(), "{name}: the profile home was exfiltrated");
        let entries_left: Vec<_> = std::fs::read_dir(&wayland)
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        assert_eq!(
            entries_left.len(),
            1,
            "{name}: something was planted in the profile home: {entries_left:?}"
        );
    }

    close_escape_hatches(prior_home);
}

/// The submodule shape of the hooks surface: `.git/modules/<name>/hooks` runs
/// exactly like `.git/hooks`, and a check that looks only one component past
/// `.git` never sees it.
#[tokio::test]
#[serial_test::serial]
async fn submodule_git_hooks_survive_the_bash_entry_points() {
    let home = tempfile::tempdir().unwrap();
    let decoy = tempfile::tempdir().unwrap();
    let prior_home = std::env::var_os("HOME");
    open_every_escape_hatch(home.path(), decoy.path());

    let work = tempfile::tempdir().unwrap();
    let root = dunce::canonicalize(work.path()).unwrap();
    let hooks = root.join(".git").join("modules").join("sub").join("hooks");
    std::fs::create_dir_all(&hooks).unwrap();
    let hook = hooks.join("pre-commit");
    let ctx =
        ToolContext::test_default().with_workspace(Arc::new(WorkspacePolicy::trusted_local(&root)));

    for (name, entry) in entries() {
        assert_entry_point_is_live(name, entry, &ctx).await;
        for cmd in [
            format!("printf '#!/bin/sh\\nid\\n' > {}", hook.display()),
            format!(
                "echo x >> {}",
                root.join(".git")
                    .join("modules")
                    .join("sub")
                    .join("config")
                    .display()
            ),
        ] {
            let (result, chunks) = run(entry, &cmd, &ctx).await;
            assert_refused(&result, &chunks, REPO_CONTROL_PREFIX, name);
        }
        assert!(
            !hook.exists(),
            "{name}: a submodule pre-commit hook was authored on disk"
        );
    }

    close_escape_hatches(prior_home);
}
