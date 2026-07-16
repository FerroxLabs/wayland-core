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

#[cfg(target_os = "linux")]
fn make_executable(path: &Path, contents: &str) {
    use std::os::unix::fs::PermissionsExt;

    std::fs::write(path, contents).expect("write executable fixture");
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
