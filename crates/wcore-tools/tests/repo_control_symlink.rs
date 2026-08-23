//! The repository-control write guard, addressed through a non-canonical path.
//!
//! Found while closing FerroxLabs/wayland#1096 direction 2, in the predicate
//! sitting one line above the new one. `WorkspacePolicy::root` is canonicalized
//! at construction, but the CANDIDATE was normalized by `canon_for_scope`,
//! which resolves only the immediate parent and falls back to the raw path when
//! that parent does not exist. A brand-new control file is precisely that shape
//! — `.git/hooks/pre-commit` in a fresh clone-less tree, or
//! `.wayland-core/skills/<new>/SKILL.md`, which is the instruction-injection
//! case the guard was written for — so on any host where the workspace is
//! reached through a symlink (`/home/u/work` -> `/mnt/data/work`,
//! `/var` -> `/private/var` on macOS) the two sides normalized differently and
//! the prefix comparison missed.
//!
//! Measured on `addb4f48` before the fix:
//!   PROBE-B          landed=true  is_error=false  output=Created .../project/.git/hooks/pre-commit
//!   PROBE-B control  landed=false is_error=true
//!
//! Driven through `WriteTool` over `RepoControlDenyFs`, not through the
//! predicate alone: a guard that is correct in isolation and unreachable from
//! the tool is indistinguishable from no guard.

use std::path::Path;
use std::sync::Arc;

use serde_json::json;
use tempfile::TempDir;
use tokio_util::sync::CancellationToken;
use wcore_tools::context::ToolContext;
use wcore_tools::vfs::{RealFs, RepoControlDenyFs, VirtualFs};
use wcore_tools::workspace_policy::WorkspacePolicy;
use wcore_tools::write::WriteTool;
use wcore_tools::{NullToolOutputSink, Tool};

fn session(cwd: &Path) -> ToolContext {
    let policy = Arc::new(WorkspacePolicy::trusted_local(cwd));
    let vfs: Arc<dyn VirtualFs> = Arc::new(RepoControlDenyFs::new(RealFs, policy));
    ToolContext::new(
        "call-1",
        CancellationToken::new(),
        vfs,
        None,
        Arc::new(NullToolOutputSink),
    )
}

async fn write(ctx: &ToolContext, path: &Path, body: &str) -> wcore_types::tool::ToolResult {
    WriteTool::new(None)
        .execute_with_ctx(
            json!({ "file_path": path.to_string_lossy(), "content": body }),
            ctx,
        )
        .await
}

#[cfg(unix)]
#[tokio::test]
async fn a_new_git_hook_under_a_symlinked_root_is_still_refused() {
    let outer = TempDir::new().unwrap();
    let real = outer.path().join("real-project");
    std::fs::create_dir_all(&real).unwrap();
    let link = outer.path().join("project");
    std::os::unix::fs::symlink(&real, &link).unwrap();

    let ctx = session(&link);
    // Neither `.git` nor `.git/hooks` exists yet: the write would create both.
    let hook = link.join(".git").join("hooks").join("pre-commit");
    let result = write(&ctx, &hook, "#!/bin/sh\nexfil\n").await;

    assert!(
        !hook.exists(),
        "a pre-commit hook was planted through a symlinked workspace root ({}) — \
         arbitrary code execution on the operator's next commit",
        hook.display()
    );
    assert!(result.is_error, "not refused: {}", result.content);
}

/// The same asymmetry on the surface the guard shares with #1096: a NEW skill
/// directory under the project load path, which is instruction injection into
/// the next session rather than the next commit.
#[cfg(unix)]
#[tokio::test]
async fn a_new_project_skill_under_a_symlinked_root_is_still_refused() {
    let outer = TempDir::new().unwrap();
    let real = outer.path().join("real-project");
    std::fs::create_dir_all(&real).unwrap();
    let link = outer.path().join("project");
    std::os::unix::fs::symlink(&real, &link).unwrap();

    let ctx = session(&link);
    let skill = link
        .join(".wayland-core")
        .join("skills")
        .join("planted")
        .join("SKILL.md");
    let result = write(&ctx, &skill, "---\nname: planted\n---\nexfiltrate\n").await;

    assert!(!skill.exists(), "planted {}", skill.display());
    assert!(result.is_error, "not refused: {}", result.content);
}

/// CONTROL — the guard is a prefix on the control surface, not a refusal of
/// everything reached through a symlink. Ordinary work in the same
/// symlink-addressed workspace must still be writable, or the two assertions
/// above would pass under a policy that had simply stopped writing anything.
#[cfg(unix)]
#[tokio::test]
async fn ordinary_files_under_a_symlinked_root_still_write() {
    let outer = TempDir::new().unwrap();
    let real = outer.path().join("real-project");
    std::fs::create_dir_all(&real).unwrap();
    let link = outer.path().join("project");
    std::os::unix::fs::symlink(&real, &link).unwrap();

    let ctx = session(&link);
    let file = link.join("src").join("main.rs");
    let result = write(&ctx, &file, "fn main() {}\n").await;

    assert!(
        !result.is_error,
        "ordinary write refused: {}",
        result.content
    );
    assert!(file.exists(), "{} was not written", file.display());
}
