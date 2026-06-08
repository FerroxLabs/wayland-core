//! v0.6.5 Task 4.1 — smoke-test the `templates/plugin-static/` scaffold.
//!
//! Strategy:
//! 1. Probe `cargo generate --help`. If absent (CI without the dev tool),
//!    log a clear skip line and exit Ok.
//! 2. Run `cargo generate --path <workspace>/templates/plugin-static
//!    --name test-plugin --define description="..." --define authors="..."`
//!    into a tempdir.
//! 3. Patch the scaffolded `Cargo.toml` to point `wcore-plugin-api` at the
//!    in-workspace path (so we test the actual current API, not the
//!    crates.io v0.2 line shipped in the template).
//! 4. Run `cargo build` in the tempdir and assert success.
//!
//! The test is dev-only: it does NOT block CI on a missing `cargo-generate`.

use std::path::PathBuf;
use std::process::Command;

fn cargo_generate_available() -> bool {
    Command::new("cargo")
        .args(["generate", "--help"])
        .output()
        .map(|out| out.status.success())
        .unwrap_or(false)
}

fn workspace_root() -> PathBuf {
    // `CARGO_MANIFEST_DIR` points at crates/wcore-plugin-api; go up twice.
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    manifest_dir
        .parent()
        .and_then(|p| p.parent())
        .expect("workspace root two levels above CARGO_MANIFEST_DIR")
        .to_path_buf()
}

#[test]
fn template_static_scaffolds_and_builds() {
    if !cargo_generate_available() {
        eprintln!(
            "SKIP template_smoke: `cargo generate` not installed. \
             Install with `cargo install cargo-generate` to run this test."
        );
        return;
    }

    let root = workspace_root();
    let template_dir = root.join("templates").join("plugin-static");
    assert!(
        template_dir.is_dir(),
        "template dir missing at {}",
        template_dir.display()
    );

    let tmp = tempfile::tempdir().expect("create tempdir");
    let out_dir = tmp.path();

    let gen_status = Command::new("cargo")
        .args(["generate", "--path"])
        .arg(&template_dir)
        .args(["--name", "test-plugin", "--destination"])
        .arg(out_dir)
        .args([
            "--define",
            "description=smoke test plugin",
            "--define",
            "authors=smoke <smoke@example.com>",
            "--silent",
        ])
        .status()
        .expect("invoke cargo generate");
    assert!(
        gen_status.success(),
        "cargo generate failed with status {gen_status:?}"
    );

    // Repoint the wcore-plugin-api dep at the in-tree path so we are
    // testing the CURRENT api, not the git-tag dep shipped in the template.
    let scaffold_dir = out_dir.join("test-plugin");
    let cargo_toml_path = scaffold_dir.join("Cargo.toml");
    let cargo_toml = std::fs::read_to_string(&cargo_toml_path).expect("read scaffolded Cargo.toml");
    let plugin_api_path = root.join("crates").join("wcore-plugin-api");
    // The template now uses a git+tag dep; replace it with an in-tree path dep.
    let git_dep = "wcore-plugin-api = { git = \"https://github.com/FerroxLabs/wayland-core\", tag = \"v0.6.5-wayland-base\" }";
    let patched = cargo_toml.replace(
        git_dep,
        &format!(
            "wcore-plugin-api = {{ path = \"{}\" }}",
            plugin_api_path.display()
        ),
    );
    assert_ne!(
        patched, cargo_toml,
        "expected to find git+tag dep for wcore-plugin-api in scaffold"
    );
    std::fs::write(&cargo_toml_path, patched).expect("rewrite Cargo.toml");

    let build_status = Command::new("cargo")
        .arg("build")
        .current_dir(&scaffold_dir)
        .status()
        .expect("invoke cargo build on scaffold");
    assert!(
        build_status.success(),
        "scaffolded plugin failed to build: {build_status:?}"
    );

    // Also exercise the embedded unit tests — they assert manifest parses
    // + factory builds, the only contract the template promises.
    let test_status = Command::new("cargo")
        .arg("test")
        .current_dir(&scaffold_dir)
        .status()
        .expect("invoke cargo test on scaffold");
    assert!(
        test_status.success(),
        "scaffolded plugin's own tests failed: {test_status:?}"
    );
}
