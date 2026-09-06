//! W15: enumerate owned detached descendants before terminating their parent.
#![cfg(target_os = "linux")]
use std::{sync::Arc, time::Duration};
use wcore_browser::supervisor::{BrowserSupervisor, SupervisorConfig};
#[path = "support/process_identity.rs"]
mod process_identity;

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
        // A second identity-backed guard only cleans this test's detached
        // group on assertion failure; it is retained THROUGH the assertion.
        let _failure_cleanup =
            wcore_sandbox::backends::process_tree::ProcessTreeGuard::new(Some(child)).unwrap();
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
