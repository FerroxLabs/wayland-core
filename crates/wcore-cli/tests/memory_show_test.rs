//! M3.4: CLI integration tests for `--memory-show <session_id>`.
//!
//! Seeds a project-tier Procedure into an isolated memory under a
//! tempdir, then invokes the CLI binary with `--memory-show` and asserts
//! the rendered text output mentions the right things.
//!
//! `WCORE_MEMORY_DIR` is set to a per-test tempdir so the global,
//! session, and audit DBs are isolated from the developer's real
//! memory state and from other parallel tests.
//!
//! Scope note: M3.4 v1 ships procedures + user-model only. Episodes
//! lack a public list-by-session API (see comment in
//! `crates/wcore-cli/src/main.rs::run_memory_show`); when an
//! `EpisodicPartition::list_by_session` landing surface is agreed,
//! a follow-up wave extends this test.

use std::fs;

use serial_test::serial;
use tempfile::TempDir;
use uuid::Uuid;

use wcore_memory::Memory;
use wcore_memory::v2_types::{AccessToken, Procedure, ProcedureId, ProcedureStatus, Tier};

/// Build a fixture project with an isolated memory base dir, seed one
/// project-tier procedure. Returns both tempdirs so the test keeps them
/// alive for the duration of the CLI invocation.
async fn fixture_with_session_data(session_id: &str) -> (TempDir, TempDir) {
    let project = TempDir::new().unwrap();
    let memory_root = TempDir::new().unwrap();

    fs::create_dir(project.path().join(".git")).unwrap();

    // SAFETY: env mutation is contained to this test's tempdir; the
    // CLI subprocess inherits this env explicitly via `Command::env`.
    unsafe {
        std::env::set_var("WCORE_MEMORY_DIR", memory_root.path());
    }

    let mem = Memory::open(project.path(), session_id)
        .await
        .expect("open memory");

    let proc = Procedure {
        id: ProcedureId(Uuid::new_v4()),
        tier: Tier::Project,
        ts: 1_700_000_000,
        name: "memory-show-fixture-proc".to_string(),
        description: "fixture procedure".to_string(),
        artifact: "---\nname: x\n---\n".to_string(),
        status: ProcedureStatus::Active,
        created_by: "test".to_string(),
        thompson_alpha: 1.0,
        thompson_beta: 1.0,
        use_count: 0,
        success_count: 0,
        last_latency_ms: 0,
    };
    mem.api()
        .upsert_procedure(proc, AccessToken::System)
        .await
        .expect("upsert_procedure");

    (project, memory_root)
}

#[tokio::test(flavor = "current_thread")]
#[serial]
async fn memory_show_renders_project_procedures_and_session() {
    let session = "test-session-m3-4";
    let (project, memory_root) = fixture_with_session_data(session).await;

    let bin = env!("CARGO_BIN_EXE_wayland-core");
    let out = std::process::Command::new(bin)
        .args(["--memory-show", session])
        .current_dir(project.path())
        .env("WCORE_MEMORY_DIR", memory_root.path())
        .output()
        .expect("run cli");

    assert!(
        out.status.success(),
        "CLI exit: {}\nstderr: {}\nstdout: {}",
        out.status,
        String::from_utf8_lossy(&out.stderr),
        String::from_utf8_lossy(&out.stdout),
    );

    let stdout = String::from_utf8_lossy(&out.stdout);
    // Section header
    assert!(
        stdout.contains("Procedures"),
        "expected 'Procedures' section header; got: {stdout}"
    );
    // Seeded procedure name
    assert!(
        stdout.contains("memory-show-fixture-proc"),
        "expected procedure name in output; got: {stdout}"
    );
    // Session identifier echoed
    assert!(
        stdout.contains(session),
        "expected session id '{session}' in output; got: {stdout}"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn memory_show_unknown_session_succeeds_with_session_echo() {
    let project = TempDir::new().unwrap();
    let memory_root = TempDir::new().unwrap();
    fs::create_dir(project.path().join(".git")).unwrap();

    let bin = env!("CARGO_BIN_EXE_wayland-core");
    let out = std::process::Command::new(bin)
        .args(["--memory-show", "no-such-session"])
        .current_dir(project.path())
        .env("WCORE_MEMORY_DIR", memory_root.path())
        .output()
        .expect("run cli");

    assert!(
        out.status.success(),
        "expected zero exit for empty session; stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("no-such-session"),
        "expected session id echoed even when empty; got: {stdout}"
    );
}
