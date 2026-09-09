#![cfg(windows)]

use std::io::Read;
use std::process::{Command, Stdio};
use std::sync::{Arc, mpsc};
use std::time::{Duration, Instant};
use wcore_sandbox::backends::{SandboxBackend, windows_job_object::WindowsJobObjectBackend};
use wcore_sandbox::{SandboxChunk, SandboxCommand, SandboxManifest};

const ROLE: &str = "WCORE_STDIN_FIXTURE_KIND";

fn assert_host_input_is_detached(mode: &str) {
    let mut child = Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "stdin_fixture_driver", "--nocapture"])
        .env_clear()
        .env("SYSTEMROOT", std::env::var_os("SYSTEMROOT").unwrap())
        .env(ROLE, mode)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    // Keep the protocol pipe open without supplying bytes. The driver owns
    // its pending read, just as Desktop's Core protocol reader does.
    let deadline = Instant::now() + Duration::from_secs(15);
    while child.try_wait().unwrap().is_none() {
        if Instant::now() >= deadline {
            child.kill().unwrap();
            child.wait().unwrap();
            panic!("shell fixture did not finish with an open host-input pipe");
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    let output = child.wait_with_output().unwrap();
    assert!(
        output.status.success(),
        "{mode}: {}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn buffered_shell_does_not_inherit_host_protocol_input() {
    assert_host_input_is_detached("buffered");
}

#[test]
fn streaming_shell_does_not_inherit_host_protocol_input() {
    assert_host_input_is_detached("streaming");
}

#[test]
fn stdin_fixture_reader() {
    if std::env::var(ROLE).as_deref() != Ok("reader") {
        return;
    }
    assert_eq!(std::io::stdin().read(&mut [0_u8; 1]).unwrap(), 0);
    println!("SHELL_STDIN_EOF");
}

#[test]
fn stdin_fixture_driver() {
    let Ok(mode) = std::env::var(ROLE) else {
        return;
    };
    let (entered, ready) = mpsc::channel();
    std::thread::spawn(move || {
        entered.send(()).unwrap();
        let _ = std::io::stdin().read(&mut [0_u8; 1]);
    });
    ready.recv().unwrap();
    std::thread::sleep(Duration::from_millis(50));
    let manifest = SandboxManifest {
        env: vec![
            (ROLE.into(), "reader".into()),
            ("SYSTEMROOT".into(), std::env::var("SYSTEMROOT").unwrap()),
        ],
        ..Default::default()
    };
    let command = SandboxCommand {
        argv: vec![
            std::env::current_exe()
                .unwrap()
                .to_string_lossy()
                .into_owned(),
            "--exact".into(),
            "stdin_fixture_reader".into(),
            "--nocapture".into(),
        ],
        cwd: None,
    };
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(async {
            let backend = Arc::new(WindowsJobObjectBackend::new());
            let (exit, stdout) = tokio::time::timeout(Duration::from_secs(3), async {
                if mode == "buffered" {
                    let output = backend.execute(&manifest, command).await.unwrap();
                    (Some(output.exit_code), output.stdout)
                } else {
                    let mut rx = backend.execute_streaming(&manifest, command).unwrap();
                    let mut stdout = Vec::new();
                    let mut exit = None;
                    while let Some(chunk) = rx.recv().await {
                        match chunk {
                            SandboxChunk::Stdout(bytes) => stdout.extend(bytes),
                            SandboxChunk::Exit { exit_code, .. } => exit = Some(exit_code),
                            _ => {}
                        }
                    }
                    (exit, stdout)
                }
            })
            .await
            .expect("shell inherited the blocked host protocol input");
            assert_eq!(exit, Some(0));
            assert!(String::from_utf8_lossy(&stdout).contains("SHELL_STDIN_EOF"));
        });
}
