//! Smoke test: 4 noop workers in parallel.

use std::collections::BTreeMap;
use std::path::Path;
use std::time::Duration;

use wcore_config::shell;
use wcore_swarm::worktree::WorktreeManager;
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
        env: fixture_env(vec![
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
        ]),
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

/// Native Windows public-dispatch Bash contract.
///
/// WHY THIS ASSERTS REFUSAL RATHER THAN CONFINEMENT. Bash cannot run under the
/// Windows AppContainer sandbox at all, and no filesystem grant can change
/// that. Two independent measurements on real SEANDESKTOP hardware settle
/// ACL-vs-architectural:
///
///   * STATIC — every object on git-bash's load chain (`C:\Program Files\Git`,
///     `\bin`, `\bin\bash.exe`, `\usr\bin`, `\usr\bin\bash.exe`, and
///     `\usr\bin\msys-2.0.dll` itself) ALREADY carries `ALL APPLICATION
///     PACKAGES` (S-1-15-2-1) and `ALL RESTRICTED APPLICATION PACKAGES`
///     (S-1-15-2-2) ReadAndExecute ACEs. The first covers every AppContainer
///     package SID; the second covers the restricted token's second access
///     check. There is no file ACL left to grant.
///
///   * DYNAMIC — spawning the absolute `…\usr\bin\bash.exe` under the real
///     restricted token fails IDENTICALLY with and without an `fs_read_allow`
///     grant on `C:\Program Files\Git` (both `0xC0000142`), and msys names its
///     own root cause on stderr:
///     `NtCreateDirectoryObject(\BaseNamedObjects\msys-2.0S5-…): 0xC0000022`
///     — STATUS_ACCESS_DENIED on the GLOBAL NT object namespace, not on any
///     file. msys/cygwin must create its shared cygheap rendezvous object in
///     `\BaseNamedObjects`; an AppContainer is confined to its own private
///     `AppContainerNamedObjects` namespace BY CONSTRUCTION. Granting that
///     would delete the sandbox's object-namespace isolation — it is the
///     containment working, not a permission gap.
///
/// So "a Bash process is confined" asserts against a process that cannot
/// exist on this platform. This test asserts the real, security-relevant
/// Windows contract instead, and that contract is STRICTLY STRONGER than the
/// unreachable one: the Bash worker is refused FAIL-CLOSED with a legible
/// reason, leaks nothing, leaves parent and sibling authority byte-intact, and
/// still releases its transaction workspace.
///
/// This cannot degrade into a silent pass. A Bash worker that actually ran —
/// whether it escaped OR was confined — reports `Succeeded`, or `Failed` with
/// some other reason, and fails the assertions below. macOS is untouched and
/// continues to prove live Bash confinement through
/// `assert_public_dispatch_bash_confines_parent_and_descendants`.
#[cfg(windows)]
#[tokio::test]
async fn required_live_windows_public_dispatch_refuses_bash_worker_and_preserves_parent_and_sibling_state()
 {
    let tmp = tempfile::tempdir().unwrap();
    init_repo(tmp.path()).await;
    let parent_git = tmp.path().join(".git");
    let sibling = tmp.path().join(".swarm-worktrees/sibling-evidence");
    std::fs::create_dir_all(&sibling).unwrap();
    std::fs::write(sibling.join("receipt"), "sibling-owned\n").unwrap();
    let credential = parent_git.join("worker-credential");
    std::fs::write(&credential, "parent-secret\n").unwrap();

    // Deliberately a script that WOULD disclose both secrets if it ever ran, so
    // the leak assertions below stay falsifiable rather than vacuous.
    let script = format!(
        "cat '{parent}'; cat '{sibling}/receipt'",
        parent = credential.to_string_lossy(),
        sibling = sibling.to_string_lossy(),
    );
    let swarm = Swarm::new(tmp.path()).unwrap();
    let handles = swarm
        .dispatch(
            SwarmBrief {
                task: "native public-dispatch bash refusal".into(),
                base_branch: "main".into(),
                worker_branch_prefix: "swarm/native-bash".into(),
                worker_command: vec!["bash".into(), "-c".into(), script],
                timeout: Duration::from_secs(60),
                env: vec![],
            },
            1,
        )
        .await
        .expect("dispatch must be admitted; the refusal belongs to worker execution");
    assert_eq!(handles.len(), 1);

    let reason = match &handles[0].status {
        WorkerStatus::Failed(reason) => reason,
        other => panic!(
            "a Bash worker must be refused fail-closed under the Windows AppContainer \
             sandbox, but the worker reported {other:?}"
        ),
    };
    // Legible: an operator can act on the message instead of decoding an
    // NTSTATUS out of an empty stderr.
    assert!(
        reason.contains("not supported under the Windows AppContainer sandbox"),
        "refusal must name the sandbox and the unsupported shell: {reason}"
    );
    assert!(
        reason.contains("bash"),
        "refusal must name argv[0]: {reason}"
    );

    // Fail-closed means nothing executed, so nothing can have been disclosed.
    for leaked in ["parent-secret", "sibling-owned"] {
        assert!(
            !reason.contains(leaked),
            "refusal leaked {leaked}: {reason}"
        );
        assert!(
            !handles[0].stdout.contains(leaked),
            "worker stdout leaked {leaked}"
        );
        assert!(
            !handles[0].stderr.contains(leaked),
            "worker stderr leaked {leaked}"
        );
    }

    // Parent and sibling authority survive the refused worker byte-intact.
    assert_eq!(
        std::fs::read_to_string(&credential).unwrap(),
        "parent-secret\n"
    );
    assert_eq!(
        std::fs::read_to_string(sibling.join("receipt")).unwrap(),
        "sibling-owned\n"
    );
    // A refused worker releases its OWN transaction workspace and destroys
    // nobody else's: the decoy sibling planted above must be the one and only
    // surviving entry. Asserting the exact residue rather than a bare count
    // catches both a leaked worker workspace AND collateral deletion of the
    // sibling, either of which a `== 0` count would have hidden.
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

/// Preconditions for the `required_live_macos_*` delegated-Docker case below.
///
/// Returns `false` after printing a `skip:` line that NAMES the missing
/// precondition, rather than returning a silent green. A skip is not a pass:
/// `.github/workflows/macos-docker-gate.yml` runs these under `--nocapture`
/// (libtest DISCARDS a passing test's stderr otherwise) and fails its gate on
/// any `skip:` in the output, so an un-opted-in or Docker-less host can never
/// be read as certification. `DockerBackend::connect` is the probe because it covers BOTH
/// ways delegated macOS execution can be unavailable — a build without the
/// `wcore-sandbox/live-docker` feature (`DockerDisabled`) and a host with no
/// reachable daemon (`DockerIo`). On macOS the sandbox-exec primary is not a
/// hard-containment backend, so `select_delegated_backend` can only admit the
/// Docker fallback; without it there is no reachable pass state at all.
#[cfg(target_os = "macos")]
async fn live_macos_docker_available() -> bool {
    if std::env::var("WAYLAND_SANDBOX_LIVE_DOCKER").is_err() {
        eprintln!(
            "skip: WAYLAND_SANDBOX_LIVE_DOCKER not set \
             (host has not opted into live delegated-Docker execution)"
        );
        return false;
    }
    match wcore_sandbox::backends::docker::DockerBackend::connect().await {
        Ok(_) => true,
        Err(error) => {
            eprintln!(
                "skip: delegated Docker backend unavailable ({error}) — needs the \
                 `wcore-sandbox/live-docker` feature and a running Docker daemon"
            );
            false
        }
    }
}

/// Native macOS public-dispatch Bash containment: it enters through the public
/// `Swarm::dispatch`, runs real Bash inside the delegated checkout, and FAILS
/// (never skips) once its preconditions hold — an unavailable or non-binding
/// containment backend surfaces as a failed worker, not as a green.
#[cfg(target_os = "macos")]
#[tokio::test]
#[ignore = "live macOS delegated-Docker acceptance; run via `--run-ignored all` (or `-- --ignored`) with WAYLAND_SANDBOX_LIVE_DOCKER=1"]
async fn required_live_macos_public_dispatch_bash_confines_parent_and_descendants() {
    if !live_macos_docker_available().await {
        return;
    }
    assert_public_dispatch_bash_confines_parent_and_descendants().await;
}

/// Native macOS composition: a real Bash worker, entered through public
/// `Swarm::dispatch`, may mutate its isolated checkout but must be denied every
/// parent/sibling read and write. A missing or non-binding native containment
/// backend surfaces as a failed worker, so the assertions below fail rather
/// than skip.
///
/// macOS-only. Windows cannot host a Bash process under its AppContainer
/// sandbox at all (see the measurement recorded on
/// `required_live_windows_public_dispatch_refuses_bash_worker_and_preserves_parent_and_sibling_state`),
/// so it proves the refusal contract instead. Nothing asserted here is reduced.
#[cfg(target_os = "macos")]
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
                env: fixture_env(vec![]),
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
            env: fixture_env(vec![(
                "WCORE_SWARM_HEARTBEAT_TARGET".into(),
                secret.to_string_lossy().into_owned(),
            )]),
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
    let container = tmp.path().join("box");
    let repo = container.join("repo");
    std::fs::create_dir_all(&repo).unwrap();
    init_repo(&repo).await;
    // Retaining the swarm keeps an open directory handle on `repo` alive for the
    // whole test — the hold that forces the Windows OS refusal below.
    let swarm = Swarm::new(&repo).unwrap();
    let moved = tmp.path().join("original-box");

    if cfg!(windows) {
        // Under this topology the swarm retains a handle on a directory INSIDE
        // `repo`, so the ancestor `container` cannot be renamed and the
        // substitution below is unconstructible VIA THIS CONSTRUCTION. The
        // software defense itself is proved on both platforms by
        // `repository_replaced_at_same_pathname_is_refused_by_retained_authority`.
        assert_rename_refused_by_open_descendant(&container, &moved);
    } else {
        replace_repo_container(&container, &moved);
        std::fs::create_dir(&repo).unwrap();
        init_repo_with_contents(&repo, "replacement\n").await;
        assert_repository_replacement_rejected(&swarm).await;
    }
}

#[tokio::test]
async fn dispatch_rejects_same_head_repository_replacement() {
    let tmp = tempfile::tempdir().unwrap();
    let container = tmp.path().join("box");
    let repo = container.join("repo");
    std::fs::create_dir_all(&repo).unwrap();
    init_repo(&repo).await;
    // Retaining the swarm keeps an open directory handle on `repo` alive for the
    // whole test — the hold that forces the Windows OS refusal below.
    let swarm = Swarm::new(&repo).unwrap();
    let moved = tmp.path().join("original-box");

    if cfg!(windows) {
        // Same descendant-handle refusal as the different-HEAD case: the swarm
        // retains a handle inside `repo`, so the ancestor `container` cannot be
        // renamed and no same-HEAD clone can be swapped in at the original path
        // VIA THIS CONSTRUCTION.
        assert_rename_refused_by_open_descendant(&container, &moved);
    } else {
        replace_repo_container(&container, &moved);
        let source = moved.join("repo").to_string_lossy().into_owned();
        let destination = repo.to_string_lossy().into_owned();
        run_git(
            tmp.path(),
            &["clone", "-q", "--no-local", "--", &source, &destination],
        )
        .await;
        assert_repository_replacement_rejected(&swarm).await;
    }
}

/// Replace the repository at the SAME pathname with a different on-disk
/// directory object, WITHOUT renaming the swarm-held `repo` directory itself
/// (UNIX ONLY — see [`assert_rename_refused_by_open_descendant`] for the
/// Windows arm).
///
/// The swarm retains open `DirectoryAuthority` handles on `repo` AND on its
/// `.swarm-worktrees` control descendants. On Unix those handles bind to the
/// directory *inode*, so renaming the ancestor `container` out and recreating a
/// fresh directory at the original `repo` path succeeds, yielding exactly the
/// "same path, different directory object" condition that the software
/// `validate_repo_authority` check rejects.
///
/// On Windows this ancestor rename is instead OS-REFUSED with
/// `Os { code: 5, PermissionDenied }` — because a `.swarm-worktrees` DESCENDANT
/// handle is open inside `repo`, not because of any share-mode property. The
/// substitution is unconstructible VIA THIS ANCESTOR CONSTRUCTION for THIS
/// topology; it is not impossible in general. The software defense is proved on
/// both platforms by
/// [`repository_replaced_at_same_pathname_is_refused_by_retained_authority`],
/// whose out-of-repository swarm root leaves no descendant handle inside
/// `repo`.
fn replace_repo_container(container: &Path, moved_container: &Path) {
    std::fs::rename(container, moved_container).unwrap();
    std::fs::create_dir(container).unwrap();
}

/// Windows OS-refusal counterpart to [`replace_repo_container`].
///
/// MEASURED WINDOWS RENAME RULE, which this assertion depends on: ANY open
/// handle to a DESCENDANT — of any kind, at any desired access, under any share
/// mode — blocks renaming ANY ancestor with `ERROR_ACCESS_DENIED`, which Rust
/// maps to `PermissionDenied`. Desired access and share mode are irrelevant to
/// that outcome. Conversely a handle on an OBJECT never blocks renaming that
/// object at all when the share mode admits delete, so the refusal here is
/// caused entirely by the `.swarm-worktrees` handle the swarm retains INSIDE
/// `repo` — nothing about the `repo` handle itself.
///
/// The assertion is non-vacuous: were the descendant hold absent, the rename
/// would succeed and `expect_err` would fail the test. What it does NOT prove
/// is that the substitution is impossible in general — only that this ancestor
/// construction cannot build it under this topology.
///
/// Compiled on all platforms (statically referenced from the `cfg!(windows)`
/// arm of the dispatch tests) but only executed on Windows.
fn assert_rename_refused_by_open_descendant(container: &Path, moved_container: &Path) {
    let error = std::fs::rename(container, moved_container)
        .expect_err("Windows must refuse renaming an ancestor of a swarm-held descendant");
    assert_eq!(
        error.kind(),
        std::io::ErrorKind::PermissionDenied,
        "expected OS-level PermissionDenied renaming an ancestor of an open descendant, got {error:?}"
    );
}

/// The SOFTWARE defense — `validate_repo_authority` ->
/// `DirectoryAuthority::validate_path` — must refuse a repository replaced at
/// the SAME pathname by a different directory object. Ungated: the invariant is
/// universal, and unix is where this stands as the permanent guard.
///
/// TOPOLOGY IS THE ENTIRE POINT, and it is why this test exists separately from
/// the two dispatch replacement tests above. `Swarm::new` builds its manager
/// through `WorktreeManager::new`, which places the swarm root at
/// `<repo>/.swarm-worktrees` and RETAINS an authority on it — a live DESCENDANT
/// handle INSIDE the repository. By the measured rename rule an open descendant
/// handle blocks renaming any ancestor, so under that topology neither `repo`
/// nor any ancestor of it can be renamed on Windows and the substitution cannot
/// be constructed at all.
///
/// `WorktreeManager::new_with_workspace_root` instead places the swarm root
/// under a SEPARATE directory outside the repository. The only handle the
/// manager then holds inside `repo` is `repo_authority` on the repository
/// OBJECT itself, and a handle on an object never blocks renaming that object
/// when the share mode admits delete. The substitution therefore becomes
/// constructible and the software defense is genuinely exercised — on Windows
/// as well as unix.
///
/// Plan 20-72 converted `repo_authority` to a read-only observational open.
/// That does not affect this analysis: desired access is irrelevant to renaming
/// the object itself.
///
/// This test is the replacement for the software-defense coverage that the
/// Windows arms of the two dispatch replacement tests lost when commit
/// `334f264d` traded it for an OS-behaviour assertion.
#[tokio::test]
async fn repository_replaced_at_same_pathname_is_refused_by_retained_authority() {
    let repo_home = tempfile::tempdir().unwrap();
    let repo = repo_home.path().join("repo");
    std::fs::create_dir(&repo).unwrap();
    init_repo(&repo).await;

    let workspace_home = tempfile::tempdir().unwrap();
    let workspace_root = workspace_home.path().join("orchestrator-workspaces");
    let manager = WorktreeManager::new_with_workspace_root(&repo, &workspace_root).unwrap();

    // The retained authority accepts the un-substituted repository, so the
    // refusal below cannot be blamed on an unrelated precondition.
    manager
        .retained_worker_count(8)
        .expect("the un-substituted repository must satisfy its retained authority");

    let moved = repo_home.path().join("original-repo");
    std::fs::rename(&repo, &moved)
        .expect("renaming the repository OBJECT itself must be permitted with no descendant held");
    std::fs::create_dir(&repo).unwrap();

    let error = manager
        .retained_worker_count(8)
        .expect_err("same-pathname repository replacement was accepted");
    assert!(
        error
            .to_string()
            .contains("directory identity changed after authority was retained"),
        "{error}"
    );
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
                env: fixture_env(vec![]),
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

/// Sentinel the parent sets on EVERY fixture dispatch.
///
/// A `#[ignore]`d fixture below is a subprocess PAYLOAD, not a test: it asserts
/// against an environment only its parent can create — cwd inside the delegated
/// checkout, `WCORE_SWARM_*` handles onto parent/sibling authority, a live
/// transaction workspace. A whole-binary sweep (`--run-ignored all`, which the
/// native proof applies to every target) invokes those payloads standalone,
/// where that environment does not exist and the payload has nothing to assert.
/// Absent this sentinel a payload therefore returns as a no-op: an honest
/// result, because a fixture with no parent has no claim to make.
///
/// WHY THE NO-OP CANNOT MASK A GENUINE FAILURE. The skip is gated on the
/// ABSENCE of a parent, and every parent independently asserts a POSITIVE
/// effect that only the payload's full body can produce:
///
///   * `standalone_authority_fixture` — the parent requires
///     `stdout.contains("standalone-authority-ok")`, a marker printed only
///     after the payload's entire assertion body has passed.
///   * `repository_replacement_must_not_execute` — the parent requires
///     `dispatch` to return the identity-change error, i.e. the payload is
///     never spawned at all; a payload that ran and no-opped would produce a
///     SUCCESSFUL dispatch and trip the parent's `expect_err`.
///   * `malformed_heartbeat_fixture` / `heartbeat_symlink_fixture` — the parent
///     requires `WorkerStatus::Failed` carrying a heartbeat reason, which only
///     the payload's side effect on `.swarm-status.json` can produce; a no-op
///     yields a succeeded worker and panics the parent.
///
/// So the skip path is reachable ONLY when there is no parent to be failed, and
/// if the sentinel were ever lost on a real dispatch the payload would no-op
/// and its parent would fail LOUDLY rather than silently pass. The skip can
/// suppress a fixture's own standalone panic; it cannot suppress a real
/// assertion, because in every case the real assertion lives in the parent.
const FIXTURE_PARENT: &str = "WCORE_SWARM_FIXTURE_PARENT";

/// True when this payload was swept standalone, with no parent dispatch.
fn fixture_without_parent() -> bool {
    std::env::var_os(FIXTURE_PARENT).is_none()
}

/// Environment for a fixture dispatch: the caller's handles plus the sentinel
/// that tells the payload a parent exists.
fn fixture_env(extra: Vec<(String, String)>) -> Vec<(String, String)> {
    let mut env = extra;
    env.push((FIXTURE_PARENT.into(), "1".into()));
    env
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
    // WHY THIS RUNS INSIDE THE CONTAINMENT WITHOUT `std::fs::canonicalize`.
    //
    // A delegated worker's filesystem grant set is exactly its own checkout and
    // its own scratch, and nothing else. Measured on SEANDESKTOP under the real
    // AppContainer restricted token: the checkout itself opens for zero-access,
    // read-attributes and generic-read, stats and enumerates — while EVERY
    // ancestor of it, up to and including `C:\`, is `Access is denied`.
    //
    // `canonicalize` cannot survive that, and must not. On Windows it opens the
    // object (which succeeds) and then calls `GetFinalPathNameByHandleW` with
    // `VOLUME_NAME_DOS`, which has to resolve the volume back to a drive letter
    // through the volume root — an object the containment deliberately withholds
    // — so it returns ERROR_ACCESS_DENIED. Resolving the volume namespace is a
    // capability a contained worker is not supposed to have; a fixture that
    // needs it is asserting against the sandbox rather than through it.
    //
    // The two checks below replace `canonicalize(...).starts_with(checkout)` and
    // are STRICTLY STRONGER than it:
    //   * the old form accepted a `.git` FILE holding a `gitdir:` redirect at
    //     the parent repository — canonicalizing a regular file inside the
    //     checkout still yields a path inside the checkout, so `starts_with`
    //     passed. `is_dir()` on the un-followed metadata refuses it.
    //   * a symlink or NTFS junction at `.git` is refused WITHOUT following it,
    //     which is the escape `canonicalize` existed here to catch.
    if fixture_without_parent() {
        return;
    }
    let checkout = std::env::current_dir().unwrap();
    let child_git = checkout.join(".git");
    let child_git_kind = std::fs::symlink_metadata(&child_git).unwrap();
    let parent_git = std::path::PathBuf::from(std::env::var("WCORE_SWARM_PARENT_GIT").unwrap());
    let sibling = std::path::PathBuf::from(std::env::var("WCORE_SWARM_SIBLING").unwrap());
    let credential = std::path::PathBuf::from(std::env::var("WCORE_SWARM_DENIED_FILE").unwrap());
    let reservation = checkout
        .parent()
        .expect("transaction root")
        .join(".wayland-reservation");

    assert!(
        child_git_kind.is_dir(),
        "child .git must be a real in-tree directory, not a `gitdir:` redirect file"
    );
    assert!(
        !child_git_kind.file_type().is_symlink(),
        "child .git must not be a symlink or junction escaping the checkout"
    );
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
    if fixture_without_parent() {
        return;
    }
    std::fs::write(".swarm-status.json", "{truncated").unwrap();
}

#[cfg(unix)]
#[test]
#[ignore = "subprocess fixture"]
fn heartbeat_symlink_fixture() {
    use std::os::unix::fs::symlink;

    if fixture_without_parent() {
        return;
    }
    symlink(
        std::env::var("WCORE_SWARM_HEARTBEAT_TARGET").unwrap(),
        ".swarm-status.json",
    )
    .unwrap();
}

#[test]
#[ignore = "subprocess fixture"]
fn repository_replacement_must_not_execute() {
    if fixture_without_parent() {
        return;
    }
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
