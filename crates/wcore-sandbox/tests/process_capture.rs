#[cfg(target_os = "linux")]
mod linux {
    use std::path::Path;
    use std::time::{Duration, Instant};

    use tokio::process::Command;
    use wcore_sandbox::process_capture::{
        CaptureLimits, ProcessCaptureError, capture_bounded_process,
    };

    async fn wait_until_gone(pid: u32) {
        let proc_entry = format!("/proc/{pid}");
        let deadline = Instant::now() + Duration::from_secs(3);
        while Path::new(&proc_entry).exists() && Instant::now() < deadline {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        assert!(
            !Path::new(&proc_entry).exists(),
            "captured process descendant {pid} survived tree cleanup"
        );
    }

    #[tokio::test]
    async fn stdout_flood_is_bounded_and_kills_descendant() {
        let fixture = tempfile::tempdir().expect("fixture");
        let pid_file = fixture.path().join("descendant.pid");
        let gate_file = fixture.path().join("go");
        let script = format!(
            "(while [ ! -f '{}' ]; do :; done; \
             while :; do printf '0123456789abcdef0123456789abcdef'; done) & \
             child=$!; printf '%s' \"$child\" > '{}'; : > '{}'; wait \"$child\"",
            gate_file.display(),
            pid_file.display(),
            gate_file.display(),
        );
        let mut command = Command::new("sh");
        command.args(["-c", &script]);
        let limits = CaptureLimits {
            stdout_bytes: 4096,
            stderr_bytes: 4096,
            timeout: Duration::from_secs(5),
        };

        let error = capture_bounded_process(command, limits, None)
            .await
            .expect_err("infinite stdout must hit the byte cap");
        assert!(matches!(
            error,
            ProcessCaptureError::OutputLimit {
                stream: "stdout",
                limit: 4096
            }
        ));

        let pid: u32 = std::fs::read_to_string(&pid_file)
            .expect("descendant pid")
            .parse()
            .expect("numeric descendant pid");
        wait_until_gone(pid).await;
    }

    #[tokio::test]
    async fn stderr_flood_is_bounded() {
        let script = "while :; do printf '0123456789abcdef' >&2; done";
        let mut command = Command::new("sh");
        command.args(["-c", script]);
        let limits = CaptureLimits {
            stdout_bytes: 4096,
            stderr_bytes: 2048,
            timeout: Duration::from_secs(5),
        };

        let error = capture_bounded_process(command, limits, None)
            .await
            .expect_err("infinite stderr must hit the byte cap");
        assert!(matches!(
            error,
            ProcessCaptureError::OutputLimit {
                stream: "stderr",
                limit: 2048
            }
        ));
    }

    #[tokio::test]
    async fn stdin_is_closed_for_noninteractive_helpers() {
        let mut command = Command::new("sh");
        command.args(["-c", "if read value; then exit 9; else printf closed; fi"]);
        let output = capture_bounded_process(
            command,
            CaptureLimits {
                stdout_bytes: 4096,
                stderr_bytes: 4096,
                timeout: Duration::from_secs(2),
            },
            None,
        )
        .await
        .expect("closed stdin should produce EOF without hanging");
        assert!(output.status.success());
        assert_eq!(output.stdout, b"closed");
    }

    #[tokio::test]
    async fn cancellation_kills_the_owned_process_tree() {
        let fixture = tempfile::tempdir().expect("fixture");
        let pid_file = fixture.path().join("cancel-descendant.pid");
        let script = format!(
            "(while :; do sleep 1; done) & child=$!; \
             printf '%s' \"$child\" > '{}'; wait \"$child\"",
            pid_file.display()
        );
        let mut command = Command::new("sh");
        command.args(["-c", &script]);
        let cancel = tokio_util::sync::CancellationToken::new();
        let task_cancel = cancel.clone();
        let task = tokio::spawn(async move {
            capture_bounded_process(
                command,
                CaptureLimits {
                    stdout_bytes: 4096,
                    stderr_bytes: 4096,
                    timeout: Duration::from_secs(5),
                },
                Some(&task_cancel),
            )
            .await
        });

        let deadline = Instant::now() + Duration::from_secs(2);
        while !pid_file.exists() && Instant::now() < deadline {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        let pid: u32 = std::fs::read_to_string(&pid_file)
            .expect("descendant pid")
            .parse()
            .expect("numeric descendant pid");
        cancel.cancel();
        let error = task
            .await
            .expect("capture task")
            .expect_err("cancel must stop capture");
        assert!(matches!(error, ProcessCaptureError::Cancelled));
        wait_until_gone(pid).await;
    }
}
