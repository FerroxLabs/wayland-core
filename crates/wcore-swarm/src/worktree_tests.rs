use super::*;

fn test_transaction_cleanup(
    owner: &str,
    root: &Path,
    swarm_root: &Path,
    quarantine_root: &Path,
) -> Arc<TransactionCleanup> {
    let reserved_bytes = 1;
    let reservation_path = root.join(RESERVATION_FILE);
    std::fs::write(&reservation_path, reserved_bytes.to_string()).unwrap();
    let root_authority = DirectoryAuthority::open(root).unwrap();
    let reservation_authority = Arc::new(RegularFileAuthority::open(&reservation_path).unwrap());
    let active_reservations = Arc::new(StdMutex::new(HashMap::new()));
    let cleanup = Arc::new(TransactionCleanup {
        owner: owner.to_owned(),
        root: root.to_path_buf(),
        root_authority: StdMutex::new(Some(root_authority)),
        swarm_root: swarm_root.to_path_buf(),
        swarm_authority: DirectoryAuthority::open(swarm_root).unwrap(),
        quarantine_root: quarantine_root.to_path_buf(),
        quarantine_authority: DirectoryAuthority::open(quarantine_root).unwrap(),
        reservation_authority: Arc::clone(&reservation_authority),
        reserved_bytes,
        active_reservations: Arc::clone(&active_reservations),
        release_lock: StdMutex::new(()),
        lease: StdMutex::new(None),
        released: AtomicBool::new(false),
    });
    active_reservations.lock().unwrap().insert(
        owner.to_owned(),
        ActiveReservation {
            root_identity: cleanup.root_authority().unwrap().identity_token(),
            authority: Arc::clone(&reservation_authority),
            bytes: reserved_bytes,
        },
    );
    cleanup
}

#[test]
fn reservation_receipts_fail_closed_on_missing_zero_range_and_size() {
    let fixture = tempfile::tempdir().expect("fixture");
    let reservation = fixture.path().join("reservation");

    assert!(read_workspace_reservation(&reservation).is_err());
    for rejected in ["0", "8589934593", "not-a-number"] {
        std::fs::write(&reservation, rejected).unwrap();
        assert!(
            read_workspace_reservation(&reservation).is_err(),
            "reservation {rejected:?} was accepted"
        );
    }
    std::fs::write(&reservation, "1".repeat(65)).unwrap();
    let error = read_workspace_reservation(&reservation)
        .expect_err("oversized reservation receipt was accepted");
    assert!(error.to_string().contains("exceeds 64 bytes"), "{error}");
}

#[cfg(unix)]
#[test]
fn retained_reservation_rejects_same_inode_truncate_and_rewrite() {
    use std::os::unix::fs::MetadataExt;

    let fixture = tempfile::tempdir().expect("fixture");
    let reservation = fixture.path().join("reservation");
    std::fs::write(&reservation, "8192").unwrap();
    let authority = RegularFileAuthority::open(&reservation).unwrap();
    let inode = std::fs::metadata(&reservation).unwrap().ino();

    std::fs::write(&reservation, "1").unwrap();
    assert_eq!(std::fs::metadata(&reservation).unwrap().ino(), inode);
    let error = validate_reservation_authority(&authority, &reservation, 8192)
        .expect_err("same-inode reservation rewrite retained its original authority");
    assert!(error.to_string().contains("changed"), "{error}");
}

#[test]
fn failed_transaction_cleanup_remains_retryable() {
    let fixture = tempfile::tempdir().expect("fixture");
    let swarm_root = fixture.path().join("swarm");
    let root = swarm_root.join("worker-retry");
    std::fs::create_dir_all(&root).unwrap();
    let quarantine_root = fixture.path().join("control");
    std::fs::create_dir(&quarantine_root).unwrap();
    let original_swarm = fixture.path().join("swarm-original");
    let cleanup = test_transaction_cleanup("worker-retry", &root, &swarm_root, &quarantine_root);

    std::fs::rename(&swarm_root, &original_swarm).unwrap();
    std::fs::create_dir(&swarm_root).unwrap();
    let first = cleanup
        .release()
        .expect_err("replaced control root accepted");
    assert!(first.to_string().contains("identity changed"), "{first}");
    assert!(
        original_swarm.join("worker-retry").exists(),
        "failed cleanup discarded owned evidence"
    );
    assert!(!cleanup.released.load(Ordering::Acquire));

    std::fs::remove_dir(&swarm_root).unwrap();
    std::fs::rename(&original_swarm, &swarm_root).unwrap();
    cleanup.release().expect("retry cleanup");
    assert!(!root.exists());
    assert!(cleanup.released.load(Ordering::Acquire));
}

#[test]
fn transaction_cleanup_preserves_same_path_replacement() {
    let fixture = tempfile::tempdir().expect("fixture");
    let swarm_root = fixture.path().join("swarm");
    let root = swarm_root.join("worker-replaced");
    let moved = swarm_root.join("worker-original");
    std::fs::create_dir_all(&root).unwrap();
    let quarantine_root = fixture.path().join("control");
    std::fs::create_dir(&quarantine_root).unwrap();
    let cleanup = test_transaction_cleanup("worker-replaced", &root, &swarm_root, &quarantine_root);
    std::fs::rename(&root, &moved).unwrap();
    std::fs::create_dir(&root).unwrap();
    std::fs::write(root.join("replacement-receipt"), "preserve-me\n").unwrap();

    cleanup
        .release()
        .expect("handle-bound cleanup should remove only the opened transaction");
    assert_eq!(
        std::fs::read_to_string(root.join("replacement-receipt")).unwrap(),
        "preserve-me\n"
    );
    assert!(
        !moved.exists(),
        "owned transaction was not cleaned by handle"
    );
    assert!(cleanup.released.load(Ordering::Acquire));
}

#[test]
fn transaction_cleanup_never_deletes_swap_after_validation() {
    let fixture = tempfile::tempdir().expect("fixture");
    let swarm_root = fixture.path().join("swarm");
    let root = swarm_root.join("worker-race");
    let moved = swarm_root.join("worker-original");
    let quarantine_root = fixture.path().join("control");
    std::fs::create_dir_all(&root).unwrap();
    std::fs::create_dir(&quarantine_root).unwrap();
    let swarm_authority = DirectoryAuthority::open(&swarm_root).unwrap();
    let root_authority = DirectoryAuthority::open(&root).unwrap();
    let quarantine_authority = DirectoryAuthority::open(&quarantine_root).unwrap();

    let result = remove_transaction_root_inner(
        &swarm_root,
        &swarm_authority,
        "worker-race",
        &root,
        root_authority,
        &quarantine_root,
        &quarantine_authority,
        || {
            std::fs::rename(&root, &moved).unwrap();
            std::fs::create_dir(&root).unwrap();
            std::fs::write(root.join("replacement-receipt"), "preserve-race\n").unwrap();
        },
    );
    result.expect("handle-bound cleanup should ignore the replacement pathname");
    assert_eq!(
        std::fs::read_to_string(root.join("replacement-receipt")).unwrap(),
        "preserve-race\n"
    );
    assert_eq!(
        std::fs::read_dir(&quarantine_root).unwrap().count(),
        0,
        "cleanup left the replacement or placeholder in control storage"
    );
    assert!(!moved.exists(), "owned original was not deleted by handle");
}

#[test]
fn git_commands_clear_ambient_overrides_and_disable_checkout_hooks() {
    let fixture = tempfile::tempdir().expect("fixture");
    let manager = WorktreeManager::new(fixture.path()).expect("worktree manager");
    let command = manager.git_command(&["status", "--porcelain"]);
    let command = command.as_std();
    let args = command
        .get_args()
        .map(|arg| arg.to_string_lossy().into_owned())
        .collect::<Vec<_>>();
    let env = command
        .get_envs()
        .map(|(key, value)| {
            (
                key.to_string_lossy().into_owned(),
                value.map(|value| value.to_string_lossy().into_owned()),
            )
        })
        .collect::<std::collections::HashMap<_, _>>();

    assert_eq!(args.first().map(String::as_str), Some("-c"));
    assert!(
        args.get(1)
            .is_some_and(|arg| arg.starts_with("core.hooksPath=")),
        "missing hooks override: {args:?}"
    );
    assert!(
        args.windows(2)
            .any(|pair| pair == ["-c", "core.fsmonitor=false"])
    );
    assert_eq!(
        env.get("GIT_CONFIG_NOSYSTEM").and_then(Option::as_deref),
        Some("1")
    );
    let empty_config = manager.empty_git_config.to_string_lossy();
    assert_eq!(
        env.get("GIT_CONFIG_GLOBAL").and_then(Option::as_deref),
        Some(empty_config.as_ref())
    );
    assert_eq!(
        env.get("GIT_TERMINAL_PROMPT").and_then(Option::as_deref),
        Some("0")
    );
    assert!(manager.empty_git_config.is_file());
    assert!(!manager.disabled_hooks.exists());
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;

        let mode = std::fs::metadata(manager._git_guard_dir.path())
            .expect("guard metadata")
            .permissions()
            .mode();
        assert_eq!(mode & 0o077, 0, "Git guard directory is not private");
    }
}

#[tokio::test]
async fn unsafe_worker_ids_and_option_like_refs_fail_before_git() {
    let fixture = tempfile::tempdir().expect("fixture");
    let manager = WorktreeManager::new(fixture.path()).expect("worktree manager");

    for worker_id in [
        "",
        ".",
        "..",
        "../escape",
        "nested/worker",
        "nested\\worker",
    ] {
        let error = manager
            .create_worker_tree(worker_id, "swarm/worker", "HEAD")
            .await
            .expect_err("unsafe worker id must fail")
            .to_string();
        assert!(
            error.contains("invalid worker id"),
            "{worker_id:?}: {error}"
        );
    }
    for (branch, base) in [("--detach", "HEAD"), ("swarm/worker", "-C")] {
        let error = manager
            .create_worker_tree("worker-1", branch, base)
            .await
            .expect_err("option-like ref must fail")
            .to_string();
        assert!(error.contains("invalid"), "{error}");
    }
}

#[cfg(unix)]
#[test]
fn linked_swarm_root_is_rejected_without_touching_target() {
    use std::os::unix::fs::symlink;

    let fixture = tempfile::tempdir().expect("fixture");
    let external = tempfile::tempdir().expect("external target");
    symlink(external.path(), fixture.path().join(".swarm-worktrees")).expect("plant linked root");
    let error = match WorktreeManager::new(fixture.path()) {
        Ok(_) => panic!("linked swarm root was accepted"),
        Err(error) => error.to_string(),
    };
    assert!(error.contains("linked worktree root"), "{error}");
    assert_eq!(std::fs::read_dir(external.path()).unwrap().count(), 0);
}

#[cfg(target_os = "linux")]
#[path = "worktree_tests/linux.rs"]
mod linux;
