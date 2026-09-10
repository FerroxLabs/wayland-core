use super::*;

#[cfg(target_os = "linux")]
async fn run_fixture_git(repo: &Path, args: &[&str]) {
    let output = fixture_git_output(repo, args).await;
    assert!(
        output.status.success(),
        "fixture git {args:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[cfg(target_os = "linux")]
async fn fixture_git_output(
    repo: &Path,
    args: &[&str],
) -> wcore_sandbox::process_capture::CapturedOutput {
    let mut command = shell::shell_command_argv("git", args);
    command.current_dir(repo);
    capture_bounded_process(
        command,
        CaptureLimits {
            stdout_bytes: 64 * 1024,
            stderr_bytes: 64 * 1024,
            // 25 s, not 5 s (wayland#1247 c4). This bounds FIXTURE SETUP --
            // `git init`, `git commit` -- whose duration nothing here asserts,
            // so a short value buys no signal and only decides whether a
            // loaded runner reds an unrelated test. 25 s still fits inside
            // nextest's 60 s hard kill, so a genuinely wedged fixture git
            // fails with a message instead of a bare harness TIMEOUT; see
            // `read_child_pid` below, where that trade-off was measured.
            timeout: Duration::from_secs(25),
        },
        None,
    )
    .await
    .expect("fixture git command")
}

#[cfg(target_os = "linux")]
fn make_executable(path: &Path, contents: &str) {
    use std::io::Write;
    use std::os::unix::fs::PermissionsExt;

    // Publish the executable only after its writer is closed. Opening the final
    // pathname for write and immediately executing it can race with Linux's
    // executable-write exclusion and surface as ETXTBSY under a parallel test
    // run, even though the fixture bytes are already complete.
    let staged = path.with_extension("fixture-pending");
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&staged)
        .expect("create staged executable fixture");
    file.write_all(contents.as_bytes())
        .expect("write executable fixture");
    file.sync_all().expect("sync executable fixture");
    drop(file);
    let mut permissions = std::fs::metadata(&staged).unwrap().permissions();
    permissions.set_mode(0o700);
    std::fs::set_permissions(&staged, permissions).expect("make staged fixture executable");
    std::fs::rename(&staged, path).expect("publish executable fixture atomically");
}

#[cfg(target_os = "linux")]
async fn init_fixture_repo(path: &Path) {
    run_fixture_git(path, &["init", "-q", "-b", "main"]).await;
    run_fixture_git(
        path,
        &[
            "-c",
            "user.email=swarm-test@example.invalid",
            "-c",
            "user.name=Swarm Test",
            "commit",
            "--allow-empty",
            "-qm",
            "fixture",
        ],
    )
    .await;
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn external_workspace_root_keeps_checkout_outside_parent_repository() {
    let fixture = tempfile::tempdir().expect("fixture");
    let control = tempfile::tempdir().expect("orchestrator control root");
    init_fixture_repo(fixture.path()).await;
    let checkouts = control.path().join("checkouts");
    let manager = WorktreeManager::new_with_workspace_root(fixture.path(), &checkouts)
        .expect("external manager");
    let head = manager.pinned_head().await.expect("pinned head");
    let common = manager.git_common_dir().await.expect("common dir");
    let tree = manager
        .create_worker_tree("child-1", "wayland-child/child-1", &head)
        .await
        .expect("external checkout");

    assert!(tree.starts_with(control.path()));
    assert!(!tree.starts_with(fixture.path()));
    assert!(!fixture.path().join(".swarm-worktrees").exists());
    assert!(common.starts_with(fixture.path()));
    assert!(std::fs::read_to_string(tree.join(".git")).is_ok());
}

#[cfg(target_os = "linux")]
#[test]
fn isolated_checkout_keeps_git_useful_without_parent_history_or_authority() {
    std::thread::Builder::new()
        .name("isolated-checkout-scenario".to_owned())
        .stack_size(16 * 1024 * 1024)
        .spawn(|| {
            tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("scenario runtime")
                .block_on(assert_isolated_checkout_keeps_git_useful());
        })
        .expect("scenario thread")
        .join()
        .expect("isolated checkout scenario");
}

#[cfg(target_os = "linux")]
async fn assert_isolated_checkout_keeps_git_useful() {
    let fixture = tempfile::tempdir().expect("fixture");
    let control = tempfile::tempdir().expect("orchestrator control root");
    init_fixture_repo(fixture.path()).await;
    std::fs::write(fixture.path().join("README.md"), "before\n").unwrap();
    std::fs::write(
        fixture.path().join("historical-secret.txt"),
        "do-not-retain\n",
    )
    .unwrap();
    run_fixture_git(
        fixture.path(),
        &["add", "README.md", "historical-secret.txt"],
    )
    .await;
    run_fixture_git(
        fixture.path(),
        &[
            "-c",
            "user.email=swarm-test@example.invalid",
            "-c",
            "user.name=Swarm Test",
            "commit",
            "-qm",
            "historical secret",
        ],
    )
    .await;
    let secret_commit = String::from_utf8_lossy(
        &fixture_git_output(fixture.path(), &["rev-parse", "HEAD"])
            .await
            .stdout,
    )
    .trim()
    .to_owned();
    std::fs::remove_file(fixture.path().join("historical-secret.txt")).unwrap();
    std::fs::write(fixture.path().join("README.md"), "current\n").unwrap();
    run_fixture_git(fixture.path(), &["add", "-A"]).await;
    run_fixture_git(
        fixture.path(),
        &[
            "-c",
            "user.email=swarm-test@example.invalid",
            "-c",
            "user.name=Swarm Test",
            "commit",
            "-qm",
            "current snapshot",
        ],
    )
    .await;

    let checkouts = control.path().join("checkouts");
    let manager = WorktreeManager::new_with_workspace_root(fixture.path(), &checkouts)
        .expect("external manager");
    let parent_head = manager.pinned_head().await.expect("pinned head");
    let parent_config_before = std::fs::read(fixture.path().join(".git/config")).unwrap();
    let parent_refs_before = fixture_git_output(fixture.path(), &["show-ref"])
        .await
        .stdout;
    let (object_dir, object_file) = parent_head.split_at(2);
    let parent_object = fixture
        .path()
        .join(".git/objects")
        .join(object_dir)
        .join(object_file);
    let parent_object_before = std::fs::read(&parent_object).expect("loose parent commit object");
    let transaction = manager
        .create_isolated_checkout(
            "child-1",
            "wayland-child/child-1",
            &parent_head,
            WorkspaceCapacity {
                available_bytes: u64::MAX,
                safety_margin_bytes: 0,
                max_transaction_bytes: u64::MAX,
                max_aggregate_bytes: u64::MAX,
            },
        )
        .await
        .expect("private checkout");
    let tree = &transaction.checkout;

    assert!(tree.join(".git").is_dir());
    assert!(transaction.scratch.is_dir());
    assert!(!transaction.scratch.starts_with(tree));
    assert!(!tree.join(".git/objects/info/alternates").exists());
    assert!(
        fixture_git_output(tree, &["remote"])
            .await
            .stdout
            .is_empty()
    );
    assert!(
        fixture_git_output(tree, &["tag", "--list"])
            .await
            .stdout
            .is_empty()
    );
    let reachable = fixture_git_output(tree, &["rev-list", "--count", "--all"]).await;
    assert_eq!(String::from_utf8_lossy(&reachable.stdout).trim(), "1");
    let old_commit = fixture_git_output(tree, &["cat-file", "-e", &secret_commit]).await;
    assert!(
        !old_commit.status.success(),
        "parent history leaked into child clone"
    );

    std::fs::write(tree.join("README.md"), "child edit\n").unwrap();
    let status = fixture_git_output(tree, &["status", "--short"]).await;
    assert!(String::from_utf8_lossy(&status.stdout).contains("README.md"));
    let diff = fixture_git_output(tree, &["diff", "--", "README.md"]).await;
    assert!(String::from_utf8_lossy(&diff.stdout).contains("child edit"));
    run_fixture_git(tree, &["add", "README.md"]).await;
    run_fixture_git(
        tree,
        &[
            "-c",
            "user.email=child@example.invalid",
            "-c",
            "user.name=Child",
            "commit",
            "-qm",
            "child-local commit",
        ],
    )
    .await;
    run_fixture_git(tree, &["config", "child.marker", "true"]).await;
    run_fixture_git(tree, &["update-ref", "refs/heads/child-private", "HEAD"]).await;
    let child_hook = tree.join(".git/hooks/pre-commit");
    std::fs::write(&child_hook, "child-only hook\n").unwrap();
    assert!(
        !fixture_git_output(tree, &["reflog", "show", "--all"])
            .await
            .stdout
            .is_empty()
    );

    let child_pack = std::fs::read_dir(tree.join(".git/objects/pack"))
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .find(|path| {
            path.extension()
                .is_some_and(|extension| extension == "pack")
        })
        .expect("child-owned pack");
    std::fs::write(&child_pack, b"corrupt child pack").unwrap();
    assert!(
        !fixture_git_output(tree, &["cat-file", "-e", &parent_head])
            .await
            .status
            .success(),
        "corrupt child object unexpectedly remained valid"
    );
    assert_eq!(manager.pinned_head().await.unwrap(), parent_head);
    assert_eq!(std::fs::read(&parent_object).unwrap(), parent_object_before);
    assert_eq!(
        std::fs::read(fixture.path().join(".git/config")).unwrap(),
        parent_config_before
    );
    assert_eq!(
        fixture_git_output(fixture.path(), &["show-ref"])
            .await
            .stdout,
        parent_refs_before
    );
    assert!(!fixture.path().join(".git/hooks/pre-commit").exists());
    assert!(
        fixture_git_output(fixture.path(), &["fsck", "--full"])
            .await
            .status
            .success(),
        "child Git mutations damaged the parent repository"
    );
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn isolated_checkout_rejects_unproven_capacity_before_materialization() {
    let fixture = tempfile::tempdir().expect("fixture");
    let control = tempfile::tempdir().expect("orchestrator control root");
    init_fixture_repo(fixture.path()).await;
    let manager =
        WorktreeManager::new_with_workspace_root(fixture.path(), &control.path().join("checkouts"))
            .expect("manager");
    let parent_head = manager.pinned_head().await.expect("pinned head");

    let error = manager
        .create_isolated_checkout(
            "child-1",
            "wayland-child/child-1",
            &parent_head,
            WorkspaceCapacity {
                available_bytes: 0,
                safety_margin_bytes: 1,
                max_transaction_bytes: 1024 * 1024 * 1024,
                max_aggregate_bytes: u64::MAX,
            },
        )
        .await
        .expect_err("missing capacity proof must fail")
        .to_string();
    assert!(error.contains("available bytes"), "{error}");
    assert!(!manager.swarm_root().join("child-1").exists());
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn compressible_checkout_is_bounded_by_logical_size_not_git_storage() {
    let fixture = tempfile::tempdir().expect("fixture");
    let control = tempfile::tempdir().expect("orchestrator control root");
    init_fixture_repo(fixture.path()).await;
    std::fs::write(
        fixture.path().join("compressible.bin"),
        vec![0_u8; 1024 * 1024],
    )
    .unwrap();
    run_fixture_git(fixture.path(), &["add", "compressible.bin"]).await;
    run_fixture_git(
        fixture.path(),
        &[
            "-c",
            "user.email=swarm-test@example.invalid",
            "-c",
            "user.name=Swarm Test",
            "commit",
            "-qm",
            "compressible content",
        ],
    )
    .await;
    let manager =
        WorktreeManager::new_with_workspace_root(fixture.path(), &control.path().join("checkouts"))
            .expect("manager");
    let head = manager.pinned_head().await.expect("head");
    let error = manager
        .create_isolated_checkout(
            "child-compressible",
            "wayland-child/child-compressible",
            &head,
            WorkspaceCapacity {
                available_bytes: u64::MAX,
                safety_margin_bytes: 0,
                max_transaction_bytes: 128 * 1024,
                max_aggregate_bytes: u64::MAX,
            },
        )
        .await
        .expect_err("compressed Git storage bypassed the logical checkout bound");
    assert!(
        error.to_string().contains("logical checkout bytes"),
        "{error}"
    );
    assert!(!manager.swarm_root().join("child-compressible").exists());
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn persisted_reservation_enforces_aggregate_budget_and_owned_cleanup() {
    let fixture = tempfile::tempdir().expect("fixture");
    let control = tempfile::tempdir().expect("orchestrator control root");
    init_fixture_repo(fixture.path()).await;
    let manager =
        WorktreeManager::new_with_workspace_root(fixture.path(), &control.path().join("checkouts"))
            .expect("manager");
    let head = manager.pinned_head().await.expect("head");
    let first = manager
        .create_isolated_checkout(
            "child-1",
            "wayland-child/child-1",
            &head,
            WorkspaceCapacity {
                available_bytes: MAX_AGGREGATE_WORKSPACE_BYTES,
                safety_margin_bytes: 0,
                max_transaction_bytes: MAX_TRANSACTION_WORKSPACE_BYTES,
                max_aggregate_bytes: MAX_AGGREGATE_WORKSPACE_BYTES,
            },
        )
        .await
        .expect("first checkout");
    let foreign = control.path().join("foreign");
    std::fs::create_dir(&foreign).unwrap();
    let error = manager
        .create_isolated_checkout(
            "child-2",
            "wayland-child/child-2",
            &head,
            WorkspaceCapacity {
                available_bytes: MAX_AGGREGATE_WORKSPACE_BYTES,
                safety_margin_bytes: 0,
                max_transaction_bytes: MAX_TRANSACTION_WORKSPACE_BYTES,
                max_aggregate_bytes: first.reserved_bytes,
            },
        )
        .await
        .expect_err("aggregate reservation must fail")
        .to_string();
    assert!(error.contains("aggregate workspace budget"), "{error}");
    manager.release_transaction(&first).expect("owned cleanup");
    assert!(!first.root.exists());
    assert!(foreign.is_dir());
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn transaction_workspace_rejects_checkout_and_scratch_replacements() {
    for replaced_name in ["checkout", "scratch"] {
        let fixture = tempfile::tempdir().expect("fixture");
        let control = tempfile::tempdir().expect("orchestrator control root");
        init_fixture_repo(fixture.path()).await;
        let manager = WorktreeManager::new_with_workspace_root(
            fixture.path(),
            &control.path().join("checkouts"),
        )
        .expect("manager");
        let head = manager.pinned_head().await.expect("head");
        let workspace = manager
            .create_isolated_checkout(
                "child-authority",
                "wayland-child/child-authority",
                &head,
                WorkspaceCapacity {
                    available_bytes: u64::MAX,
                    safety_margin_bytes: 0,
                    max_transaction_bytes: u64::MAX,
                    max_aggregate_bytes: u64::MAX,
                },
            )
            .await
            .expect("checkout");
        let replaced = workspace.root.join(replaced_name);
        std::fs::rename(
            &replaced,
            workspace.root.join(format!("{replaced_name}-original")),
        )
        .unwrap();
        std::fs::create_dir(&replaced).unwrap();

        let error = workspace
            .validate_execution_authority()
            .expect_err("same-path replacement retained execution authority");
        assert!(error.to_string().contains("identity changed"), "{error}");
        manager
            .release_transaction(&workspace)
            .expect("owned cleanup after refusal");
    }
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn transaction_workspace_rejects_smaller_same_path_reservation() {
    let fixture = tempfile::tempdir().expect("fixture");
    let control = tempfile::tempdir().expect("orchestrator control root");
    init_fixture_repo(fixture.path()).await;
    let manager =
        WorktreeManager::new_with_workspace_root(fixture.path(), &control.path().join("checkouts"))
            .expect("manager");
    let head = manager.pinned_head().await.expect("head");
    let workspace = manager
        .create_isolated_checkout(
            "child-reservation",
            "wayland-child/child-reservation",
            &head,
            WorkspaceCapacity {
                available_bytes: u64::MAX,
                safety_margin_bytes: 0,
                max_transaction_bytes: 64 * 1024 * 1024,
                max_aggregate_bytes: MAX_AGGREGATE_WORKSPACE_BYTES,
            },
        )
        .await
        .expect("checkout");
    let reservation = workspace.root.join(RESERVATION_FILE);
    let original = workspace.root.join("reservation-original");
    std::fs::rename(&reservation, &original).unwrap();
    std::fs::write(&reservation, "1").unwrap();

    let error = workspace
        .validate_execution_authority()
        .expect_err("smaller same-path reservation retained transaction authority");
    assert!(error.to_string().contains("identity changed"), "{error}");
    let aggregate_error = manager
        .reserved_workspace_bytes()
        .expect_err("active replacement reduced aggregate reservation authority");
    assert!(
        aggregate_error.to_string().contains("identity changed"),
        "{aggregate_error}"
    );

    std::fs::remove_file(&reservation).unwrap();
    std::fs::rename(&original, &reservation).unwrap();
    manager
        .release_transaction(&workspace)
        .expect("owned cleanup after authoritative receipt restoration");
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn valid_held_reservations_preserve_declared_concurrency() {
    let fixture = tempfile::tempdir().expect("fixture");
    let control = tempfile::tempdir().expect("orchestrator control root");
    init_fixture_repo(fixture.path()).await;
    let manager =
        WorktreeManager::new_with_workspace_root(fixture.path(), &control.path().join("checkouts"))
            .expect("manager");
    let head = manager.pinned_head().await.expect("head");
    let reservation = 64 * 1024 * 1024;
    let capacity = WorkspaceCapacity {
        available_bytes: u64::MAX,
        safety_margin_bytes: 0,
        max_transaction_bytes: reservation,
        max_aggregate_bytes: reservation * 2,
    };
    let first = manager
        .create_isolated_checkout("child-small-1", "swarm/child-small-1", &head, capacity)
        .await
        .expect("first small reservation");
    let second = manager
        .create_isolated_checkout("child-small-2", "swarm/child-small-2", &head, capacity)
        .await
        .expect("second small reservation");

    assert_eq!(manager.reserved_workspace_bytes().unwrap(), reservation * 2);

    manager.release_transaction(&first).expect("release first");
    manager
        .release_transaction(&second)
        .expect("release second");
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn exported_checkout_capability_survives_same_path_replacement() {
    let fixture = tempfile::tempdir().expect("fixture");
    let control = tempfile::tempdir().expect("orchestrator control root");
    init_fixture_repo(fixture.path()).await;
    let manager =
        WorktreeManager::new_with_workspace_root(fixture.path(), &control.path().join("checkouts"))
            .expect("manager");
    let head = manager.pinned_head().await.expect("head");
    let workspace = manager
        .create_isolated_checkout(
            "child-capability",
            "wayland-child/child-capability",
            &head,
            WorkspaceCapacity {
                available_bytes: u64::MAX,
                safety_margin_bytes: 0,
                max_transaction_bytes: MAX_TRANSACTION_WORKSPACE_BYTES,
                max_aggregate_bytes: MAX_AGGREGATE_WORKSPACE_BYTES,
            },
        )
        .await
        .expect("checkout");
    let authority = workspace.checkout_authority();
    let original = workspace.root.join("checkout-original");
    std::fs::rename(&workspace.checkout, &original).unwrap();
    std::fs::create_dir(&workspace.checkout).unwrap();

    authority
        .create_child_file("authority-marker", b"retained\n")
        .expect("capability-relative write");

    assert_eq!(
        std::fs::read_to_string(original.join("authority-marker")).unwrap(),
        "retained\n"
    );
    assert!(!workspace.checkout.join("authority-marker").exists());

    std::fs::remove_dir(&workspace.checkout).unwrap();
    std::fs::rename(&original, &workspace.checkout).unwrap();
    manager
        .release_transaction(&workspace)
        .expect("owned cleanup after checkout restoration");
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn failed_post_clone_setup_removes_only_the_owned_partial_transaction() {
    let fixture = tempfile::tempdir().expect("fixture");
    let control = tempfile::tempdir().expect("orchestrator control root");
    init_fixture_repo(fixture.path()).await;
    let manager =
        WorktreeManager::new_with_workspace_root(fixture.path(), &control.path().join("checkouts"))
            .expect("manager");
    let head = manager.pinned_head().await.expect("head");
    let foreign = control.path().join("foreign");
    std::fs::create_dir(&foreign).unwrap();

    let error = manager
        .create_isolated_checkout(
            "child-1",
            "invalid branch name",
            &head,
            WorkspaceCapacity {
                available_bytes: u64::MAX,
                safety_margin_bytes: 0,
                max_transaction_bytes: u64::MAX,
                max_aggregate_bytes: u64::MAX,
            },
        )
        .await
        .expect_err("invalid branch must fail after clone")
        .to_string();
    assert!(error.contains("isolated Git command failed"), "{error}");
    assert!(!manager.swarm_root().join("child-1").exists());
    assert!(foreign.is_dir());
}

/// Wait for a worker descendant to actually be gone.
///
/// This used to poll `/proc/<pid>` for existence, which a **zombie**
/// satisfies — the `/proc` entry outlives the process until something reaps
/// it, so on a host with no reaping init two tests here read a
/// successfully-killed descendant as a survivor. Centralised in
/// `wcore_types::process_liveness`; see `.planning/ZOMBIE-PROBE.md`.
#[cfg(target_os = "linux")]
async fn wait_until_process_gone(pid: u32) {
    use wcore_types::process_liveness::{process_is_alive, process_liveness};

    // 25 s, not 3 s (wayland#1247 c4). This is a LIVENESS BACKSTOP, not a
    // performance assertion: nothing here claims cleanup is fast, only that it
    // happens. A 3 s bound turned a slow-but-correct reap on a loaded runner
    // into `process <pid> survived cleanup`, which reads as a containment
    // failure and is not one. 25 s is the same figure `read_child_pid` below
    // arrived at, and for the same measured reason -- it fits inside nextest's
    // 60 s hard kill, so the failure carries this message rather than a bare
    // harness TIMEOUT that cannot tell a hang from a slow host.
    let deadline = tokio::time::Instant::now() + Duration::from_secs(25);
    while process_is_alive(pid) && tokio::time::Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(
        !process_is_alive(pid),
        "process {pid} survived cleanup (state: {:?})",
        process_liveness(pid)
    );
}

/// One-shot read of a PID written by a fixture git script. `None` while the
/// value is not there YET — file absent, or present but not yet holding a
/// parseable number.
///
/// The "present but incomplete" case is the whole reason this exists, and it is
/// not theoretical. Every fixture script publishes its child PID with
/// `echo "$child" > "$WAYLAND_TEST_PID_FILE"`, and a `>` redirection CREATES
/// AND TRUNCATES the file when the shell sets the redirection up — strictly
/// before `echo` puts any bytes in it. So there is a window in which the file
/// exists and is zero bytes long.
///
/// `status_output_cap_kills_git_descendant` lands in that window: its child
/// floods stdout in a busy loop, so `assert_clean()` can hit the 4096-byte cap
/// and return while the parent shell has not finished writing the PID. The test
/// then did `read_to_string(..).unwrap().parse::<u32>().unwrap()` and panicked
/// with `ParseIntError { kind: Empty }` — in 0.057s, so this was never a timeout.
///
/// Measured on hetzner-dsm 2026-07-31: the test passes 16/16 run alone and
/// failed in 2 of 3 full-workspace runs (13,609 tests, 592 binaries), all three
/// attempts of `retries = 2` included. Load widens the gap between the
/// redirection and the write; it does not create it. Checking `.exists()` first
/// does not help, because `exists()` is true for the empty file.
///
/// THE TERMINATOR IS THE ATOMICITY, and it is deliberately NOT a rename. The
/// fixtures published to `$F.tmp` and `mv`-ed into place for one day, and `mv`
/// is an EXTERNAL BINARY: a publish that had cost one `write(2)` inside a shell
/// whose process tree is killed mid-flight became a `fork`, an `exec`, a `PATH`
/// resolution and a second path operation. `status_output_cap_kills_git_
/// descendant` then HARD-FAILED BOTH outer attempts of `CI (linux-container-
/// ized)` in run 33713740549 with `exists: false` — the final path never
/// appeared at all — while every main-based tree in the same window passed that
/// test. `echo` is a shell BUILTIN and supplies the newline itself, so the
/// publish is one write with a terminator on the end: a reader that requires
/// the terminator can never observe a prefix of the pid, and nothing new has to
/// resolve on `PATH` or outlive a fork.
#[cfg(target_os = "linux")]
fn try_read_child_pid(path: &std::path::Path) -> Option<u32> {
    let record = std::fs::read_to_string(path).ok()?;
    // A record with no terminator is a PARTIAL WRITE, not a short pid: `1234`
    // observed as `12` parses happily and then names an unrelated process.
    if !record.ends_with('\n') {
        return None;
    }
    record.trim().parse::<u32>().ok()
}

/// Poll until a fixture script's child PID is readable, or fail loudly.
///
/// Deliberately a bounded wait and NOT a bare retry-forever: if the script never
/// writes a PID that is a real failure and must still fail, just not by racing.
///
/// The bound is a LIVENESS BACKSTOP, not the property under test. What these
/// tests assert is that the script DID write a pid and that the process was
/// reaped -- never that it managed it inside some budget. A 3s deadline made
/// the budget the assertion, so a loaded CI box failed the test for being busy:
/// `linux.rs:693` was the single most frequent red in `CI (linux-containerized)`
/// and it reddened `report` for lanes that had not touched this crate
/// (wayland#1247). The window it was fighting is now closed at the source --
/// the fixtures terminate the pid with a newline and `try_read_child_pid`
/// refuses an unterminated record -- and what remains is only "has the shell
/// reached that line yet", which under full-workspace load is a scheduling
/// question with no honest short answer.
///
/// 25s is chosen so it can only fire on a genuine hang while STILL FITTING
/// INSIDE THE HARNESS'S OWN KILL. The default nextest profile is
/// `slow-timeout = { period = "30s", terminate-after = 2 }`, i.e. a 60s hard
/// kill, so a 60s deadline here is exactly the budget nextest allows and this
/// assertion can never reach a developer running the default profile: the
/// message that says "this is a hang, not a slow runner" was replaced by a bare
/// `TIMEOUT [60.005s]`. Measured, mutating the fixture to publish an
/// unterminated record: at 60s the run reports only the harness timeout; the
/// diagnostic below is what makes that failure readable. 25s still leaves ~550x
/// the isolated pass time (0.045s) and ~8x the 3s budget that was too tight,
/// and a generous value costs nothing on a passing run because the loop returns
/// the instant the pid appears.
#[cfg(target_os = "linux")]
async fn read_child_pid(path: &std::path::Path) -> u32 {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(25);
    loop {
        if let Some(pid) = try_read_child_pid(path) {
            return pid;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "fixture script never wrote a parseable child PID to {} within 25s \
             -- that is a hang, not a slow runner (exists: {}, contents: {:?})",
            path.display(),
            path.exists(),
            std::fs::read_to_string(path).ok()
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn local_checkout_filter_is_refused_before_execution() {
    let fixture = tempfile::tempdir().expect("fixture");
    init_fixture_repo(fixture.path()).await;
    let filter = fixture.path().join("evil-filter.sh");
    make_executable(&filter, "#!/bin/sh\nprintf executed > \"${0}.ran\"\ncat\n");
    run_fixture_git(
        fixture.path(),
        &["config", "filter.evil.smudge", &filter.to_string_lossy()],
    )
    .await;

    let manager = WorktreeManager::new(fixture.path()).expect("manager");
    let error = manager
        .create_worker_tree("worker-1", "swarm/worker-1", "HEAD")
        .await
        .expect_err("filter must fail closed")
        .to_string();
    assert!(error.contains("filter.evil.smudge"), "{error}");
    assert!(!PathBuf::from(format!("{}.ran", filter.display())).exists());
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn ambient_global_filter_is_ignored_and_repository_hook_is_disabled() {
    let fixture = tempfile::tempdir().expect("fixture");
    init_fixture_repo(fixture.path()).await;
    std::fs::write(fixture.path().join("README.md"), "safe checkout\n").unwrap();
    std::fs::write(
        fixture.path().join(".gitattributes"),
        "*.payload filter=evil\n",
    )
    .unwrap();
    std::fs::write(fixture.path().join("canary.payload"), "safe input\n").unwrap();
    run_fixture_git(
        fixture.path(),
        &["add", "README.md", ".gitattributes", "canary.payload"],
    )
    .await;
    run_fixture_git(
        fixture.path(),
        &[
            "-c",
            "user.email=swarm-test@example.invalid",
            "-c",
            "user.name=Swarm Test",
            "commit",
            "-qm",
            "content",
        ],
    )
    .await;
    let filter = fixture.path().join("global-filter.sh");
    make_executable(&filter, "#!/bin/sh\nprintf executed > \"${0}.ran\"\ncat\n");
    let hostile_global = fixture.path().join("hostile.config");
    std::fs::write(
        &hostile_global,
        format!("[filter \"evil\"]\n\tsmudge = {}\n", filter.display()),
    )
    .unwrap();
    let hook = fixture.path().join(".git/hooks/post-checkout");
    make_executable(&hook, "#!/bin/sh\nprintf executed > \"${0}.ran\"\n");

    let mut manager = WorktreeManager::new(fixture.path()).expect("manager");
    manager.set_ambient_git_env("GIT_CONFIG_GLOBAL", hostile_global.as_os_str());
    let tree = manager
        .create_worker_tree("worker-1", "swarm/worker-1", "HEAD")
        .await
        .expect("protected checkout");
    assert!(tree.join("README.md").is_file());
    assert!(tree.join("canary.payload").is_file());
    assert!(!PathBuf::from(format!("{}.ran", filter.display())).exists());
    assert!(!PathBuf::from(format!("{}.ran", hook.display())).exists());
    manager
        .cleanup_all(&CancellationToken::new())
        .await
        .unwrap();
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn linked_worker_destination_and_cleanup_entry_never_touch_external_target() {
    use std::os::unix::fs::symlink;

    let fixture = tempfile::tempdir().expect("fixture");
    init_fixture_repo(fixture.path()).await;
    let external = tempfile::tempdir().expect("external target");
    let manager = WorktreeManager::new(fixture.path()).expect("manager");
    let linked = manager.swarm_root().join("worker-1");
    symlink(external.path(), &linked).expect("plant linked worker entry");

    let create_error = manager
        .create_worker_tree("worker-1", "swarm/worker-1", "HEAD")
        .await
        .expect_err("linked destination must fail before Git")
        .to_string();
    assert!(
        create_error.contains("existing or linked"),
        "{create_error}"
    );

    let cleanup_error = manager
        .cleanup_all(&CancellationToken::new())
        .await
        .expect_err("linked cleanup entry must be reported")
        .to_string();
    assert!(
        cleanup_error.contains("linked cleanup entry"),
        "{cleanup_error}"
    );
    assert!(cleanup_error.contains(&linked.display().to_string()));
    assert!(linked.is_symlink());
    assert_eq!(std::fs::read_dir(external.path()).unwrap().count(), 0);
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn conditional_include_is_refused_before_checkout() {
    use std::io::Write;

    let fixture = tempfile::tempdir().expect("fixture");
    init_fixture_repo(fixture.path()).await;
    let included = fixture.path().join("included.config");
    std::fs::write(&included, "[filter \"evil\"]\n\tsmudge = false\n").unwrap();
    let mut local = std::fs::OpenOptions::new()
        .append(true)
        .open(fixture.path().join(".git/config"))
        .unwrap();
    writeln!(
        local,
        "[includeIf \"gitdir:{}/**\"]\n\tpath = {}",
        fixture.path().join(".swarm-worktrees").display(),
        included.display()
    )
    .unwrap();

    let manager = WorktreeManager::new(fixture.path()).expect("manager");
    let error = manager
        .create_worker_tree("worker-1", "swarm/worker-1", "HEAD")
        .await
        .expect_err("conditional include must fail closed")
        .to_string();
    assert!(error.contains("includeif."), "{error}");
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn worktree_config_filter_and_include_are_refused_before_checkout() {
    let fixture = tempfile::tempdir().expect("fixture");
    init_fixture_repo(fixture.path()).await;
    std::fs::write(
        fixture.path().join(".gitattributes"),
        "*.payload filter=evil\n",
    )
    .unwrap();
    std::fs::write(fixture.path().join("canary.payload"), "safe input\n").unwrap();
    run_fixture_git(fixture.path(), &["add", ".gitattributes", "canary.payload"]).await;
    run_fixture_git(
        fixture.path(),
        &[
            "-c",
            "user.email=swarm-test@example.invalid",
            "-c",
            "user.name=Swarm Test",
            "commit",
            "-qm",
            "content",
        ],
    )
    .await;

    let filter = fixture.path().join("worktree-filter.sh");
    make_executable(&filter, "#!/bin/sh\nprintf executed > \"${0}.ran\"\ncat\n");
    let included = fixture.path().join("worktree-include.config");
    std::fs::write(&included, "[filter \"evil\"]\n\tsmudge = false\n").unwrap();
    run_fixture_git(
        fixture.path(),
        &["config", "extensions.worktreeConfig", "true"],
    )
    .await;
    run_fixture_git(
        fixture.path(),
        &[
            "config",
            "--worktree",
            "filter.evil.smudge",
            &filter.to_string_lossy(),
        ],
    )
    .await;
    run_fixture_git(
        fixture.path(),
        &[
            "config",
            "--worktree",
            "include.path",
            &included.to_string_lossy(),
        ],
    )
    .await;

    let manager = WorktreeManager::new(fixture.path()).expect("manager");
    let error = manager
        .create_worker_tree("worker-1", "swarm/worker-1", "HEAD")
        .await
        .expect_err("worktree checkout config must fail closed")
        .to_string();
    assert!(error.contains("--worktree"), "{error}");
    assert!(error.contains("filter.evil.smudge"), "{error}");
    assert!(error.contains("include.path"), "{error}");
    assert!(!PathBuf::from(format!("{}.ran", filter.display())).exists());
    assert!(!manager.swarm_root().join("worker-1").exists());
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn status_output_cap_kills_git_descendant() {
    let fixture = tempfile::tempdir().expect("fixture");
    let pid_file = fixture.path().join("flood-child.pid");
    let mut manager = WorktreeManager::new_with_git_script_and_limits(
        fixture.path(),
        // Publish the PID before allowing output to trigger process-tree cleanup.
        // Otherwise the child can fill the cap before the parent reaches echo.
        "case \" $* \" in *\" config \"*) exit 1;; esac\n(while [ ! -f \"$WAYLAND_TEST_PID_FILE.ready\" ]; do :; done; while :; do printf 0123456789abcdef; done) &\nchild=$!\necho \"$child\" > \"$WAYLAND_TEST_PID_FILE\"\n: > \"$WAYLAND_TEST_PID_FILE.ready\"\nwait \"$child\"",
        CaptureLimits {
            stdout_bytes: 4096,
            stderr_bytes: 4096,
            timeout: Duration::from_secs(2),
        },
    )
    .unwrap();
    manager.set_ambient_git_env("WAYLAND_TEST_PID_FILE", pid_file.as_os_str());
    let error = manager.assert_clean().await.unwrap_err().to_string();
    assert!(error.contains("stdout exceeded the 4096-byte"), "{error}");
    let pid = read_child_pid(&pid_file).await;
    wait_until_process_gone(pid).await;
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn worktree_add_timeout_kills_tree_and_reports_preserved_residual() {
    let fixture = tempfile::tempdir().expect("fixture");
    let pid_file = fixture.path().join("hung-child.pid");
    let mut manager = WorktreeManager::new_with_git_script_and_limits(
        fixture.path(),
        // The hung grandchild BLOCKS rather than busy-spinning. It simulates a
        // wedged git at least as faithfully — the point is a process that is
        // alive and will not exit on its own — and it costs no CPU, so a test
        // binary that is SIGKILLed mid-run leaves an idle process instead of a
        // permanent core-burner. Five such orphans were found on the shared
        // build host at PPID 1, alive 7d11h, each pinning ~99% of a core.
        // `sleep 2147483647` is portable to plain `sh`; `sleep infinity` is a
        // GNU extension and is deliberately not used.
        "case \" $* \" in *\" config \"*) : > \"$WAYLAND_TEST_CONFIG_ACK\"; exit 1;; esac\nmkdir -p .swarm-worktrees/worker-1\n(sleep 2147483647) &\nchild=$!\necho \"$child\" > \"$WAYLAND_TEST_PID_FILE\"\nwait \"$child\"",
        CaptureLimits {
            stdout_bytes: 4096,
            stderr_bytes: 4096,
            // WHAT THIS BUDGET NOW BOUNDS, and what it no longer decides.
            //
            // It used to decide WHICH STAGE timed out. It applied to every git
            // invocation including the `git config` safety check that runs
            // first, so at 200 ms a loaded runner spent the whole budget
            // spawning that fast stage: the timeout fired at CONFIG, before
            // `mkdir -p .swarm-worktrees/worker-1` had run, and the residual
            // assertion failed while the "timed out" assertion still passed
            // (wayland#1247). Load was choosing what the test measured.
            //
            // That is FIXED AT ITS CAUSE, not by this number: the safety check
            // now carries its own floor (`config_check_limits`, 30 s) and can
            // no longer be starved by a small operation budget, whatever this
            // value is. `config_safety_check_outlives_a_short_worktree_budget`
            // below is the control -- it makes the config stage take THREE
            // TIMES this budget and still reaches `git worktree add`.
            //
            // What is left for this value to bound is the stage under test:
            // spawn, `mkdir`, fork the grandchild, publish its pid, then block
            // forever. It must clear that setup and nothing more.
            timeout: Duration::from_secs(2),
        },
    )
    .unwrap();
    manager.set_ambient_git_env("WAYLAND_TEST_PID_FILE", pid_file.as_os_str());
    let config_ack = fixture.path().join("config.ack");
    manager.set_ambient_git_env("WAYLAND_TEST_CONFIG_ACK", config_ack.as_os_str());
    let error = manager
        .create_worker_tree("worker-1", "swarm/worker-1", "HEAD")
        .await
        .unwrap_err()
        .to_string();
    // The config stage RAN TO COMPLETION -- an event the fixture records, not
    // an elapsed duration inferred after the fact. This is the assertion that
    // makes the stage pin below mean something: without it, "the error is not
    // a config timeout" is equally true of a run where the config stage never
    // started.
    assert!(
        config_ack.exists(),
        "the `git config` safety check did not run to completion, so this run \
         never reached the `git worktree add` stage it exists to measure"
    );
    // PIN THE STAGE. Asserting only "timed out" cannot tell the intended
    // `git worktree add` timeout from a `git config safety check` timeout, and
    // those are different outcomes: the second means the test never reached the
    // behaviour it exists to check. Without this, a config-stage timeout was
    // reported as a missing residual -- a red naming the wrong cause.
    assert!(
        !error.contains("git config safety check"),
        "the SAFETY CHECK timed out, not `git worktree add` -- this test never \
         reached the behaviour it asserts, so a residual could not exist: {error}"
    );
    assert!(error.contains("timed out after"), "{error}");
    assert!(
        error.contains("residual worktree path preserved"),
        "{error}"
    );
    assert!(manager.swarm_root().join("worker-1").is_dir());
    let pid = read_child_pid(&pid_file).await;
    wait_until_process_gone(pid).await;
}

/// CONTROL for wayland#1247 c2: which stage times out is a property of the
/// stages, not of the host.
///
/// The sibling above reds when the `git config` safety check spends the
/// operation budget before `git worktree add` is ever reached. Under load that
/// took a runner slow enough to burn 200 ms on a process spawn, which is
/// exactly the sort of condition a test cannot summon on demand -- so this one
/// summons it deterministically instead: the config branch sleeps THREE
/// SECONDS per scope against a TWO SECOND operation budget. On the pre-fix
/// code, where the check shared `capture_limits`, that is an unconditional
/// `git config safety check ... timed out`. With the floor in
/// `config_check_limits` it is not, and control reaches the stage under test.
///
/// This is deliberately NOT closed by widening the budget: the budget below is
/// the same 2 s as the sibling, and it is the CONFIG stage that got its own
/// floor. Raising this number would make the test pass for the wrong reason.
#[cfg(target_os = "linux")]
#[tokio::test]
async fn config_safety_check_outlives_a_short_worktree_budget() {
    let fixture = tempfile::tempdir().expect("fixture");
    let pid_file = fixture.path().join("slow-config.pid");
    let config_ack = fixture.path().join("config.ack");
    let mut manager = WorktreeManager::new_with_git_script_and_limits(
        fixture.path(),
        // `sleep 3` per config scope, against the 2 s budget below. The two
        // scopes (`--local`, `--worktree`) make it 6 s of config stage in
        // total, three times the budget the caller set.
        "case \" $* \" in *\" config \"*) sleep 3; : > \"$WAYLAND_TEST_CONFIG_ACK\"; exit 1;; esac\nmkdir -p .swarm-worktrees/worker-1\n(sleep 2147483647) &\nchild=$!\necho \"$child\" > \"$WAYLAND_TEST_PID_FILE\"\nwait \"$child\"",
        CaptureLimits {
            stdout_bytes: 4096,
            stderr_bytes: 4096,
            timeout: Duration::from_secs(2),
        },
    )
    .unwrap();
    manager.set_ambient_git_env("WAYLAND_TEST_PID_FILE", pid_file.as_os_str());
    manager.set_ambient_git_env("WAYLAND_TEST_CONFIG_ACK", config_ack.as_os_str());
    let error = manager
        .create_worker_tree("worker-1", "swarm/worker-1", "HEAD")
        .await
        .unwrap_err()
        .to_string();
    assert!(
        config_ack.exists(),
        "the config stage was cut short by the operation budget: {error}"
    );
    assert!(
        !error.contains("git config safety check"),
        "a config stage three times longer than the operation budget still \
         must not be the stage that times out: {error}"
    );
    assert!(error.contains("timed out after"), "{error}");
    assert!(
        error.contains("residual worktree path preserved"),
        "{error}"
    );
    let pid = read_child_pid(&pid_file).await;
    wait_until_process_gone(pid).await;
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn cancelled_cleanup_kills_git_and_reports_residual() {
    let fixture = tempfile::tempdir().expect("fixture");
    let pid_file = fixture.path().join("hung-cleanup.pid");
    let mut manager = WorktreeManager::new_with_git_script_and_limits(
        fixture.path(),
        // Blocks instead of busy-spinning, for the reason given on the hung
        // grandchild above: an interrupted run must not leave a core-burner on
        // a shared host. The recorded pid is this shell's own, and it stays
        // alive and unkillable-by-itself either way.
        "echo \"$$\" > \"$WAYLAND_TEST_PID_FILE\"\nsleep 2147483647",
        GIT_CAPTURE_LIMITS,
    )
    .unwrap();
    manager.set_ambient_git_env("WAYLAND_TEST_PID_FILE", pid_file.as_os_str());
    let residual = manager.swarm_root().join("worker-still-present");
    std::fs::create_dir(&residual).unwrap();
    let cancel = CancellationToken::new();
    let cleanup = manager.cleanup_all(&cancel);
    tokio::pin!(cleanup);
    // Wait for a PARSEABLE pid, not merely for the path to exist. `>` creates the
    // file empty before `printf` writes to it, so `exists()` goes true one step
    // too early and the read below could still land on zero bytes — the same race
    // that made `status_output_cap_kills_git_descendant` flaky. The select! is
    // kept because this test must also fail loudly if cleanup returns before the
    // cancellation, which a bare poll would silently wait out.
    let pid = loop {
        if let Some(pid) = try_read_child_pid(&pid_file) {
            break pid;
        }
        tokio::select! {
            result = &mut cleanup => panic!("cleanup returned before cancellation: {result:?}"),
            _ = tokio::time::sleep(Duration::from_millis(10)) => {}
        }
    };
    cancel.cancel();
    // 25 s, not 1 s (wayland#1247 c4). The assertion is that cancellation
    // UNBLOCKS cleanup, not that it does so within a second; the bound exists
    // only so a cleanup that never returns fails loudly instead of hanging the
    // suite. Same 25 s liveness figure as the two backstops above.
    let error = tokio::time::timeout(Duration::from_secs(25), &mut cleanup)
        .await
        .expect("cleanup remained blocked")
        .unwrap_err()
        .to_string();
    assert!(
        error.contains("cleanup escalated by cancellation"),
        "{error}"
    );
    assert!(error.contains(&residual.display().to_string()), "{error}");
    wait_until_process_gone(pid).await;
}

/// wayland#1247 c4: the FAMILY is closed, not the two instances that were
/// noticed.
///
/// Two hard-coded short deadlines in this file were found one at a time --
/// `read_child_pid`'s 3 s poll from CI, and `worktree_add_timeout_…`'s budget
/// on the first reproduction attempt. Finding them one per cycle is the defect;
/// a grep run once by a human closes today's instances and nothing else. This
/// test is that grep, run by the suite, so a THIRD one cannot be added quietly.
///
/// The rule, and why each half of it is where the line falls:
///
/// * A `Duration::from_millis(..)` here is a POLL INTERVAL. It bounds how
///   often a loop looks, never how long it is allowed to take, so a small
///   value costs nothing. Capped anyway, because a large one would be a
///   deadline wearing a poll interval's spelling.
/// * A `Duration::from_secs(..)` is a DEADLINE. Under `LIVENESS_FLOOR_SECS` it
///   is a bet on how fast the host is, and this file has lost that bet twice.
///   The one legitimate exception is a `CaptureLimits { timeout: .. }`, which
///   is not a bet about the host at all: it is the budget the test is FEEDING
///   THE PRODUCT to make it time out, i.e. the behaviour under test. Those are
///   spelled `timeout:` and are exempt for that reason alone.
#[test]
fn no_hard_coded_short_deadline_remains_in_this_file() {
    /// Below this, a `from_secs` deadline is a guess about host speed. 25 s is
    /// the figure `read_child_pid` measured: wide enough to survive a loaded
    /// runner, inside nextest's 60 s hard kill so the failure still carries a
    /// diagnostic.
    const LIVENESS_FLOOR_SECS: u64 = 25;
    /// Above this, a `from_millis` value has stopped being a poll interval.
    const POLL_INTERVAL_CEILING_MS: u64 = 50;

    let source = include_str!("linux.rs");
    let mut secs_deadlines = 0usize;
    let mut secs_budgets = 0usize;
    let mut poll_intervals = 0usize;
    let mut offenders: Vec<String> = Vec::new();

    for (index, raw) in source.lines().enumerate() {
        // Prose is not code. A doc comment that QUOTES a deadline must not be
        // read as one -- that mistake has graded a comment as a live call site
        // before now.
        if raw.trim_start().starts_with("//") {
            continue;
        }
        for (unit, ceiling_is_floor) in [("from_secs", true), ("from_millis", false)] {
            let needle = format!("Duration::{unit}(");
            let mut cursor = 0usize;
            while let Some(found) = raw[cursor..].find(&needle) {
                let start = cursor + found;
                let arg_start = start + needle.len();
                let arg_end = match raw[arg_start..].find(')') {
                    Some(offset) => arg_start + offset,
                    None => break,
                };
                let arg = raw[arg_start..arg_end].trim().replace('_', "");
                cursor = arg_end;
                let Ok(value) = arg.parse::<u64>() else {
                    continue;
                };
                let line = index + 1;
                if ceiling_is_floor {
                    if raw[..start].trim_end().ends_with("timeout:") {
                        secs_budgets += 1;
                        continue;
                    }
                    secs_deadlines += 1;
                    if value < LIVENESS_FLOOR_SECS {
                        offenders.push(format!(
                            "linux.rs:{line}: Duration::from_secs({value}) is a deadline \
                             under the {LIVENESS_FLOOR_SECS}s liveness floor -- either raise \
                             it or make it a `CaptureLimits {{ timeout: .. }}` budget that \
                             the test is deliberately feeding the product"
                        ));
                    }
                } else {
                    poll_intervals += 1;
                    if value > POLL_INTERVAL_CEILING_MS {
                        offenders.push(format!(
                            "linux.rs:{line}: Duration::from_millis({value}) is too long to \
                             be a poll interval -- if it is a deadline, spell it in seconds \
                             and clear the {LIVENESS_FLOOR_SECS}s floor"
                        ));
                    }
                }
            }
        }
    }

    // POSITIVE CONTROL. A scanner that matches nothing passes this test while
    // proving nothing, which is the failure mode a self-grep is most prone to.
    // Each class must be non-empty, so a change that breaks the scan (a moved
    // file, a renamed constructor, a `use Duration::*` import shortening the
    // call) reds here instead of silently certifying an unscanned file.
    assert!(
        secs_deadlines >= 3,
        "scanner found only {secs_deadlines} second-valued deadlines; it has \
         stopped matching this file"
    );
    assert!(
        secs_budgets >= 2,
        "scanner found only {secs_budgets} `timeout:` budgets; the exemption \
         branch is unexercised and the rule above is untested"
    );
    assert!(
        poll_intervals >= 2,
        "scanner found only {poll_intervals} poll intervals; it has stopped \
         matching this file"
    );
    assert!(
        offenders.is_empty(),
        "hard-coded short deadlines remain in this file:\n{}",
        offenders.join("\n")
    );
}
