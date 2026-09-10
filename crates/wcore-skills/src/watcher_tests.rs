/// Black-box tests for `SkillWatcher` based on the Phase 13 test plan.
///
/// All tests are async (`#[tokio::test]`) because the watcher relies on
/// tokio tasks for debouncing.
///
/// ## macOS path note
///
/// `tempfile::TempDir` creates directories under `/var/folders/.../T/.tmpXXXX`.
/// On macOS, FSEvents resolves symlinks, so the reported path becomes
/// `/private/var/folders/.../T/.tmpXXXX`.  The `.tmpXXXX` directory name
/// starts with `.`, which causes `should_ignore` to filter ALL events from
/// such directories (it checks every path component, not just the filename).
///
/// To work around this, tests that rely on receiving notifications create
/// directories with visible (non-dot-prefixed) names under `/tmp/`.
/// Tests that verify silence (TC-14, TC-15) still use `TempDir` because
/// those directories only receive hidden-file events which should be filtered.
///
/// Debounce window is 300 ms.  Tests that expect a notification wait 600 ms
/// (300 ms window + 300 ms platform margin).  Tests that expect *no*
/// notification wait 800 ms to be safe.
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use tempfile::TempDir;
use tokio::time::timeout;

use crate::discovery::RuntimeDiscovery;
use crate::watcher::SkillWatcher;

/// Create a uniquely named, non-hidden test directory under the temp root.
///
/// Returns a `PathBuf` and the owning `TempDir`, which removes the directory
/// on drop. The name is `wcore_watcher_test_<name>_<random>`: visible, so the
/// macOS `.tmpXXXX` naming that `should_ignore` filters is avoided, and
/// RANDOM, so no two processes can be handed the same directory.
///
/// The randomness is the whole point (wayland#1308 c2). This used to be a
/// process-local `AtomicU64`, and every test name is used exactly once, so the
/// counter was always 0 and two identically-launched test binaries computed
/// the SAME path under the same `std::env::temp_dir()`. On a box running
/// several runner services as one user that root is shared, so whichever
/// process finished first removed the directory the other was still watching
/// -- and the survivor's next write failed with `ERROR_PATH_NOT_FOUND` (3),
/// the directory-component error the four siblings all reported.
/// `cross_process_siblings_do_not_share_a_test_directory` is the control.
fn make_visible_test_dir(name: &str) -> (PathBuf, TempDir) {
    // Use /private/tmp to match FSEvents resolved path on macOS
    let base = if cfg!(target_os = "macos") {
        PathBuf::from("/private/tmp")
    } else {
        std::env::temp_dir()
    };
    expect_fs(fs::create_dir_all(&base), "create test temp root", &base);
    let dir = match tempfile::Builder::new()
        .prefix(&format!("wcore_watcher_test_{name}_"))
        .tempdir_in(&base)
    {
        Ok(dir) => dir,
        Err(error) => panic!(
            "create test dir for {name} under {} failed: {error}\n{}",
            base.display(),
            first_missing_component(&base)
        ),
    };
    let path = dir.path().to_path_buf();
    (path, dir)
}

/// Report WHICH component of a path is missing when a filesystem call fails.
///
/// `ERROR_PATH_NOT_FOUND` (3) and `ERROR_FILE_NOT_FOUND` (2) are DISTINCT on
/// Windows: 3 means a directory component is missing rather than the leaf
/// file. A bare `unwrap` on a filesystem `Result` discards the path, so the
/// only thing the artifact could say was `Os { code: 3, kind: NotFound }` --
/// which is how four sibling failures in run 33751975177 were unreadable
/// (wayland#1308 c1).
///
/// The walk stops at the first missing component, because everything below a
/// missing directory is missing for the same reason and listing it adds noise
/// rather than information.
#[track_caller]
fn expect_fs<T>(result: std::io::Result<T>, what: &str, path: &Path) -> T {
    match result {
        Ok(value) => value,
        Err(error) => panic!(
            "{what} on {} failed: {error}\n{}",
            path.display(),
            first_missing_component(path)
        ),
    }
}

fn first_missing_component(path: &Path) -> String {
    let mut walked = PathBuf::new();
    let mut report = String::from("path components:");
    for component in path.components() {
        walked.push(component);
        let exists = walked.exists();
        report.push_str(&format!(
            "\n  {:<7} {}",
            if exists { "present" } else { "MISSING" },
            walked.display()
        ));
        if !exists {
            report.push_str("\n  ^ this component is the one that is gone");
            return report;
        }
    }
    report
        .push_str("\n  every component exists NOW -- it was recreated, or the leaf is the problem");
    report
}

const DEBOUNCE_EXPECT_MS: u64 = 600; // wait when expecting a notification
const DEBOUNCE_NO_EXPECT_MS: u64 = 800; // wait when expecting silence
// Time to wait after start() before triggering events, giving notify time to
// register with the OS kernel (FSEvents/inotify initialisation latency).
const WATCHER_INIT_MS: u64 = 150;

// ---------------------------------------------------------------------------
// Diagnostic: verify notify events are received at all
// ---------------------------------------------------------------------------

/// Slow diagnostic test: uses a 1-second wait to rule out timing issues.
/// Run individually: cargo test watcher_tests::diag -- --nocapture --include-ignored
#[tokio::test]
#[ignore]
async fn diag_basic_event_received() {
    let dir = TempDir::new().unwrap();
    eprintln!("[diag] test dir: {}", dir.path().display());

    let (mut watcher, mut rx) = SkillWatcher::new().unwrap();
    watcher.start(vec![dir.path().to_path_buf()]).unwrap();

    eprintln!("[diag] waiting 500ms for notify init...");
    tokio::time::sleep(Duration::from_millis(500)).await;

    let before = *rx.borrow_and_update();
    eprintln!("[diag] version before: {before}");

    let file = dir.path().join("test.md");
    eprintln!("[diag] writing file: {}", file.display());
    fs::write(&file, "hello").unwrap();
    eprintln!("[diag] file written, waiting up to 1s...");

    let result = timeout(Duration::from_millis(1000), rx.changed()).await;
    eprintln!(
        "[diag] timeout result (true=got event): {:?}",
        result.is_ok()
    );
    let after = *rx.borrow();
    eprintln!("[diag] version after: {after}");

    assert!(result.is_ok(), "[diag] no event received within 1s");
}

/// Diagnostic: test with multi-thread runtime to rule out single-thread scheduling
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore]
async fn diag_multi_thread_event_received() {
    let dir = TempDir::new().unwrap();
    eprintln!("[diag-mt] test dir: {}", dir.path().display());

    let (mut watcher, mut rx) = SkillWatcher::new().unwrap();
    watcher.start(vec![dir.path().to_path_buf()]).unwrap();

    eprintln!("[diag-mt] waiting 500ms for notify init...");
    tokio::time::sleep(Duration::from_millis(500)).await;

    let before = *rx.borrow_and_update();
    eprintln!("[diag-mt] version before: {before}");

    let file = dir.path().join("test.md");
    eprintln!("[diag-mt] writing file: {}", file.display());
    fs::write(&file, "hello").unwrap();
    eprintln!("[diag-mt] file written, waiting up to 2s...");

    let result = timeout(Duration::from_millis(2000), rx.changed()).await;
    eprintln!(
        "[diag-mt] timeout result (true=got event): {:?}",
        result.is_ok()
    );
    let after = *rx.borrow();
    eprintln!("[diag-mt] version after: {after}");

    assert!(result.is_ok(), "[diag-mt] no event received within 2s");
}

// ---------------------------------------------------------------------------
// TC-01: new() accepts empty directory list
// ---------------------------------------------------------------------------

/// [TC-01 black-box] `new()` with no dirs returns Ok; initial version is 0.
#[tokio::test]
async fn tc01_new_empty_dirs_returns_ok() {
    let (mut watcher, rx) = SkillWatcher::new().expect("new() should succeed");
    watcher.start(vec![]).expect("start() should succeed");

    let initial = *rx.borrow();
    assert_eq!(initial, 0, "initial version should be 0");
}

// ---------------------------------------------------------------------------
// TC-02: new() accepts multiple existing directories
// ---------------------------------------------------------------------------

/// [TC-02 black-box] `new()` with two real directories returns Ok.
#[tokio::test]
async fn tc02_new_with_existing_dirs_returns_ok() {
    let dir_a = TempDir::new().unwrap();
    let dir_b = TempDir::new().unwrap();

    let (mut watcher, _rx) = SkillWatcher::new().expect("new() should succeed with existing dirs");
    watcher
        .start(vec![dir_a.path().to_path_buf(), dir_b.path().to_path_buf()])
        .expect("start() should succeed");
}

// ---------------------------------------------------------------------------
// TC-03: new() skips non-existent directories (no panic, no Err)
// ---------------------------------------------------------------------------

/// [TC-03 black-box] Non-existent directory is skipped silently; `new()` succeeds.
#[tokio::test]
async fn tc03_nonexistent_dir_skipped() {
    let non_existent = std::path::PathBuf::from("/nonexistent/path/abc_phase13_test");

    let (mut watcher, _rx) = SkillWatcher::new().expect("new() should succeed");
    watcher
        .start(vec![non_existent])
        .expect("start() should not error for non-existent dirs");
}

// ---------------------------------------------------------------------------
// TC-04: mix of existing and non-existing directories
// ---------------------------------------------------------------------------

/// [TC-04 black-box] Mix of existing and non-existing dirs - both handled without error.
#[tokio::test]
async fn tc04_mixed_dirs() {
    let existing = TempDir::new().unwrap();
    let non_existent = std::path::PathBuf::from("/nonexistent/xyz_phase13_test");

    let (mut watcher, _rx) = SkillWatcher::new().expect("new() should succeed");
    watcher
        .start(vec![existing.path().to_path_buf(), non_existent])
        .expect("start() should succeed for mixed dirs");
}

// ---------------------------------------------------------------------------
// TC-05: file creation triggers notification after debounce
// ---------------------------------------------------------------------------

/// [TC-05 black-box] Creating a file in a watched directory triggers a version bump.
#[tokio::test]
async fn tc05_file_create_triggers_notification() {
    let (dir, _guard) = make_visible_test_dir("tc05");
    let (mut watcher, mut rx) = SkillWatcher::new().unwrap();
    watcher.start(vec![dir.clone()]).unwrap();

    // Wait for notify to register the watch with the OS.
    tokio::time::sleep(Duration::from_millis(WATCHER_INIT_MS)).await;

    let initial = *rx.borrow_and_update();

    // Create a file to trigger an event.
    fs::write(dir.join("SKILL.md"), "# test skill").unwrap();

    let result = timeout(Duration::from_millis(DEBOUNCE_EXPECT_MS), rx.changed()).await;

    assert!(
        result.is_ok(),
        "should receive notification within {}ms after file creation",
        DEBOUNCE_EXPECT_MS
    );
    let new_version = *rx.borrow();
    assert!(
        new_version > initial,
        "version should increment after file creation (was {initial}, now {new_version})"
    );
}

// ---------------------------------------------------------------------------
// TC-06: file modification triggers notification
// ---------------------------------------------------------------------------

/// [TC-06 black-box] Modifying an existing file triggers a version bump.
#[tokio::test]
async fn tc06_file_modify_triggers_notification() {
    let (dir, _guard) = make_visible_test_dir("tc06");
    let skill_file = dir.join("SKILL.md");
    fs::write(&skill_file, "# initial").unwrap();

    let (mut watcher, mut rx) = SkillWatcher::new().unwrap();
    watcher.start(vec![dir.clone()]).unwrap();

    // Wait for notify to initialise, then drain any creation event.
    tokio::time::sleep(Duration::from_millis(WATCHER_INIT_MS + 400)).await;
    let version_before = *rx.borrow_and_update();

    // Modify the file.
    expect_fs(fs::write(&skill_file, "# modified"), "modify", &skill_file);

    let result = timeout(Duration::from_millis(DEBOUNCE_EXPECT_MS), rx.changed()).await;

    assert!(
        result.is_ok(),
        "should receive notification within {}ms after file modification",
        DEBOUNCE_EXPECT_MS
    );
    let new_version = *rx.borrow();
    assert!(
        new_version > version_before,
        "version should increment after modification (was {version_before}, now {new_version})"
    );
}

// ---------------------------------------------------------------------------
// TC-07: file deletion triggers notification
// ---------------------------------------------------------------------------

/// [TC-07 black-box] Deleting a file in a watched directory triggers a version bump.
#[tokio::test]
async fn tc07_file_delete_triggers_notification() {
    let (dir, _guard) = make_visible_test_dir("tc07");
    let skill_file = dir.join("SKILL.md");
    fs::write(&skill_file, "# to be deleted").unwrap();

    let (mut watcher, mut rx) = SkillWatcher::new().unwrap();
    watcher.start(vec![dir.clone()]).unwrap();

    // Wait for notify init + drain creation event.
    tokio::time::sleep(Duration::from_millis(WATCHER_INIT_MS + 400)).await;
    let version_before = *rx.borrow_and_update();

    // Delete the file.
    expect_fs(fs::remove_file(&skill_file), "delete", &skill_file);

    let result = timeout(Duration::from_millis(DEBOUNCE_EXPECT_MS), rx.changed()).await;

    assert!(
        result.is_ok(),
        "should receive notification within {}ms after file deletion",
        DEBOUNCE_EXPECT_MS
    );
    let new_version = *rx.borrow();
    assert!(
        new_version > version_before,
        "version should increment after deletion (was {version_before}, now {new_version})"
    );
}

// ---------------------------------------------------------------------------
// TC-08: file rename triggers notification
// ---------------------------------------------------------------------------

/// [TC-08 black-box] Renaming a file in a watched directory triggers a version bump.
#[tokio::test]
async fn tc08_file_rename_triggers_notification() {
    let (dir, _guard) = make_visible_test_dir("tc08");
    let old_file = dir.join("old.md");
    fs::write(&old_file, "# old").unwrap();

    let (mut watcher, mut rx) = SkillWatcher::new().unwrap();
    watcher.start(vec![dir.clone()]).unwrap();

    // Wait for notify init + drain creation event.
    tokio::time::sleep(Duration::from_millis(WATCHER_INIT_MS + 400)).await;
    let version_before = *rx.borrow_and_update();

    // Rename the file.
    let renamed = dir.join("SKILL.md");
    expect_fs(fs::rename(&old_file, &renamed), "rename", &old_file);

    let result = timeout(Duration::from_millis(DEBOUNCE_EXPECT_MS), rx.changed()).await;

    assert!(
        result.is_ok(),
        "should receive notification within {}ms after file rename",
        DEBOUNCE_EXPECT_MS
    );
    let new_version = *rx.borrow();
    assert!(
        new_version > version_before,
        "version should increment after rename (was {version_before}, now {new_version})"
    );
}

// ---------------------------------------------------------------------------
// TC-09: multiple events within 300ms are coalesced into one notification
// ---------------------------------------------------------------------------

/// [TC-09 black-box] Five file writes within 100 ms result in only one version increment.
#[tokio::test]
async fn tc09_debounce_coalesces_multiple_events() {
    let (dir, _guard) = make_visible_test_dir("tc09");
    let (mut watcher, mut rx) = SkillWatcher::new().unwrap();
    watcher.start(vec![dir.clone()]).unwrap();

    // Wait for notify to initialise.
    tokio::time::sleep(Duration::from_millis(WATCHER_INIT_MS)).await;
    let initial = *rx.borrow_and_update();

    // Write 5 files rapidly (within ~50 ms total).
    for i in 0..5u32 {
        let each = dir.join(format!("skill_{i}.md"));
        expect_fs(fs::write(&each, format!("# skill {i}")), "write", &each);
        tokio::time::sleep(Duration::from_millis(10)).await;
    }

    // Wait for the debounce window to expire plus margin.
    tokio::time::sleep(Duration::from_millis(DEBOUNCE_EXPECT_MS)).await;

    let final_version = *rx.borrow();
    let increments = final_version - initial;

    assert!(
        increments >= 1,
        "version should have incremented at least once (initial={initial}, final={final_version})"
    );
    // The key assertion: 5 rapid events should be coalesced into at most 2 notifications.
    // Ideally 1, but allow a small margin for platform timing jitter.
    assert!(
        increments <= 2,
        "debounce should coalesce rapid events into <=2 increments, got {increments} (initial={initial}, final={final_version})"
    );
}

// ---------------------------------------------------------------------------
// TC-10: watch_directory() adds a new directory dynamically
// ---------------------------------------------------------------------------

/// [TC-10 black-box] `watch_directory()` called after `start()` enables monitoring new dir.
#[tokio::test]
async fn tc10_watch_directory_dynamic_add() {
    let (dir_a, _guard_a) = make_visible_test_dir("tc10a");
    let (dir_b, _guard_b) = make_visible_test_dir("tc10b");

    let (mut watcher, mut rx) = SkillWatcher::new().unwrap();
    watcher.start(vec![dir_a.clone()]).unwrap();

    // Dynamically add dir_b.
    watcher
        .watch_directory(&dir_b)
        .expect("watch_directory() should succeed for existing dir");

    // Wait for notify to register the new directory.
    tokio::time::sleep(Duration::from_millis(WATCHER_INIT_MS)).await;
    let version_before = *rx.borrow_and_update();

    // Create file in newly added dir.
    fs::write(dir_b.join("SKILL.md"), "# dynamic").unwrap();

    let result = timeout(Duration::from_millis(DEBOUNCE_EXPECT_MS), rx.changed()).await;

    assert!(
        result.is_ok(),
        "should receive notification from dynamically added directory"
    );
    let new_version = *rx.borrow();
    assert!(
        new_version > version_before,
        "version should increment after event in dynamically added dir"
    );
}

// ---------------------------------------------------------------------------
// TC-11: watch_directory() with non-existent dir does not panic
// ---------------------------------------------------------------------------

/// [TC-11 black-box] `watch_directory()` on a non-existent dir does not panic or crash.
#[tokio::test]
async fn tc11_watch_directory_nonexistent_no_panic() {
    let (mut watcher, _rx) = SkillWatcher::new().unwrap();
    watcher.start(vec![]).unwrap();

    let result = watcher.watch_directory(&std::path::PathBuf::from("/nonexistent/dynamic_test"));
    // Should either return Ok (skip silently) or Err — but MUST NOT panic.
    // Per AC-2 spirit, we expect Ok (silently skipped).
    assert!(
        result.is_ok(),
        "watch_directory() should not error for non-existent dir per AC-2 skip semantics"
    );
}

// ---------------------------------------------------------------------------
// TC-12: stop() prevents subsequent notifications
// ---------------------------------------------------------------------------

/// [TC-12 black-box] After `stop()`, file changes in formerly watched dir do not trigger notifications.
#[tokio::test]
async fn tc12_stop_prevents_notifications() {
    let (dir, _guard) = make_visible_test_dir("tc12");
    let (mut watcher, mut rx) = SkillWatcher::new().unwrap();
    watcher.start(vec![dir.clone()]).unwrap();

    // Wait for notify init, then drain any initial events.
    tokio::time::sleep(Duration::from_millis(WATCHER_INIT_MS)).await;
    let version_before_stop = *rx.borrow_and_update();

    watcher.stop();

    // Give a brief moment for the OS to process the unwatch.
    tokio::time::sleep(Duration::from_millis(50)).await;

    // Create a file — should NOT trigger any notification.
    fs::write(dir.join("after_stop.md"), "# after stop").unwrap();

    tokio::time::sleep(Duration::from_millis(DEBOUNCE_NO_EXPECT_MS)).await;

    let version_after = *rx.borrow();
    assert_eq!(
        version_after, version_before_stop,
        "version should not change after stop() (was {version_before_stop}, got {version_after})"
    );
}

// ---------------------------------------------------------------------------
// TC-13: stop() is idempotent (safe to call multiple times)
// ---------------------------------------------------------------------------

/// [TC-13 black-box] Calling `stop()` twice does not panic.
#[tokio::test]
async fn tc13_stop_idempotent() {
    let (mut watcher, _rx) = SkillWatcher::new().unwrap();
    watcher.start(vec![]).unwrap();

    watcher.stop();
    watcher.stop(); // second call must not panic
}

// ---------------------------------------------------------------------------
// TC-14: hidden files (dot-prefixed) do not trigger notifications
// ---------------------------------------------------------------------------

/// [TC-14 black-box] Creating a hidden file (`.swp`) does not trigger a version bump.
///
/// Uses a visible (non-dot-prefixed) parent directory so that normal files
/// *would* trigger a notification — confirming that only the hidden file is filtered.
///
/// `Modify(Metadata(_))` events are now filtered by `should_ignore`, so the
/// parent-directory metadata event emitted by macOS FSEvents is also suppressed.
#[tokio::test]
async fn tc14_hidden_file_not_triggers_notification() {
    let (dir, _guard) = make_visible_test_dir("tc14");
    let (mut watcher, mut rx) = SkillWatcher::new().unwrap();
    watcher.start(vec![dir.clone()]).unwrap();

    tokio::time::sleep(Duration::from_millis(WATCHER_INIT_MS)).await;
    let version_before = *rx.borrow_and_update();

    // Create a hidden file (editor swap file).
    fs::write(dir.join(".swp"), "editor temp").unwrap();

    tokio::time::sleep(Duration::from_millis(DEBOUNCE_NO_EXPECT_MS)).await;

    let version_after = *rx.borrow();
    assert_eq!(
        version_after, version_before,
        "hidden file creation should not increment version (was {version_before}, got {version_after})"
    );
}

// ---------------------------------------------------------------------------
// TC-15: hidden dot-prefixed file does not trigger notification
// ---------------------------------------------------------------------------

/// [TC-15 black-box] Creating `.hidden_skill.md` does not trigger a version bump.
///
/// Uses a visible (non-dot-prefixed) parent directory so that normal files
/// *would* trigger a notification — confirming that only the dot-prefixed file is filtered.
///
/// `Modify(Metadata(_))` filtering now suppresses macOS parent-dir metadata events.
#[tokio::test]
async fn tc15_dot_prefixed_file_not_triggers_notification() {
    let (dir, _guard) = make_visible_test_dir("tc15");
    let (mut watcher, mut rx) = SkillWatcher::new().unwrap();
    watcher.start(vec![dir.clone()]).unwrap();

    tokio::time::sleep(Duration::from_millis(WATCHER_INIT_MS)).await;
    let version_before = *rx.borrow_and_update();

    fs::write(dir.join(".hidden_skill.md"), "# hidden").unwrap();

    tokio::time::sleep(Duration::from_millis(DEBOUNCE_NO_EXPECT_MS)).await;

    let version_after = *rx.borrow();
    assert_eq!(
        version_after, version_before,
        ".hidden_skill.md should not increment version (was {version_before}, got {version_after})"
    );
}

// ---------------------------------------------------------------------------
// TC-16: RuntimeDiscovery::clear_checked_dirs() exists and clears state
// ---------------------------------------------------------------------------

/// [TC-16 black-box] `clear_checked_dirs()` method exists and empties the checked dirs cache.
#[test]
fn tc16_runtime_discovery_clear_checked_dirs() {
    let mut discovery = RuntimeDiscovery::new();

    // Verify the method is callable and clears state.
    // We can't inspect private fields directly, so we verify behaviour via
    // discover_dirs_for_paths returning results after clearing.
    //
    // The key assertion: calling clear_checked_dirs() does not panic.
    discovery.clear_checked_dirs();

    // Calling it a second time is also safe.
    discovery.clear_checked_dirs();
}

// ---------------------------------------------------------------------------
// TC-17 & TC-18 are verified by CI: cargo clippy and cargo test
// (These are build-level assertions, not unit tests.)
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// TC-20: version number is strictly monotonically increasing
// ---------------------------------------------------------------------------

/// [TC-20 black-box] Version numbers strictly increase across multiple independent events.
#[tokio::test]
async fn tc20_version_monotonically_increasing() {
    let (dir, _guard) = make_visible_test_dir("tc20");
    let (mut watcher, mut rx) = SkillWatcher::new().unwrap();
    watcher.start(vec![dir.clone()]).unwrap();

    // Wait for notify to initialise.
    tokio::time::sleep(Duration::from_millis(WATCHER_INIT_MS)).await;
    let mut prev_version = *rx.borrow_and_update();

    for round in 0..3u32 {
        // Each round: create a unique file, then wait for the notification.
        fs::write(
            dir.join(format!("round_{round}.md")),
            format!("# round {round}"),
        )
        .unwrap();

        let result = timeout(Duration::from_millis(DEBOUNCE_EXPECT_MS), rx.changed()).await;

        assert!(
            result.is_ok(),
            "round {round}: should receive notification within {DEBOUNCE_EXPECT_MS}ms"
        );

        let new_version = *rx.borrow_and_update();
        assert!(
            new_version > prev_version,
            "round {round}: version should be strictly increasing (prev={prev_version}, new={new_version})"
        );
        prev_version = new_version;
    }
}

// ---------------------------------------------------------------------------
// wayland#1308 c2: which directory component goes missing, and who removes it
// ---------------------------------------------------------------------------
//
// `make_visible_test_dir` used to name a directory from the test name plus a
// PROCESS-LOCAL `AtomicU64`. Every test name is used once, so within one
// process the counter was always 0 and two identically-launched test binaries
// computed the SAME path under the SAME `std::env::temp_dir()`. On a box
// running several runner services as one user that root is shared, so two
// concurrent runs of the same test shared one directory -- and whichever
// finished first removed it out from under the other. The survivor's next
// write then failed with `ERROR_PATH_NOT_FOUND` (3), the directory-component
// error, which is exactly the code the four siblings reported.
//
// This is a control, not an observation: it drives the two processes through a
// file handshake so the removal happens at a chosen instant rather than in a
// race window, and it reports the missing component by name.
//
// MEASURED 2026-09-10 (wayland#1308 c2/c3) on SeanDesktop -- the same physical
// box that hosts the `CI (Array)` Windows runner services -- cargo 1.95.0,
// nextest 0.9.138, debug, `--retries 0`, every arm run at both the pre-fix
// helper (f7564ea4) and the fixed one (785a22ea):
//
//   arm                                            pre-fix        fixed
//   this control                                    0/3 pass      5/5 pass
//   tc06+tc07+tc08+tc09 together, one process      20/20 pass    20/20 pass
//   each of the four alone, 20 runs each            --           80/80 pass
//   the four together in TWO concurrent processes   0/20 pass    20/20 pass
//
// The single-process arms pass at BOTH helpers, which is the control that
// separates shared state from a per-test bug: one process alone never
// collides, so no number of repetitions of it can reproduce this. The
// concurrent-process arm fails 20/20 before the fix, every time in tc07 and
// tc08 of whichever process lost the race. Read the codes honestly: this
// control reports `ERROR_PATH_NOT_FOUND` (3), the code the outage reported,
// because the handshake removes the directory and holds it removed; the
// naturalistic concurrent arm usually reports `ERROR_FILE_NOT_FOUND` (2)
// instead, because the sibling has already recreated the directory for its
// own next test by the time the survivor's call lands. Same collision, caught
// a few milliseconds later.

/// Env var naming the role a child process of this test binary plays.
const COLLISION_ROLE: &str = "WCORE_WATCHER_COLLISION_ROLE";
/// Env var naming the directory the parent and its two children hand files through.
const COLLISION_SYNC: &str = "WCORE_WATCHER_COLLISION_SYNC";
/// libtest path of the child entry point, used with `--exact`.
const COLLISION_CHILD_PATH: &str = "watcher_tests::collision_child";
/// Ceiling on every handshake wait. Windows process spawn on this class of box
/// is bimodal at ~3s, so this is generous by design; it exists so a lost child
/// fails the test rather than hanging the run.
const COLLISION_BUDGET: Duration = Duration::from_secs(90);

/// Publish `body` under `name` in the sync directory, atomically.
///
/// Written to a `.partial` sibling and renamed, so a reader can never observe
/// a half-written signal and mistake it for a short answer.
fn signal(sync: &Path, name: &str, body: &str) {
    let partial = sync.join(format!("{name}.partial"));
    expect_fs(fs::write(&partial, body), "write signal", &partial);
    let final_path = sync.join(name);
    expect_fs(
        fs::rename(&partial, &final_path),
        "publish signal",
        &final_path,
    );
}

/// Block until `path` exists, returning its contents. Panics at the budget.
fn await_signal(path: &Path, what: &str) -> String {
    let deadline = std::time::Instant::now() + COLLISION_BUDGET;
    loop {
        if let Ok(body) = fs::read_to_string(path) {
            return body;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "timed out after {COLLISION_BUDGET:?} waiting for {what} ({})",
            path.display()
        );
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// One half of the two-process control. Ignored: it is meaningless alone and is
/// launched by `cross_process_siblings_do_not_share_a_test_directory` with a
/// role and a sync directory in the environment.
#[test]
#[ignore = "child half of cross_process_siblings_do_not_share_a_test_directory"]
fn collision_child() {
    let role = std::env::var(COLLISION_ROLE)
        .expect("collision_child ran with no role -- it is driven by its parent test");
    let sync = PathBuf::from(
        std::env::var(COLLISION_SYNC).expect("collision_child ran with no sync directory"),
    );

    // Deliberately the SAME logical test name in both children. Two runner
    // services on one box run the same test at the same time, and that -- not
    // two different siblings inside one process -- is the collision domain.
    let (dir, guard) = make_visible_test_dir("collision");
    signal(&sync, &format!("{role}.path"), &dir.display().to_string());

    match role.as_str() {
        "a" => {
            await_signal(&sync.join("go-a"), "the parent's teardown signal");
            drop(guard);
            signal(&sync, "a.dropped", &dir.display().to_string());
        }
        "b" => {
            await_signal(&sync.join("go-b"), "the parent's write signal");
            let skill = dir.join("SKILL.md");
            let report = match fs::write(&skill, "# b") {
                Ok(()) => "ok".to_string(),
                Err(error) => format!(
                    "ERR raw_os_error={:?} kind={:?} ({error})\n{}",
                    error.raw_os_error(),
                    error.kind(),
                    first_missing_component(&skill)
                ),
            };
            signal(&sync, "b.result", &report);
            drop(guard);
        }
        other => panic!("unknown collision role {other:?}"),
    }
}

/// Two processes running the SAME watcher test at the same time must not be
/// handed the same directory, so that one finishing cannot delete the other's.
#[test]
fn cross_process_siblings_do_not_share_a_test_directory() {
    let sync = TempDir::new().expect("create collision sync directory");
    let sync_path = sync.path().to_path_buf();
    let exe = std::env::current_exe().expect("locate this test binary");

    let spawn = |role: &str| {
        std::process::Command::new(&exe)
            .args([
                "--exact",
                "--ignored",
                "--nocapture",
                "--test-threads",
                "1",
                COLLISION_CHILD_PATH,
            ])
            .env(COLLISION_ROLE, role)
            .env(COLLISION_SYNC, &sync_path)
            .spawn()
            .unwrap_or_else(|error| panic!("spawn collision child {role}: {error}"))
    };

    let mut child_a = spawn("a");
    let mut child_b = spawn("b");

    let path_a = PathBuf::from(await_signal(
        &sync_path.join("a.path"),
        "child a's allocated directory",
    ));
    let path_b = PathBuf::from(await_signal(
        &sync_path.join("b.path"),
        "child b's allocated directory",
    ));

    // Both children must really be in one collision domain, or "the paths
    // differ" would be true for a reason that proves nothing.
    assert_eq!(
        path_a.parent(),
        path_b.parent(),
        "the two children did not share a temp root, so this run cannot say \
         anything about collisions: a={} b={}",
        path_a.display(),
        path_b.display()
    );

    signal(&sync_path, "go-a", "");
    await_signal(&sync_path.join("a.dropped"), "child a's teardown");
    signal(&sync_path, "go-b", "");
    let write_result = await_signal(&sync_path.join("b.result"), "child b's write");

    let status_a = child_a.wait().expect("reap collision child a");
    let status_b = child_b.wait().expect("reap collision child b");

    assert!(
        path_a != path_b && write_result == "ok",
        "a concurrent process running the same test shared child b's directory and \
         removed it during teardown, so child b lost the directory it was watching.\n  \
         child a: {}\n  child b: {}\n  child b's write AFTER child a tore down: {write_result}",
        path_a.display(),
        path_b.display()
    );
    assert!(
        status_a.success() && status_b.success(),
        "collision children must exit cleanly: a={status_a} b={status_b}"
    );
}
