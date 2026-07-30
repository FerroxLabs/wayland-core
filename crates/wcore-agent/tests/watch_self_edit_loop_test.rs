//! Regression: the engine must not report its OWN writes as user edits.
//!
//! Reported by `lane/fix-tui-tool-results`, measured not theorised: with the
//! file watcher armed on cwd (`bootstrap.rs` installs it unconditionally), the
//! engine creating its own `.wayland-core/` state tree caused the synthetic
//! "User edited N files while I was thinking — re-read each before proceeding"
//! message to be injected. The model then answers the injected notice instead
//! of the user's prompt, and in one observed run looped on "Re-reading now."
//!
//! `is_wcore_internal_path` (watch.rs) filters by path COMPONENT, so it drops
//! `<cwd>/.wayland-core/...` but NOT a bare `<cwd>` — and a mutation of a
//! directory's own entries or attributes is reported against that directory.
//!
//! Both directions matter and both are pinned here. A filter that suppressed
//! every event would be exactly as broken as one that suppressed none, so the
//! genuine-external-edit cases below are load-bearing, not decoration.

use std::time::Duration;

use wcore_agent::watch::{FileWatcher, render_external_edit_message};

/// Time given to the platform notifier to arm before we touch anything.
/// FSEvents registration is asynchronous.
const ARM: Duration = Duration::from_millis(300);
/// Time given for events to be delivered into the channel before draining.
const SETTLE: Duration = Duration::from_millis(600);

/// Drain and render, returning both the injection and the raw surfaced paths
/// so a failure REPORTS THE SHAPE rather than merely asserting a boolean.
fn drain_report(watcher: &FileWatcher) -> (Option<String>, Vec<String>) {
    let events = watcher.drain_external_events();
    let paths = events
        .iter()
        .map(|e| format!("{} [{:?}]", e.path.display(), e.kind))
        .collect::<Vec<_>>();
    (render_external_edit_message(&events), paths)
}

// ── Direction 1: the engine's own writes must NOT surface ────────────────

/// The exact first-run sequence: bootstrap creates `.wayland-core/` and the
/// memory backend lands its SQLite trio inside it. Nothing here is a user
/// edit, so no injection may be produced.
#[tokio::test]
async fn engine_state_dir_creation_does_not_surface_as_user_edit() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path();
    let watcher = FileWatcher::new(root).expect("watcher");
    tokio::time::sleep(ARM).await;

    std::fs::create_dir_all(root.join(".wayland-core/memory")).expect("mkdir");
    std::fs::write(root.join(".wayland-core/memory/memory.db"), b"sqlite").expect("db");
    std::fs::write(root.join(".wayland-core/memory/memory.db-wal"), b"wal").expect("wal");
    std::fs::write(root.join(".wayland-core/memory/memory.db-shm"), b"shm").expect("shm");

    tokio::time::sleep(SETTLE).await;
    let (msg, paths) = drain_report(&watcher);
    eprintln!("[shape] engine_state_dir_creation surfaced: {paths:#?}");
    assert!(
        msg.is_none(),
        "the engine's own state-dir writes were reported to the model as user \
         edits.\n  injected: {msg:?}\n  surfaced paths: {paths:#?}\n  watch root: {}",
        root.display()
    );
}

/// An attribute change on the watch root itself (the reporting lane ran with
/// `chmod 555 cwd`). The root directory is not a re-readable file, so it can
/// never be a "user edit" no matter who caused it.
#[cfg(unix)]
#[tokio::test]
async fn attribute_change_on_watch_root_does_not_surface_as_user_edit() {
    use std::os::unix::fs::PermissionsExt;

    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path();
    let watcher = FileWatcher::new(root).expect("watcher");
    tokio::time::sleep(ARM).await;

    let original = std::fs::metadata(root).expect("stat").permissions();
    std::fs::set_permissions(root, std::fs::Permissions::from_mode(0o555)).expect("chmod 555");
    tokio::time::sleep(SETTLE).await;
    // Restore before asserting so a failure still lets tempdir clean up.
    std::fs::set_permissions(root, original).expect("restore perms");

    let (msg, paths) = drain_report(&watcher);
    eprintln!("[shape] chmod_on_watch_root surfaced: {paths:#?}");
    assert!(
        msg.is_none(),
        "an attribute change on the watch root itself was reported as a user \
         edit.\n  injected: {msg:?}\n  surfaced paths: {paths:#?}\n  watch root: {}",
        root.display()
    );
}

/// Direct synthetic control: a watch-root path handed to the renderer must
/// never produce an injection. Independent of any platform notifier, so this
/// one pins the predicate itself rather than the delivery mechanism.
#[tokio::test]
async fn watch_root_path_is_never_rendered_as_an_edit() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path();
    let watcher = FileWatcher::new(root).expect("watcher");
    tokio::time::sleep(ARM).await;

    // Touching a file then removing it churns the root's own entry list.
    let scratch = root.join("scratch.bin");
    std::fs::write(&scratch, b"x").expect("write");
    std::fs::remove_file(&scratch).expect("rm");
    tokio::time::sleep(SETTLE).await;

    let events = watcher.drain_external_events();
    let root_canon = std::fs::canonicalize(root).expect("canon root");
    let root_events: Vec<_> = events
        .iter()
        .filter(|e| e.path == root_canon || e.path == root)
        .map(|e| format!("{} [{:?}]", e.path.display(), e.kind))
        .collect();
    assert!(
        root_events.is_empty(),
        "the watch root itself was surfaced as an event: {root_events:#?}"
    );
}

// ── Direction 2: genuine external edits MUST still surface ───────────────
// Without these, a filter that drops everything would pass Direction 1.

/// A user editing a file in a SUBDIRECTORY of the watched tree.
#[tokio::test]
async fn genuine_edit_in_subdirectory_still_surfaces() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path();
    std::fs::create_dir_all(root.join("src")).expect("mkdir src");
    let watcher = FileWatcher::new(root).expect("watcher");
    tokio::time::sleep(ARM).await;

    std::fs::write(root.join("src/main.rs"), b"fn main() {}").expect("write");
    tokio::time::sleep(SETTLE).await;

    let (msg, paths) = drain_report(&watcher);
    let msg = msg.unwrap_or_else(|| {
        panic!("a genuine user edit MUST still be reported; surfaced paths: {paths:#?}")
    });
    assert!(
        msg.contains("main.rs"),
        "the edited file must be named in the injection, got: {msg}"
    );
}

/// A user editing a file sitting DIRECTLY in the watch root. This is the case
/// most at risk from a root-directory filter: the file's parent IS the root,
/// so an over-broad filter keyed on "anything at the root" would silence it.
#[tokio::test]
async fn genuine_edit_of_file_directly_in_watch_root_still_surfaces() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path();
    let watcher = FileWatcher::new(root).expect("watcher");
    tokio::time::sleep(ARM).await;

    std::fs::write(root.join("README.md"), b"# hello").expect("write");
    tokio::time::sleep(SETTLE).await;

    let (msg, paths) = drain_report(&watcher);
    let msg = msg.unwrap_or_else(|| {
        panic!(
            "a genuine user edit of a file in the watch root MUST still be \
             reported; surfaced paths: {paths:#?}"
        )
    });
    assert!(
        msg.contains("README.md"),
        "the edited file must be named in the injection, got: {msg}"
    );
}

/// The two must coexist: engine churn in `.wayland-core/` happening in the
/// same window as a real user edit must yield an injection naming ONLY the
/// user's file. This is the case the product actually hits.
#[tokio::test]
async fn real_edit_survives_concurrent_engine_state_churn() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path();
    let watcher = FileWatcher::new(root).expect("watcher");
    tokio::time::sleep(ARM).await;

    std::fs::create_dir_all(root.join(".wayland-core/sessions")).expect("mkdir");
    std::fs::write(root.join("notes.txt"), b"user typed this").expect("user write");
    std::fs::write(root.join(".wayland-core/sessions/s.json"), b"{}").expect("engine write");

    tokio::time::sleep(SETTLE).await;
    let (msg, paths) = drain_report(&watcher);
    let msg = msg.unwrap_or_else(|| {
        panic!("the user's edit was lost amid engine churn; surfaced paths: {paths:#?}")
    });
    assert!(
        msg.contains("notes.txt"),
        "the user's file must be named, got: {msg}"
    );
    assert!(
        !msg.contains(".wayland-core"),
        "engine state leaked into the injection: {msg}"
    );
    assert!(
        !msg.contains("files while I was thinking"),
        "exactly one real edit occurred, so the message must be singular; got: {msg}"
    );
}
