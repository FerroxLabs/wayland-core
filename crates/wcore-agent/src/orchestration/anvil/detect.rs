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

/// One detected gate candidate: the argv to run, plus (for TRAMPOLINE gates —
/// commands that dispatch through a repo-controlled script file) the manifest
/// to pin. A builder that edits the pinned trampoline in its worktree fails a
/// Safety-class `gate-integrity` check instead of minting a false `verified`
/// (the manual's condition-gaming failure mode). Direct gates (cargo/go/
/// pytest) execute real project code, not a trampoline — nothing to pin.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GateCandidate {
    /// The gate command argv.
    pub argv: Vec<String>,
    /// Repo-relative trampoline file to content-pin, when the argv is a
    /// dispatcher into it (`package.json`, the matched just/make file).
    pub pin: Option<String>,
}

/// Detect candidate gates for `workspace`, most specific first.
///
/// Order: Cargo → npm → go → pytest → just → make. Manifest specificity, not
/// popularity: a `Cargo.toml` workspace is more definitively "tested by
/// `cargo test`" than a `Makefile` is by `make test`.
pub fn detect_gate_candidates(workspace: &Path) -> Vec<GateCandidate> {
    let mut candidates = Vec::new();
    let arg = |parts: &[&str]| parts.iter().map(|s| s.to_string()).collect::<Vec<_>>();

    if workspace.join("Cargo.toml").is_file() {
        candidates.push(GateCandidate {
            argv: arg(&["cargo", "test"]),
            pin: None,
        });
    }
    if has_npm_test_script(workspace) {
        candidates.push(GateCandidate {
            argv: arg(&["npm", "test"]),
            pin: Some("package.json".to_string()),
        });
    }
    if workspace.join("go.mod").is_file() {
        candidates.push(GateCandidate {
            argv: arg(&["go", "test", "./..."]),
            pin: None,
        });
    }
    if has_pytest_markers(workspace) {
        // Windows installs expose `python`, Unix convention is `python3`.
        let python = if cfg!(windows) { "python" } else { "python3" };
        candidates.push(GateCandidate {
            argv: arg(&[python, "-m", "pytest"]),
            pin: None,
        });
    }
    if let Some(name) = find_recipe_file(workspace, &["justfile", "Justfile", ".justfile"], "test")
    {
        candidates.push(GateCandidate {
            argv: arg(&["just", "test"]),
            pin: Some(name),
        });
    }
    if let Some(name) =
        find_recipe_file(workspace, &["Makefile", "makefile", "GNUmakefile"], "test")
    {
        candidates.push(GateCandidate {
            argv: arg(&["make", "test"]),
            pin: Some(name),
        });
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
        Some(script) => {
            let t = script.trim();
            // Reject the npm scaffold placeholder AND vacuous always-green
            // scripts — a gate that cannot fail would mint meaningless
            // `verified` stamps.
            !t.is_empty()
                && !t.contains("no test specified")
                && !matches!(t, "true" | ":" | "exit 0" | "exit0")
        }
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
        && has_section_line(&pyproject, "[tool.pytest")
    {
        return true;
    }
    for ini in ["tox.ini", "setup.cfg"] {
        if let Ok(body) = std::fs::read_to_string(workspace.join(ini))
            && (has_section_line(&body, "[pytest]") || has_section_line(&body, "[tool:pytest]"))
        {
            return true;
        }
    }
    false
}

/// A section header counts only at line start (comments/embedded strings
/// don't declare pytest).
fn has_section_line(body: &str, prefix: &str) -> bool {
    body.lines()
        .any(|l| l.trim_start().starts_with(prefix) && !l.trim_start().starts_with('#'))
}

/// Returns the FIRST of `names` that exists and defines a `test` recipe —
/// line-anchored `test:` (allowing recipe args before the colon for just).
/// Variable assignments (`test := x`, `test = x`) are NOT recipes.
fn find_recipe_file(workspace: &Path, names: &[&str], recipe: &str) -> Option<String> {
    for name in names {
        let Ok(body) = std::fs::read_to_string(workspace.join(name)) else {
            continue;
        };
        let found = body.lines().any(|line| {
            let line = line.trim_end();
            // `test:` / `test arg1 arg2:` / `test: deps` — but not `retest:`,
            // not indented (recipe bodies), not comments, and not `test :=` /
            // `test =` variable assignments.
            if line.starts_with([' ', '\t', '#']) {
                return false;
            }
            let Some(colon) = line.find(':') else {
                return false;
            };
            if line[colon + 1..].starts_with('=') {
                return false; // `:=` assignment, not a recipe
            }
            let head = &line[..colon];
            let mut toks = head.split_whitespace();
            if toks.next() != Some(recipe) {
                return false;
            }
            // `test = a:b` is a variable assignment, not a recipe — but just
            // recipe args with defaults (`test filter='':`) are fine.
            !matches!(toks.next(), Some(t) if t.starts_with('='))
        });
        if found {
            return Some((*name).to_string());
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn write(dir: &Path, name: &str, body: &str) {
        std::fs::write(dir.join(name), body).unwrap();
    }

    fn argvs(dir: &Path) -> Vec<Vec<String>> {
        detect_gate_candidates(dir)
            .into_iter()
            .map(|c| c.argv)
            .collect()
    }

    #[test]
    fn empty_workspace_detects_nothing() {
        let dir = tempdir().unwrap();
        assert!(detect_gate_candidates(dir.path()).is_empty());
    }

    #[test]
    fn cargo_workspace_detects_cargo_test_first_and_unpinned() {
        let dir = tempdir().unwrap();
        write(dir.path(), "Cargo.toml", "[package]\nname = \"x\"\n");
        write(dir.path(), "Makefile", "test:\n\tcargo test\n");
        let got = detect_gate_candidates(dir.path());
        assert_eq!(got[0].argv, vec!["cargo", "test"]);
        assert_eq!(got[0].pin, None); // direct gate: nothing to pin
        // Makefile still surfaces as a fallback candidate, WITH its pin.
        assert_eq!(got[1].argv, vec!["make", "test"]);
        assert_eq!(got[1].pin.as_deref(), Some("Makefile"));
    }

    #[test]
    fn npm_placeholder_and_vacuous_scripts_are_rejected() {
        let dir = tempdir().unwrap();
        write(
            dir.path(),
            "package.json",
            r#"{"scripts":{"test":"echo \"Error: no test specified\" && exit 1"}}"#,
        );
        assert!(detect_gate_candidates(dir.path()).is_empty());
        // A gate that cannot fail must not become a `verified` mill.
        for vacuous in [
            r#"{"scripts":{"test":"true"}}"#,
            r#"{"scripts":{"test":"exit 0"}}"#,
        ] {
            write(dir.path(), "package.json", vacuous);
            assert!(detect_gate_candidates(dir.path()).is_empty(), "{vacuous}");
        }
    }

    #[test]
    fn npm_real_test_script_is_detected_and_pinned() {
        let dir = tempdir().unwrap();
        write(
            dir.path(),
            "package.json",
            r#"{"scripts":{"test":"vitest run"}}"#,
        );
        let got = detect_gate_candidates(dir.path());
        assert_eq!(got[0].argv, vec!["npm", "test"]);
        assert_eq!(got[0].pin.as_deref(), Some("package.json"));
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
        let got = argvs(dir.path());
        assert_eq!(got[0], vec!["go", "test", "./..."]);
        assert_eq!(got[1][1..], ["-m".to_string(), "pytest".to_string()]);
    }

    #[test]
    fn commented_pytest_table_is_ignored() {
        let dir = tempdir().unwrap();
        write(
            dir.path(),
            "pyproject.toml",
            "# [tool.pytest.ini_options] not really\n[project]\nname = \"x\"\n",
        );
        assert!(detect_gate_candidates(dir.path()).is_empty());
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
        let got = detect_gate_candidates(dir.path());
        assert_eq!(got[0].argv, vec!["just", "test"]);
        assert_eq!(got[0].pin.as_deref(), Some("justfile"));
    }

    #[test]
    fn make_variable_assignments_and_retest_do_not_count() {
        let dir = tempdir().unwrap();
        write(
            dir.path(),
            "Makefile",
            "retest:\n\techo no\n\ntest := lies\ntest = more lies\n\nbuild:\n\ttest -f out || make real\n",
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
