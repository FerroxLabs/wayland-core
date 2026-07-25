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
        checkout_authority: std::sync::OnceLock::new(),
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

/// A held transaction lease must be observable as held by an INDEPENDENT
/// observer, and must stop being observable once released.
///
/// The second `DirectoryAuthority` is what makes this non-vacuous: it holds a
/// genuinely separate open file description on unix and a genuinely separate
/// handle on Windows, so it observes the lease rather than its own lock. A
/// probe through the holder's own handle would prove nothing.
///
/// The invariant asserted is exclusion-then-release, never any particular
/// failure shape. This is the regression guard against retargeting the lease at
/// a directory handle: on Windows byte-range locking is undefined on directory
/// objects so acquisition itself would fail, and on unix the lock would be
/// taken on a different object than this probe inspects.
#[test]
fn transaction_lease_is_mutually_exclusive() {
    let fixture = tempfile::tempdir().expect("fixture");
    let root = fixture.path().join("worker-lease");
    std::fs::create_dir(&root).unwrap();
    let holder = DirectoryAuthority::open(&root).unwrap();
    let probe = DirectoryAuthority::open(&root).unwrap();

    let mut lease = ActiveLease::acquire(transaction_lease_handle(&holder).unwrap())
        .expect("the transaction lease must be acquirable");
    assert!(
        transaction_is_active(&probe, &root).unwrap(),
        "an independently opened authority did not observe the held transaction lease"
    );

    lease.close();
    assert!(
        !transaction_is_active(&probe, &root).unwrap(),
        "the transaction lease was still reported held after it was released"
    );
}

/// While a critical section holds the swarm sentinel, an INDEPENDENT attempt to
/// take that sentinel must observe contention, and must succeed once the
/// critical section ends.
///
/// As above, the separately opened authority is what makes the assertion
/// non-vacuous. The only error condition inspected is the would-block
/// contention signal, which is evidence that exclusion held — never the shape
/// of any platform's failure. The release half is the strong guard: if a lock
/// were ever retargeted at a directory handle again, acquisition on Windows
/// would fail outright and this test would go red.
///
/// Every wait is bounded, so a side that never arrives fails the test instead
/// of blocking the runner.
#[test]
fn swarm_lock_is_mutually_exclusive() {
    let fixture = tempfile::tempdir().expect("fixture");
    let swarm_root = fixture.path().join("swarm");
    std::fs::create_dir(&swarm_root).unwrap();

    let (ready_tx, ready_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel();
    let held_root = swarm_root.clone();
    let critical_section = std::thread::spawn(move || {
        let authority = DirectoryAuthority::open(&held_root).unwrap();
        with_directory_lock(&held_root, &authority, || {
            ready_tx.send(()).unwrap();
            release_rx
                .recv_timeout(Duration::from_secs(30))
                .expect("the observer never released the critical section");
            Ok(())
        })
        .expect("the swarm sentinel must be acquirable");
    });

    ready_rx
        .recv_timeout(Duration::from_secs(30))
        .expect("the critical section never reported holding the swarm sentinel");

    let probe = DirectoryAuthority::open(&swarm_root).unwrap();
    let mut contended = fd_lock::RwLock::new(swarm_lock_handle(&probe).unwrap());
    let contention = match contended.try_write() {
        Ok(_) => panic!("the swarm sentinel was acquirable while a critical section held it"),
        Err(error) => error,
    };
    assert_eq!(
        contention.kind(),
        ErrorKind::WouldBlock,
        "expected a contention signal from the held swarm sentinel, got {contention:?}"
    );
    drop(contended);

    release_tx.send(()).unwrap();
    critical_section.join().expect("critical section thread");

    let mut released = fd_lock::RwLock::new(swarm_lock_handle(&probe).unwrap());
    assert!(
        released.try_write().is_ok(),
        "the swarm sentinel stayed locked after its critical section ended"
    );
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

    if cfg!(windows) {
        // The cleanup retains open handles on `swarm_root` AND on its
        // `worker-retry` DESCENDANT, and the descendant handle is the entire
        // cause of the refusal below. Measured Windows rename rule: ANY open
        // handle to a descendant — of any kind, at any desired access, under any
        // share mode — blocks renaming ANY ancestor with ERROR_ACCESS_DENIED,
        // which Rust maps to `PermissionDenied`. Share mode does not enter into
        // it; a handle on an object never blocks renaming THAT object when the
        // share mode admits delete. The replaced-control-root substitution that
        // `release()`'s identity check defends against on Unix therefore cannot
        // arise here. Prove the refusal, then that the un-substituted
        // transaction still releases cleanly and stays retryable.
        let error = std::fs::rename(&swarm_root, &original_swarm)
            .expect_err("Windows must refuse renaming a swarm-held control root");
        assert_eq!(
            error.kind(),
            std::io::ErrorKind::PermissionDenied,
            "expected OS-level PermissionDenied renaming the held control root, got {error:?}"
        );
        assert!(!cleanup.released.load(Ordering::Acquire));
        cleanup
            .release()
            .expect("cleanup of the un-substituted control root");
        assert!(!root.exists());
        assert!(cleanup.released.load(Ordering::Acquire));
    } else {
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
}

#[cfg(unix)]
#[test]
fn release_refuses_while_checkout_loan_outstanding() {
    let fixture = tempfile::tempdir().expect("fixture");
    let swarm_root = fixture.path().join("swarm");
    let root = swarm_root.join("worker-loan");
    let checkout = root.join("checkout");
    std::fs::create_dir_all(&checkout).unwrap();
    let quarantine_root = fixture.path().join("control");
    std::fs::create_dir(&quarantine_root).unwrap();
    let cleanup = test_transaction_cleanup("worker-loan", &root, &swarm_root, &quarantine_root);

    let checkout_authority = DirectoryAuthority::open(&checkout).unwrap();
    cleanup.bind_checkout_authority(checkout_authority.clone());

    // Model an escaped worker descendant that still holds the retained checkout
    // descriptor. The shared loan counter must fail the cleanup closed.
    let loan = checkout_authority.to_sandbox().try_clone_handle().unwrap();

    let error = cleanup
        .release()
        .expect_err("cleanup deleted the checkout while a loan was outstanding");
    assert!(error.to_string().contains("worker descendant"), "{error}");
    assert!(
        root.exists(),
        "fail-closed cleanup deleted the retained transaction root"
    );
    assert!(
        cleanup
            .active_reservations
            .lock()
            .unwrap()
            .contains_key("worker-loan"),
        "fail-closed cleanup dropped the capacity reservation"
    );
    assert!(!cleanup.released.load(Ordering::Acquire));

    drop(loan);
    cleanup
        .release()
        .expect("cleanup must succeed once the checkout loan is released");
    assert!(
        !root.exists(),
        "retained root survived cleanup after loan drop"
    );
    assert!(
        !cleanup
            .active_reservations
            .lock()
            .unwrap()
            .contains_key("worker-loan"),
        "reservation retained after successful cleanup"
    );
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

    if cfg!(windows) {
        // FIXTURE COUPLING, stated so it cannot be lost: this refusal depends on
        // `test_transaction_cleanup` writing RESERVATION_FILE into `root` and
        // RETAINING a `RegularFileAuthority` on it. That open DESCENDANT is what
        // refuses the rename, by the measured Windows rule that any open handle
        // to a descendant blocks renaming any ancestor with ERROR_ACCESS_DENIED.
        // It is NOT the share mode: a handle on an object never blocks renaming
        // THAT object when the share mode admits delete, and the root handle
        // does admit it. Drop the reservation file from the fixture and this
        // assertion silently flips to failing.
        //
        // Unlike `transaction_cleanup_never_deletes_swap_after_validation`, the
        // swap is NOT constructible here: `TransactionCleanup` structurally
        // requires the `reservation_authority` field and `release()` validates
        // it, so the descendant handle cannot be dropped without dropping the
        // transaction. That is a fixture limitation, not a coverage hole — the
        // same-path-replacement software defense IS exercised on Windows by that
        // now-unified test, whose fixture holds no descendant handle at all.
        //
        // Prove the refusal, then that the un-substituted transaction still
        // releases cleanly by handle.
        let error = std::fs::rename(&root, &moved)
            .expect_err("Windows must refuse renaming a swarm-held transaction root");
        assert_eq!(
            error.kind(),
            std::io::ErrorKind::PermissionDenied,
            "expected OS-level PermissionDenied renaming the held root, got {error:?}"
        );
        cleanup
            .release()
            .expect("handle-bound cleanup of the un-substituted root");
        assert!(
            !root.exists(),
            "owned transaction was not cleaned by handle"
        );
        assert!(cleanup.released.load(Ordering::Acquire));
    } else {
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
}

/// Handle-bound cleanup must delete the retained transaction OBJECT, never
/// whatever directory occupies its former pathname after a mid-validation swap.
///
/// ONE body runs on BOTH platforms, and the swap is genuinely constructible on
/// Windows for a specific, checkable reason: this fixture opens NO descendant
/// handle inside the transaction root — only a `DirectoryAuthority` on the root
/// OBJECT itself. By the measured Windows rename rule, a handle on an object
/// never blocks renaming THAT object provided the share mode admits delete, and
/// the retained handle does admit it. (It is an open handle to a DESCENDANT
/// that blocks renaming an ancestor, and this fixture has none — which is
/// exactly what `transaction_cleanup_preserves_same_path_replacement` cannot
/// arrange.)
///
/// What the assertions then exercise is the SOFTWARE defense, not an OS
/// guarantee: destructive cleanup enumerates through the RETAINED handle
/// (`NtQueryDirectoryFile` on Windows, the held descriptor on unix), opens each
/// child handle-relative (`NtCreateFile` with `RootDirectory =
/// authority.handle`) and disposes it by handle
/// (`SetFileInformationByHandle`). Nothing re-resolves the pathname, so the
/// handle follows the OBJECT wherever the pathname now points: the substituted
/// directory at the original pathname survives untouched with its replacement
/// receipt readable, and the moved original is deleted.
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

/// Every root a manager stores must be reproducible by re-deriving it through
/// the crate's ONE path-representation helper, for ALL THREE constructors, and
/// on Windows all four roots must share a single path `Prefix` variant.
///
/// The assertion is deliberately the INVARIANT rather than today's symptom: it
/// does not claim the roots are plain (the de-verbatimizing strip is
/// conditional and may legitimately be refused), it claims they agree with each
/// other and with their own re-derivation. That cross-operand agreement is
/// exactly what a partial application of the representation destroys, so this
/// test fails on a future re-split even if the strip itself still works.
#[test]
fn worktree_roots_share_one_path_representation() {
    let repo_a = tempfile::tempdir().expect("repo a");
    let manager_a = WorktreeManager::new(repo_a.path()).expect("manager from new");

    let repo_b = tempfile::tempdir().expect("repo b");
    let control_b = tempfile::tempdir().expect("control b");
    let manager_b = WorktreeManager::new_with_workspace_root(
        repo_b.path(),
        &control_b.path().join("checkouts"),
    )
    .expect("manager from new_with_workspace_root");

    let repo_c = tempfile::tempdir().expect("repo c");
    let control_c = tempfile::tempdir().expect("control c");
    let workspace_c = control_c.path().join("workspace");
    std::fs::create_dir(&workspace_c).expect("workspace c");
    let authority_c =
        wcore_sandbox::DirectoryAuthority::open(&workspace_c).expect("workspace c authority");
    let manager_c = WorktreeManager::new_with_workspace_authority(repo_c.path(), authority_c)
        .expect("manager from new_with_workspace_authority");

    for (label, manager) in [
        ("new", &manager_a),
        ("new_with_workspace_root", &manager_b),
        ("new_with_workspace_authority", &manager_c),
    ] {
        let roots: [(&str, &Path); 4] = [
            ("repo_root", manager.repo_root()),
            ("swarm_root", manager.swarm_root()),
            ("swarm_parent", manager.swarm_parent.as_path()),
            ("control_root", manager.control_root.as_path()),
        ];
        for (field, root) in roots {
            let rederived = normalized_root(root)
                .unwrap_or_else(|error| panic!("{label}/{field}: re-derivation failed: {error}"));
            assert_eq!(
                rederived.as_path(),
                root,
                "{label}/{field} is not in the crate's one path representation"
            );
        }
        #[cfg(windows)]
        {
            let tags: Vec<(&str, u8)> = roots
                .iter()
                .map(|(field, root)| {
                    let prefix = match root.components().next() {
                        Some(std::path::Component::Prefix(prefix)) => prefix.kind(),
                        _ => panic!(
                            "{label}/{field}: root has no path prefix: {}",
                            root.display()
                        ),
                    };
                    let tag = match prefix {
                        std::path::Prefix::Verbatim(_) => 0_u8,
                        std::path::Prefix::VerbatimUNC(_, _) => 1,
                        std::path::Prefix::VerbatimDisk(_) => 2,
                        std::path::Prefix::DeviceNS(_) => 3,
                        std::path::Prefix::UNC(_, _) => 4,
                        std::path::Prefix::Disk(_) => 5,
                    };
                    (*field, tag)
                })
                .collect();
            for (field, tag) in &tags[1..] {
                assert_eq!(
                    *tag, tags[0].1,
                    "{label}: {field} and {} disagree on the path prefix variant",
                    tags[0].0
                );
            }
        }
    }
}

/// BL-5 security regression. `new_with_workspace_authority` is the public
/// constructor for handing a transaction root across an authority boundary, and
/// its repository-overlap guard is the only thing keeping a delegated child's
/// checkout root out of the source repository.
///
/// Before this repair the guard could NOT fire on Windows: `repo_root` came
/// from a bare canonicalize (verbatim) while the workspace parent derived from
/// the authority's plain display path, so both `starts_with` arms were
/// unconditionally false and an overlapping workspace was silently ACCEPTED.
/// The assertion is on the constructor's own refusal text, so a different error
/// firing first would not satisfy it.
#[test]
fn workspace_authority_constructor_refuses_repo_overlap() {
    let repo = tempfile::tempdir().expect("repo");
    let inside = repo.path().join("inside-workspace");
    std::fs::create_dir(&inside).expect("workspace inside repository");
    let authority = wcore_sandbox::DirectoryAuthority::open(&inside).expect("workspace authority");

    // `WorktreeManager` is not `Debug`, so match rather than `expect_err`.
    let error = match WorktreeManager::new_with_workspace_authority(repo.path(), authority) {
        Ok(_) => panic!("a workspace inside the repository was accepted"),
        Err(error) => error.to_string(),
    };
    assert!(
        error.contains("orchestrator worktree root must not overlap repository"),
        "expected the repository-overlap refusal, got: {error}"
    );
}

// --- CandidateSeal: pure helpers (no git required) ---

#[cfg(unix)]
#[test]
fn candidate_config_scan_is_deny_by_default() {
    use super::candidate::scan_config;

    // A fresh-clone-shaped config passes the allowlist.
    scan_config(
        "[core]\n\trepositoryformatversion = 0\n\tfilemode = true\n\tbare = false\n\
         \tlogallrefupdates = true\n\tsymlinks = false\n\tignorecase = false\n\
         \tprecomposeunicode = true\n\tquotepath = false\n\
         [branch \"main\"]\n\tremote = origin\n\tmerge = refs/heads/main\n\
         [extensions]\n\tobjectformat = sha1\n",
    )
    .expect("benign fresh-clone config must pass");

    // A configured remote is rejected with its own diagnostic.
    let remote = scan_config("[remote \"origin\"]\n\turl = https://example.invalid\n")
        .expect_err("configured remote must be rejected");
    assert!(remote.to_string().contains("configured remote"), "{remote}");

    // Command / relocation / redirect directives are all rejected.
    for poison in [
        "[core]\n\thooksPath = ../scratch/hooks\n",
        "[core]\n\tfsmonitor = /bin/evil\n",
        "[core]\n\tsshCommand = evil\n",
        "[core]\n\tpager = evil\n",
        "[core]\n\teditor = evil\n",
        "[core]\n\talternateRefsCommand = evil\n",
        // core is now a deny-by-default allowlist: gitProxy (command exec) and
        // any unknown core key are rejected outright.
        "[core]\n\tgitProxy = /x\n",
        "[core]\n\tsomethingWeird = 1\n",
        "[filter \"evil\"]\n\tprocess = proc\n",
        "[filter \"evil\"]\n\tsmudge = smudge\n",
        "[include]\n\tpath = /etc/hostile\n",
        "[includeIf \"gitdir:/x/**\"]\n\tpath = /etc/hostile\n",
        "[alias]\n\tst = !evil\n",
        "[credential]\n\thelper = evil\n",
        "[url \"evil:\"]\n\tinsteadOf = https://\n",
        "[extensions]\n\tworktreeConfig = true\n",
        // Value-less boolean form of worktreeConfig still means `true`.
        "[extensions]\n\tworktreeConfig\n",
    ] {
        assert!(
            scan_config(poison).is_err(),
            "poisoned config accepted: {poison:?}"
        );
    }

    // An explicitly-disabled worktreeConfig is not poison.
    scan_config("[extensions]\n\tworktreeConfig = false\n")
        .expect("disabled worktreeConfig must pass");
}

#[cfg(unix)]
#[test]
fn candidate_manifest_digest_tracks_working_tree_changes() {
    use super::candidate::manifest_digest;

    let dir = tempfile::tempdir().expect("fixture");
    std::fs::write(dir.path().join("a.txt"), "one").unwrap();
    let authority = DirectoryAuthority::open(dir.path()).unwrap().to_sandbox();

    let baseline = manifest_digest(&authority).unwrap();
    // SHA-256 hex is 64 lowercase hex chars.
    assert_eq!(baseline.len(), 64, "digest must be SHA-256 hex: {baseline}");
    assert!(baseline.bytes().all(|b| b.is_ascii_hexdigit()));
    assert_eq!(
        baseline,
        manifest_digest(&authority).unwrap(),
        "manifest digest must be deterministic for an unchanged tree"
    );

    std::fs::write(dir.path().join("a.txt"), "two").unwrap();
    let mutated = manifest_digest(&authority).unwrap();
    assert_ne!(baseline, mutated, "content change must change the digest");

    std::fs::write(dir.path().join("b.txt"), "one").unwrap();
    let extended = manifest_digest(&authority).unwrap();
    assert_ne!(mutated, extended, "an added entry must change the digest");

    std::fs::remove_file(dir.path().join("b.txt")).unwrap();
    let restored = manifest_digest(&authority).unwrap();
    assert_eq!(
        restored, mutated,
        "removing the added entry restores the digest"
    );
}

#[cfg(unix)]
#[test]
fn candidate_manifest_digest_boundary_cases() {
    use super::candidate::manifest_digest;

    let dir = tempfile::tempdir().expect("fixture");
    std::fs::write(dir.path().join("keep.txt"), "keep").unwrap();
    let authority = DirectoryAuthority::open(dir.path()).unwrap().to_sandbox();
    let baseline = manifest_digest(&authority).unwrap();

    // An added *empty* subdirectory must perturb the digest.
    std::fs::create_dir(dir.path().join("emptydir")).unwrap();
    let with_dir = manifest_digest(&authority).unwrap();
    assert_ne!(
        baseline, with_dir,
        "an added empty subdirectory must perturb"
    );
    std::fs::remove_dir(dir.path().join("emptydir")).unwrap();
    assert_eq!(
        baseline,
        manifest_digest(&authority).unwrap(),
        "removing the empty subdirectory restores the digest"
    );

    // A regular-file <-> directory replacement at the same name must perturb.
    std::fs::write(dir.path().join("node"), "file-shape").unwrap();
    let as_file = manifest_digest(&authority).unwrap();
    std::fs::remove_file(dir.path().join("node")).unwrap();
    std::fs::create_dir(dir.path().join("node")).unwrap();
    let as_dir = manifest_digest(&authority).unwrap();
    assert_ne!(
        as_file, as_dir,
        "a file<->directory type swap at the same path must perturb"
    );

    // `.git` is excluded from the manifest: planting metadata under it must not
    // change the digest.
    std::fs::remove_dir(dir.path().join("node")).unwrap();
    let clean = manifest_digest(&authority).unwrap();
    std::fs::create_dir(dir.path().join(".git")).unwrap();
    std::fs::write(dir.path().join(".git/config"), "[core]\n").unwrap();
    assert_eq!(
        clean,
        manifest_digest(&authority).unwrap(),
        "top-level .git must be excluded from the source manifest"
    );
}

#[cfg(unix)]
#[test]
fn candidate_manifest_digest_binds_executable_bit() {
    use std::os::unix::fs::PermissionsExt;

    use super::candidate::manifest_digest;

    let dir = tempfile::tempdir().expect("fixture");
    let file = dir.path().join("script.sh");
    std::fs::write(&file, "same content").unwrap();
    std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o644)).unwrap();
    let authority = DirectoryAuthority::open(dir.path()).unwrap().to_sandbox();

    let non_exec = manifest_digest(&authority).unwrap();
    // Flip only the exec bit; content and size are unchanged.
    std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o755)).unwrap();
    let exec = manifest_digest(&authority).unwrap();
    assert_ne!(
        non_exec, exec,
        "a chmod +x with identical content must perturb the digest (git tree mode)"
    );
}

#[cfg(unix)]
#[test]
fn candidate_manifest_digest_tracks_git_owner_exec_bit() {
    use std::os::unix::fs::PermissionsExt;

    use super::candidate::manifest_digest;

    let dir = tempfile::tempdir().expect("fixture");
    let file = dir.path().join("script.sh");
    std::fs::write(&file, "same content").unwrap();
    let authority = DirectoryAuthority::open(dir.path()).unwrap().to_sandbox();

    // Git derives a regular file's tree mode from the OWNER execute bit alone
    // (100644 vs 100755). A non-owner exec bit is not part of the tree identity,
    // so 0o644 -> 0o645 keeps git's mode at 100644 and must NOT perturb the digest.
    std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o644)).unwrap();
    let plain = manifest_digest(&authority).unwrap();
    std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o645)).unwrap();
    let other_exec = manifest_digest(&authority).unwrap();
    assert_eq!(
        plain, other_exec,
        "a non-owner exec bit is not part of git's tree mode and must not perturb the digest"
    );

    // Adding the OWNER exec bit from a non-canonical mode (0o645 -> 0o745) flips
    // git's tree mode 100644 -> 100755 and must be detected — the case a raw
    // `mode & 0o111` mask would silently miss.
    std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o745)).unwrap();
    let owner_exec = manifest_digest(&authority).unwrap();
    assert_ne!(
        other_exec, owner_exec,
        "adding the owner exec bit (git 100644 -> 100755) must perturb the digest"
    );
}

// --- CandidateSeal: live isolated-checkout scenarios (git-backed) ---

#[cfg(any(target_os = "linux", windows))]
async fn seal_run_git(repo: &Path, args: &[&str]) {
    let mut command = shell::shell_command_argv("git", args);
    command.current_dir(repo);
    let output = capture_bounded_process(
        command,
        CaptureLimits {
            stdout_bytes: 64 * 1024,
            stderr_bytes: 64 * 1024,
            timeout: Duration::from_secs(5),
        },
        None,
    )
    .await
    .expect("fixture git command");
    assert!(
        output.status.success(),
        "fixture git {args:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[cfg(any(target_os = "linux", windows))]
async fn seal_init_repo(path: &Path) {
    seal_run_git(path, &["init", "-q", "-b", "main"]).await;
    std::fs::write(path.join("README.md"), "seed\n").unwrap();
    seal_run_git(path, &["add", "README.md"]).await;
    seal_run_git(
        path,
        &[
            "-c",
            "user.email=swarm-test@example.invalid",
            "-c",
            "user.name=Swarm Test",
            "commit",
            "-qm",
            "seed",
        ],
    )
    .await;
}

/// Build a real isolated checkout and return the tempdirs (kept alive), the
/// manager, and the transaction workspace.
#[cfg(any(target_os = "linux", windows))]
async fn seal_workspace() -> (
    tempfile::TempDir,
    tempfile::TempDir,
    WorktreeManager,
    TransactionWorkspace,
) {
    let fixture = tempfile::tempdir().expect("fixture");
    let control = tempfile::tempdir().expect("orchestrator control root");
    seal_init_repo(fixture.path()).await;
    let manager =
        WorktreeManager::new_with_workspace_root(fixture.path(), &control.path().join("checkouts"))
            .expect("external manager");
    let head = manager.pinned_head().await.expect("pinned head");
    let workspace = manager
        .create_isolated_checkout(
            "child-seal",
            "wayland-child/child-seal",
            &head,
            WorkspaceCapacity {
                available_bytes: u64::MAX,
                safety_margin_bytes: 0,
                max_transaction_bytes: u64::MAX,
                max_aggregate_bytes: u64::MAX,
            },
        )
        .await
        .expect("isolated checkout");
    (fixture, control, manager, workspace)
}

/// A NEED_WORK_TREE git subcommand must still be able to chdir into the
/// repository root while `WorktreeManager`s hold their repository authority.
///
/// Windows failure mode this exists for: a retained directory handle carrying
/// the `DELETE` access right blocks `SetCurrentDirectory` into that directory,
/// so git's in-process chdir inside `setup_work_tree()` fails and `git status`
/// dies with `fatal: this operation must be run in a work tree`. That made
/// `assert_clean` refuse unconditionally once a manager existed, which gates
/// dispatch, isolated checkout and integration checkout alike. The assertion
/// below is on the INVARIANT (success), never on that message, so it survives a
/// reworded git error and still fails if the DELETE bit is reintroduced.
///
/// The Linux-plus-Windows gate is deliberate and is not a dodge of the
/// Windows-specific requirement: `seal_run_git` and `seal_init_repo` carry the
/// same gate, so a `#[cfg(windows)]`-only test could not compile against them
/// without duplicating a git fixture, and the assertion is a universal
/// invariant that costs nothing to hold on Linux while guarding the unix path
/// against a future regression in the shared observational shim.
#[cfg(any(target_os = "linux", windows))]
#[tokio::test]
async fn git_status_succeeds_in_repo_root_while_manager_is_alive() {
    let fixture = tempfile::tempdir().expect("fixture");
    let control = tempfile::tempdir().expect("orchestrator control root");
    seal_init_repo(fixture.path()).await;
    // `WorktreeManager::new` creates its swarm root INSIDE the repository, so
    // without this commit the working tree would be untracked-dirty and
    // `assert_clean` would report `DirtyCheckout` for a reason unrelated to the
    // chdir defect under test.
    std::fs::write(fixture.path().join(".gitignore"), ".swarm-worktrees/\n").unwrap();
    seal_run_git(fixture.path(), &["add", ".gitignore"]).await;
    seal_run_git(
        fixture.path(),
        &[
            "-c",
            "user.email=swarm-test@example.invalid",
            "-c",
            "user.name=Swarm Test",
            "commit",
            "-qm",
            "ignore swarm worktrees",
        ],
    )
    .await;

    let in_repo_manager = WorktreeManager::new(fixture.path()).expect("in-repo manager");
    let external_manager =
        WorktreeManager::new_with_workspace_root(fixture.path(), &control.path().join("checkouts"))
            .expect("external manager");

    // Both managers stay alive across both assertions, so this also covers two
    // repository authorities retained on one repository root.
    in_repo_manager.assert_clean().await.expect(
        "a NEED_WORK_TREE git subcommand must chdir into the repository root while the in-repo manager holds its repository authority",
    );
    external_manager.assert_clean().await.expect(
        "a NEED_WORK_TREE git subcommand must chdir into the repository root while the external-workspace manager holds its repository authority",
    );
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn candidate_seal_mints_and_revalidates_from_fresh_checkout() {
    let (_fixture, _control, _manager, workspace) = seal_workspace().await;
    let seal = workspace
        .seal_candidate()
        .expect("mint seal from fresh checkout");
    seal.revalidate()
        .expect("fresh checkout is a clean candidate");
    workspace.cleanup.release().expect("release");
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn candidate_seal_revalidates_repeatedly_while_quiescent() {
    let (_fixture, _control, _manager, workspace) = seal_workspace().await;
    let seal = workspace.seal_candidate().expect("mint seal");
    for _ in 0..3 {
        seal.revalidate()
            .expect("quiescent checkout revalidates repeatedly");
    }
    workspace.cleanup.release().expect("release");
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn candidate_seal_revalidation_fails_on_blocked_plumbing_inspection() {
    let (_fixture, _control, _manager, workspace) = seal_workspace().await;
    let seal = workspace.seal_candidate().expect("mint seal");
    // Remove a required plumbing file; the authority-based read fails closed.
    std::fs::remove_file(workspace.checkout.join(".git/HEAD")).unwrap();
    let error = seal
        .revalidate()
        .expect_err("missing HEAD must fail the inspection closed");
    assert!(error.to_string().contains("HEAD"), "{error}");
    workspace.cleanup.release().expect("release");
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn candidate_seal_revalidation_detects_source_drift() {
    let (_fixture, _control, _manager, workspace) = seal_workspace().await;
    let seal = workspace.seal_candidate().expect("mint seal");
    std::fs::write(workspace.checkout.join("README.md"), "drifted\n").unwrap();
    let error = seal
        .revalidate()
        .expect_err("working-tree drift must invalidate the seal");
    assert!(error.to_string().contains("source manifest"), "{error}");
    workspace.cleanup.release().expect("release");
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn candidate_seal_revalidation_detects_repository_substitution() {
    let (_fixture, _control, _manager, workspace) = seal_workspace().await;
    let seal = workspace.seal_candidate().expect("mint seal");
    let moved = workspace.root.join("checkout-original");
    std::fs::rename(&workspace.checkout, &moved).unwrap();
    std::fs::create_dir(&workspace.checkout).unwrap();
    let error = seal
        .revalidate()
        .expect_err("same-path repository substitution must fail closed");
    assert!(error.to_string().contains("identity changed"), "{error}");
    // Restore so owned cleanup removes the retained transaction root cleanly.
    std::fs::remove_dir(&workspace.checkout).unwrap();
    std::fs::rename(&moved, &workspace.checkout).unwrap();
    workspace.cleanup.release().expect("release");
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn candidate_seal_revalidation_rejects_alternate_object_store() {
    let (_fixture, _control, _manager, workspace) = seal_workspace().await;
    let seal = workspace.seal_candidate().expect("mint seal");
    let info = workspace.checkout.join(".git/objects/info");
    std::fs::create_dir_all(&info).unwrap();
    std::fs::write(info.join("alternates"), "/some/foreign/objects\n").unwrap();
    let error = seal
        .revalidate()
        .expect_err("alternate object store must fail closed");
    assert!(
        error.to_string().contains("alternate object store"),
        "{error}"
    );
    workspace.cleanup.release().expect("release");
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn candidate_seal_revalidation_rejects_config_poisoning() {
    use std::io::Write;

    let (_fixture, _control, _manager, workspace) = seal_workspace().await;
    let seal = workspace.seal_candidate().expect("mint seal");
    let mut config = std::fs::OpenOptions::new()
        .append(true)
        .open(workspace.checkout.join(".git/config"))
        .unwrap();
    writeln!(config, "\n[filter \"evil\"]\n\tprocess = evil-proc").unwrap();
    drop(config);
    let error = seal
        .revalidate()
        .expect_err("poisoned filter config must fail closed");
    assert!(
        error.to_string().contains("disallowed Git configuration"),
        "{error}"
    );
    workspace.cleanup.release().expect("release");
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn candidate_seal_revalidation_rejects_planted_hook() {
    let (_fixture, _control, _manager, workspace) = seal_workspace().await;
    let seal = workspace.seal_candidate().expect("mint seal");
    let hooks = workspace.checkout.join(".git/hooks");
    std::fs::create_dir_all(&hooks).unwrap();
    std::fs::write(hooks.join("pre-commit"), "#!/bin/sh\nexit 0\n").unwrap();
    let error = seal
        .revalidate()
        .expect_err("planted hook must fail closed");
    assert!(error.to_string().contains("hook"), "{error}");
    workspace.cleanup.release().expect("release");
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn candidate_seal_fails_closed_after_transaction_release() {
    let (_fixture, _control, _manager, workspace) = seal_workspace().await;
    let seal = workspace.seal_candidate().expect("mint seal");
    workspace
        .cleanup
        .release()
        .expect("release the transaction");
    let error = seal
        .revalidate()
        .expect_err("a released transaction grants the seal no authority");
    assert!(error.to_string().contains("released"), "{error}");
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn candidate_seal_cannot_outlive_its_checkout() {
    let (_fixture, _control, _manager, workspace) = seal_workspace().await;
    let seal = workspace.seal_candidate().expect("mint seal");
    let checkout = workspace.checkout.clone();
    workspace
        .cleanup
        .release()
        .expect("release the transaction");
    assert!(
        !checkout.exists(),
        "owned cleanup must remove the isolated checkout"
    );
    assert!(
        seal.revalidate().is_err(),
        "the seal must not revalidate once its checkout is gone"
    );
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn candidate_seal_rejects_commondir_redirect() {
    let (_fixture, _control, _manager, workspace) = seal_workspace().await;
    let seal = workspace.seal_candidate().expect("mint seal");
    // A planted commondir would make git resolve objects/refs/config elsewhere.
    std::fs::write(workspace.checkout.join(".git/commondir"), "../evil\n").unwrap();
    let error = seal
        .revalidate()
        .expect_err(".git/commondir redirect must fail closed");
    assert!(error.to_string().contains("commondir"), "{error}");
    std::fs::remove_file(workspace.checkout.join(".git/commondir")).unwrap();
    workspace.cleanup.release().expect("release");
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn candidate_seal_rejects_worktree_scoped_config_file() {
    let (_fixture, _control, _manager, workspace) = seal_workspace().await;
    let seal = workspace.seal_candidate().expect("mint seal");
    std::fs::write(
        workspace.checkout.join(".git/config.worktree"),
        "[core]\n\thooksPath = ../scratch/h\n",
    )
    .unwrap();
    let error = seal
        .revalidate()
        .expect_err(".git/config.worktree must fail closed");
    assert!(
        error.to_string().contains("worktree-scoped config"),
        "{error}"
    );
    std::fs::remove_file(workspace.checkout.join(".git/config.worktree")).unwrap();
    workspace.cleanup.release().expect("release");
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn candidate_seal_rejects_worktree_config_extension() {
    use std::io::Write;

    let (_fixture, _control, _manager, workspace) = seal_workspace().await;
    let seal = workspace.seal_candidate().expect("mint seal");
    let mut config = std::fs::OpenOptions::new()
        .append(true)
        .open(workspace.checkout.join(".git/config"))
        .unwrap();
    writeln!(config, "\n[extensions]\n\tworktreeConfig = true").unwrap();
    drop(config);
    let error = seal
        .revalidate()
        .expect_err("extensions.worktreeConfig=true must fail closed");
    assert!(
        error.to_string().contains("disallowed Git configuration"),
        "{error}"
    );
    workspace.cleanup.release().expect("release");
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn candidate_seal_rejects_relocated_hooks_path() {
    use std::io::Write;

    let (_fixture, _control, _manager, workspace) = seal_workspace().await;
    let seal = workspace.seal_candidate().expect("mint seal");
    // Relocate hooks into scratch and plant an executable there. The `.git/hooks`
    // scan would miss it — the config allowlist must reject core.hooksPath.
    let relocated = workspace.scratch.join("hooks");
    std::fs::create_dir_all(&relocated).unwrap();
    std::fs::write(relocated.join("pre-commit"), "#!/bin/sh\nexit 0\n").unwrap();
    let mut config = std::fs::OpenOptions::new()
        .append(true)
        .open(workspace.checkout.join(".git/config"))
        .unwrap();
    writeln!(config, "\n[core]\n\thooksPath = {}", relocated.display()).unwrap();
    drop(config);
    let error = seal
        .revalidate()
        .expect_err("core.hooksPath relocation must fail closed");
    assert!(
        error.to_string().contains("disallowed Git configuration"),
        "{error}"
    );
    workspace.cleanup.release().expect("release");
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn candidate_seal_rejects_fsmonitor_program() {
    use std::io::Write;

    let (_fixture, _control, _manager, workspace) = seal_workspace().await;
    let seal = workspace.seal_candidate().expect("mint seal");
    let mut config = std::fs::OpenOptions::new()
        .append(true)
        .open(workspace.checkout.join(".git/config"))
        .unwrap();
    writeln!(config, "\n[core]\n\tfsmonitor = /bin/evil").unwrap();
    drop(config);
    let error = seal
        .revalidate()
        .expect_err("core.fsmonitor program must fail closed");
    assert!(
        error.to_string().contains("disallowed Git configuration"),
        "{error}"
    );
    workspace.cleanup.release().expect("release");
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn candidate_seal_reports_tracked_symlink_clearly() {
    use std::os::unix::fs::symlink;

    let (_fixture, _control, _manager, workspace) = seal_workspace().await;
    // Plant a tracked working-tree symlink, then attempt to mint. The walk must
    // surface a specific "does not support tracked symlinks" error, not a
    // generic sandbox I/O failure — and never a false PASS.
    symlink("README.md", workspace.checkout.join("link")).unwrap();
    let error = workspace
        .seal_candidate()
        .expect_err("tracked symlink must be reported, not silently accepted");
    assert!(error.to_string().contains("tracked symlinks"), "{error}");
    std::fs::remove_file(workspace.checkout.join("link")).unwrap();
    workspace.cleanup.release().expect("release");
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn candidate_seal_detects_executable_bit_drift_after_mint() {
    use std::os::unix::fs::PermissionsExt;

    let (_fixture, _control, _manager, workspace) = seal_workspace().await;
    let seal = workspace.seal_candidate().expect("mint seal");
    // Flip only the exec bit of a tracked file — same content and size. The
    // manifest must now bind mode, so this is drift, not a silent no-op.
    let tracked = workspace.checkout.join("README.md");
    let mut perms = std::fs::metadata(&tracked).unwrap().permissions();
    perms.set_mode(perms.mode() | 0o111);
    std::fs::set_permissions(&tracked, perms).unwrap();
    let error = seal
        .revalidate()
        .expect_err("a chmod +x after mint must be detected as drift");
    assert!(error.to_string().contains("source manifest"), "{error}");
    workspace.cleanup.release().expect("release");
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn candidate_seal_rejects_git_proxy_command() {
    use std::io::Write;

    let (_fixture, _control, _manager, workspace) = seal_workspace().await;
    let seal = workspace.seal_candidate().expect("mint seal");
    let mut config = std::fs::OpenOptions::new()
        .append(true)
        .open(workspace.checkout.join(".git/config"))
        .unwrap();
    writeln!(config, "\n[core]\n\tgitProxy = /bin/evil").unwrap();
    drop(config);
    let error = seal
        .revalidate()
        .expect_err("core.gitProxy command directive must fail closed");
    assert!(
        error.to_string().contains("disallowed Git configuration"),
        "{error}"
    );
    workspace.cleanup.release().expect("release");
}

/// A real isolated checkout's workspace triple must carry the manager's path
/// representation and the exact parent chain `RetainedWorkspaceAuthority::new`
/// walks — the chain whose display-path comparison PathDenied every worker
/// dispatch while the transaction root and the checkout were rendered
/// differently. Asserting the chain rather than the failure keeps the test
/// meaningful after the symptom is gone.
///
/// The `any(...)` operands are ordered windows-first on purpose: the seal
/// fixture's own gate is spelled linux-first, so the source-level check that
/// exactly three fixture helpers were widened still counts only those three.
#[cfg(any(windows, target_os = "linux"))]
#[tokio::test]
async fn transaction_workspace_paths_share_manager_representation() {
    let (_fixture, _control, manager, workspace) = seal_workspace().await;

    for (field, path) in [
        ("root", workspace.root.as_path()),
        ("checkout", workspace.checkout.as_path()),
        ("scratch", workspace.scratch.as_path()),
    ] {
        let rederived = normalized_root(path)
            .unwrap_or_else(|error| panic!("workspace {field}: re-derivation failed: {error}"));
        assert_eq!(
            rederived.as_path(),
            path,
            "workspace {field} is not in the manager's path representation"
        );
    }
    assert_eq!(
        workspace.root.parent(),
        Some(manager.swarm_root()),
        "transaction root is not a direct child of the swarm root"
    );
    assert_eq!(
        workspace.checkout.parent(),
        Some(workspace.root.as_path()),
        "checkout is not a direct child of the transaction root"
    );
    assert_eq!(
        workspace.scratch.parent(),
        Some(workspace.root.as_path()),
        "scratch is not a direct child of the transaction root"
    );
    workspace.cleanup.release().expect("release");
}

#[cfg(target_os = "linux")]
#[path = "worktree_tests/linux.rs"]
mod linux;
