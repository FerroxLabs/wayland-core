//! CLI integration: `wayland-core --skills-audit` against a fixture project.

use std::fs;

use tempfile::TempDir;

fn fixture_project() -> TempDir {
    let tmp = TempDir::new().unwrap();
    fs::create_dir(tmp.path().join(".git")).unwrap();
    let dir = tmp.path().join(".wayland-core").join("skills").join("ok");
    fs::create_dir_all(&dir).unwrap();
    fs::write(
        dir.join("SKILL.md"),
        "---\nname: ok\ndescription: ok\n---\n\nbody\n",
    )
    .unwrap();
    tmp
}

#[test]
fn skills_audit_writes_json_and_renders_markdown() {
    let tmp = fixture_project();
    let bin = env!("CARGO_BIN_EXE_wayland-core");
    let out = std::process::Command::new(bin)
        .args(["--skills-audit"])
        .current_dir(tmp.path())
        .output()
        .expect("run");
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("# Skills Audit"),
        "stdout missing Markdown header: {stdout}"
    );

    let json_path = tmp.path().join(".wayland-core").join("skills-audit.json");
    assert!(json_path.exists(), "expected JSON report at {json_path:?}");
    let json = std::fs::read_to_string(&json_path).unwrap();
    let value: serde_json::Value = serde_json::from_str(&json).unwrap();
    assert!(
        value["total_skills"].as_u64().unwrap_or(0) >= 1,
        "expected total_skills >= 1, got {json}"
    );
}
