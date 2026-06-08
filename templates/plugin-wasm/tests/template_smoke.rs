//! Smoke test for the `plugin-wasm` cargo-generate template.
//!
//! Renders the template into a tempdir via `cargo generate` and runs
//! `cargo check` on the result. We intentionally do NOT run
//! `cargo component build` here — that requires the `wasm32-wasip1`
//! target + `cargo-component` binary, which may not be on CI workers.
//!
//! The test is skipped (with a printed reason) when either `cargo` or
//! `cargo generate` is unavailable, so it stays toolchain-agnostic at
//! the workspace level.

use std::path::PathBuf;
use std::process::Command;

fn template_dir() -> PathBuf {
    // CARGO_MANIFEST_DIR points at the rendered crate when this test
    // runs from a generated project; when run from the workspace it
    // points at the workspace root. Resolve the template path relative
    // to the workspace root.
    let mut p = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    // Walk up to a directory that contains `templates/plugin-wasm`.
    loop {
        if p.join("templates/plugin-wasm/cargo-generate.toml").exists() {
            return p.join("templates/plugin-wasm");
        }
        if !p.pop() {
            panic!("could not locate templates/plugin-wasm from CARGO_MANIFEST_DIR");
        }
    }
}

fn command_available(bin: &str, args: &[&str]) -> bool {
    Command::new(bin)
        .args(args)
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

#[test]
fn template_renders_and_checks() {
    if !command_available("cargo", &["--version"]) {
        eprintln!("skip: cargo unavailable");
        return;
    }
    if !command_available("cargo", &["generate", "--version"]) {
        eprintln!("skip: `cargo generate` unavailable (install cargo-generate to run this smoke)");
        return;
    }

    let tmp = tempfile::tempdir().expect("tempdir");
    let tmp_path = tmp.path();
    let template = template_dir();

    let out = Command::new("cargo")
        .arg("generate")
        .arg("--path")
        .arg(&template)
        .arg("--name")
        .arg("smoke-plugin")
        .arg("--destination")
        .arg(tmp_path)
        .arg("--define")
        .arg("description=smoke")
        .arg("--define")
        .arg("authors=tester <t@example.com>")
        .arg("--silent")
        .output()
        .expect("invoke cargo generate");
    assert!(
        out.status.success(),
        "cargo generate failed: stdout={} stderr={}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr),
    );

    let project = tmp_path.join("smoke-plugin");
    assert!(project.join("Cargo.toml").exists(), "Cargo.toml rendered");
    assert!(project.join("plugin.toml").exists(), "plugin.toml rendered");
    assert!(project.join("src/lib.rs").exists(), "src/lib.rs rendered");
    assert!(project.join("wit/world.wit").exists(), "wit/world.wit rendered");

    // `cargo check` validates the rendered Cargo.toml + wit-bindgen
    // crate-graph without requiring the wasm32 toolchain. wit-bindgen
    // expands its macro against the rendered WIT, so a green check
    // proves the WIT + lib.rs are consistent.
    let check = Command::new("cargo")
        .arg("check")
        .current_dir(&project)
        .output()
        .expect("cargo check");
    assert!(
        check.status.success(),
        "cargo check failed in rendered project: stdout={} stderr={}",
        String::from_utf8_lossy(&check.stdout),
        String::from_utf8_lossy(&check.stderr),
    );
}
