use super::*;

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
            timeout: Duration::from_secs(5),
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

    let mut file = std::fs::File::create(path).expect("create executable fixture");
    file.write_all(contents.as_bytes())
        .expect("write executable fixture");
    file.sync_all().expect("sync executable fixture");
    drop(file);
    let mut permissions = std::fs::metadata(path).unwrap().permissions();
    permissions.set_mode(0o700);
    std::fs::set_permissions(path, permissions).expect("make executable");
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
    assert_eq!(manager.pinned_head().await.unwrap(), parent_head);
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
                max_transaction_bytes: u64::MAX,
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
                available_bytes: u64::MAX,
                safety_margin_bytes: 0,
                max_transaction_bytes: u64::MAX,
                max_aggregate_bytes: u64::MAX,
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
                available_bytes: u64::MAX,
                safety_margin_bytes: 0,
                max_transaction_bytes: u64::MAX,
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
async fn wait_until_process_gone(pid: u32) {
    let process = format!("/proc/{pid}");
    let deadline = tokio::time::Instant::now() + Duration::from_secs(3);
    while Path::new(&process).exists() && tokio::time::Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(
        !Path::new(&process).exists(),
        "process {pid} survived cleanup"
    );
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
    let fake_git = fixture.path().join("flood-git.sh");
    make_executable(
        &fake_git,
        "#!/bin/sh\ncase \" $* \" in *\" config \"*) exit 1;; esac\n(while :; do printf 0123456789abcdef; done) &\nchild=$!\nprintf %s \"$child\" > \"${0}.pid\"\nwait \"$child\"\n",
    );
    let manager = WorktreeManager::new_with_git_program_and_limits(
        fixture.path(),
        &fake_git,
        CaptureLimits {
            stdout_bytes: 4096,
            stderr_bytes: 4096,
            timeout: Duration::from_secs(2),
        },
    )
    .unwrap();
    let error = manager.assert_clean().await.unwrap_err().to_string();
    assert!(error.contains("stdout exceeded the 4096-byte"), "{error}");
    let pid = std::fs::read_to_string(format!("{}.pid", fake_git.display()))
        .unwrap()
        .parse::<u32>()
        .unwrap();
    wait_until_process_gone(pid).await;
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn worktree_add_timeout_kills_tree_and_reports_preserved_residual() {
    let fixture = tempfile::tempdir().expect("fixture");
    let fake_git = fixture.path().join("hung-git.sh");
    make_executable(
        &fake_git,
        "#!/bin/sh\ncase \" $* \" in *\" config \"*) exit 1;; esac\nmkdir -p .swarm-worktrees/worker-1\n(while :; do :; done) &\nchild=$!\nprintf %s \"$child\" > \"${0}.pid\"\nwait \"$child\"\n",
    );
    let manager = WorktreeManager::new_with_git_program_and_limits(
        fixture.path(),
        &fake_git,
        CaptureLimits {
            stdout_bytes: 4096,
            stderr_bytes: 4096,
            timeout: Duration::from_millis(200),
        },
    )
    .unwrap();
    let error = manager
        .create_worker_tree("worker-1", "swarm/worker-1", "HEAD")
        .await
        .unwrap_err()
        .to_string();
    assert!(error.contains("timed out after 200ms"), "{error}");
    assert!(
        error.contains("residual worktree path preserved"),
        "{error}"
    );
    assert!(manager.swarm_root().join("worker-1").is_dir());
    let pid = std::fs::read_to_string(format!("{}.pid", fake_git.display()))
        .unwrap()
        .parse::<u32>()
        .unwrap();
    wait_until_process_gone(pid).await;
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn cancelled_cleanup_kills_git_and_reports_residual() {
    let fixture = tempfile::tempdir().expect("fixture");
    let fake_git = fixture.path().join("hung-cleanup-git.sh");
    make_executable(
        &fake_git,
        "#!/bin/sh\nprintf %s \"$$\" > \"${0}.pid\"\nwhile :; do :; done\n",
    );
    let manager = WorktreeManager::new_with_git_program(fixture.path(), &fake_git).unwrap();
    let residual = manager.swarm_root().join("worker-still-present");
    std::fs::create_dir(&residual).unwrap();
    let pid_file = PathBuf::from(format!("{}.pid", fake_git.display()));
    let cancel = CancellationToken::new();
    let cleanup = manager.cleanup_all(&cancel);
    tokio::pin!(cleanup);
    while !pid_file.exists() {
        tokio::select! {
            result = &mut cleanup => panic!("cleanup returned before cancellation: {result:?}"),
            _ = tokio::time::sleep(Duration::from_millis(10)) => {}
        }
    }
    let pid = std::fs::read_to_string(&pid_file)
        .unwrap()
        .parse::<u32>()
        .unwrap();
    cancel.cancel();
    let error = tokio::time::timeout(Duration::from_secs(1), &mut cleanup)
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
