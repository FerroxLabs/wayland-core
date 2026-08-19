//! FerroxLabs/wayland#1098 — the authority half of `render_artifact`.
//!
//! Written from the contract: a render event costs the sandbox nothing, so
//! rendering must never be able to reach a byte an ordinary `read` could not.
//! Every test here is an attempt to use the display path as a file-read
//! primitive, plus the positive controls that keep the refusals from being
//! vacuous.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use async_trait::async_trait;
use serde_json::json;
use tokio_util::sync::CancellationToken;

use wcore_protocol::events::RENDER_ARTIFACT_CONTENT_LIMIT_BYTES;
use wcore_tools::NullToolOutputSink;
use wcore_tools::Tool;
use wcore_tools::context::ToolContext;
use wcore_tools::render::{CapturingRenderSink, RenderArtifactTool};
use wcore_tools::vfs::{
    IdentifiedFileObservation, RealFs, SandboxedFs, SecretDenyFs, VfsError, VfsMetadata, VirtualFs,
};
use wcore_tools::workspace_policy::WorkspacePolicy;

fn ctx_with(vfs: Arc<dyn VirtualFs>) -> ToolContext {
    ToolContext::new(
        "call-render-test",
        CancellationToken::new(),
        vfs,
        None,
        Arc::new(NullToolOutputSink),
    )
}

fn tool_and_sink() -> (RenderArtifactTool, Arc<CapturingRenderSink>) {
    let sink = Arc::new(CapturingRenderSink::new());
    (RenderArtifactTool::new(sink.clone()), sink)
}

/// A vfs that panics on ANY call. Used to prove a code path never touches the
/// filesystem — an assertion no "the sink saw nothing" check can make, because
/// a read that happened and was then discarded looks identical.
struct PanickingFs;

#[async_trait]
impl VirtualFs for PanickingFs {
    async fn read(&self, path: &Path) -> Result<Vec<u8>, VfsError> {
        panic!("this path must not read {}", path.display());
    }
    async fn write(&self, path: &Path, _contents: &[u8]) -> Result<(), VfsError> {
        panic!("this path must not write {}", path.display());
    }
    async fn exists(&self, path: &Path) -> Result<bool, VfsError> {
        panic!("this path must not stat {}", path.display());
    }
    async fn list(&self, dir: &Path) -> Result<Vec<PathBuf>, VfsError> {
        panic!("this path must not list {}", dir.display());
    }
    async fn remove_file(&self, path: &Path) -> Result<(), VfsError> {
        panic!("this path must not remove {}", path.display());
    }
    async fn metadata(&self, path: &Path) -> Result<VfsMetadata, VfsError> {
        panic!("this path must not stat {}", path.display());
    }
    async fn observe_file(&self, path: &Path) -> Result<IdentifiedFileObservation, VfsError> {
        panic!("this path must not observe {}", path.display());
    }
}

/// POSITIVE CONTROL. Without this the refusals below could all be produced by
/// a tool that refuses everything, and would measure nothing.
#[tokio::test]
async fn a_file_inside_the_sandbox_renders() {
    let ws = tempfile::tempdir().unwrap();
    std::fs::write(ws.path().join("report.md"), b"# inside\n").unwrap();

    let (tool, sink) = tool_and_sink();
    let ctx = ctx_with(Arc::new(SandboxedFs::new(RealFs, ws.path())));
    let result = tool
        .execute_with_ctx(
            json!({"title": "Report", "file_path": ws.path().join("report.md")}),
            &ctx,
        )
        .await;

    assert!(!result.is_error, "{}", result.content);
    let captured = sink.snapshot();
    assert_eq!(captured.len(), 1);
    assert_eq!(captured[0].content, "# inside\n");
    assert_eq!(captured[0].call_id, "call-render-test");
}

/// The DoD clause: a file the agent could not read cannot be rendered.
///
/// GUARD: the `ctx.vfs.read(path)` call in `execute_with_ctx`. Swap it for
/// `std::fs::read` and the bytes reach the sink — this fails.
#[tokio::test]
async fn a_file_outside_the_sandbox_cannot_be_rendered() {
    let ws = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    let secret = outside.path().join("board-notes.md");
    std::fs::write(&secret, b"acquisition price is 40m").unwrap();

    let (tool, sink) = tool_and_sink();
    let ctx = ctx_with(Arc::new(SandboxedFs::new(RealFs, ws.path())));
    let result = tool
        .execute_with_ctx(json!({"title": "Notes", "file_path": secret}), &ctx)
        .await;

    assert!(result.is_error, "out-of-sandbox render must be refused");
    assert!(
        sink.snapshot().is_empty(),
        "nothing may reach the host: {:?}",
        sink.snapshot()
    );
    assert!(
        !result.content.contains("acquisition price"),
        "the refusal must not leak the content it refused: {}",
        result.content
    );
}

/// A grant widens WHERE the agent may look, never WHAT. A secret inside an
/// explicitly granted folder stays refused through the render path too.
///
/// GUARD: the `is_secret_path_static(&target)` check inside
/// `SandboxedFs::contain_read`. Delete it and the render succeeds.
#[tokio::test]
async fn a_secret_inside_a_granted_folder_cannot_be_rendered() {
    let ws = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    std::fs::write(outside.path().join("brief.md"), b"# quarterly\n").unwrap();
    std::fs::write(outside.path().join(".env"), b"OPENAI_API_KEY=sk-live-42").unwrap();

    let policy = Arc::new(WorkspacePolicy::contained(ws.path()).with_local_operator_principal());
    policy
        .grant_session_read_root(outside.path(), false)
        .expect("a local operator may grant a folder");

    let vfs = Arc::new(
        SandboxedFs::new(SecretDenyFs::new(RealFs, Arc::clone(&policy)), ws.path())
            .with_read_grants(policy.session_read_grant_handle()),
    );
    let (tool, sink) = tool_and_sink();
    let ctx = ctx_with(vfs);

    // The grant works: the ordinary file in the granted folder renders.
    let allowed = tool
        .execute_with_ctx(
            json!({"title": "Brief", "file_path": outside.path().join("brief.md")}),
            &ctx,
        )
        .await;
    assert!(!allowed.is_error, "{}", allowed.content);
    assert_eq!(sink.snapshot().len(), 1);

    // The secret beside it does not.
    let refused = tool
        .execute_with_ctx(
            json!({"title": "Env", "file_path": outside.path().join(".env")}),
            &ctx,
        )
        .await;
    assert!(
        refused.is_error,
        "a secret in a granted folder stays refused"
    );
    assert_eq!(
        sink.snapshot().len(),
        1,
        "the secret must not have reached the host"
    );
    assert!(
        !refused.content.contains("sk-live-42"),
        "the refusal leaked the key: {}",
        refused.content
    );
}

/// SECURITY MAJOR #14: the entry point with no `ToolContext` has no vfs, so a
/// read from it would be unconfined. It must refuse, not fall through.
///
/// GUARD: the explicit refusal body in `RenderArtifactTool::execute`.
#[tokio::test]
async fn the_legacy_execute_entry_refuses_instead_of_reading_unconfined() {
    let outside = tempfile::tempdir().unwrap();
    let target = outside.path().join("secrets.md");
    std::fs::write(&target, b"unconfined bytes").unwrap();

    let (tool, sink) = tool_and_sink();
    let result = tool
        .execute(json!({"title": "Anything", "file_path": target}))
        .await;

    assert!(result.is_error, "the ctx-less entry must refuse");
    assert!(sink.snapshot().is_empty());
    assert!(
        !result.content.contains("unconfined bytes"),
        "the ctx-less entry read the file: {}",
        result.content
    );
}

/// Inline content is bytes the model already holds, so it adds no exfiltration
/// surface — and it must not acquire one by growing a filesystem touch.
///
/// GUARD: the inline branch. Make it stat or canonicalize a path and the
/// panicking vfs fires.
#[tokio::test]
async fn inline_content_never_touches_the_filesystem() {
    let (tool, sink) = tool_and_sink();
    let ctx = ctx_with(Arc::new(PanickingFs));
    let result = tool
        .execute_with_ctx(
            json!({"title": "Generated", "mime": "text/html", "content": "<h1>hi</h1>"}),
            &ctx,
        )
        .await;

    assert!(!result.is_error, "{}", result.content);
    assert_eq!(sink.snapshot()[0].content, "<h1>hi</h1>");
}

/// An undeclared mime is refused BEFORE any read, so a call that could never
/// reach a host cannot double as a file-existence probe.
///
/// GUARD: the closed `RenderMime` parse in `RenderArtifactTool::parse`. Widen
/// it to accept free text and an unknown mime reaches the wire, where our own
/// published schema's `enum` rejects our own frame.
#[tokio::test]
async fn an_undeclared_mime_is_refused_before_any_read() {
    let (tool, sink) = tool_and_sink();
    let ctx = ctx_with(Arc::new(PanickingFs));
    let result = tool
        .execute_with_ctx(
            json!({
                "title": "Script",
                "mime": "application/x-shellscript",
                "file_path": "/etc/passwd"
            }),
            &ctx,
        )
        .await;

    assert!(result.is_error);
    assert!(
        result.content.contains("text/markdown"),
        "{}",
        result.content
    );
    assert!(sink.snapshot().is_empty());
}

/// The size gate. A file bigger than the render cap is refused with the size
/// known in advance, rather than pulled into memory to be thrown away.
///
/// GUARD: the `ctx.vfs.metadata` size check.
#[tokio::test]
async fn a_file_over_the_cap_is_refused_before_it_is_read() {
    let ws = tempfile::tempdir().unwrap();
    let big = ws.path().join("huge.md");
    std::fs::write(&big, vec![b'x'; RENDER_ARTIFACT_CONTENT_LIMIT_BYTES + 1]).unwrap();

    let (tool, sink) = tool_and_sink();
    let ctx = ctx_with(Arc::new(SandboxedFs::new(RealFs, ws.path())));
    let result = tool
        .execute_with_ctx(json!({"title": "Huge", "file_path": big}), &ctx)
        .await;

    assert!(result.is_error);
    assert!(
        result
            .content
            .contains(&RENDER_ARTIFACT_CONTENT_LIMIT_BYTES.to_string()),
        "the refusal must name the cap so the model knows what to do: {}",
        result.content
    );
    assert!(sink.snapshot().is_empty());
}

/// The honesty gate. A tool whose sink cannot render must never be advertised
/// to the model — otherwise the model believes it showed something and the
/// user saw nothing.
///
/// GUARD: the `is_available()` override.
#[test]
fn a_tool_with_no_render_surface_is_not_registered() {
    let mut registry = wcore_tools::registry::ToolRegistry::new();
    registry.register(Box::new(RenderArtifactTool::default()));
    assert!(
        registry.get("render_artifact").is_none(),
        "a null render sink must keep the tool out of the model's tool list"
    );

    let mut live = wcore_tools::registry::ToolRegistry::new();
    live.register(Box::new(RenderArtifactTool::new(Arc::new(
        CapturingRenderSink::new(),
    ))));
    assert!(
        live.get("render_artifact").is_some(),
        "with a live sink it must be registered — otherwise the check above is vacuous"
    );
}
