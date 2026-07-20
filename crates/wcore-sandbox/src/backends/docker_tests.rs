use super::*;

#[test]
fn backend_name_is_stable() {
    assert_eq!(DockerBackend::new().name(), "docker");
}

#[cfg(feature = "live-docker")]
#[tokio::test]
async fn cleanup_failure_is_a_terminal_error() {
    let error = require_container_removal(
        "hostile-cleanup-failure",
        std::time::Duration::from_secs(1),
        std::future::ready(Err::<(), _>("daemon refused removal")),
    )
    .await
    .expect_err("unconfirmed removal must fail closed");

    assert!(
        matches!(error, SandboxError::DockerIo(message) if message.contains("daemon refused removal")),
        "cleanup failure must remain visible to the caller"
    );
}

#[cfg(feature = "live-docker")]
#[tokio::test]
async fn cleanup_timeout_is_a_terminal_error() {
    let error = require_container_removal::<&'static str, _>(
        "hostile-cleanup-timeout",
        std::time::Duration::from_millis(1),
        std::future::pending(),
    )
    .await
    .expect_err("timed-out removal must fail closed");

    assert!(
        matches!(error, SandboxError::DockerIo(message) if message.contains("not confirmed")),
        "cleanup timeout must remain visible to the caller"
    );
}

/// sandbox-4: with the `live-docker` feature OFF a docker backend can
/// never be available and execution is refused with `DockerDisabled`
/// rather than silently degrading. (The loud warning is emitted via
/// `is_available`; we assert the security-relevant outcomes here.)
#[cfg(not(feature = "live-docker"))]
#[tokio::test]
async fn docker_disabled_is_unavailable_and_refuses() {
    let backend = DockerBackend::new();
    assert!(
        !backend.is_available(),
        "without live-docker the backend must be unavailable"
    );
    let err = backend
        .execute(
            &SandboxManifest::default(),
            SandboxCommand {
                argv: vec!["/bin/echo".into()],
                cwd: None,
            },
        )
        .await
        .unwrap_err();
    assert!(
        matches!(err, SandboxError::DockerDisabled),
        "execute must refuse with DockerDisabled, got {err:?}"
    );
}

/// Task 5: without the `live-docker` feature the backend enforces nothing
/// and must keep the trait default `false` so the exec-time capability
/// gate remains truthful.
#[cfg(not(feature = "live-docker"))]
#[test]
fn enforces_read_deny_is_false_without_live_docker() {
    assert!(
        !DockerBackend::new().enforces_read_deny(),
        "non-live-docker build must not claim to enforce read-deny"
    );
}

/// Task 5 (live): with the `live-docker` feature ON the backend declares
/// it enforces `fs_read_deny`. This is a capability claim without needing
/// a running daemon — the implementation is in `execute` and CI exercises
/// it end-to-end.
#[cfg(feature = "live-docker")]
#[test]
fn enforces_read_deny_is_true_with_live_docker() {
    assert!(
        DockerBackend::new().enforces_read_deny(),
        "live-docker build must claim to enforce read-deny"
    );
}

#[cfg(feature = "live-docker")]
#[test]
fn buffered_output_accepts_exact_limit() {
    let stdout = vec![0_u8; super::super::BUFFERED_OUTPUT_LIMIT_BYTES - 1];
    assert!(reserve_docker_output(&stdout, &[], 1).is_ok());
}

#[cfg(feature = "live-docker")]
#[test]
fn buffered_output_rejects_first_byte_over_limit() {
    let stdout = vec![0_u8; super::super::BUFFERED_OUTPUT_LIMIT_BYTES];
    assert!(matches!(
        reserve_docker_output(&stdout, &[], 1),
        Err(SandboxError::OutputLimitExceeded { limit_bytes })
            if limit_bytes == super::super::BUFFERED_OUTPUT_LIMIT_BYTES
    ));
}

/// Task 5 (live integration): a file that is read-allowed under a mounted
/// root but also listed in `fs_read_deny` must read as empty inside the
/// container (the `/dev/null` bind shadows it).
///
/// Skips when the Docker daemon is unavailable — this is a live-only test.
#[cfg(feature = "live-docker")]
#[tokio::test]
async fn docker_denies_read_of_secret_under_allowed_root() {
    let backend = match DockerBackend::connect().await {
        Ok(b) => b,
        Err(_) => {
            eprintln!("skip: docker daemon unavailable");
            return;
        }
    };

    // Create a temporary directory on the host containing a "secret" file.
    let workspace = tempfile::TempDir::new().expect("tempdir");
    let secret = workspace.path().join(".env");
    std::fs::write(&secret, b"SECRET=hunter2").expect("write secret");

    let manifest = SandboxManifest {
        // Allow the workspace root (so the container can see the dir).
        fs_read_allow: vec![workspace.path().to_path_buf()],
        // Deny the specific secret file inside the allowed root.
        fs_read_deny: vec![secret.clone()],
        network: NetworkPolicy::Deny,
        image: "alpine:3.19".into(),
        ..Default::default()
    };

    let out = match backend
        .execute(
            &manifest,
            SandboxCommand {
                argv: vec!["cat".into(), secret.to_string_lossy().into_owned()],
                cwd: None,
            },
        )
        .await
    {
        Ok(o) => o,
        Err(e) => {
            eprintln!("skip: docker execute failed ({e:?})");
            return;
        }
    };

    // The deny bind shadows .env with /dev/null — `cat /dev/null` exits 0
    // and produces empty output. Assert that secret bytes are absent.
    let output = String::from_utf8_lossy(&out.stdout);
    assert!(
        !output.contains("SECRET"),
        "secret bytes must not be readable under Docker read-deny; got: {output:?}"
    );
}

/// Cancellation proof for the RAII path: once a container exists, dropping
/// the future that owns its cleanup guard must schedule force-removal.
/// Skips if Docker or the small live-test image is unavailable.
#[cfg(feature = "live-docker")]
#[tokio::test]
async fn cancelled_owner_force_removes_live_container() {
    use bollard::container::{
        Config, CreateContainerOptions, InspectContainerOptions, RemoveContainerOptions,
        StartContainerOptions,
    };

    let backend = match DockerBackend::connect().await {
        Ok(backend) => backend,
        Err(_) => {
            eprintln!("skip: docker daemon unavailable");
            return;
        }
    };
    let client = backend.client_ref().await.unwrap().clone();
    let created = match client
        .create_container(
            None::<CreateContainerOptions<String>>,
            Config {
                image: Some("alpine:3.19".to_string()),
                cmd: Some(vec![
                    "sh".to_string(),
                    "-c".to_string(),
                    "sleep 30".to_string(),
                ]),
                ..Default::default()
            },
        )
        .await
    {
        Ok(created) => created,
        Err(error) => {
            eprintln!("skip: live-test image unavailable ({error})");
            return;
        }
    };
    let id = created.id;
    let cleanup = ContainerCleanup::new(client.clone(), id.clone());
    client
        .start_container(&id, None::<StartContainerOptions<String>>)
        .await
        .unwrap();

    let owner = tokio::spawn(async move {
        let _cleanup = cleanup;
        std::future::pending::<()>().await;
    });
    tokio::task::yield_now().await;
    owner.abort();
    let _ = owner.await;

    let mut removed = false;
    for _ in 0..50 {
        if client
            .inspect_container(&id, None::<InspectContainerOptions>)
            .await
            .is_err()
        {
            removed = true;
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    if !removed {
        let _ = client
            .remove_container(
                &id,
                Some(RemoveContainerOptions {
                    force: true,
                    ..Default::default()
                }),
            )
            .await;
    }
    assert!(removed, "cancelled Docker owner leaked container {id}");
}

#[cfg(all(feature = "live-docker", target_os = "macos"))]
#[tokio::test]
async fn required_live_macos_retained_transport_roundtrips_output_and_deletion() {
    let backend = DockerBackend::connect()
        .await
        .expect("required Docker Desktop daemon");
    let owner = tempfile::tempdir().expect("owner");
    let checkout = owner.path().join("checkout");
    let scratch = owner.path().join("scratch");
    std::fs::create_dir(&checkout).unwrap();
    std::fs::create_dir(&scratch).unwrap();
    std::fs::write(checkout.join("old"), b"before").unwrap();
    let root = crate::DirectoryAuthority::open(owner.path()).unwrap();
    let retained = crate::RetainedWorkspaceAuthority::new(
        root.clone(),
        root.open_child_directory("checkout").unwrap(),
        "required-macos-roundtrip",
    )
    .unwrap();
    let manifest = SandboxManifest {
        fs_read_allow: vec![checkout.clone(), scratch.clone()],
        fs_write_allow: vec![checkout.clone(), scratch],
        network: NetworkPolicy::Deny,
        image: "alpine:3.19".to_owned(),
        ..Default::default()
    };
    let output = crate::SandboxRegistry::new(std::sync::Arc::new(backend))
        .execute_with_workspace_authority(
            &manifest,
            SandboxCommand {
                argv: vec![
                    "sh".into(),
                    "-c".into(),
                    "rm old; printf after > new".into(),
                ],
                cwd: Some(checkout.clone()),
            },
            retained,
            1024 * 1024,
            || Ok(()),
            tokio_util::sync::CancellationToken::new(),
        )
        .await
        .expect("required retained Docker transport");
    assert_eq!(output.exit_code, 0);
    assert!(!checkout.join("old").exists());
    assert_eq!(std::fs::read(checkout.join("new")).unwrap(), b"after");
}

#[cfg(all(feature = "live-docker", target_os = "macos"))]
#[tokio::test]
async fn required_live_macos_retained_transport_rejects_path_replacement() {
    let backend = DockerBackend::connect()
        .await
        .expect("required Docker Desktop daemon");
    let owner = tempfile::tempdir().expect("owner");
    let checkout = owner.path().join("checkout");
    let scratch = owner.path().join("scratch");
    let replacement = owner.path().join("replacement");
    let displaced = owner.path().join("displaced");
    for path in [&checkout, &scratch, &replacement] {
        std::fs::create_dir(path).unwrap();
    }
    std::fs::write(checkout.join("original"), b"authoritative").unwrap();
    std::fs::write(replacement.join("foreign"), b"unchanged").unwrap();
    let root = crate::DirectoryAuthority::open(owner.path()).unwrap();
    let retained = crate::RetainedWorkspaceAuthority::new(
        root.clone(),
        root.open_child_directory("checkout").unwrap(),
        "required-macos-replacement",
    )
    .unwrap();
    let manifest = SandboxManifest {
        fs_read_allow: vec![checkout.clone(), scratch.clone()],
        fs_write_allow: vec![checkout.clone(), scratch],
        network: NetworkPolicy::Deny,
        timeout: Some(std::time::Duration::from_secs(10)),
        image: "alpine:3.19".to_owned(),
        ..Default::default()
    };
    let registry = crate::SandboxRegistry::new(std::sync::Arc::new(backend));
    let execution = registry.execute_with_workspace_authority(
        &manifest,
        SandboxCommand {
            argv: vec![
                "sh".into(),
                "-c".into(),
                "sleep 2; printf result > result".into(),
            ],
            cwd: Some(checkout.clone()),
        },
        retained,
        1024 * 1024,
        || Ok(()),
        tokio_util::sync::CancellationToken::new(),
    );
    tokio::pin!(execution);
    assert!(
        tokio::time::timeout(std::time::Duration::from_millis(500), &mut execution)
            .await
            .is_err(),
        "required live transport terminated before replacement attack"
    );
    std::fs::rename(&checkout, &displaced).unwrap();
    std::fs::rename(&replacement, &checkout).unwrap();
    let error = execution
        .await
        .expect_err("same-path replacement must reject Docker result");
    assert!(matches!(error, SandboxError::PathDenied(_)), "{error:?}");
    assert_eq!(
        std::fs::read(displaced.join("original")).unwrap(),
        b"authoritative"
    );
    assert_eq!(
        std::fs::read(checkout.join("foreign")).unwrap(),
        b"unchanged"
    );
    assert!(!checkout.join("result").exists());
}

/// Linux live acceptance (runs on the committed-head Docker gate). The delegated
/// Docker path admits only a hard, read-deny-enforcing, workspace-authority
/// binding backend; runs a real container through the retained archive transport
/// into container-owned `/workspace`; enforces checkout mutation plus parent-read
/// denial; and force-removes the container on the terminal path (an unconfirmed
/// removal is itself a terminal error, so a clean round-trip proves teardown).
/// Non-skipping: it asserts the Docker daemon is present and FAILS on absence.
#[cfg(feature = "live-docker")]
#[tokio::test]
async fn required_live_docker_admission_enforcement_and_teardown() {
    let backend = DockerBackend::connect()
        .await
        .expect("required live Docker daemon for delegated admission");
    assert!(
        backend.binds_workspace_authority(),
        "delegated Docker backend must bind retained workspace authority"
    );
    assert!(
        backend.owns_descendants_hard(),
        "delegated Docker backend must own its container descendants"
    );
    assert!(
        backend.enforces_read_deny(),
        "delegated Docker backend must enforce read-deny"
    );

    let owner = tempfile::tempdir().expect("owner");
    let checkout = owner.path().join("checkout");
    let scratch = owner.path().join("scratch");
    std::fs::create_dir(&checkout).unwrap();
    std::fs::create_dir(&scratch).unwrap();
    std::fs::write(checkout.join("seed"), b"seeded").unwrap();
    let secret = owner.path().join("parent-secret");
    std::fs::write(&secret, b"parent-only").unwrap();

    let root = crate::DirectoryAuthority::open(owner.path()).unwrap();
    let retained = crate::RetainedWorkspaceAuthority::new(
        root.clone(),
        root.open_child_directory("checkout").unwrap(),
        "required-linux-admission",
    )
    .unwrap();
    let manifest = SandboxManifest {
        fs_read_allow: vec![checkout.clone(), scratch.clone()],
        fs_write_allow: vec![checkout.clone(), scratch],
        network: NetworkPolicy::Deny,
        timeout: Some(std::time::Duration::from_secs(30)),
        image: "alpine:3.19".to_owned(),
        ..Default::default()
    };
    let script = format!(
        "set -e; rm seed; printf mutated > artifact; \
         if cat '{}' 2>/dev/null; then echo LEAK; exit 1; fi; printf ok",
        secret.to_string_lossy()
    );
    let output = crate::SandboxRegistry::new(std::sync::Arc::new(backend))
        .execute_with_workspace_authority(
            &manifest,
            SandboxCommand {
                argv: vec!["sh".into(), "-c".into(), script],
                cwd: Some(checkout.clone()),
            },
            retained,
            1024 * 1024,
            || Ok(()),
            tokio_util::sync::CancellationToken::new(),
        )
        .await
        .expect("required retained Docker admission + transport");
    assert_eq!(output.exit_code, 0, "{output:?}");
    assert!(
        !String::from_utf8_lossy(&output.stdout).contains("parent-only"),
        "parent secret leaked into the delegated container"
    );
    assert!(!checkout.join("seed").exists(), "delete did not round-trip");
    assert_eq!(
        std::fs::read(checkout.join("artifact")).unwrap(),
        b"mutated"
    );
    assert_eq!(
        std::fs::read(&secret).unwrap(),
        b"parent-only",
        "parent state must remain unchanged"
    );
}
