use std::ffi::OsString;
use std::process::Command;

fn main() {
    println!("cargo:rerun-if-env-changed=WAYLAND_BUILD_SOURCE_SHA");
    let sha = resolve_source_sha(
        std::env::var_os("WAYLAND_BUILD_SOURCE_SHA"),
        std::env::var_os("PROFILE"),
        || git_output(&["rev-parse", "HEAD"]),
    )
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

/// Resolve the source identity embedded into the binary as `WAYLAND_SOURCE_SHA`.
///
/// `profile` is Cargo's `PROFILE` build-script variable — `release` for release
/// builds, `debug` for everything else.
///
/// **Release builds fail closed.** Before 2026-07-29 a build with neither an
/// explicit `WAYLAND_BUILD_SOURCE_SHA` nor a usable git checkout silently
/// embedded the string `"unknown"`, so a release binary produced anywhere
/// without git carried an unattributable source identity and said so only if
/// somebody ran `--build-info` and read it. Phase 29's release-integrity ledger
/// is built on exactly this attribution, and `wcore-eval-scenarios` rejects the
/// degenerate value at seal time — which is how the hole surfaced at all
/// (CI run 30434804220: the `ci-linux` container runs `docker run` as **root**
/// against a workspace owned by the runner uid, so `git rev-parse HEAD` exits
/// 128 with "detected dubious ownership" and the fallback fired).
///
/// A hard failure is affordable because there is no source-distribution path
/// today: `wcore-cli` is not published to crates.io (verified 2026-07-29 —
/// neither `wcore-cli` nor `wayland-core` exists there) and no workflow runs
/// `cargo publish`. Every shipped artifact is built from a git checkout or from
/// CI, and CI can always pass `WAYLAND_BUILD_SOURCE_SHA`.
///
/// Debug builds keep the `"unknown"` fallback: a developer building a scratch
/// tree without git should not be blocked, and a debug binary is not an artifact
/// anyone attributes.
pub fn resolve_source_sha(
    explicit: Option<OsString>,
    profile: Option<OsString>,
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
            None if profile.as_deref() == Some(std::ffi::OsStr::new("release")) => {
                Err("no source identity is available for a RELEASE build: \
                 WAYLAND_BUILD_SOURCE_SHA is unset and `git rev-parse HEAD` did not \
                 succeed. A release binary must carry an attributable source \
                 identity. Set WAYLAND_BUILD_SOURCE_SHA to 40 lowercase hexadecimal \
                 characters (in GitHub Actions: ${{ github.sha }}), or build inside \
                 a git checkout the building user can read (a container running as \
                 root over a workspace owned by another uid is refused by git with \
                 'detected dubious ownership')"
                    .to_string())
            }
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
