//! A Rust toolchain installed OUTSIDE `$HOME` must still work under the
//! sandbox.
//!
//! The measured defect (round4 CI run 31544636609, job 93954392773,
//! `CI (linux-containerized)`, image `rust:1.95-slim-bookworm`): every
//! sandboxed `cargo` exited 1 with
//!
//! ```text
//! error: rustup could not choose a version of cargo to run, because one
//! wasn't specified explicitly, and no default is configured.
//! ```
//!
//! `HOME` was set and correct (`/root`) — this is NOT the "HOME unset"
//! residual documented on `bwrap::synthetic_etc_scaffold`. The toolchain simply
//! did not live under `$HOME`: the image sets `RUSTUP_HOME=/usr/local/rustup`
//! and `CARGO_HOME=/usr/local/cargo`, and BOTH halves of the product were
//! `$HOME`-anchored —
//!
//! 1. `RUSTUP_HOME` / `CARGO_HOME` were absent from
//!    `env_passthrough::BASE_SANDBOX_ENV_ALLOWLIST`, so they were stripped from
//!    the child and the rustup shim fell back to `$HOME/.rustup`; and
//! 2. `workspace_policy`'s `minimal_toolchain_read_dirs` hardcoded
//!    `$HOME/.rustup` + `$HOME/.cargo/bin`, so the real store was never granted
//!    a read mount either.
//!
//! Reachable straight through `BashTool` on the official `rust:*` images, most
//! devcontainers, Nix, and plenty of corporate CI images.
//!
//! The end-to-end test carries a DISCRIMINATION CONTROL: the same command with
//! only the two variables stripped must FAIL. Without it, a `cargo` that works
//! for some unrelated reason (a toolchain reachable through the blanket `/usr`
//! bind, a `rustup default` configured under the fake home) would make the
//! green arm vacuous, and this file would prove nothing.

use serial_test::serial;
use wcore_tools::env_passthrough::build_sandboxed_env;
use wcore_tools::workspace_policy::WorkspacePolicy;

struct EnvGuard(Vec<(&'static str, Option<String>)>);

impl EnvGuard {
    fn set(vars: &[(&'static str, String)]) -> Self {
        let saved = vars
            .iter()
            .map(|(k, _)| (*k, std::env::var(k).ok()))
            .collect();
        for (k, v) in vars {
            // SAFETY: guarded by `#[serial]`; no other test in this binary
            // mutates env concurrently, and the guard restores prior state on
            // drop.
            unsafe { std::env::set_var(k, v) };
        }
        Self(saved)
    }
}

impl Drop for EnvGuard {
    fn drop(&mut self) {
        for (k, prev) in &self.0 {
            // SAFETY: see `EnvGuard::set`.
            match prev {
                Some(v) => unsafe { std::env::set_var(k, v) },
                None => unsafe { std::env::remove_var(k) },
            }
        }
    }
}

fn text(path: &std::path::Path) -> String {
    path.to_string_lossy().into_owned()
}

/// Half 1 — the two toolchain pointers must survive the sandbox env allowlist.
#[test]
#[serial]
fn the_sandbox_env_allowlist_forwards_the_rust_toolchain_stores() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let rustup = tmp.path().join("store/rustup");
    let cargo = tmp.path().join("store/cargo");
    std::fs::create_dir_all(&rustup).expect("mkdir rustup");
    std::fs::create_dir_all(cargo.join("bin")).expect("mkdir cargo/bin");
    let _guard = EnvGuard::set(&[("RUSTUP_HOME", text(&rustup)), ("CARGO_HOME", text(&cargo))]);

    let env = build_sandboxed_env(&[]);
    let value = |name: &str| {
        env.iter()
            .find(|(k, _)| k == name)
            .map(|(_, v)| v.clone())
            .unwrap_or_else(|| {
                panic!(
                    "{name} was stripped from the sandboxed child env; the rustup \
                     shim then falls back to $HOME/.rustup and every sandboxed \
                     `cargo` exits 1 on a host whose toolchain is not under \
                     $HOME. env = {env:?}"
                )
            })
    };
    assert_eq!(value("RUSTUP_HOME"), text(&rustup));
    assert_eq!(value("CARGO_HOME"), text(&cargo));
}

/// Half 2 — the read grants must follow the same two pointers, not `$HOME`.
#[test]
#[serial]
fn the_toolchain_read_grants_follow_rustup_home_outside_the_home_directory() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let fake_home = tmp.path().join("home");
    let rustup = tmp.path().join("elsewhere/rustup");
    let cargo = tmp.path().join("elsewhere/cargo");
    let workspace = tmp.path().join("ws");
    for dir in [&fake_home, &rustup, &cargo.join("bin"), &workspace] {
        std::fs::create_dir_all(dir).expect("mkdir");
    }
    // The `$HOME` fallback must be unreachable: nothing is installed under it.
    let _guard = EnvGuard::set(&[
        ("HOME", text(&fake_home)),
        ("RUSTUP_HOME", text(&rustup)),
        ("CARGO_HOME", text(&cargo)),
    ]);

    let roots = WorkspacePolicy::contained(&workspace).readable_roots();
    for expected in [rustup.clone(), cargo.join("bin")] {
        assert!(
            roots.contains(&expected),
            "the contained profile did not grant a read mount for {}; the \
             sandboxed rustup shim can then see the toolchain path but not the \
             toolchain. readable_roots = {roots:?}",
            expected.display()
        );
    }
}

/// A relative or filesystem-root pointer must never become a read mount: the
/// first cannot be a mount source at all, the second would bind the whole host
/// read-only, which is the opposite of what this policy exists to do.
#[test]
#[serial]
fn an_unusable_toolchain_pointer_is_refused_rather_than_bound() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let fake_home = tmp.path().join("home");
    let workspace = tmp.path().join("ws");
    for dir in [&fake_home, &workspace] {
        std::fs::create_dir_all(dir).expect("mkdir");
    }
    let root = if cfg!(windows) { "C:\\" } else { "/" };
    let _guard = EnvGuard::set(&[
        ("HOME", text(&fake_home)),
        ("RUSTUP_HOME", root.to_owned()),
        ("CARGO_HOME", "relative/cargo".to_owned()),
    ]);

    let roots = WorkspacePolicy::contained(&workspace).readable_roots();
    assert!(
        !roots.iter().any(|p| p == std::path::Path::new(root)),
        "the filesystem root was bound read-only into the sandbox from \
         RUSTUP_HOME: readable_roots = {roots:?}"
    );
    assert!(
        !roots
            .iter()
            .any(|p| p.ends_with("relative/cargo/bin") || p.ends_with("relative\\cargo\\bin")),
        "a relative CARGO_HOME became a read grant: readable_roots = {roots:?}"
    );
}

/// End to end, under the real bwrap backend: a toolchain outside `$HOME` runs.
#[cfg(target_os = "linux")]
#[serial]
#[tokio::test]
async fn a_toolchain_outside_home_still_runs_cargo_under_the_real_sandbox() {
    use std::collections::HashSet;
    use std::path::PathBuf;
    use wcore_sandbox::backends::SandboxBackend;
    use wcore_sandbox::backends::bwrap::BubblewrapBackend;
    use wcore_sandbox::{SandboxCommand, SandboxManifest};

    let backend = BubblewrapBackend::new();
    if !backend.is_available() {
        eprintln!("skip: bwrap not available on this host");
        return;
    }
    // Resolve the rustup SHIM off PATH — `std::env::var("CARGO")` may point at
    // a toolchain binary directly, which would not consult RUSTUP_HOME at all
    // and would make this probe vacuous.
    let Some(cargo_shim) = std::env::var_os("PATH")
        .into_iter()
        .flat_map(|p| std::env::split_paths(&p).collect::<Vec<_>>())
        .map(|dir| dir.join("cargo"))
        .find(|c| c.is_file())
    else {
        eprintln!("skip: no `cargo` on PATH");
        return;
    };
    // The store as it is TODAY, captured before HOME is redirected.
    let host_home = std::env::var("HOME").expect("HOME is set");
    let rustup_home = std::env::var_os("RUSTUP_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(&host_home).join(".rustup"));
    let cargo_home = std::env::var_os("CARGO_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(&host_home).join(".cargo"));
    if !rustup_home.is_dir() {
        eprintln!("skip: no rustup store at {}", rustup_home.display());
        return;
    }

    let tmp = tempfile::tempdir().expect("tempdir");
    let fake_home = tmp.path().join("home");
    let workspace = tmp.path().join("ws");
    for dir in [&fake_home, &workspace] {
        std::fs::create_dir_all(dir).expect("mkdir");
    }
    // Reproduce the `rust:*` image topology on any host: HOME is real and
    // correct, and the toolchain is somewhere else entirely.
    let _guard = EnvGuard::set(&[
        ("HOME", text(&fake_home)),
        ("RUSTUP_HOME", text(&rustup_home)),
        ("CARGO_HOME", text(&cargo_home)),
    ]);

    let policy = WorkspacePolicy::contained(&workspace);
    // The same assembly `bash::build_sandbox_pieces_for_session` performs: the
    // curated passthrough, then the policy's confined values replacing any
    // same-named entry.
    let mut env = build_sandboxed_env(&[]);
    let overridden: HashSet<&str> = policy.cache_env().iter().map(|(k, _)| k.as_str()).collect();
    env.retain(|(k, _)| !overridden.contains(k.as_str()));
    env.extend(policy.cache_env().iter().cloned());
    let manifest = SandboxManifest {
        fs_read_allow: policy.readable_roots(),
        fs_write_allow: policy.writable_roots(),
        network: policy.network(),
        env,
        ..Default::default()
    };
    let command = || SandboxCommand {
        argv: vec![text(&cargo_shim), "--version".into()],
        cwd: Some(workspace.clone()),
    };

    // DISCRIMINATION CONTROL, run first: with the two pointers stripped — the
    // pre-fix behaviour, and nothing else changed — the child MUST fail. If it
    // succeeds, this host cannot exhibit the defect and the green arm below
    // would be vacuous.
    let mut stripped = manifest.clone();
    stripped
        .env
        .retain(|(k, _)| k != "RUSTUP_HOME" && k != "CARGO_HOME");
    let control = backend
        .execute(&stripped, command())
        .await
        .expect("bwrap execute (control)");
    let control_err = String::from_utf8_lossy(&control.stderr).into_owned();
    assert_ne!(
        control.exit_code,
        0,
        "INSTRUMENT BLIND: `cargo --version` succeeded under the sandbox even \
         with RUSTUP_HOME/CARGO_HOME stripped, so the green arm below measures \
         nothing. stdout={:?} stderr={control_err:?}",
        String::from_utf8_lossy(&control.stdout)
    );
    // ...and it failed for the RIGHT reason. An unreachable binary produces a
    // non-zero exit just as well as a shim that cannot find its toolchain, and
    // would make the control agree with any fix at all.
    assert!(
        !control_err.contains("execvp"),
        "the control failed at exec, not at toolchain resolution, so it does \
         not discriminate the defect under test: {control_err:?}"
    );

    let out = backend
        .execute(&manifest, command())
        .await
        .expect("bwrap execute");
    let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
    assert_eq!(
        out.exit_code,
        0,
        "`cargo --version` must run under the sandbox when the toolchain lives \
         outside $HOME (RUSTUP_HOME={}, CARGO_HOME={}); exit={} stdout={stdout:?} \
         stderr={stderr:?}",
        rustup_home.display(),
        cargo_home.display(),
        out.exit_code
    );
    assert!(
        stdout.chars().any(|c| c.is_ascii_digit()),
        "cargo --version produced no version string: stdout={stdout:?} stderr={stderr:?}"
    );
    eprintln!("cargo: exit 0 — {}", stdout.trim());
}
