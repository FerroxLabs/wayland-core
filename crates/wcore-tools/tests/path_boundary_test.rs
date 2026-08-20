//! The pre-flight boundary classifier (#1099).
//!
//! Written from the contract: a card must appear exactly when a read would be
//! refused for being outside the sandbox AND a folder grant would fix it, must
//! name the FOLDER rather than the file, and must never appear for a session
//! whose "always allow this folder" button would be refused.

use serde_json::json;
use wcore_tools::path_boundary::{READ_PATH_ARGS, read_path_boundary};
use wcore_tools::workspace_policy::WorkspacePolicy;

/// A genuinely-local contained session — the only posture that installs a jail
/// AND can mint a grant, and therefore the only one that raises a card.
fn local_policy(root: &std::path::Path) -> WorkspacePolicy {
    WorkspacePolicy::contained(root).with_local_operator_principal()
}

#[test]
fn read_outside_workspace_suggests_the_containing_folder() {
    let ws = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    let report = outside.path().join("morning-brief.html");
    std::fs::write(&report, b"<html/>").unwrap();
    let policy = local_policy(ws.path());

    let boundary = read_path_boundary(&policy, "Read", &json!({ "file_path": report }))
        .expect("a read outside every reachable root must raise a card");

    assert_eq!(boundary.target, std::fs::canonicalize(&report).unwrap());
    assert_eq!(
        boundary.suggested_root,
        std::fs::canonicalize(outside.path()).unwrap(),
        "a grant opens a FOLDER; showing the file name on an 'always allow \
         this folder' button would be a button that lies about its scope"
    );
}

#[test]
fn read_inside_workspace_raises_nothing() {
    let ws = tempfile::tempdir().unwrap();
    let inside = ws.path().join("src.rs");
    std::fs::write(&inside, b"fn main() {}").unwrap();
    let policy = local_policy(ws.path());

    assert_eq!(
        read_path_boundary(&policy, "Read", &json!({ "file_path": inside })),
        None,
        "the workspace's own files must never prompt"
    );
}

#[test]
fn an_already_granted_folder_raises_nothing() {
    let ws = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    let first = outside.path().join("a.txt");
    let sibling = outside.path().join("b.txt");
    std::fs::write(&first, b"a").unwrap();
    std::fs::write(&sibling, b"b").unwrap();
    let policy = local_policy(ws.path());

    assert!(read_path_boundary(&policy, "Read", &json!({ "file_path": first })).is_some());
    policy
        .grant_session_read_root(outside.path(), false)
        .unwrap();

    assert_eq!(
        read_path_boundary(&policy, "Read", &json!({ "file_path": sibling })),
        None,
        "answering the card once must silence it for every sibling in the \
         same folder — re-prompting per file is the dead end this replaced"
    );
}

#[test]
fn a_relative_path_resolves_against_the_workspace_root() {
    let ws = tempfile::tempdir().unwrap();
    std::fs::create_dir(ws.path().join("src")).unwrap();
    std::fs::write(ws.path().join("src/lib.rs"), b"").unwrap();
    let outside = tempfile::tempdir().unwrap();
    std::fs::write(outside.path().join("x.txt"), b"").unwrap();
    let policy = local_policy(ws.path());

    assert_eq!(
        read_path_boundary(&policy, "Read", &json!({ "file_path": "src/lib.rs" })),
        None,
        "a relative path is the workspace's own, not an escape"
    );

    let escape = format!(
        "../{}/x.txt",
        outside.path().file_name().unwrap().to_str().unwrap()
    );
    let relative_escape = std::path::Path::new(&escape);
    // Only meaningful when the two temp dirs are actually siblings, which they
    // are under one `TMPDIR`; assert that so the case cannot silently degrade.
    assert_eq!(ws.path().parent(), outside.path().parent());
    assert!(
        read_path_boundary(
            &policy,
            "Read",
            &json!({ "file_path": relative_escape.to_str().unwrap() })
        )
        .is_some(),
        "a relative path that walks out of the workspace still crosses the boundary"
    );
}

#[test]
fn write_tools_are_never_offered_a_grant() {
    let ws = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    let target = outside.path().join("x.txt");
    std::fs::write(&target, b"x").unwrap();
    let policy = local_policy(ws.path());

    for tool in ["Write", "Edit"] {
        assert_eq!(
            read_path_boundary(&policy, tool, &json!({ "file_path": target })),
            None,
            "{tool} outside the workspace must not offer a folder grant — \
             write authority outside the workspace is not grantable, so the \
             button could only refuse itself"
        );
    }
}

#[test]
fn a_folder_that_would_be_refused_raises_no_card() {
    let ws = tempfile::tempdir().unwrap();
    let policy = local_policy(ws.path());

    // Directly under the filesystem root: the grant would be `FilesystemRoot`.
    // The root is taken from a real path rather than spelled literally, so the
    // case means the same thing on Windows (`C:\`) as on Unix (`/`).
    let fs_root = ws.path().ancestors().last().unwrap().to_path_buf();
    let at_root = fs_root.join("wayland-1099-not-a-real-file.txt");
    assert_eq!(
        read_path_boundary(&policy, "Read", &json!({ "file_path": at_root })),
        None,
        "Core must not offer a folder the policy will refuse"
    );

    // The credential store itself: `Grep` over `~/.ssh` would be refused as
    // `CredentialPath` (or, if it does not exist, as `TooBroad` on `$HOME`).
    if let Some(home) = dirs::home_dir() {
        assert_eq!(
            read_path_boundary(&policy, "Grep", &json!({ "path": home.join(".ssh") })),
            None,
            "a credential store is never offered"
        );
    }
}

#[test]
fn a_secret_inside_an_ordinary_folder_raises_no_card() {
    let ws = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    let secret = outside.path().join(".env");
    std::fs::write(&secret, b"TOKEN=1").unwrap();
    let policy = local_policy(ws.path());

    assert_eq!(
        read_path_boundary(&policy, "Read", &json!({ "file_path": secret })),
        None,
        "a grant widens WHERE the agent may look, never WHAT — the secret \
         stays refused after the grant, so offering the folder would be \
         offering a remedy that does not remedy"
    );
}

#[test]
fn a_trusted_local_policy_raises_no_card() {
    let ws = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    let target = outside.path().join("x.txt");
    std::fs::write(&target, b"x").unwrap();
    // No jail is installed for a trusted-local session, so this read SUCCEEDS
    // today. Prompting would interrupt something that already works.
    let policy = WorkspacePolicy::trusted_local(ws.path()).with_local_operator_principal();

    assert_eq!(
        read_path_boundary(&policy, "Read", &json!({ "file_path": target })),
        None
    );
}

#[test]
fn a_remote_session_raises_no_card() {
    let ws = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    let target = outside.path().join("x.txt");
    std::fs::write(&target, b"x").unwrap();
    // No `with_local_operator_principal`: a channel / managed engine, whose
    // grant would be refused with `RequiresLocalOperator`.
    let policy = WorkspacePolicy::contained(ws.path());

    assert_eq!(
        read_path_boundary(&policy, "Read", &json!({ "file_path": target })),
        None,
        "a card whose button is guaranteed to fail is worse than no card"
    );
}

#[test]
fn a_missing_or_empty_path_argument_raises_nothing() {
    let ws = tempfile::tempdir().unwrap();
    let policy = local_policy(ws.path());

    assert_eq!(
        read_path_boundary(&policy, "Grep", &json!({ "pattern": "needle" })),
        None,
        "Grep defaults to the cwd when `path` is absent"
    );
    assert_eq!(
        read_path_boundary(
            &policy,
            "Glob",
            &json!({ "pattern": "**/*.rs", "path": "" })
        ),
        None
    );
}

#[test]
fn the_path_arg_table_matches_each_tools_schema() {
    use wcore_tools::Tool;

    let tools: Vec<Box<dyn Tool>> = vec![
        Box::new(wcore_tools::read::ReadTool::new(None)),
        Box::new(wcore_tools::grep::GrepTool),
        Box::new(wcore_tools::glob::GlobTool),
    ];
    assert_eq!(
        tools.len(),
        READ_PATH_ARGS.len(),
        "every entry in the table needs a tool here, or the agreement check \
         silently stops covering it"
    );

    for tool in &tools {
        let (_, key) = READ_PATH_ARGS
            .iter()
            .find(|(name, _)| *name == tool.name())
            .unwrap_or_else(|| panic!("{} is not in READ_PATH_ARGS", tool.name()));
        let schema = tool.input_schema();
        assert!(
            schema["properties"].get(*key).is_some(),
            "{} has no `{}` property — the classifier would read a key the \
             tool does not accept and never raise a card",
            tool.name(),
            key
        );
    }
}
