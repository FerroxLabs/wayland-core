//! Does the remote runner clean up after a task that FAILS?
//!
//! Phase 25's Criterion 1 names `cleanup` as one of the four properties that
//! must be equivalent across the four backends, and the ssh surface did not
//! have it. `25-HOSTS-SUMMARY.md` FINDING 5, MEDIUM, open at the time this file
//! was written: the remote runner's `set -e` aborted the script at `wait` for
//! every task that exited non-zero, so `rm -rf "$root"` never ran and
//! `input.bin` — the TASK'S OWN INPUT BYTES — stayed on the far end. Six such
//! roots were found on a real Windows node and purged by hand.
//!
//! The only honest way to test a shell script is to run it in a shell, so this
//! drives the SHIPPED [`REMOTE_RUNNER`] constant — not a paraphrase of it —
//! under a real `sh`, with `TMPDIR` pointed at a private directory so the task
//! root is directly countable.
//!
//! Linux-only, and deliberately not silently skipped anywhere else: the runner
//! requires `setsid(1)`, which macOS does not ship as a binary, so its far end
//! is Linux/POSIX by construction. A `#[cfg(unix)]` here would produce a test
//! binary that exits 0 having run nothing on macOS, which is the vacuity shape
//! this program keeps finding.

#![cfg(target_os = "linux")]

use wcore_exec_backend::backends::ssh::REMOTE_RUNNER;

/// The runner exactly as it stood before 2026-07-29, kept as the NEGATIVE
/// CONTROL.
///
/// Without it this file would pass just as happily against a runner that never
/// had the defect, and would therefore prove nothing about the repair. With it,
/// the test asserts three things rather than two: the fixed script cleans up,
/// the fixed script still reports the task's status, and **the pre-fix script
/// measurably leaks in this same harness**.
const RUNNER_BEFORE_THE_FIX: &str = r#"
set -eu
nonce="$1"; shift
b64input="$1"; shift
root="${TMPDIR:-/tmp}/wayland-f25-$nonce"
mkdir -p "$root"
printf '%s' "$b64input" | base64 -d > "$root/input.bin"
cd "$root"
export WAYLAND_TASK_NONCE="$nonce"
setsid "$@" &
child=$!
echo "$child" > "$root/.pid"
wait "$child"
status=$?
rm -rf "$root"
exit "$status"
"#;

struct Outcome {
    exit_code: i32,
    root_exists: bool,
    input_bin_exists: bool,
}

/// Run one runner script against one task, in a private `TMPDIR`.
///
/// `exit_with` is the status the task itself exits with, so the caller can ask
/// for a failing task without any quoting games: the task is a tiny script file
/// and reaches the runner as a single argv element.
async fn drive(runner: &str, nonce: &str, exit_with: i32) -> Outcome {
    let dir = tempfile::tempdir().expect("a private TMPDIR");
    let tmp = dir.path();

    let runner_path = tmp.join("runner.sh");
    std::fs::write(&runner_path, runner).expect("writing the runner");

    let task_path = tmp.join("task.sh");
    std::fs::write(&task_path, format!("#!/bin/sh\nexit {exit_with}\n")).expect("writing the task");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::set_permissions(&task_path, std::fs::Permissions::from_mode(0o755))
            .expect("marking the task executable");
    }

    // "hello" — five bytes of task input that must not be left behind. Base64
    // is how the real backend carries input across, so the runner exercises its
    // real decode path.
    let input_b64 = "aGVsbG8=";
    let command = format!(
        "TMPDIR={} sh {} {} {} {}",
        tmp.display(),
        runner_path.display(),
        nonce,
        input_b64,
        task_path.display()
    );
    let output = wcore_config::shell::shell_command(&command)
        .await
        .expect("a POSIX shell must be runnable on a linux test host");

    let root = tmp.join(format!("wayland-f25-{nonce}"));
    Outcome {
        exit_code: output.status.code().unwrap_or(-1),
        root_exists: root.exists(),
        input_bin_exists: root.join("input.bin").exists(),
    }
}

/// The repair, and the control that proves the repair does something.
#[tokio::test]
async fn a_failing_task_leaves_nothing_on_the_far_end() {
    // 1. THE NEGATIVE CONTROL, FIRST. If the pre-fix runner does not leak in
    //    this harness, every assertion below is vacuous and the harness — not
    //    the product — is what needs fixing. Running it first means a broken
    //    harness fails here, loudly, rather than silently certifying the fix.
    let before = drive(RUNNER_BEFORE_THE_FIX, "f25-c1-control", 7).await;
    assert!(
        before.root_exists && before.input_bin_exists,
        "the pre-fix runner did NOT leak its task root in this harness, so this \
         test cannot detect the defect it exists to detect (root_exists={}, \
         input_bin_exists={})",
        before.root_exists,
        before.input_bin_exists
    );
    assert_eq!(
        before.exit_code, 7,
        "the control did not even run the task; the harness is measuring nothing"
    );

    // 2. The shipped runner, same failing task, same harness.
    let after = drive(REMOTE_RUNNER, "f25-c1-failing", 7).await;
    assert!(
        !after.input_bin_exists,
        "input.bin — the task's own input bytes — was left on the far end after a failing task"
    );
    assert!(
        !after.root_exists,
        "the task root survived a failing task; cleanup is still conditional on success"
    );

    // 3. And the fix must not have bought cleanup by losing the outcome. A
    //    runner that swallowed the status would clean up beautifully and report
    //    every failure as a success.
    assert_eq!(
        after.exit_code, 7,
        "the task's exit status no longer reaches the caller"
    );
}

/// The success path must not have regressed: it cleaned up before, and the
/// status it reports is still zero.
#[tokio::test]
async fn a_succeeding_task_still_leaves_nothing_and_still_reports_success() {
    let outcome = drive(REMOTE_RUNNER, "f25-c1-succeeding", 0).await;
    assert_eq!(outcome.exit_code, 0);
    assert!(
        !outcome.root_exists,
        "the task root survived a successful task"
    );
    assert!(!outcome.input_bin_exists);
}

/// The trap that is deliberately absent, asserted so nobody adds it back as an
/// "obvious" improvement.
///
/// `trap 'rm -rf "$root"' EXIT` would clean up when the runner is SIGNALLED —
/// which is exactly the case where the `setsid` child deliberately survives as
/// an orphan. It would delete `$root/.pid`, the primary signal the orphan sweep
/// reads, and turn Criterion 4's only unplanted positive control into a clean
/// zero.
#[test]
fn cleanup_is_not_bought_by_deleting_a_live_orphans_evidence() {
    assert!(
        !REMOTE_RUNNER.contains("trap"),
        "an EXIT trap would remove $root/.pid out from under a surviving orphan"
    );
    // Positive control on that absence: this instrument must be able to find
    // something that IS in the script, or the assertion above is free.
    assert!(
        REMOTE_RUNNER.contains(r#"echo "$child" > "$root/.pid""#),
        "the orphan sweep's primary signal is no longer written"
    );
    // The cleanup itself is unconditional on the child's status.
    assert!(REMOTE_RUNNER.contains(r#"wait "$child" || status=$?"#));
}

/// The private sshd is PID 1 in a task-owned namespace. Killing its unshare
/// supervisor kills the namespace and its descendants, including on failure.
struct PrivateSshTarget {
    daemon: std::process::Child,
    root: tempfile::TempDir,
}

impl Drop for PrivateSshTarget {
    fn drop(&mut self) {
        let _ = self.daemon.kill();
        let _ = self.daemon.wait();
    }
}

/// Trusted SSH transport proof, not a remote OS sandbox claim. Requires the
/// explicitly provisioned Linux root buildbox; never silently skips on failure.
#[tokio::test]
#[ignore = "starts a task-owned loopback sshd; requires Linux root and openssh-server"]
async fn ssh_transport_binds_artifact_and_cancels_observed_remote_task() {
    use std::time::Duration;
    use wcore_exec_backend::backends::ssh::{CONFIG_ENV, SshBackend, TARGET_ENV};
    use wcore_exec_backend::conformance::{reference_budget, reference_task};
    use wcore_exec_backend::contract::ExecutionBackend;
    use wcore_exec_backend::receipt::{TerminalStatus, sha256_public};

    let root = tempfile::tempdir().unwrap();
    let at = |name: &str| root.path().join(name).to_string_lossy().into_owned();
    for name in ["host", "client", "wrong-host"] {
        let status = wcore_config::shell::shell_command_argv(
            "ssh-keygen",
            &["-q", "-t", "ed25519", "-N", "", "-f", &at(name)],
        )
        .status()
        .await
        .unwrap();
        assert!(status.success(), "task-owned key generation failed");
    }
    std::fs::copy(at("client.pub"), at("authorized_keys")).unwrap();
    let socket = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = socket.local_addr().unwrap().port();
    let config = format!(
        "ListenAddress 127.0.0.1\nPort {port}\nHostKey {}\nPidFile {}\nAuthorizedKeysFile {}\n\
         AllowUsers root\nPermitRootLogin prohibit-password\nPubkeyAuthentication yes\n\
         PasswordAuthentication no\nKbdInteractiveAuthentication no\nUsePAM no\n\
         AllowAgentForwarding no\nAllowTcpForwarding no\nAllowStreamLocalForwarding no\n\
         X11Forwarding no\nPermitTTY no\nPermitUserRC no\nStrictModes yes\n\
         SetEnv HOME={} TMPDIR={}\n",
        at("host"),
        at("sshd.pid"),
        at("authorized_keys"),
        root.path().display(),
        root.path().display()
    );
    std::fs::write(at("sshd_config"), config).unwrap();
    let client_config = format!(
        "Host w15-private\n HostName 127.0.0.1\n Port {port}\n User root\n IdentityFile {}\n\
         IdentitiesOnly yes\n IdentityAgent none\n UserKnownHostsFile {}\n GlobalKnownHostsFile /dev/null\n\
         StrictHostKeyChecking yes\n BatchMode yes\n",
        at("client"),
        at("known_hosts")
    );
    std::fs::write(at("ssh_config"), client_config).unwrap();
    let known_host = |key: &str| {
        format!(
            "[127.0.0.1]:{port} {}",
            std::fs::read_to_string(at(key)).unwrap()
        )
    };
    std::fs::write(at("known_hosts"), known_host("host.pub")).unwrap();
    let state = tempfile::tempdir().unwrap();
    let _state = wcore_exec_backend::registry::StateDirGuard::set(state.path());
    // This ignored test runs on a single-thread Tokio runtime in its own nextest
    // process. Environment changes precede all backend/transport construction.
    unsafe {
        std::env::set_var(TARGET_ENV, "w15-private");
        std::env::set_var(CONFIG_ENV, at("ssh_config"));
    }
    drop(socket);
    let daemon = wcore_config::shell::shell_command_argv(
        "unshare",
        &[
            "--mount",
            "--pid",
            "--fork",
            "--kill-child=KILL",
            "--mount-proc",
            "/usr/sbin/sshd",
            "-D",
            "-e",
            "-f",
            &at("sshd_config"),
        ],
    )
    .stderr(std::fs::File::create(at("sshd.log")).unwrap())
    .as_std_mut()
    .spawn()
    .unwrap();
    let mut target = PrivateSshTarget { daemon, root };
    let backend = SshBackend::new(reference_budget()).unwrap();
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            assert!(
                target.daemon.try_wait().unwrap().is_none(),
                "private sshd exited"
            );
            if backend.availability().await.available {
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .expect("private pinned SSH handshake required");

    // The pinned host key must be enforced, not just recorded as metadata.
    let known = target.root.path().join("known_hosts");
    let correct = std::fs::read(&known).unwrap();
    let wrong = std::fs::read_to_string(target.root.path().join("wrong-host.pub")).unwrap();
    std::fs::write(&known, format!("[127.0.0.1]:{port} {wrong}")).unwrap();
    assert!(
        !backend.availability().await.available,
        "wrong SSH host key must refuse"
    );
    std::fs::write(&known, correct).unwrap();

    let nonce = format!("w15-ssh-ok-{}", std::process::id());
    let task = reference_task(&nonce, &nonce, reference_budget());
    let receipt = backend.execute(&task).await.unwrap();
    receipt
        .verify(backend.identity(), &backend.verifying_key())
        .unwrap();
    assert_eq!(receipt.body.terminal, TerminalStatus::Success);
    assert_eq!(receipt.body.transport.endpoint, "w15-private");
    assert_eq!(receipt.body.task.input_sha256, sha256_public(&task.input));
    assert_eq!(
        receipt.body.artifact.as_ref().unwrap().sha256,
        sha256_public(&task.input)
    );
    assert!(
        !target
            .root
            .path()
            .join(format!("wayland-f25-{nonce}"))
            .exists()
    );
    let mut wrong_identity = backend.identity().clone();
    wrong_identity.instance_id.push_str("-wrong");
    assert!(
        receipt
            .verify(&wrong_identity, &backend.verifying_key())
            .is_err()
    );
    let mut tampered = receipt.clone();
    tampered.body.artifact.as_mut().unwrap().sha256 = sha256_public(b"wrong artifact");
    assert!(
        tampered
            .verify(backend.identity(), &backend.verifying_key())
            .is_err()
    );

    let nonce = format!("w15-ssh-cancel-{}", std::process::id());
    let mut task = reference_task(&nonce, &nonce, reference_budget());
    task.argv = vec!["sleep".into(), "20".into()];
    let (result, cleanup) = tokio::time::timeout(Duration::from_secs(15), async {
        tokio::join!(backend.execute(&task), async {
            loop {
                let scan = backend.scan_orphans(&nonce).await.unwrap();
                assert!(
                    scan.enumerated,
                    "remote process observation must be available"
                );
                if !scan.found.is_empty() {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(25)).await;
            }
            backend.cancel(&task.task_id).await.unwrap()
        })
    })
    .await
    .expect("observed remote task cancellation must terminate");
    assert!(
        cleanup.residual.is_empty(),
        "remote residuals: {:?}",
        cleanup.residual
    );
    let cancelled = result.unwrap();
    cancelled
        .verify(backend.identity(), &backend.verifying_key())
        .unwrap();
    assert!(matches!(
        cancelled.body.terminal,
        TerminalStatus::Cancelled { .. }
    ));
    let scan = backend.scan_orphans(&nonce).await.unwrap();
    assert!(scan.enumerated && scan.found.is_empty());
    assert!(
        !target
            .root
            .path()
            .join(format!("wayland-f25-{nonce}"))
            .exists()
    );
    println!(
        "SSH witness: pinned peer, artifact/identity negative controls, observed task cancellation, zero residuals; daemon PID {} port {port}",
        target.daemon.id()
    );
    let owned_root = target.root.path().to_owned();
    drop(target);
    assert!(
        !owned_root.exists(),
        "private keys and daemon workspace removed"
    );
    // The unshare supervisor exits before the kernel finishes tearing down
    // its PID namespace. Observe listener closure within a fixed deadline.
    tokio::time::timeout(Duration::from_secs(5), async {
        while tokio::net::TcpStream::connect((std::net::Ipv4Addr::LOCALHOST, port))
            .await
            .is_ok()
        {
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    })
    .await
    .expect("private listener must be removed within five seconds");
}
