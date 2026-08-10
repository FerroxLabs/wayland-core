//! SEC-05 / SEC-07 / SEC-10 — the Linux bwrap backend must not hand the child
//! read access to host-private state outside the granted workspace.
//!
//! The measured defect (FOUNDATION-W2-W3 gate, hetzner-dsm, Ubuntu 24.04,
//! bwrap 0.9.0): under the ACTIVE sandbox, `cat /etc/hosts`, `cat /etc/passwd`
//! (including via `../../../..` traversal) and `ls -a /root` all returned real
//! host content, and those bytes reached the provider. macOS/sandbox-exec
//! denies all three — its profile grants `(literal "/etc")` (the symlink node
//! only), never `(subpath "/etc")`. Linux was the permissive outlier because
//! `SYSTEM_RO_DIRS` bound the whole of `/etc` read-only.
//!
//! Every containment assertion in this file is paired with TWO controls.
//!
//! 1. A NEGATIVE CONTROL that runs the identical probe with the sandbox OFF and
//!    asserts the same marker IS observed. The gate recorded an instrument
//!    defect here — a wire predicate looking for macOS-only passwd shapes
//!    (`/usr/bin/false`, `nobody:*:`) that cannot exist on Ubuntu, which
//!    reported a false green — so markers are derived from the host file AT RUN
//!    TIME, never hardcoded, and a control that does not fire fails the test
//!    rather than passing it.
//!
//! 2. A POSITIVE CONTROL that the child actually RAN INSIDE the namespace.
//!    "The marker is not in the output" is satisfied just as well by a child
//!    that never executed, and that is not a hypothetical: restoring `/etc` to
//!    `SYSTEM_RO_DIRS` makes bwrap fail to build the namespace
//!    (`bwrap: Can't create file at /etc/localtime: No such file or directory`),
//!    every child including `/usr/bin/echo` exits 1 with empty stdout — and the
//!    W1 revision of this file reported all four containment tests as PASS in
//!    exactly that state. A security gate that goes green while the sandbox is
//!    completely broken is worse than no gate. So each probe is wrapped as
//!    `echo <nonce>-START; <escape>; echo <nonce>-END=$?`, and BOTH sentinels
//!    must appear in the sandboxed output before the containment assertion is
//!    allowed to mean anything. `-START` proves the shell ran; `-END=` proves
//!    the escape command itself ran to completion rather than the child dying
//!    partway.

#![cfg(target_os = "linux")]

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};
use wcore_sandbox::backends::SandboxBackend;
use wcore_sandbox::backends::bwrap::BubblewrapBackend;
use wcore_sandbox::{SandboxCommand, SandboxManifest};

fn backend() -> Option<BubblewrapBackend> {
    let b = BubblewrapBackend::new();
    b.is_available().then_some(b)
}

fn bin(name: &str) -> Option<PathBuf> {
    which::which(name).ok()
}

/// A marker that provably exists on THIS host: the last non-empty line of
/// `path`. Returns `None` when the file cannot be read, so the probe skips
/// rather than asserting against nothing.
fn host_file_marker(path: &str) -> Option<String> {
    let text = std::fs::read_to_string(path).ok()?;
    let line = text
        .lines()
        .map(str::trim)
        .rfind(|l| !l.is_empty() && !l.starts_with('#'))?
        .to_owned();
    (line.len() >= 8).then_some(line)
}

fn workspace_manifest(root: &Path) -> SandboxManifest {
    SandboxManifest {
        fs_read_allow: vec![root.to_path_buf()],
        fs_write_allow: vec![root.to_path_buf()],
        env: vec![("PATH".into(), "/usr/bin:/bin".into())],
        ..Default::default()
    }
}

/// Run `argv` under the real bwrap backend with `root` as the only granted
/// workspace, returning the child's combined stdout+stderr.
async fn sandboxed(root: &Path, argv: Vec<String>) -> String {
    let backend = backend().expect("bwrap available (checked by caller)");
    let out = backend
        .execute(
            &workspace_manifest(root),
            SandboxCommand {
                argv,
                cwd: Some(root.to_path_buf()),
            },
        )
        .await
        .expect("bwrap execute");
    format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    )
}

/// The same probe with the sandbox OFF — the negative control that proves the
/// marker is observable at all, i.e. that the containment assertion is
/// measuring the sandbox and not a missing file or a broken command.
fn unsandboxed(root: &Path, argv: &[String]) -> String {
    let out = Command::new(&argv[0])
        .args(&argv[1..])
        .current_dir(root)
        .output()
        .expect("spawn unsandboxed control");
    format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    )
}

/// A nonce unique to this probe and this run, echoed by the child so its
/// execution is observable. Derived from the clock and the pid so two probes in
/// the same binary — and two runs against the same host — cannot collide.
fn nonce(probe: &str) -> String {
    static SEQ: AtomicU64 = AtomicU64::new(0);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock after epoch")
        .as_nanos();
    let seq = SEQ.fetch_add(1, Ordering::Relaxed);
    let tag: String = probe
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .collect();
    format!("WLPROOF{tag}{}{nanos:x}{seq:x}", std::process::id())
}

/// Wrap an escape command so the child proves it ran. `-START` is emitted
/// before the escape, `-END=<status>` after it; see the module doc for why both
/// are required.
fn probe_argv(sh: &Path, nonce: &str, body: &str) -> Vec<String> {
    vec![
        sh.to_string_lossy().into_owned(),
        "-c".into(),
        format!("echo {nonce}-START; {body}; echo {nonce}-END=$?"),
    ]
}

/// Fail unless `observed` carries both proof-of-execution sentinels. Without
/// this, a containment assertion passes when the child never ran at all.
fn assert_child_ran(probe: &str, nonce: &str, where_: &str, observed: &str) {
    let start = format!("{nonce}-START");
    let end = format!("{nonce}-END=");
    assert!(
        observed.contains(&start) && observed.contains(&end),
        "{probe}: POSITIVE CONTROL DID NOT FIRE ({where_}) — the child did not \
         run to completion, so any containment assertion against its output is \
         VACUOUS. This is what a broken sandbox namespace looks like; do not \
         read a green containment result out of it.\n  \
         expected sentinels: {start:?} and {end:?}\n  \
         saw -START: {}\n  saw -END: {}\n  child output: {observed:?}",
        observed.contains(&start),
        observed.contains(&end)
    );
}

async fn assert_contained(probe: &str, host_path: &str, body: &str) {
    if backend().is_none() {
        eprintln!("skip {probe}: bwrap not available");
        return;
    }
    let Some(sh) = bin("sh") else {
        eprintln!("skip {probe}: no sh");
        return;
    };
    let Some(marker) = host_file_marker(host_path) else {
        eprintln!("skip {probe}: {host_path} is not readable on this host");
        return;
    };
    let tmp = tempfile::tempdir().expect("tempdir");
    let root = std::fs::canonicalize(tmp.path()).expect("canonicalize");
    let nonce = nonce(probe);
    let argv = probe_argv(&sh, &nonce, body);

    // Negative control first: with no sandbox the marker MUST be observed.
    let control = unsandboxed(&root, &argv);
    assert_child_ran(probe, &nonce, "sandbox OFF", &control);
    assert!(
        control.contains(&marker),
        "{probe}: NEGATIVE CONTROL DID NOT FIRE — the sandbox-off probe did not \
         reproduce the host marker, so the containment assertion below would be \
         vacuous.\n  marker: {marker:?}\n  control output: {control:?}"
    );

    let observed = sandboxed(&root, argv).await;
    // Positive control: the child ran INSIDE the sandbox. Must precede the
    // containment assertion — an empty output satisfies the containment
    // assertion trivially and proves nothing.
    assert_child_ran(probe, &nonce, "sandbox ON", &observed);
    assert!(
        !observed.contains(&marker),
        "{probe}: HOST READ ESCAPE — the sandboxed child read host-private \
         content from {host_path}.\n  marker: {marker:?}\n  child output: {observed:?}"
    );
}

/// The positive control, on its own, as a named test. Every other test in this
/// file depends on the bwrap namespace actually building; when it does not,
/// this fails FIRST and says so, instead of leaving four silent green
/// containment results to be misread as a security proof.
#[tokio::test]
async fn bwrap_namespace_builds_and_runs_a_trivial_child() {
    if backend().is_none() {
        eprintln!("skip: bwrap not available");
        return;
    }
    let Some(sh) = bin("sh") else {
        eprintln!("skip: no sh");
        return;
    };
    let tmp = tempfile::tempdir().expect("tempdir");
    let root = std::fs::canonicalize(tmp.path()).expect("canonicalize");
    let nonce = nonce("namespace health");
    let argv = probe_argv(&sh, &nonce, "true");

    let backend = backend().expect("checked");
    let out = backend
        .execute(
            &workspace_manifest(&root),
            SandboxCommand {
                argv,
                cwd: Some(root.clone()),
            },
        )
        .await
        .expect("bwrap execute");
    let observed = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert_child_ran("namespace health", &nonce, "sandbox ON", &observed);
    assert_eq!(
        out.exit_code, 0,
        "the bwrap namespace must build and run a trivial child; \
         exit={} output={observed:?}",
        out.exit_code
    );
}

#[tokio::test]
async fn bwrap_denies_host_etc_passwd() {
    assert_contained("SEC-07 /etc/passwd", "/etc/passwd", "cat /etc/passwd").await;
}

#[tokio::test]
async fn bwrap_denies_host_etc_hosts() {
    assert_contained("SEC-05 /etc/hosts", "/etc/hosts", "cat /etc/hosts").await;
}

/// The traversal spelling the gate actually measured — the workspace-relative
/// `../../../../../../../../etc/passwd` that walks off the granted root.
#[tokio::test]
async fn bwrap_denies_host_etc_passwd_via_parent_traversal() {
    assert_contained(
        "SEC-07 traversal",
        "/etc/passwd",
        "cat ../../../../../../../../etc/passwd",
    )
    .await;
}

/// SEC-10 — `ls -a /root` returned real host content under the active sandbox.
/// Skips when the test user cannot read `/root` at all (no marker to assert).
#[tokio::test]
async fn bwrap_denies_host_root_home_listing() {
    if backend().is_none() {
        eprintln!("skip: bwrap not available");
        return;
    }
    let Some(sh) = bin("sh") else {
        eprintln!("skip: no sh");
        return;
    };
    let Ok(entries) = std::fs::read_dir("/root") else {
        eprintln!("skip: /root is not readable on this host");
        return;
    };
    let Some(marker) = entries
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| n.len() >= 4)
        .max()
    else {
        eprintln!("skip: /root has no entry distinctive enough to assert on");
        return;
    };

    let tmp = tempfile::tempdir().expect("tempdir");
    let root = std::fs::canonicalize(tmp.path()).expect("canonicalize");
    let nonce = nonce("SEC-10 /root");
    let argv = probe_argv(&sh, &nonce, "ls -a /root");

    let control = unsandboxed(&root, &argv);
    assert_child_ran("SEC-10 /root", &nonce, "sandbox OFF", &control);
    assert!(
        control.contains(&marker),
        "SEC-10 /root: NEGATIVE CONTROL DID NOT FIRE — sandbox-off `ls -a /root` \
         did not list {marker:?}.\n  control output: {control:?}"
    );

    let observed = sandboxed(&root, argv).await;
    assert_child_ran("SEC-10 /root", &nonce, "sandbox ON", &observed);
    assert!(
        !observed.contains(&marker),
        "SEC-10 /root: HOST READ ESCAPE — the sandboxed child listed the host's \
         /root.\n  marker: {marker:?}\n  child output: {observed:?}"
    );
}

/// The acceptance bar's other half: narrowing the read posture must NOT break
/// the toolchains, which are the one thing Linux gets right today. Every
/// toolchain present on the host must still run under the sandbox when its own
/// installation directory is granted — exactly what `WorkspacePolicy`'s
/// readable roots do in production.
#[tokio::test]
async fn bwrap_still_runs_the_host_toolchains() {
    if backend().is_none() {
        eprintln!("skip: bwrap not available");
        return;
    }
    let backend = backend().expect("checked");
    let tmp = tempfile::tempdir().expect("tempdir");
    let root = std::fs::canonicalize(tmp.path()).expect("canonicalize");

    let mut exercised = 0usize;
    for tool in ["git", "node", "python3", "cargo"] {
        let Some(path) = bin(tool) else {
            eprintln!("skip {tool}: not installed on this host");
            continue;
        };
        let mut manifest = workspace_manifest(&root);
        // Grant the toolchain's own install tree — the production
        // `readable_roots()` equivalent for a toolchain outside /usr.
        if let Some(dir) = path.parent()
            && !dir.starts_with("/usr")
            && !dir.starts_with("/bin")
        {
            manifest.fs_read_allow.push(dir.to_path_buf());
            if let Some(home) = dir.parent() {
                manifest.fs_read_allow.push(home.to_path_buf());
            }
        }
        if let Ok(home) = std::env::var("HOME") {
            // HOME itself stays the workspace — the host home is NOT granted,
            // so `~/.gitconfig` and friends remain unreadable. The rustup shim
            // is pointed at its real stores explicitly, mirroring the
            // toolchain-path grants production makes.
            manifest
                .env
                .push(("HOME".into(), root.to_string_lossy().into_owned()));
            for (var, sub) in [("CARGO_HOME", ".cargo"), ("RUSTUP_HOME", ".rustup")] {
                let p = Path::new(&home).join(sub);
                if p.exists() {
                    manifest.env.push((
                        var.into(),
                        std::env::var(var).unwrap_or_else(|_| p.to_string_lossy().into_owned()),
                    ));
                    manifest.fs_read_allow.push(p);
                }
            }
        }
        let out = backend
            .execute(
                &manifest,
                SandboxCommand {
                    argv: vec![path.to_string_lossy().into_owned(), "--version".into()],
                    cwd: Some(root.clone()),
                },
            )
            .await
            .unwrap_or_else(|e| panic!("{tool}: bwrap execute failed: {e}"));
        let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
        let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
        assert_eq!(
            out.exit_code, 0,
            "{tool} --version must still run under the sandbox; exit={} stdout={stdout:?} stderr={stderr:?}",
            out.exit_code
        );
        assert!(
            stdout.chars().any(|c| c.is_ascii_digit()),
            "{tool} --version produced no version string: stdout={stdout:?} stderr={stderr:?}"
        );
        eprintln!("{tool}: exit 0 — {}", stdout.trim());
        exercised += 1;
    }
    assert!(
        exercised > 0,
        "no toolchain was exercised — this regression gate proved nothing"
    );
}

/// `fs_metadata_read_allow` must not become a readable bind on bwrap.
///
/// The field exists for seatbelt, where an ungranted path is EPERM and libgit2
/// treats that as fatal. bwrap has no metadata-only bind, so the only way to
/// "honour" the grant here would be `--ro-bind`, which hands over the file's
/// CONTENTS — a widening the caller never asked for. The backend drops the
/// entry instead, and it can afford to: bwrap builds the namespace
/// constructively, so the unbound path answers ENOENT, which is exactly the
/// answer libgit2 tolerates. Measured on Ubuntu 24.04 / kernel 6.8 with `/root`
/// unbound: `stat("/root/.gitconfig")` → errno 2.
///
/// Both controls, per this file's discipline: the child must prove it ran, and
/// the same read with the sandbox OFF must produce the secret.
#[tokio::test]
async fn bwrap_never_binds_a_metadata_only_path() {
    const PROBE: &str = "metadata_only_path";
    let Some(backend) = backend() else {
        eprintln!("skip {PROBE}: bwrap not available");
        return;
    };
    let Some(sh) = bin("sh") else {
        eprintln!("skip {PROBE}: no sh");
        return;
    };
    let tmp = tempfile::tempdir().expect("tempdir");
    let root = std::fs::canonicalize(tmp.path()).expect("canonicalize");
    // Outside the workspace, so the ONLY thing that could expose it is the
    // metadata grant being translated into a bind.
    let outside = tempfile::tempdir().expect("tempdir");
    let secret = std::fs::canonicalize(outside.path())
        .expect("canonicalize")
        .join(".gitconfig");
    let marker = "WLMETADATAONLYSECRETMARKER";
    std::fs::write(&secret, format!("[user]\n\tname = {marker}\n")).expect("write secret");

    let nonce = nonce(PROBE);
    let body = format!("cat {} ; stat -c %s {}", secret.display(), secret.display());
    let argv = probe_argv(&sh, &nonce, &body);

    // NEGATIVE CONTROL — with no sandbox the marker IS observable, so the
    // assertion below measures the sandbox and not a missing file.
    let control = unsandboxed(&root, &argv);
    assert_child_ran(PROBE, &nonce, "sandbox OFF", &control);
    assert!(
        control.contains(marker),
        "{PROBE}: NEGATIVE CONTROL DID NOT FIRE — {control:?}"
    );

    let mut manifest = workspace_manifest(&root);
    manifest.fs_metadata_read_allow = vec![secret.clone()];
    let out = backend
        .execute(
            &manifest,
            SandboxCommand {
                argv,
                cwd: Some(root.clone()),
            },
        )
        .await
        .expect("bwrap execute");
    let observed = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert_child_ran(PROBE, &nonce, "sandbox ON", &observed);
    assert!(
        !observed.contains(marker),
        "{PROBE}: a metadata grant was widened into a content read — {observed:?}"
    );
}
