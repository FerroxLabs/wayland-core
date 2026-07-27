// F25-04: `plugin new` and `plugin test` — the author's two ends of the loop.
//
// `plugin new` drives the templates that ALREADY EXIST under `templates/`, the
// same ones `wcore-plugin-api/tests/template_smoke.rs` generates from and
// builds. It does not carry a second copy of a plugin skeleton: two templates
// drift, and the one the product uses would be the one nobody smoke-tests.
//
// When `cargo generate` is absent the verb says so with the exact install
// command and exits non-zero. It does NOT half-succeed. A scaffold verb that
// prints a success line and leaves an empty directory is worse than one that
// refuses, because the author only finds out at build time.

use std::path::{Path, PathBuf};
use std::process::Command;

use crate::plugin::error::{PluginCliError, Result};

/// Which shipped template to generate from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Template {
    /// `templates/plugin-static/` — compiled into the engine via `inventory`.
    Static,
    /// `templates/plugin-wasm/` — an on-disk WASM component plugin.
    Wasm,
}

impl Template {
    pub fn parse(s: &str) -> Result<Self> {
        match s {
            "static" => Ok(Self::Static),
            "wasm" => Ok(Self::Wasm),
            other => Err(PluginCliError::Quarantine(format!(
                "unknown template '{other}' (expected 'static' or 'wasm')"
            ))),
        }
    }

    fn dir_name(self) -> &'static str {
        match self {
            Self::Static => "plugin-static",
            Self::Wasm => "plugin-wasm",
        }
    }
}

fn cargo_generate_available() -> bool {
    Command::new("cargo")
        .args(["generate", "--help"])
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// Locate `templates/<name>/`.
///
/// An installed binary has no workspace beside it, so `$WAYLAND_TEMPLATES_DIR`
/// is honoured first; then the compile-time workspace root, which covers every
/// developer and CI invocation; then the current directory, which covers a
/// checkout the binary was copied into.
pub fn template_dir(t: Template) -> Result<PathBuf> {
    let mut tried: Vec<PathBuf> = Vec::new();
    if let Ok(v) = std::env::var("WAYLAND_TEMPLATES_DIR")
        && !v.is_empty()
    {
        let p = PathBuf::from(v).join(t.dir_name());
        if p.is_dir() {
            return Ok(p);
        }
        tried.push(p);
    }
    // `CARGO_MANIFEST_DIR` is `<workspace>/crates/wcore-cli` at compile time.
    let compiled = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .map(|r| r.join("templates").join(t.dir_name()));
    if let Some(p) = compiled {
        if p.is_dir() {
            return Ok(p);
        }
        tried.push(p);
    }
    if let Ok(cwd) = std::env::current_dir() {
        let p = cwd.join("templates").join(t.dir_name());
        if p.is_dir() {
            return Ok(p);
        }
        tried.push(p);
    }
    Err(PluginCliError::Quarantine(format!(
        "could not find the '{}' template. Looked in: {}. Point WAYLAND_TEMPLATES_DIR \
         at the repo's templates/ directory.",
        t.dir_name(),
        tried
            .iter()
            .map(|p| p.display().to_string())
            .collect::<Vec<_>>()
            .join(", ")
    )))
}

/// `plugin new <name> --path <dir> [--template static|wasm]`.
pub fn run_new(name: &str, dest: &Path, template: Template) -> Result<()> {
    crate::plugin::resolver::validate_plugin_name(name)?;
    let tdir = template_dir(template)?;

    if !cargo_generate_available() {
        return Err(PluginCliError::Quarantine(format!(
            "`cargo generate` is not installed, so the '{}' template cannot be \
             expanded. Install it with:\n  cargo install cargo-generate\n\
             (the template itself is present at {})",
            tdir.file_name()
                .and_then(|s| s.to_str())
                .unwrap_or("plugin"),
            tdir.display()
        )));
    }

    std::fs::create_dir_all(dest)?;
    let out = dest.join(name);
    // `authors` is deliberately NOT passed. cargo-generate reserves
    // `project-name`, `crate_name`, `crate_type`, `authors` and `os-arch` as
    // built-ins and REFUSES the whole run if you try to `--define` one of them:
    //
    //   Error: placeholder `authors` is not valid as you can't override ...
    //
    // Both shipped templates declare an `authors` placeholder, so overriding it
    // looked reasonable and fails outright on cargo-generate 0.23. It reaches
    // the template as a built-in regardless.
    let status = Command::new("cargo")
        .args(["generate", "--path"])
        .arg(&tdir)
        .args(["--name", name, "--destination"])
        .arg(dest)
        .args(["--define", "description=A Wayland plugin", "--silent"])
        .status()
        .map_err(|e| PluginCliError::Quarantine(format!("invoking cargo generate: {e}")))?;
    if !status.success() {
        // A failed generate can leave a partial tree behind, and a half-written
        // scaffold is worse than none: the author only finds out at build time.
        if out.exists() {
            std::fs::remove_dir_all(&out).ok();
        }
        return Err(PluginCliError::Quarantine(format!(
            "cargo generate failed ({status}); no scaffold was left behind"
        )));
    }
    // The template ships a git+tag dependency so a plugin generated OUTSIDE
    // this repo builds against a published API. Generated INSIDE the workspace
    // that would silently test a released tag instead of the tree in front of
    // you — the exact substitution `template_smoke.rs` makes for the same
    // reason. Do it here too, and say so, rather than leaving the author with a
    // scaffold that compiles against code they are not editing.
    match repoint_to_in_tree_api(&out) {
        Ok(true) => println!("repointed wcore-plugin-api at the in-tree crate"),
        Ok(false) => {}
        Err(e) => println!("note: could not repoint wcore-plugin-api ({e})"),
    }

    println!("scaffolded {name} → {}", out.display());
    println!("next:");
    println!("  wayland-core plugin test {}", out.display());
    println!("  wayland-core plugin verify {}", out.display());
    Ok(())
}

/// Rewrite the scaffold's git+tag `wcore-plugin-api` dep to the in-tree path.
/// Returns whether a rewrite happened.
fn repoint_to_in_tree_api(scaffold: &Path) -> Result<bool> {
    let api = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .map(|c| c.join("wcore-plugin-api"));
    let Some(api) = api.filter(|p| p.is_dir()) else {
        return Ok(false);
    };
    let cargo_toml = scaffold.join("Cargo.toml");
    if !cargo_toml.is_file() {
        return Ok(false);
    }
    let body = std::fs::read_to_string(&cargo_toml)?;
    let mut out = String::with_capacity(body.len());
    let mut changed = false;
    for line in body.lines() {
        if line.trim_start().starts_with("wcore-plugin-api")
            && line.contains("git =")
            && let Some((lhs, _)) = line.split_once('=')
        {
            out.push_str(&format!(
                "{}= {{ path = \"{}\" }}\n",
                lhs,
                api.display().to_string().replace('\\', "/")
            ));
            changed = true;
            continue;
        }
        out.push_str(line);
        out.push('\n');
    }
    if changed {
        std::fs::write(&cargo_toml, out)?;
    }
    Ok(changed)
}

/// `plugin test <dir>` — run the plugin's own suite and return its verdict
/// faithfully. A failing plugin test MUST produce a non-zero exit here, or the
/// verb is a rubber stamp.
pub fn run_test(dir: &Path) -> Result<()> {
    if !dir.join("Cargo.toml").is_file() {
        return Err(PluginCliError::Quarantine(format!(
            "{} has no Cargo.toml — `plugin test` runs the plugin's own cargo suite",
            dir.display()
        )));
    }
    let status = Command::new("cargo")
        .arg("test")
        .current_dir(dir)
        .status()
        .map_err(|e| PluginCliError::Quarantine(format!("invoking cargo test: {e}")))?;
    if !status.success() {
        return Err(PluginCliError::Quarantine(format!(
            "plugin tests FAILED in {} ({status})",
            dir.display()
        )));
    }
    println!("plugin tests passed in {}", dir.display());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn both_shipped_templates_resolve() {
        assert!(template_dir(Template::Static).unwrap().is_dir());
        assert!(template_dir(Template::Wasm).unwrap().is_dir());
    }

    #[test]
    fn unknown_template_is_rejected() {
        assert!(Template::parse("perl").is_err());
        assert_eq!(Template::parse("wasm").unwrap(), Template::Wasm);
    }

    #[test]
    fn an_invalid_plugin_name_never_reaches_cargo_generate() {
        let tmp = TempDir::new().unwrap();
        let err = run_new("../escape", tmp.path(), Template::Static).unwrap_err();
        assert!(matches!(err, PluginCliError::InvalidName(_)), "{err:?}");
    }

    #[test]
    fn plugin_test_refuses_a_directory_with_no_cargo_manifest() {
        let tmp = TempDir::new().unwrap();
        let err = run_test(tmp.path()).unwrap_err();
        assert!(err.to_string().contains("no Cargo.toml"), "{err:?}");
    }

    /// A failing suite must surface as a non-zero verb, not a printed note.
    #[test]
    fn plugin_test_propagates_a_failing_suite() {
        let tmp = TempDir::new().unwrap();
        let dir = tmp.path();
        std::fs::create_dir_all(dir.join("src")).unwrap();
        std::fs::write(
            dir.join("Cargo.toml"),
            "[package]\nname = \"failing-fixture\"\nversion = \"0.0.0\"\nedition = \"2021\"\n\
             [workspace]\n[dependencies]\n",
        )
        .unwrap();
        std::fs::write(
            dir.join("src").join("lib.rs"),
            "#[test]\nfn always_fails() { panic!(\"deliberate\"); }\n",
        )
        .unwrap();
        let err = run_test(dir).unwrap_err();
        assert!(err.to_string().contains("FAILED"), "{err:?}");
    }
}
