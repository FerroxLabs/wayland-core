//! FerroxLabs/wayland#1098 — `render_artifact`: "show this to the user" as a
//! protocol RENDER capability instead of an OS `open`.
//!
//! Handing a path to LaunchServices (`open`), `xdg-open` or `cmd /c start` is a
//! filesystem + process capability doing a UI job. It is also why #1102 exists:
//! the macOS seatbelt profile is `(deny default)` and never grants the SBPL
//! operation `lsopen`, so `open` returns `-54`. Granting `lsopen` would be an
//! execution-confinement escape — a sandboxed shell could ask launchd to start
//! any installed app OUTSIDE our profile — and Sean's decision is that we will
//! not grant it. This tool is what makes that decision cost nothing: the host
//! receives CONTENT, needs zero filesystem authority to display it, and the
//! path works headless, over SSH, and identically on all three platforms.
//!
//! ## Why a built-in tool and not an existing surface
//!
//! The obvious alternative is `ToolContext::sink` (`ToolOutputSink`), which
//! tools already hold. It cannot carry this: its surface is `emit_chunk` /
//! `emit_progress` only, and — decisively — the main-agent dispatcher hands
//! every tool a `NullToolOutputSink` (`orchestration/mod.rs`), so anything
//! routed through it is a guaranteed no-op for the main agent. The precedent
//! that DOES fit is `SendMessageTool`: a host-supplied boundary trait injected
//! at construction (`MessageTransport`) with a fail-loud null default. This
//! module mirrors that exactly rather than inventing a third mechanism —
//! including the part where the tool is registered unconditionally and a
//! missing backend is a loud error, not a missing tool. Registration must not
//! depend on the output sink: `tool_inventory` is inside the recovery authority
//! digest, so a tool set that moves with the sink makes a session unresumable
//! across a seed-under-NullSink / resume-under-ProtocolSink boundary.
//!
//! ## Authority
//!
//! The model must not be able to use this to exfiltrate a file it could not
//! otherwise read. Content therefore comes through the SAME vfs/policy path as
//! an ordinary `read`: `validate_user_path` for the shape check, then
//! `ctx.vfs` — which is `SandboxedFs ∘ SecretDenyFs` for a contained session,
//! honours standing path grants for the pure-read ops, and refuses secrets
//! inside a granted folder. A file the agent may not read is a file it may not
//! render, structurally, because it is the same call.
//!
//! The legacy `Tool::execute` entry (no `ToolContext`, therefore no vfs)
//! refuses outright rather than falling through to an unconfined `RealFs` —
//! that fall-through is SECURITY MAJOR #14, which
//! `tests/legacy_execute_path_validation_test.rs` exists to prevent for
//! Read/Write/Edit.

use std::path::Path;
use std::sync::Arc;

use async_trait::async_trait;
use serde_json::{Value, json};

use wcore_protocol::events::{
    RENDER_ARTIFACT_CONTENT_LIMIT_BYTES, RenderMime, ToolCategory, truncate_render_title,
};
use wcore_types::tool::{JsonSchema, ToolResult};

use crate::Tool;
use crate::context::ToolContext;
use crate::path_validation::validate_user_path;

/// One artifact handed to the host for display.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RenderedArtifact {
    pub call_id: String,
    pub title: String,
    pub mime: RenderMime,
    pub content: String,
}

/// Host-supplied render boundary. The engine never touches a UI; the host
/// (json-stream protocol sink, or a test double) implements this and binds it
/// at registration time. Mirrors `send_message::MessageTransport`.
pub trait RenderSink: Send + Sync {
    /// Hand `content` to the host for display. Fire-and-forget: rendering is
    /// not an operation the agent can wait on or learn the outcome of, which
    /// is precisely what makes it free of authority.
    fn render(&self, artifact: RenderedArtifact);

    /// Whether a host is actually listening. `false` makes every call fail
    /// LOUDLY rather than silently discarding, so the model never finishes a
    /// turn believing it showed the user something nothing rendered.
    fn is_live(&self) -> bool;
}

/// Default sink when nothing is wired: every render fails loudly. Mirrors
/// `NullMessageTransport` — a stub that succeeds is worse than no tool.
pub struct NullRenderSink;

impl RenderSink for NullRenderSink {
    fn render(&self, _artifact: RenderedArtifact) {}
    fn is_live(&self) -> bool {
        false
    }
}

/// In-memory sink that records every render for assertions. Lives in the prod
/// module so downstream crates can use it without `#[cfg(test)]` symbols —
/// same placement rationale as `CapturingMessageTransport`.
#[derive(Default)]
pub struct CapturingRenderSink {
    captured: parking_lot::Mutex<Vec<RenderedArtifact>>,
}

impl CapturingRenderSink {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn snapshot(&self) -> Vec<RenderedArtifact> {
        self.captured.lock().clone()
    }
}

impl RenderSink for CapturingRenderSink {
    fn render(&self, artifact: RenderedArtifact) {
        self.captured.lock().push(artifact);
    }
    fn is_live(&self) -> bool {
        true
    }
}

/// `render_artifact` — hand the host content to display.
pub struct RenderArtifactTool {
    sink: Arc<dyn RenderSink>,
}

impl Default for RenderArtifactTool {
    fn default() -> Self {
        Self::new(Arc::new(NullRenderSink))
    }
}

impl RenderArtifactTool {
    pub fn new(sink: Arc<dyn RenderSink>) -> Self {
        Self { sink }
    }
}

/// The two input modes, already validated.
enum Source {
    /// Model-authored bytes. Adds no exfiltration surface: they were already
    /// in the model's context, so nothing crosses a boundary it had not
    /// already crossed. This is the actual #1098 scenario — a skill that has
    /// just generated a report and wants it shown.
    Inline(String),
    /// A workspace path, read through `ctx.vfs`.
    File(String),
}

fn error(message: impl Into<String>) -> ToolResult {
    ToolResult {
        content: message.into(),
        is_error: true,
    }
}

impl RenderArtifactTool {
    /// Parse and validate the call WITHOUT touching the filesystem.
    ///
    /// Ordering is load-bearing: an undeclared `mime` is refused here, before
    /// any read, so a call that could never reach a host cannot be used to
    /// probe for a file's existence through the error it returns.
    fn parse(input: &Value) -> Result<(String, RenderMime, Source), ToolResult> {
        let title = match input.get("title").and_then(Value::as_str) {
            Some(t) if !t.trim().is_empty() => truncate_render_title(t),
            _ => return Err(error("Missing required parameter: 'title'")),
        };

        let mime_token = input
            .get("mime")
            .and_then(Value::as_str)
            .unwrap_or("text/markdown");
        let Some(mime) = RenderMime::from_wire(mime_token) else {
            return Err(error(format!(
                "Unsupported mime '{mime_token}'. render_artifact carries a closed \
                 vocabulary: {}.",
                RenderMime::all().join(", ")
            )));
        };

        let inline = input
            .get("content")
            .and_then(Value::as_str)
            .filter(|c| !c.is_empty());
        let file_path = input
            .get("file_path")
            .and_then(Value::as_str)
            .filter(|p| !p.trim().is_empty());

        let source = match (inline, file_path) {
            (Some(content), None) => Source::Inline(content.to_string()),
            (None, Some(path)) => Source::File(path.to_string()),
            (Some(_), Some(_)) => {
                return Err(error("Provide either 'content' or 'file_path', not both."));
            }
            (None, None) => {
                return Err(error(
                    "Missing content: provide 'content' (inline) or 'file_path'.",
                ));
            }
        };
        Ok((title, mime, source))
    }
}

#[async_trait]
impl Tool for RenderArtifactTool {
    fn name(&self) -> &str {
        "render_artifact"
    }

    fn description(&self) -> &str {
        "Display content to the user in the host UI. Use this instead of \
         opening a file with a shell command — it works headless and over SSH, \
         and needs no filesystem access on the host.\n\n\
         Provide EITHER 'content' (text you have already written) OR \
         'file_path' (an absolute path you are allowed to read; it is read \
         through the same permission checks as the read tool, so a file you \
         cannot read is a file you cannot render).\n\n\
         'mime' is one of text/plain, text/markdown (default), text/html. \
         Content over 1 MiB is shown truncated with a marker saying so."
    }

    fn input_schema(&self) -> JsonSchema {
        json!({
            "type": "object",
            "properties": {
                "title": {
                    "type": "string",
                    "description": "Short label for the rendered surface."
                },
                "mime": {
                    "type": "string",
                    "enum": RenderMime::all(),
                    "description": "How to render the content. Defaults to text/markdown."
                },
                "content": {
                    "type": "string",
                    "description": "The content to display. Mutually exclusive with file_path."
                },
                "file_path": {
                    "type": "string",
                    "description": "Absolute path to read and display. \
                        Mutually exclusive with content."
                }
            },
            "required": ["title"]
        })
    }

    fn is_concurrency_safe(&self, _input: &Value) -> bool {
        true
    }

    fn category(&self) -> ToolCategory {
        // Identical authority to `read`, plus a display. Not Exec: nothing is
        // spawned and nothing outside this process is mutated.
        ToolCategory::Info
    }

    /// Rendering reaches the same surface the assistant's own text already
    /// does. It mutates no file, spawns no process, and changes no durable
    /// state, for any input this tool accepts — and a "look, don't touch"
    /// session is exactly the one where showing the user what you found is the
    /// whole job.
    fn read_only_safe(&self, _input: &Value) -> bool {
        true
    }

    /// No `ToolContext` means no vfs, and a render without a vfs would have to
    /// read through an unconfined `RealFs` — an out-of-sandbox read reachable
    /// from a tool the model calls. Refuse instead (SECURITY MAJOR #14).
    async fn execute(&self, _input: Value) -> ToolResult {
        error(
            "render_artifact requires the context-aware execution path \
             (the sandboxed filesystem view); it cannot run unconfined.",
        )
    }

    async fn execute_with_ctx(&self, input: Value, ctx: &ToolContext) -> ToolResult {
        // The honesty gate. Fail LOUDLY when there is no display, exactly as
        // `NullMessageTransport` does for an unwired `send_message` — the model
        // learns inside the same turn and can put the content in its reply
        // instead. A silent discard would leave it believing it had shown the
        // user something.
        //
        // Deliberately NOT an `is_available()` override, which would have kept
        // the tool out of the registry entirely. `is_available()` is for tools
        // whose backend is fixed by process configuration; liveness here is a
        // property of the SINK, and the sink differs between a session that
        // writes a recovery checkpoint and the one that resumes it. Gating
        // registration on it made `tool_inventory` — which the recovery
        // authority digest covers — depend on the sink, and a resume across the
        // two was refused with "tool authority changed"
        // (`wcore-cli/tests/f14_sigkill_recovery.rs` caught it). The tool set
        // must not move with the output surface.
        if !self.sink.is_live() {
            return error(
                "This session has no display surface (no connected host UI), so \
                 render_artifact cannot show anything. Put the content in your \
                 reply instead.",
            );
        }

        let (title, mime, source) = match Self::parse(&input) {
            Ok(parsed) => parsed,
            Err(refusal) => return refusal,
        };

        let content = match source {
            Source::Inline(content) => content,
            Source::File(file_path) => {
                // Same shape check as ReadTool's two entry points — relative
                // paths, traversal, null bytes and the system-secret deny list
                // are refused before any filesystem touch.
                let validated = match validate_user_path(Path::new(&file_path)) {
                    Ok(p) => p,
                    Err(e) => return error(format!("Refused to render {file_path}: {e}")),
                };
                let path = validated.as_path();

                // Size first, through the SAME containment as the read below,
                // so an oversized file is refused instead of being pulled into
                // memory to be thrown away. Unlike a command's output (whose
                // size cannot be known in advance, so `wcore-sandbox` truncates
                // it — wayland#1071), the size is knowable here and the caller
                // has a first-class way to select a part: `read` with
                // offset/limit, then render the result inline.
                match ctx.vfs.metadata(path).await {
                    Ok(meta) if meta.is_dir => {
                        return error(format!("Cannot render {file_path}: it is a directory."));
                    }
                    Ok(meta) if meta.size as usize > RENDER_ARTIFACT_CONTENT_LIMIT_BYTES => {
                        return error(format!(
                            "Refused to render {file_path}: {} bytes exceeds the \
                             {RENDER_ARTIFACT_CONTENT_LIMIT_BYTES}-byte render cap. \
                             Read the part you want and pass it as 'content'.",
                            meta.size
                        ));
                    }
                    Ok(_) => {}
                    Err(e) => return error(format!("Failed to render {file_path}: {e}")),
                }

                let bytes = match ctx.vfs.read(path).await {
                    Ok(bytes) => bytes,
                    Err(e) => return error(format!("Failed to render {file_path}: {e}")),
                };
                match String::from_utf8(bytes) {
                    Ok(text) => text,
                    Err(_) => {
                        return error(format!(
                            "Cannot render {file_path}: it is not UTF-8 text. \
                             render_artifact carries text only."
                        ));
                    }
                }
            }
        };

        let bytes = content.len();
        self.sink.render(RenderedArtifact {
            call_id: ctx.call_id.clone(),
            title: title.clone(),
            mime,
            content,
        });
        ToolResult {
            content: json!({
                "rendered": true,
                "title": title,
                "mime": mime.as_str(),
                "bytes": bytes,
            })
            .to_string(),
            is_error: false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn call(tool: &RenderArtifactTool, input: Value, ctx: &ToolContext) -> ToolResult {
        futures::executor::block_on(tool.execute_with_ctx(input, ctx))
    }

    /// A tool that cannot display anything must SAY so. Registration is
    /// deliberately unconditional (see the `is_live` gate's comment), so the
    /// loud error is the only thing standing between the model and a silent
    /// discard.
    #[test]
    fn a_session_with_no_display_fails_loudly_instead_of_discarding() {
        let tool = RenderArtifactTool::default();
        let ctx = ToolContext::test_default();
        let result = call(&tool, json!({"title": "R", "content": "x"}), &ctx);
        assert!(
            result.is_error,
            "a render with nowhere to go must not succeed"
        );
        assert!(
            result.content.contains("no display surface"),
            "{}",
            result.content
        );
    }

    #[test]
    fn inline_content_reaches_the_sink_with_the_declared_mime() {
        let sink = Arc::new(CapturingRenderSink::new());
        let tool = RenderArtifactTool::new(sink.clone());
        let ctx = ToolContext::test_default();
        let result = call(
            &tool,
            json!({"title": "Report", "mime": "text/html", "content": "<p>hi</p>"}),
            &ctx,
        );
        assert!(!result.is_error, "{}", result.content);
        let captured = sink.snapshot();
        assert_eq!(captured.len(), 1);
        assert_eq!(captured[0].mime, RenderMime::Html);
        assert_eq!(captured[0].content, "<p>hi</p>");
    }

    #[test]
    fn mime_defaults_to_markdown() {
        let sink = Arc::new(CapturingRenderSink::new());
        let tool = RenderArtifactTool::new(sink.clone());
        let ctx = ToolContext::test_default();
        call(&tool, json!({"title": "R", "content": "# x"}), &ctx);
        assert_eq!(sink.snapshot()[0].mime, RenderMime::Markdown);
    }

    #[test]
    fn both_inputs_at_once_is_refused() {
        let sink = Arc::new(CapturingRenderSink::new());
        let tool = RenderArtifactTool::new(sink.clone());
        let ctx = ToolContext::test_default();
        let result = call(
            &tool,
            json!({"title": "R", "content": "x", "file_path": "/etc/hosts"}),
            &ctx,
        );
        assert!(result.is_error);
        assert!(sink.snapshot().is_empty());
    }

    #[test]
    fn an_overlong_title_is_shortened_not_refused() {
        let sink = Arc::new(CapturingRenderSink::new());
        let tool = RenderArtifactTool::new(sink.clone());
        let ctx = ToolContext::test_default();
        let long = "t".repeat(4096);
        let result = call(&tool, json!({"title": long, "content": "x"}), &ctx);
        assert!(!result.is_error);
        assert_eq!(
            sink.snapshot()[0].title.len(),
            wcore_protocol::events::RENDER_ARTIFACT_TITLE_LIMIT_BYTES
        );
    }
}
