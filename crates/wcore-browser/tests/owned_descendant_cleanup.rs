//! W15: enumerate owned detached descendants before terminating their parent.
#![cfg(target_os = "linux")]
use std::{sync::Arc, time::Duration};
use wcore_browser::supervisor::{BrowserSupervisor, SupervisorConfig};
#[path = "support/process_identity.rs"]
mod process_identity;

// An escaped session cannot accept our process-group sentinel. A pidfd gives
// the fixture a failure-only cleanup handle without signalling a recycled PID.
struct FailureCleanup(std::os::fd::OwnedFd);
impl FailureCleanup {
    fn new(pid: u32) -> Self {
        use std::os::fd::FromRawFd;
        // SAFETY: the ready child is still owned by the live test parent.
        let fd = unsafe { libc::syscall(libc::SYS_pidfd_open, pid, 0) };
        assert!(fd >= 0, "pidfd_open: {}", std::io::Error::last_os_error());
        Self(unsafe { std::os::fd::OwnedFd::from_raw_fd(fd as i32) })
    }
}
impl Drop for FailureCleanup {
    fn drop(&mut self) {
        use std::os::fd::AsRawFd;
        // SAFETY: the fd binds this exact test child, including after reparenting.
        unsafe {
            libc::syscall(
                libc::SYS_pidfd_send_signal,
                self.0.as_raw_fd(),
                libc::SIGKILL,
                std::ptr::null::<libc::siginfo_t>(),
                0,
            );
        }
    }
}

#[tokio::test]
async fn session_end_and_drop_terminate_ready_detached_descendants() {
    for end_session in [true, false] {
        let dir = tempfile::tempdir().unwrap();
        let ready = dir.path().join("ready.pid");
        let supervisor = Arc::new(BrowserSupervisor::with_config(SupervisorConfig {
            pid_dir: dir.path().join("pids"),
            ..Default::default()
        }));
        let root = supervisor
            .launch_camoufox(
                std::path::Path::new("sh"),
                &[
                    "-c",
                    "setsid sh -c 'echo $$ > \"$1\"; exec sleep 60' _ \"$1\" & wait",
                    "_",
                    ready.to_str().unwrap(),
                ],
                "detached-cleanup",
            )
            .await
            .unwrap();
        let child: u32 = tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                if let Ok(text) = std::fs::read_to_string(&ready)
                    && let Ok(pid) = text.trim().parse()
                {
                    break pid;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("detached child must publish readiness");
        // Retained THROUGH the assertion: never substitutes for product cleanup.
        let _failure_cleanup = FailureCleanup::new(child);
        let owned = process_identity::snapshot_tree(root);
        assert!(
            owned
                .iter()
                .any(|id| id.pid == child && id.birth > 0 && !id.name.is_empty())
        );
        eprintln!("owned before cleanup: {owned:?}");
        if end_session {
            assert!(supervisor.on_session_end("detached-cleanup"));
        }
        drop(supervisor);
        process_identity::assert_terminated(&owned).await;
    }
}
