#![cfg(feature = "live-docker")]

use std::sync::Arc;
use wcore_sandbox::backends::SandboxBackend;
use wcore_sandbox::backends::docker::DockerBackend;
use wcore_sandbox::{
    NetworkPolicy, ResourceLimitEnforcement, SandboxCommand, SandboxManifest, SandboxRegistry,
};

#[tokio::test]
async fn docker_runs_hello_world() {
    let backend = match DockerBackend::connect().await {
        Ok(b) => b,
        Err(_) => {
            eprintln!("skip: docker daemon unavailable");
            return;
        }
    };
    let reg = SandboxRegistry::new(Arc::new(backend));
    let manifest = SandboxManifest {
        network: NetworkPolicy::Deny,
        max_memory_bytes: Some(64 * 1024 * 1024),
        max_cpu_secs: Some(1),
        image: "alpine:3.19".into(),
        ..Default::default()
    };
    let exec = reg
        .execute(
            &manifest,
            SandboxCommand {
                argv: vec!["echo".into(), "hello-sandbox".into()],
                cwd: None,
            },
        )
        .await
        .unwrap();
    assert_eq!(exec.exit_code, 0);
    assert!(String::from_utf8_lossy(&exec.stdout).contains("hello-sandbox"));
}

#[tokio::test]
async fn docker_rejects_allow_hosts_policy() {
    // Audit B H4: AllowHosts must NOT silently downgrade on the Docker
    // backend; it returns PolicyNotSupported instead.
    let backend = match DockerBackend::connect().await {
        Ok(b) => b,
        Err(_) => {
            eprintln!("skip: docker daemon unavailable");
            return;
        }
    };
    let reg = SandboxRegistry::new(Arc::new(backend));
    let manifest = SandboxManifest {
        network: NetworkPolicy::AllowHosts(vec!["api.example.com".into()]),
        image: "alpine:3.19".into(),
        ..Default::default()
    };
    let err = reg
        .execute(
            &manifest,
            SandboxCommand {
                argv: vec!["true".into()],
                cwd: None,
            },
        )
        .await
        .unwrap_err();
    assert!(
        matches!(err, wcore_sandbox::SandboxError::PolicyNotSupported(_)),
        "expected PolicyNotSupported, got {err:?}"
    );
}

#[tokio::test]
async fn docker_returns_enforced_resource_limits() {
    // Audit B C3: when Docker is the active backend, completed runs MUST
    // report `Enforced` resource limits because `--memory` / `--cpus` are
    // applied by the daemon via cgroups. If this drifts to `BestEffort`
    // or `None`, BashTool will mis-warn the operator.
    let backend = match DockerBackend::connect().await {
        Ok(b) => b,
        Err(_) => {
            eprintln!("skip: docker daemon unavailable");
            return;
        }
    };
    let manifest = SandboxManifest {
        network: NetworkPolicy::Deny,
        max_memory_bytes: Some(256 * 1024 * 1024),
        max_cpu_secs: Some(1),
        image: "alpine:3.19".into(),
        ..Default::default()
    };
    let out = match backend
        .execute(
            &manifest,
            SandboxCommand {
                argv: vec!["echo".into(), "ok".into()],
                cwd: None,
            },
        )
        .await
    {
        Ok(o) => o,
        Err(e) => {
            // Daemon socket exists but isn't responding (e.g. Docker
            // Desktop installed but not running). Skip rather than fail
            // — this test asserts a property of successful runs.
            eprintln!("skip: docker execute failed ({e:?})");
            return;
        }
    };
    assert_eq!(out.exit_code, 0);
    assert_eq!(
        out.resource_limits,
        ResourceLimitEnforcement::Enforced,
        "Docker backend must report Enforced (cgroup-backed)"
    );
}

#[tokio::test]
async fn docker_is_available_uses_socket_probe() {
    // S7: `is_available()` is a cheap socket-existence probe. On a host
    // without dockerd, this returns false without hitting the network.
    let backend = DockerBackend::new();
    let probed = backend.is_available();
    let socket_exists = if cfg!(unix) {
        std::path::Path::new("/var/run/docker.sock").exists()
    } else if cfg!(windows) {
        std::path::Path::new(r"\\.\pipe\docker_engine").exists()
    } else {
        false
    };
    assert_eq!(
        probed, socket_exists,
        "is_available() must match the socket-existence probe"
    );
}

/// Sanity check: building via `SandboxRegistry::new(Arc<dyn …>)` still
/// compiles after the S7 lazy-client refactor. Doesn't touch the daemon.
#[test]
fn docker_registry_construction_is_sync_safe() {
    let backend = DockerBackend::new();
    let _reg = SandboxRegistry::new(Arc::new(backend));
}
