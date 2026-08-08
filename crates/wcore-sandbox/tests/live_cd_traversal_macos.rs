//! Live proof for the macOS `cd <absolute path>` traversal fix.
//!
//! Seatbelt is PATH-based: `(allow file-read* (subpath "<ws>"))` says nothing
//! about the directories between `/` and `<ws>`. macOS `/bin/sh` is bash 3.2,
//! whose *logical* `cd` stat(2)s every intermediate prefix before calling
//! chdir(2), and renders a prefix it cannot stat as ENOTDIR against the
//! ORIGINAL operand — so `cd /abs/path` fails with a false
//! "Not a directory" even when the target is the process's own cwd.
//! `build_profile` now grants `file-read-metadata` (stat only) on the proper
//! ancestors of every manifest path.
//!
//! This file is the BEHAVIOURAL leg. The profile-TEXT leg lives in
//! `sandbox_exec.rs`'s unit tests and runs on every platform.
//!
//! Gating: run by default. The only skip is `is_available() == false` — these
//! cases need no privileges, no runner opt-in and no special host, so an
//! `#[ignore]` + env opt-in here would be a gate that can never fail. A case
//! counter makes a silently-dropped case a failure rather than a shrinking
//! proof.

#![cfg(target_os = "macos")]

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use wcore_sandbox::backends::SandboxBackend;
use wcore_sandbox::backends::sandbox_exec::SandboxExecBackend;
use wcore_sandbox::{SandboxCommand, SandboxManifest, SandboxOutput};

/// Every case authored below. If one is deleted or short-circuited, the
/// counter assertion at the end of the test fails.
const AUTHORED_CASES: usize = 5;
static CASES_RUN: AtomicUsize = AtomicUsize::new(0);

/// `/bin/sh -c '<script>' -- <arg>`: the operand is a positional (`$1`), never
/// interpolated into the script text, so no shell metacharacter in the path is
/// interpreted. Matches `write_command()` in `live_integrity_macos.rs`.
fn sh(script: &str, arg: &Path) -> SandboxCommand {
    SandboxCommand {
        argv: vec![
            "/bin/sh".into(),
            "-c".into(),
            script.into(),
            "wcore-cd-traversal".into(),
            arg.to_string_lossy().into_owned(),
        ],
        cwd: None,
    }
}

fn out_text(o: &SandboxOutput) -> (String, String) {
    (
        String::from_utf8_lossy(&o.stdout).into_owned(),
        String::from_utf8_lossy(&o.stderr).into_owned(),
    )
}

/// A nonce that cannot be hardcoded, cached, or copied out of a committed
/// fixture: it is minted per run.
fn nonce(tag: &str) -> String {
    let t = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    format!("SBX-{tag}-{}-{t}", std::process::id())
}

struct Fixture {
    _dir: tempfile::TempDir,
    /// `<tmp>/a/b/c/ws` — the ONLY granted path.
    ws: PathBuf,
    /// `<tmp>/a/b` — a proper ancestor of the workspace, holding a canary.
    ancestor: PathBuf,
    pass_nonce: String,
    leak_nonce: String,
}

/// Nesting three levels below the tempdir is mandatory: a workspace directly
/// under an already-granted root does not exercise the bug at all.
fn fixture() -> Fixture {
    let dir = tempfile::tempdir().expect("tempdir");
    // Canonicalize: macOS spells the tempdir through the /var -> /private/var
    // symlink, and the SBPL literals must match the resolved path.
    let root = std::fs::canonicalize(dir.path()).expect("canonicalize tempdir");
    let ws = root.join("a/b/c/ws");
    std::fs::create_dir_all(ws.join("sub")).expect("mkdir ws/sub");
    std::fs::create_dir_all(root.join("a/b/c/sibling")).expect("mkdir sibling");

    let pass_nonce = nonce("CD");
    let leak_nonce = nonce("LEAK");
    std::fs::write(ws.join("sub/proof.txt"), &pass_nonce).expect("write proof");
    std::fs::write(root.join("a/b/secret.txt"), &leak_nonce).expect("write ancestor secret");
    std::fs::write(root.join("a/b/c/sibling/s.txt"), &leak_nonce).expect("write sibling secret");

    Fixture {
        ancestor: root.join("a/b"),
        ws,
        _dir: dir,
        pass_nonce,
        leak_nonce,
    }
}

fn manifest(ws: &Path, read_deny: Vec<PathBuf>) -> SandboxManifest {
    SandboxManifest {
        fs_read_allow: vec![ws.to_path_buf()],
        fs_write_allow: vec![ws.to_path_buf()],
        fs_read_deny: read_deny,
        env: vec![("PATH".into(), "/usr/bin:/bin".into())],
        ..Default::default()
    }
}

#[tokio::test]
async fn live_macos_cd_traversal_works_and_grants_nothing_but_stat() {
    let backend = SandboxExecBackend::new();
    if !backend.is_available() {
        eprintln!("skip: sandbox-exec probe failed on this host");
        return;
    }
    let f = fixture();
    let m = manifest(&f.ws, vec![]);

    // ── 1. POSITIVE: `cd <abs>` succeeds, and prove it by reading a file
    //       reachable ONLY through a RELATIVE path resolved against the new
    //       cwd. `&&` short-circuits, so a failed cd means `cat` never runs;
    //       and the process cwd is <ws>, not <ws>/sub, so `cat proof.txt`
    //       cannot find it any other way. A fix that merely suppressed the
    //       error or returned exit 0 produces empty stdout and fails here.
    let o = backend
        .execute(
            &m,
            sh("cd \"$1\" && pwd && cat proof.txt", &f.ws.join("sub")),
        )
        .await
        .expect("execute");
    let (stdout, stderr) = out_text(&o);
    assert_eq!(o.exit_code, 0, "cd into the workspace failed: {stderr}");
    assert!(
        stdout.contains(&f.pass_nonce),
        "the relative read after cd produced no nonce; stdout={stdout} stderr={stderr}"
    );
    assert!(
        !stderr.contains("Not a directory"),
        "the false ENOTDIR is back: {stderr}"
    );
    CASES_RUN.fetch_add(1, Ordering::Relaxed);

    // ── 2. NEGATIVE: the ancestor is stat-able but NOT readable. This is the
    //       only assertion that pins the metadata-ONLY shape in both
    //       directions.
    let o = backend
        .execute(
            &m,
            sh("ls -ld \"$1\" >/dev/null && ls -1 \"$1\"", &f.ancestor),
        )
        .await
        .expect("execute");
    let (stdout, _) = out_text(&o);
    assert_ne!(
        o.exit_code, 0,
        "listing a granted ancestor must stay denied"
    );
    assert!(
        !stdout.contains("secret.txt"),
        "ancestor directory listing leaked: {stdout}"
    );
    CASES_RUN.fetch_add(1, Ordering::Relaxed);

    // ── 3. NEGATIVE: ancestor file CONTENTS stay denied.
    let o = backend
        .execute(&m, sh("cat \"$1\"", &f.ancestor.join("secret.txt")))
        .await
        .expect("execute");
    let (stdout, stderr) = out_text(&o);
    assert_ne!(o.exit_code, 0, "reading an ancestor file must stay denied");
    assert!(
        !stdout.contains(&f.leak_nonce) && !stderr.contains(&f.leak_nonce),
        "ancestor file contents leaked"
    );
    CASES_RUN.fetch_add(1, Ordering::Relaxed);

    // ── 4. NEGATIVE: a host path that is neither the workspace nor any
    //       ancestor of it must stay UNSTATABLE. This is the case that kills
    //       `(allow file-read-metadata (subpath "/"))` — the rejected
    //       alternative that passes every other case here. A second tempdir is
    //       used rather than a well-known host path because `/usr`, `/System`,
    //       `/Library`, `/bin` and `/sbin` are read-granted by the static
    //       profile head and would make the assertion vacuous.
    let stranger = tempfile::tempdir().expect("second tempdir");
    let stranger = std::fs::canonicalize(stranger.path()).expect("canonicalize");
    let o = backend
        .execute(&m, sh("ls -ld \"$1\"", &stranger))
        .await
        .expect("execute");
    assert_ne!(
        o.exit_code, 0,
        "a host path outside the manifest must not be stat-able"
    );
    CASES_RUN.fetch_add(1, Ordering::Relaxed);

    // ── 5. NEGATIVE: an ancestor that is explicitly read-DENIED must not be
    //       stat-able either. Seatbelt resolves per operation node, so this
    //       only holds because `build_profile` both subtracts denied ancestors
    //       from the metadata grant AND emits a same-operation
    //       `(deny file-read-metadata ...)` backstop.
    let m_denied = manifest(&f.ws, vec![f.ancestor.clone()]);
    let o = backend
        .execute(&m_denied, sh("ls -ld \"$1\"", &f.ancestor))
        .await
        .expect("execute");
    assert_ne!(
        o.exit_code, 0,
        "a denied ancestor must not be stat-able through the traversal grant"
    );
    CASES_RUN.fetch_add(1, Ordering::Relaxed);

    assert_eq!(
        CASES_RUN.load(Ordering::Relaxed),
        AUTHORED_CASES,
        "a case was dropped; the proof shrank"
    );
}
