//! Gate auto-detection — the zero-config half of "the gate is the anvil".
//!
//! When `[anvil] gate` is empty, the forge probes the workspace for its native
//! test suite and proposes candidate gate argvs in priority order. Detection is
//! manifest-driven and deliberately dumb: it reads marker files, never runs
//! anything. ADOPTION is decided by the existing pre-climb sandbox probe
//! (spec §5) — the first candidate that actually EXECUTES on the baseline wins,
//! so a detected-but-uninstalled toolchain (e.g. `package.json` present, `npm`
//! missing) falls through to the next candidate instead of wedging the climb.
//!
//! An explicitly configured gate always wins and skips detection entirely.

use std::path::Path;

/// Detect candidate gate argvs for `workspace`, most specific first.
///
/// Order: Cargo → npm → go → pytest → just → make. Manifest specificity, not
/// popularity: a `Cargo.toml` workspace is more definitively "tested by
/// `cargo test`" than a `Makefile` is by `make test`.
pub fn detect_gate_candidates(workspace: &Path) -> Vec<Vec<String>> {
    let mut candidates = Vec::new();
    let arg = |parts: &[&str]| parts.iter().map(|s| s.to_string()).collect::<Vec<_>>();

    if workspace.join("Cargo.toml").is_file() {
        candidates.push(arg(&["cargo", "test"]));
    }
    if has_npm_test_script(workspace) {
        candidates.push(arg(&["npm", "test"]));
    }
    if workspace.join("go.mod").is_file() {
        candidates.push(arg(&["go", "test", "./..."]));
    }
    if has_pytest_markers(workspace) {
        // Windows installs expose `python`, Unix convention is `python3`.
        let python = if cfg!(windows) { "python" } else { "python3" };
        candidates.push(arg(&[python, "-m", "pytest"]));
    }
    if has_recipe(workspace, &["justfile", "Justfile", ".justfile"], "test") {
        candidates.push(arg(&["just", "test"]));
    }
    if has_recipe(workspace, &["Makefile", "makefile", "GNUmakefile"], "test") {
        candidates.push(arg(&["make", "test"]));
    }
    candidates
}

/// `package.json` counts only when it has a REAL `scripts.test` entry — npm's
/// scaffold placeholder (`echo "Error: no test specified" && exit 1`) would
/// make every baseline red with an unfixable gate.
fn has_npm_test_script(workspace: &Path) -> bool {
    let Ok(raw) = std::fs::read_to_string(workspace.join("package.json")) else {
        return false;
    };
    let Ok(json) = serde_json::from_str::<serde_json::Value>(&raw) else {
        return false;
    };
    match json.pointer("/scripts/test").and_then(|v| v.as_str()) {
        Some(script) => !script.trim().is_empty() && !script.contains("no test specified"),
        None => false,
    }
}

/// pytest is declared via `pytest.ini`, a `[tool.pytest.ini_options]` table in
/// `pyproject.toml`, or a `[pytest]` section in `tox.ini`/`setup.cfg`.
fn has_pytest_markers(workspace: &Path) -> bool {
    if workspace.join("pytest.ini").is_file() {
        return true;
    }
    if let Ok(pyproject) = std::fs::read_to_string(workspace.join("pyproject.toml"))
        && pyproject.contains("[tool.pytest")
    {
        return true;
    }
    for ini in ["tox.ini", "setup.cfg"] {
        if let Ok(body) = std::fs::read_to_string(workspace.join(ini))
            && (body.contains("[pytest]") || body.contains("[tool:pytest]"))
        {
            return true;
        }
    }
    false
}

/// A just/make file counts only when it actually defines a `test` recipe —
/// line-anchored `test:` (allowing recipe args before the colon for just).
fn has_recipe(workspace: &Path, names: &[&str], recipe: &str) -> bool {
    for name in names {
        let Ok(body) = std::fs::read_to_string(workspace.join(name)) else {
            continue;
        };
        let found = body.lines().any(|line| {
            let line = line.trim_end();
            // `test:` / `test arg1 arg2:` / `test: deps` — but not `retest:`
            // and not indented (recipe bodies) or comment lines.
            !line.starts_with([' ', '\t', '#'])
                && line
                    .split(':')
                    .next()
                    .is_some_and(|head| head.split_whitespace().next() == Some(recipe))
        });
        if found {
            return true;
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn write(dir: &Path, name: &str, body: &str) {
        std::fs::write(dir.join(name), body).unwrap();
    }

    #[test]
    fn empty_workspace_detects_nothing() {
        let dir = tempdir().unwrap();
        assert!(detect_gate_candidates(dir.path()).is_empty());
    }

    #[test]
    fn cargo_workspace_detects_cargo_test_first() {
        let dir = tempdir().unwrap();
        write(dir.path(), "Cargo.toml", "[package]\nname = \"x\"\n");
        write(dir.path(), "Makefile", "test:\n\tcargo test\n");
        let got = detect_gate_candidates(dir.path());
        assert_eq!(got[0], vec!["cargo", "test"]);
        // Makefile still surfaces as a fallback candidate.
        assert_eq!(got[1], vec!["make", "test"]);
    }

    #[test]
    fn npm_placeholder_test_script_is_rejected() {
        let dir = tempdir().unwrap();
        write(
            dir.path(),
            "package.json",
            r#"{"scripts":{"test":"echo \"Error: no test specified\" && exit 1"}}"#,
        );
        assert!(detect_gate_candidates(dir.path()).is_empty());
    }

    #[test]
    fn npm_real_test_script_is_detected() {
        let dir = tempdir().unwrap();
        write(
            dir.path(),
            "package.json",
            r#"{"scripts":{"test":"vitest run"}}"#,
        );
        assert_eq!(
            detect_gate_candidates(dir.path()),
            vec![vec!["npm".to_string(), "test".to_string()]]
        );
    }

    #[test]
    fn go_and_pytest_markers_detect() {
        let dir = tempdir().unwrap();
        write(dir.path(), "go.mod", "module example.com/x\n");
        write(
            dir.path(),
            "pyproject.toml",
            "[tool.pytest.ini_options]\ntestpaths = [\"tests\"]\n",
        );
        let got = detect_gate_candidates(dir.path());
        assert_eq!(got[0], vec!["go", "test", "./..."]);
        assert_eq!(got[1][1..], ["-m".to_string(), "pytest".to_string()]);
    }

    #[test]
    fn justfile_requires_a_test_recipe() {
        let dir = tempdir().unwrap();
        write(dir.path(), "justfile", "build:\n\tcargo build\n");
        assert!(detect_gate_candidates(dir.path()).is_empty());
        write(
            dir.path(),
            "justfile",
            "build:\n\tcargo build\n\ntest filter='':\n\tcargo test {{filter}}\n",
        );
        assert_eq!(
            detect_gate_candidates(dir.path()),
            vec![vec!["just".to_string(), "test".to_string()]]
        );
    }

    #[test]
    fn makefile_retest_and_indented_lines_do_not_count() {
        let dir = tempdir().unwrap();
        write(
            dir.path(),
            "Makefile",
            "retest:\n\techo no\n\nbuild:\n\ttest -f out || make real\n",
        );
        assert!(detect_gate_candidates(dir.path()).is_empty());
    }

    #[test]
    fn pyproject_without_pytest_table_is_ignored() {
        let dir = tempdir().unwrap();
        write(dir.path(), "pyproject.toml", "[project]\nname = \"x\"\n");
        assert!(detect_gate_candidates(dir.path()).is_empty());
    }
}
