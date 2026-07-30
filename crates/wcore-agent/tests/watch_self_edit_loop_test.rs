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

use std::path::PathBuf;
use std::time::Duration;

use wcore_agent::watch::{FileWatcher, render_external_edit_message};

/// Time given to the platform notifier to arm before we touch anything.
/// FSEvents registration is asynchronous.
const ARM: Duration = Duration::from_millis(300);
/// Time given for events to be delivered into the channel before draining.
const SETTLE: Duration = Duration::from_millis(600);

/// HARNESS REPAIR — do not watch `tempdir()` directly.
///
/// `tempfile::tempdir()` names its directory `.tmpXXXXXX`, and
/// `path_should_surface_as_edit` drops any path whose FILE NAME starts with
/// `.tmp` (the atomic-write scratch filter, watch.rs). A watch root named
/// `.tmpXXXXXX` is therefore swallowed by a filter that has nothing to do with
/// the property under test, so every Direction-1 assertion taken against it
/// passes no matter what the watcher does — a permanently-green gate.
///
/// Measured, not theorised: the first run of this file reported 6/6 passing on
/// Linux while the raw event list contained `/tmp/.tmpbNcTnJ
/// [Modify(Metadata(Any))]` — i.e. the watch root HAD leaked and the gate could
/// not see it. Production cwds are ordinary directory names, so the harness
/// uses one. `harness_selftest_*` below pins this.
fn project_root(tmp: &tempfile::TempDir) -> PathBuf {
    let root = tmp.path().join("project");
    std::fs::create_dir_all(&root).expect("mkdir project root");
    root
}

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

/// HARNESS REPAIR #2 — a fixed `sleep(SETTLE)` before a Direction-2 assertion
/// is a BUDGET, not a deadline: the same defect as the `await_session_switch`
/// reschedule budget this lane also fixed. FSEvents delivery latency varies,
/// and the same Direction-2 test was measured PASSING and FAILING at the same
/// pre-fix commit because 600 ms sometimes was and sometimes was not enough.
///
/// Wait until the wanted path actually appears, or a real wall-clock deadline
/// expires. Events must be ACCUMULATED across polls because `drain` empties
/// the channel — draining twice and looking only at the second result loses
/// the first batch, which is its own way to fail.
async fn drain_until(
    watcher: &FileWatcher,
    want: &str,
    budget: Duration,
) -> (Option<String>, Vec<String>) {
    let deadline = std::time::Instant::now() + budget;
    let mut acc = Vec::new();
    loop {
        acc.extend(watcher.drain_external_events());
        let msg = render_external_edit_message(&acc);
        let hit = msg.as_deref().is_some_and(|m| m.contains(want));
        if hit || std::time::Instant::now() >= deadline {
            let paths = acc
                .iter()
                .map(|e| format!("{} [{:?}]", e.path.display(), e.kind))
                .collect::<Vec<_>>();
            return (msg, paths);
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

/// Wall-clock budget for a genuine edit to be delivered and rendered.
const EDIT_BUDGET: Duration = Duration::from_secs(10);

// ── Harness self-test (LANE-BRIEF §6b-ii) ────────────────────────────────
// Three assertions, because two would pass on the BROKEN harness too:
//   (a) known-positive — a realistically-named root renders as an edit;
//   (b) known-negative — a genuinely-filtered path does not;
//   (c) the OLD harness would have missed it — a `.tmp`-named root is
//       swallowed by an unrelated filter, which is the exact masking that
//       made this file report 6/6 green while the watch root was leaking.
// (c) is the only one that proves the repair changed anything.

/// Pins the masking directly, with no watcher and no platform notifier
/// involved, so it cannot itself go stale on a timing change.
#[test]
fn harness_selftest_tempdir_root_name_masked_the_result() {
    use notify::EventKind;
    use std::time::Instant;
    use wcore_agent::watch::ExternalEvent;

    let ev = |p: &str| ExternalEvent {
        path: PathBuf::from(p),
        kind: EventKind::Any,
        at: Instant::now(),
    };

    // (a) known-positive: the name the repaired harness uses is visible.
    assert!(
        render_external_edit_message(&[ev("/tmp/.tmpbNcTnJ/project")]).is_some(),
        "the repaired harness's root name must be renderable, else Direction-1 \
         assertions taken against it are vacuous"
    );

    // (b) known-negative: a path the renderer is SUPPOSED to drop is dropped,
    // proving the renderer is not simply returning Some for everything.
    assert!(
        render_external_edit_message(&[ev("/tmp/proj/target/debug/x.rlib")]).is_none(),
        "renderer must still drop build artefacts"
    );

    // (c) the old harness would have missed it: `tempfile`'s own directory
    // name is eaten by the `.tmp` scratch filter, so an event for the OLD
    // watch root rendered as nothing whatever the watcher did.
    assert!(
        render_external_edit_message(&[ev("/tmp/.tmpbNcTnJ")]).is_none(),
        "if a `.tmp`-prefixed root were renderable the original harness would \
         have been sound and this repair would be pointless — the measured \
         run showed it is not"
    );
}

// ── Direction 1: the engine's own writes must NOT surface ────────────────

/// The exact first-run sequence: bootstrap creates `.wayland-core/` and the
/// memory backend lands its SQLite trio inside it. Nothing here is a user
/// edit, so no injection may be produced.
#[tokio::test]
async fn engine_state_dir_creation_does_not_surface_as_user_edit() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let root = project_root(&tmp);
    let watcher = FileWatcher::new(&root).expect("watcher");
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
    let root = project_root(&tmp);
    let watcher = FileWatcher::new(&root).expect("watcher");
    tokio::time::sleep(ARM).await;

    let original = std::fs::metadata(&root).expect("stat").permissions();
    std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o555)).expect("chmod 555");
    tokio::time::sleep(SETTLE).await;
    // Restore before asserting so a failure still lets tempdir clean up.
    std::fs::set_permissions(&root, original).expect("restore perms");

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
    let root = project_root(&tmp);
    let watcher = FileWatcher::new(&root).expect("watcher");
    tokio::time::sleep(ARM).await;

    // Touching a file then removing it churns the root's own entry list.
    let scratch = root.join("scratch.bin");
    std::fs::write(&scratch, b"x").expect("write");
    std::fs::remove_file(&scratch).expect("rm");
    tokio::time::sleep(SETTLE).await;

    let events = watcher.drain_external_events();
    let root_canon = std::fs::canonicalize(&root).expect("canon root");
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
    let root = project_root(&tmp);
    std::fs::create_dir_all(root.join("src")).expect("mkdir src");
    let watcher = FileWatcher::new(&root).expect("watcher");
    tokio::time::sleep(ARM).await;

    std::fs::write(root.join("src/main.rs"), b"fn main() {}").expect("write");

    let (msg, paths) = drain_until(&watcher, "main.rs", EDIT_BUDGET).await;
    eprintln!("[shape] genuine_edit_in_subdirectory surfaced: {paths:#?}");
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
    let root = project_root(&tmp);
    let watcher = FileWatcher::new(&root).expect("watcher");
    tokio::time::sleep(ARM).await;

    std::fs::write(root.join("README.md"), b"# hello").expect("write");

    let (msg, paths) = drain_until(&watcher, "README.md", EDIT_BUDGET).await;
    eprintln!("[shape] genuine_edit_directly_in_watch_root surfaced: {paths:#?}");
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
    let root = project_root(&tmp);
    let watcher = FileWatcher::new(&root).expect("watcher");
    tokio::time::sleep(ARM).await;

    std::fs::create_dir_all(root.join(".wayland-core/sessions")).expect("mkdir");
    std::fs::write(root.join("notes.txt"), b"user typed this").expect("user write");
    std::fs::write(root.join(".wayland-core/sessions/s.json"), b"{}").expect("engine write");

    let (msg, paths) = drain_until(&watcher, "notes.txt", EDIT_BUDGET).await;
    eprintln!("[shape] real_edit_with_engine_churn surfaced: {paths:#?}");
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
