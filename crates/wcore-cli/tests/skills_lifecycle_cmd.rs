//! W9.1 T4 (T11): CLI integration tests for `--skills-promote` and
//! `--skills-archive` against a fixture project. Pre-seeds a `Staged`
//! procedure into the project's `.wayland-core/memory/memory.db`,
//! invokes the CLI binary with the lifecycle flag, then re-opens
//! memory and asserts the procedure's status transitioned correctly.
//!
//! `WCORE_MEMORY_DIR` is set to a per-test tempdir so the global,
//! session, and audit DBs are isolated from the developer's real
//! memory state and from other parallel tests.

use std::fs;

use serial_test::serial;
use tempfile::TempDir;
use uuid::Uuid;

use wcore_memory::Memory;
use wcore_memory::v2_types::{AccessToken, Procedure, ProcedureId, ProcedureStatus, Tier};

/// Build a fixture project with an isolated memory base dir, seed a
/// staged procedure inside it, and return the temp roots + the seeded
/// procedure id. Both temp dirs are kept alive by returning them.
async fn fixture_with_staged_procedure(name: &str) -> (TempDir, TempDir, ProcedureId) {
    let project = TempDir::new().unwrap();
    let memory_root = TempDir::new().unwrap();

    // Marker so any tooling that requires a project root sees one.
    fs::create_dir(project.path().join(".git")).unwrap();

    // Isolate global/session/audit DBs to `memory_root` for this test.
    // `Memory::open` reads WCORE_MEMORY_DIR via `paths::memory_base_dir`.
    // SAFETY: env mutation is contained to this test's tempdir; the
    // CLI subprocess inherits this env explicitly via `Command::env`.
    unsafe {
        std::env::set_var("WCORE_MEMORY_DIR", memory_root.path());
    }

    let mem = Memory::open(project.path(), "cli-skills-cmd")
        .await
        .expect("open memory");

    let proc_id = ProcedureId(Uuid::new_v4());
    let proc = Procedure {
        id: proc_id,
        tier: Tier::Project,
        ts: 1_700_000_000,
        name: name.to_string(),
        description: format!("seed proc for {name}"),
        artifact: "---\nname: x\n---\n".to_string(),
        status: ProcedureStatus::Staged,
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

    (project, memory_root, proc_id)
}

async fn read_procedure_status(
    project_root: &std::path::Path,
    id: ProcedureId,
) -> Option<ProcedureStatus> {
    let mem = Memory::open(project_root, "cli-skills-cmd")
        .await
        .expect("reopen memory");
    let procs = mem
        .api()
        .list_procedures(Tier::Project, AccessToken::System)
        .await
        .expect("list");
    procs.into_iter().find(|p| p.id == id).map(|p| p.status)
}

#[tokio::test(flavor = "current_thread")]
#[serial]
async fn skills_promote_is_suspended_before_db_or_filesystem_mutation() {
    let (project, memory_root, id) =
        fixture_with_staged_procedure("auto-grep-glob-grep-glob-grep").await;
    let wayland_home = TempDir::new().unwrap();
    let unrelated = wayland_home
        .path()
        .join("skills")
        .join("auto")
        .join("auto-unrelated");
    fs::create_dir_all(&unrelated).unwrap();
    fs::write(unrelated.join("SKILL.md"), "# generated draft\n").unwrap();

    let bin = env!("CARGO_BIN_EXE_wayland-core");
    let out = std::process::Command::new(bin)
        .args(["--skills-promote", &id.0.to_string()])
        .current_dir(project.path())
        .env("WCORE_MEMORY_DIR", memory_root.path())
        .env("WAYLAND_HOME", wayland_home.path())
        .output()
        .expect("run cli");
    assert!(
        !out.status.success(),
        "promotion must fail closed until F23 supplies governed activation"
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("temporarily unavailable") && stderr.contains("governed"),
        "expected stable containment diagnostic; got: {stderr}"
    );

    let status = read_procedure_status(project.path(), id).await;
    assert_eq!(
        status,
        Some(ProcedureStatus::Staged),
        "rejected promotion must not change procedure state"
    );
    assert!(
        !wayland_home
            .path()
            .join("skills")
            .join("auto-unrelated")
            .join("SKILL.md")
            .exists(),
        "rejected promotion must not bulk-copy unrelated drafts"
    );
}

#[tokio::test(flavor = "current_thread")]
#[serial]
async fn skills_archive_transitions_staged_to_archived() {
    // W9 T0.5 amendment: Staged → Archived is a legal direct
    // transition so curators can dismiss losing drafts without
    // detouring through Active.
    let (project, memory_root, id) = fixture_with_staged_procedure("auto-loser-pattern").await;

    let bin = env!("CARGO_BIN_EXE_wayland-core");
    let out = std::process::Command::new(bin)
        .args(["--skills-archive", &id.0.to_string()])
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
    assert!(
        stdout.contains("archived procedure") && stdout.contains("staged → archived"),
        "expected archive confirmation in stdout; got: {stdout}"
    );

    let status = read_procedure_status(project.path(), id).await;
    assert_eq!(
        status,
        Some(ProcedureStatus::Archived),
        "procedure must be Archived after archive"
    );
}

#[tokio::test(flavor = "current_thread")]
#[serial]
async fn skills_promote_unknown_id_exits_nonzero() {
    let project = TempDir::new().unwrap();
    let memory_root = TempDir::new().unwrap();
    fs::create_dir(project.path().join(".git")).unwrap();

    let bogus = Uuid::new_v4().to_string();
    let bin = env!("CARGO_BIN_EXE_wayland-core");
    let out = std::process::Command::new(bin)
        .args(["--skills-promote", &bogus])
        .current_dir(project.path())
        .env("WCORE_MEMORY_DIR", memory_root.path())
        .output()
        .expect("run cli");
    assert!(
        !out.status.success(),
        "expected nonzero exit for unknown id; stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("temporarily unavailable") && stderr.contains("governed"),
        "suspension must precede database lookup; got stderr: {stderr}"
    );
}

#[tokio::test(flavor = "current_thread")]
#[serial]
async fn skills_promote_rejects_non_uuid() {
    let project = TempDir::new().unwrap();
    let memory_root = TempDir::new().unwrap();
    fs::create_dir(project.path().join(".git")).unwrap();

    let bin = env!("CARGO_BIN_EXE_wayland-core");
    let out = std::process::Command::new(bin)
        .args(["--skills-promote", "not-a-uuid"])
        .current_dir(project.path())
        .env("WCORE_MEMORY_DIR", memory_root.path())
        .output()
        .expect("run cli");
    assert!(
        !out.status.success(),
        "expected nonzero exit for bad UUID; stdout: {}",
        String::from_utf8_lossy(&out.stdout)
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("temporarily unavailable") && stderr.contains("governed"),
        "suspension must precede identifier parsing; got stderr: {stderr}"
    );
}
