use std::ffi::OsString;
use std::process::Command;

fn main() {
    println!("cargo:rerun-if-env-changed=WAYLAND_BUILD_SOURCE_SHA");
    let sha = resolve_source_sha(std::env::var_os("WAYLAND_BUILD_SOURCE_SHA"), || {
        git_output(&["rev-parse", "HEAD"])
    })
    .unwrap_or_else(|error| panic!("invalid WAYLAND_BUILD_SOURCE_SHA: {error}"));
    println!("cargo:rustc-env=WAYLAND_SOURCE_SHA={sha}");

    // `HEAD` usually contains only `ref: refs/heads/<branch>` and therefore
    // does not change when that branch advances. Watch both the worktree HEAD
    // and its resolved symbolic ref so a commit invalidates embedded
    // provenance. `git --git-path` handles normal repos and linked worktrees.
    if let Some(head_path) = git_output(&["rev-parse", "--git-path", "HEAD"]) {
        println!("cargo:rerun-if-changed={head_path}");
    }
    if let Some(head_ref) = git_output(&["symbolic-ref", "-q", "HEAD"])
        && let Some(ref_path) = git_output(&["rev-parse", "--git-path", &head_ref])
    {
        println!("cargo:rerun-if-changed={ref_path}");
    }
}

pub fn resolve_source_sha(
    explicit: Option<OsString>,
    git_source: impl FnOnce() -> Option<String>,
) -> Result<String, String> {
    match explicit {
        Some(value) => {
            let source = value
                .into_string()
                .map_err(|_| "value is not valid Unicode".to_string())?;
            validate_source_sha(source)
        }
        None => match git_source() {
            Some(source) => validate_source_sha(source),
            None => Ok("unknown".to_string()),
        },
    }
}

fn validate_source_sha(source: String) -> Result<String, String> {
    if source.len() == 40
        && source
            .bytes()
            .all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'))
    {
        Ok(source)
    } else {
        Err("expected exactly 40 lowercase hexadecimal characters".to_string())
    }
}

fn git_output(args: &[&str]) -> Option<String> {
    Command::new("git")
        .args(args)
        .output()
        .ok()
        .filter(|output| output.status.success())
        .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_string())
        .filter(|value| !value.is_empty())
}
