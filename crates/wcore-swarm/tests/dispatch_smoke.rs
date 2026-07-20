//! Smoke test: 4 noop workers in parallel.

use std::collections::BTreeMap;
use std::path::Path;
use std::time::Duration;

use wcore_config::shell;
use wcore_swarm::{Swarm, SwarmBrief, WorkerStatus};

#[tokio::test]
async fn dispatches_4_noop_workers_in_parallel() {
    let tmp = tempfile::tempdir().unwrap();
    init_repo(tmp.path()).await;

    let swarm = Swarm::new(tmp.path()).unwrap();

    let brief = SwarmBrief {
        task: "noop".into(),
        base_branch: "main".into(),
        worker_branch_prefix: "swarm/noop".into(),
        worker_command: noop_argv(),
        timeout: Duration::from_secs(30),
        env: vec![],
    };

    let handles = swarm.dispatch(brief, 4).await.unwrap();
    assert_eq!(handles.len(), 4, "expected 4 handles");

    let results = swarm.collect(handles).await.unwrap();
    assert_eq!(results.len(), 4, "expected 4 results");
    for r in &results {
        assert!(
            matches!(r.status, WorkerStatus::Succeeded),
            "worker {} failed: {:?} (stderr: {})",
            r.worker_id,
            r.status,
            r.stderr
        );
        assert!(r.branch.starts_with("swarm/noop/"));
    }
    assert_eq!(
        transaction_entries(tmp.path()),
        0,
        "successful workers must release their transaction workspaces"
    );

    swarm.cleanup().await.unwrap();
}

#[tokio::test]
async fn public_dispatch_owns_git_authority_and_preserves_parent_and_sibling_state() {
    let tmp = tempfile::tempdir().unwrap();
    init_repo(tmp.path()).await;
    let parent_git = tmp.path().join(".git");
    let sibling = tmp.path().join(".swarm-worktrees/sibling-evidence");
    std::fs::create_dir_all(&sibling).unwrap();
    std::fs::write(sibling.join("receipt"), "sibling-owned\n").unwrap();
    let credential = parent_git.join("worker-credential");
    std::fs::write(&credential, "parent-secret\n").unwrap();

    let parent_config = std::fs::read(parent_git.join("config")).unwrap();
    let parent_refs = snapshot_tree(&parent_git.join("refs"));
    let parent_hooks = snapshot_tree(&parent_git.join("hooks"));
    let parent_objects = snapshot_tree(&parent_git.join("objects"));
    let parent_worktrees = snapshot_tree(&parent_git.join("worktrees"));

    let swarm = Swarm::new(tmp.path()).unwrap();
    let brief = SwarmBrief {
        task: "prove public standalone checkout".into(),
        base_branch: "main".into(),
        worker_branch_prefix: "swarm/authority".into(),
        worker_command: fixture_argv("standalone_authority_fixture"),
        timeout: Duration::from_secs(30),
        env: vec![
            (
                "WCORE_SWARM_PARENT_GIT".into(),
                parent_git.to_string_lossy().into_owned(),
            ),
            (
                "WCORE_SWARM_SIBLING".into(),
                sibling.to_string_lossy().into_owned(),
            ),
            (
                "WCORE_SWARM_DENIED_FILE".into(),
                credential.to_string_lossy().into_owned(),
            ),
            ("OPENAI_API_KEY".into(), "must-not-reach-worker".into()),
        ],
    };

    let handles = swarm.dispatch(brief, 1).await.unwrap();
    assert_eq!(handles.len(), 1);
    assert_eq!(
        handles[0].status,
        WorkerStatus::Succeeded,
        "{:?}",
        handles[0]
    );
    assert!(handles[0].stdout.contains("standalone-authority-ok"));

    assert_eq!(
        std::fs::read(parent_git.join("config")).unwrap(),
        parent_config
    );
    assert_eq!(snapshot_tree(&parent_git.join("refs")), parent_refs);
    assert_eq!(snapshot_tree(&parent_git.join("hooks")), parent_hooks);
    assert_eq!(snapshot_tree(&parent_git.join("objects")), parent_objects);
    assert_eq!(
        snapshot_tree(&parent_git.join("worktrees")),
        parent_worktrees
    );
    assert_eq!(
        std::fs::read_to_string(sibling.join("receipt")).unwrap(),
        "sibling-owned\n"
    );
    assert_eq!(
        std::fs::read_to_string(credential).unwrap(),
        "parent-secret\n"
    );
    let retained = std::fs::read_dir(tmp.path().join(".swarm-worktrees"))
        .unwrap()
        .filter(|entry| {
            entry
                .as_ref()
                .is_ok_and(|entry| entry.file_name() != ".wayland-control")
        })
        .map(|entry| entry.unwrap().file_name())
        .collect::<Vec<_>>();
    assert_eq!(retained, vec![std::ffi::OsString::from("sibling-evidence")]);
}

/// Native Windows public-dispatch Bash containment. Native EXECUTION is
/// deferred to plan 20-08, but this identity exists and is non-skipping: it
/// enters through the public `Swarm::dispatch`, runs real Bash inside the
/// delegated checkout, and FAILS (never prints a skip and returns success) when
/// the native AppContainer containment backend cannot bind the retained
/// workspace on this host.
#[cfg(windows)]
#[tokio::test]
async fn required_live_windows_public_dispatch_bash_confines_parent_and_descendants() {
    assert_public_dispatch_bash_confines_parent_and_descendants().await;
}

/// Native macOS public-dispatch Bash containment. Native EXECUTION is deferred
/// to plan 20-08 (delegated macOS execution rides the Docker transport from
/// Task 1D), but this identity exists and is non-skipping: it enters through the
/// public `Swarm::dispatch`, runs real Bash inside the delegated checkout, and
/// FAILS (never skips) when the native containment backend is unavailable.
#[cfg(target_os = "macos")]
#[tokio::test]
async fn required_live_macos_public_dispatch_bash_confines_parent_and_descendants() {
    assert_public_dispatch_bash_confines_parent_and_descendants().await;
}

/// Shared native composition: a real Bash worker, entered through public
/// `Swarm::dispatch`, may mutate its isolated checkout but must be denied every
/// parent/sibling read and write. A missing or non-binding native containment
/// backend surfaces as a failed worker, so the assertions below fail rather
/// than skip.
#[cfg(any(windows, target_os = "macos"))]
async fn assert_public_dispatch_bash_confines_parent_and_descendants() {
    let tmp = tempfile::tempdir().unwrap();
    init_repo(tmp.path()).await;
    let parent_git = tmp.path().join(".git");
    let sibling = tmp.path().join(".swarm-worktrees/sibling-evidence");
    std::fs::create_dir_all(&sibling).unwrap();
    std::fs::write(sibling.join("receipt"), "sibling-owned\n").unwrap();
    let credential = parent_git.join("worker-credential");
    std::fs::write(&credential, "parent-secret\n").unwrap();

    let script = format!(
        "set -e; printf 'child-owned\\n' > worker-artifact; \
         if cat '{parent}' 2>/dev/null; then echo LEAK; exit 1; fi; \
         if cat '{sibling}/receipt' 2>/dev/null; then echo LEAK; exit 1; fi; \
         if printf x > '{parent}' 2>/dev/null; then echo LEAK; exit 1; fi; \
         echo public-dispatch-bash-ok",
        parent = credential.to_string_lossy(),
        sibling = sibling.to_string_lossy(),
    );
    let swarm = Swarm::new(tmp.path()).unwrap();
    let handles = swarm
        .dispatch(
            SwarmBrief {
                task: "native public-dispatch bash containment".into(),
                base_branch: "main".into(),
                worker_branch_prefix: "swarm/native-bash".into(),
                worker_command: vec!["bash".into(), "-c".into(), script],
                timeout: Duration::from_secs(60),
                env: vec![],
            },
            1,
        )
        .await
        .expect("native public dispatch was refused before worker execution");
    assert_eq!(handles.len(), 1);
    assert_eq!(
        handles[0].status,
        WorkerStatus::Succeeded,
        "native containment backend unavailable or Bash worker escaped: {:?}",
        handles[0]
    );
    assert!(handles[0].stdout.contains("public-dispatch-bash-ok"));
    assert!(!handles[0].stdout.contains("parent-secret"));
    assert!(!handles[0].stdout.contains("sibling-owned"));
    assert_eq!(
        std::fs::read_to_string(&credential).unwrap(),
        "parent-secret\n"
    );
    assert_eq!(
        std::fs::read_to_string(sibling.join("receipt")).unwrap(),
        "sibling-owned\n"
    );
    assert_eq!(transaction_entries(tmp.path()), 0);
}

#[tokio::test]
async fn malformed_heartbeat_fails_closed_and_preserves_bounded_diagnostic() {
    let tmp = tempfile::tempdir().unwrap();
    init_repo(tmp.path()).await;
    let swarm = Swarm::new(tmp.path()).unwrap();
    let handles = swarm
        .dispatch(
            SwarmBrief {
                task: "malformed heartbeat".into(),
                base_branch: "main".into(),
                worker_branch_prefix: "swarm/malformed-heartbeat".into(),
                worker_command: fixture_argv("malformed_heartbeat_fixture"),
                timeout: Duration::from_secs(30),
                env: vec![],
            },
            1,
        )
        .await
        .unwrap();
    let reason = match &handles[0].status {
        WorkerStatus::Failed(reason) => reason,
        other => panic!("malformed heartbeat reported {other:?}"),
    };
    assert!(reason.contains("malformed worker heartbeat"), "{reason}");
    assert!(handles[0].stderr.contains("{truncated"));
    assert_eq!(transaction_entries(tmp.path()), 0);
}

#[cfg(unix)]
#[tokio::test]
async fn heartbeat_symlink_cannot_make_parent_disclose_host_data_or_hang() {
    let tmp = tempfile::tempdir().unwrap();
    init_repo(tmp.path()).await;
    let secret = tmp.path().join(".git/heartbeat-secret");
    let sentinel = "heartbeat-parent-secret-must-not-escape";
    std::fs::write(&secret, sentinel).unwrap();
    let swarm = Swarm::new(tmp.path()).unwrap();
    let dispatch = swarm.dispatch(
        SwarmBrief {
            task: "hostile heartbeat symlink".into(),
            base_branch: "main".into(),
            worker_branch_prefix: "swarm/heartbeat-symlink".into(),
            worker_command: fixture_argv("heartbeat_symlink_fixture"),
            timeout: Duration::from_secs(30),
            env: vec![(
                "WCORE_SWARM_HEARTBEAT_TARGET".into(),
                secret.to_string_lossy().into_owned(),
            )],
        },
        1,
    );
    let handles = tokio::time::timeout(Duration::from_secs(10), dispatch)
        .await
        .expect("heartbeat authority check hung")
        .unwrap();
    let reason = match &handles[0].status {
        WorkerStatus::Failed(reason) => reason,
        other => panic!("linked heartbeat reported {other:?}"),
    };
    assert!(reason.contains("heartbeat"), "{reason}");
    assert!(!reason.contains(sentinel), "{reason}");
    assert!(!handles[0].stdout.contains(sentinel));
    assert!(!handles[0].stderr.contains(sentinel));
    assert_eq!(transaction_entries(tmp.path()), 0);
}

#[tokio::test]
async fn dispatch_rejects_different_head_repository_replacement() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = tmp.path().join("repo");
    std::fs::create_dir(&repo).unwrap();
    init_repo(&repo).await;
    let swarm = Swarm::new(&repo).unwrap();
    std::fs::rename(&repo, tmp.path().join("original-repo")).unwrap();
    std::fs::create_dir(&repo).unwrap();
    init_repo_with_contents(&repo, "replacement\n").await;

    assert_repository_replacement_rejected(&swarm).await;
}

#[tokio::test]
async fn dispatch_rejects_same_head_repository_replacement() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = tmp.path().join("repo");
    std::fs::create_dir(&repo).unwrap();
    init_repo(&repo).await;
    let swarm = Swarm::new(&repo).unwrap();
    let original = tmp.path().join("original-repo");
    std::fs::rename(&repo, &original).unwrap();
    let source = original.to_string_lossy().into_owned();
    let destination = repo.to_string_lossy().into_owned();
    run_git(
        tmp.path(),
        &["clone", "-q", "--no-local", "--", &source, &destination],
    )
    .await;

    assert_repository_replacement_rejected(&swarm).await;
}

async fn assert_repository_replacement_rejected(swarm: &Swarm) {
    let result = swarm
        .dispatch(
            SwarmBrief {
                task: "repository replacement must fail before execution".into(),
                base_branch: "main".into(),
                worker_branch_prefix: "swarm/replaced-parent".into(),
                worker_command: fixture_argv("repository_replacement_must_not_execute"),
                timeout: Duration::from_secs(30),
                env: vec![],
            },
            1,
        )
        .await;
    let error = result.expect_err("same-path repository replacement was accepted");
    assert!(
        error.to_string().contains("directory identity changed"),
        "{error}"
    );
}

fn transaction_entries(repo: &Path) -> usize {
    std::fs::read_dir(repo.join(".swarm-worktrees"))
        .unwrap()
        .filter(|entry| {
            entry
                .as_ref()
                .is_ok_and(|entry| entry.file_name() != ".wayland-control")
        })
        .count()
}

fn fixture_argv(name: &str) -> Vec<String> {
    vec![
        std::env::current_exe()
            .expect("current test executable")
            .to_string_lossy()
            .into_owned(),
        "--ignored".into(),
        "--exact".into(),
        name.into(),
        "--nocapture".into(),
    ]
}

#[test]
#[ignore = "subprocess fixture"]
fn standalone_authority_fixture() {
    let checkout = std::env::current_dir().unwrap().canonicalize().unwrap();
    let child_git = checkout.join(".git").canonicalize().unwrap();
    let parent_git = std::path::PathBuf::from(std::env::var("WCORE_SWARM_PARENT_GIT").unwrap());
    let sibling = std::path::PathBuf::from(std::env::var("WCORE_SWARM_SIBLING").unwrap());
    let credential = std::path::PathBuf::from(std::env::var("WCORE_SWARM_DENIED_FILE").unwrap());
    let reservation = checkout
        .parent()
        .expect("transaction root")
        .join(".wayland-reservation");

    assert!(child_git.starts_with(&checkout));
    assert_ne!(child_git, parent_git);
    assert!(child_git.join("objects").is_dir());
    assert!(!child_git.join("objects/info/alternates").exists());
    assert!(!child_git.join("worktrees").exists());
    let config = std::fs::read_to_string(child_git.join("config")).unwrap();
    assert!(!config.contains("[remote"));
    assert!(!config.contains(&parent_git.to_string_lossy().to_string()));

    for denied in [
        parent_git.join("config"),
        sibling.join("receipt"),
        credential,
        reservation,
    ] {
        assert!(
            std::fs::read(&denied).is_err(),
            "worker unexpectedly read denied authority {}",
            denied.display()
        );
        assert!(
            std::fs::write(&denied, b"worker-controlled\n").is_err(),
            "worker unexpectedly wrote denied authority {}",
            denied.display()
        );
    }
    assert!(
        std::env::var_os("OPENAI_API_KEY").is_none(),
        "secret-shaped environment reached delegated worker"
    );

    std::fs::write(child_git.join("config"), "[swarm]\n\tchild = true\n").unwrap();
    std::fs::create_dir_all(child_git.join("refs/heads")).unwrap();
    std::fs::write(child_git.join("refs/heads/child-only"), "child-owned\n").unwrap();
    std::fs::create_dir_all(child_git.join("hooks")).unwrap();
    std::fs::write(child_git.join("hooks/child-only"), "child-owned\n").unwrap();
    std::fs::write(child_git.join("objects/child-only"), "child-owned\n").unwrap();
    let scratch = std::path::PathBuf::from(std::env::var("WAYLAND_SWARM_SCRATCH").unwrap());
    std::fs::write(scratch.join("worker-output"), "child-owned\n").unwrap();
    println!("standalone-authority-ok");
}

#[test]
#[ignore = "subprocess fixture"]
fn malformed_heartbeat_fixture() {
    std::fs::write(".swarm-status.json", "{truncated").unwrap();
}

#[cfg(unix)]
#[test]
#[ignore = "subprocess fixture"]
fn heartbeat_symlink_fixture() {
    use std::os::unix::fs::symlink;

    symlink(
        std::env::var("WCORE_SWARM_HEARTBEAT_TARGET").unwrap(),
        ".swarm-status.json",
    )
    .unwrap();
}

#[test]
#[ignore = "subprocess fixture"]
fn repository_replacement_must_not_execute() {
    panic!("worker executed after repository authority replacement");
}

fn snapshot_tree(root: &Path) -> BTreeMap<std::path::PathBuf, Vec<u8>> {
    fn visit(root: &Path, path: &Path, snapshot: &mut BTreeMap<std::path::PathBuf, Vec<u8>>) {
        let Ok(entries) = std::fs::read_dir(path) else {
            return;
        };
        let mut entries = entries.map(|entry| entry.unwrap()).collect::<Vec<_>>();
        entries.sort_by_key(std::fs::DirEntry::file_name);
        for entry in entries {
            let path = entry.path();
            if path.is_dir() {
                visit(root, &path, snapshot);
            } else {
                snapshot.insert(
                    path.strip_prefix(root).unwrap().to_path_buf(),
                    std::fs::read(path).unwrap(),
                );
            }
        }
    }

    let mut snapshot = BTreeMap::new();
    visit(root, root, &mut snapshot);
    snapshot
}

/// Cross-platform "do nothing successfully" argv. On Unix `true` exits
/// 0 with no args. On Windows we spawn `cmd /c rem` (rem is a no-op
/// builtin).
fn noop_argv() -> Vec<String> {
    if cfg!(windows) {
        vec!["cmd".into(), "/c".into(), "rem".into()]
    } else {
        vec!["true".into()]
    }
}

async fn init_repo(path: &Path) {
    init_repo_with_contents(path, "swarm-test\n").await;
}

async fn init_repo_with_contents(path: &Path, readme: &str) {
    let cwd = path.to_path_buf();
    run_git(&cwd, &["init", "-q", "-b", "main"]).await;
    std::fs::write(path.join("README.md"), readme).unwrap();
    std::fs::write(path.join(".gitignore"), ".swarm-worktrees/\n").unwrap();
    run_git(&cwd, &["add", "."]).await;
    run_git(
        &cwd,
        &[
            "-c",
            "user.email=t@e.com",
            "-c",
            "user.name=t",
            "commit",
            "-q",
            "-m",
            "init",
        ],
    )
    .await;
}

async fn run_git(cwd: &Path, args: &[&str]) {
    let mut cmd = shell::shell_command_argv("git", args);
    cmd.current_dir(cwd);
    let st = cmd.status().await.expect("spawn git");
    assert!(st.success(), "git {args:?} failed");
}
