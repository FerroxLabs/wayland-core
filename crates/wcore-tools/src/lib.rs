// T7: read-only archive handling tool (zip / tar / tar.gz) with
// zip-slip path-traversal rejection.
pub mod archive_tool;
// v0.9.3 W0.4: AskUserQuestion — structured multi-choice question whose
// answer is routed back via the approval channel (ProtocolCommand::ToolApprove
// `answer` field, W0.1) and synthesized into the ToolResult by orchestration
// at `orchestration::mod.rs:911` (W0.3). `execute()` is a loud-defensive
// fallback only — the happy path never dispatches.
pub mod ask_user_question;
// Memory write tool: assert a durable (subject, predicate, object) fact into P3
// semantic memory. Read side is `session_search`.
pub mod assert_fact;
// T14 (v0.6.3 Tier 2B): read-only AWS CLI wrapper — `aws <service> <op>` with
// a read-only operation allowlist; mutating ops rejected. Sandboxed via Tier S.
pub mod aws_cli_tool;
pub mod bash;
// Native per-command Bash output compaction (cargo/git/test/grep) — engine-owned,
// fail-open, size-gated. See `.planning/2026-06-11-NATIVE-BASH-COMPACTION-DESIGN.md`.
pub mod bash_compact;
// T3-3.2.1: pure-string binary extension filter ported from the prior Wayland Python engine (sub-wave 2).
pub mod binary_extensions;
// T3-3.1.1: clarify_tool ported from the prior Wayland Python engine (sub-wave 1).
pub mod clarify;
// W8a A.3: ToolContext threaded into every tool dispatch (cancel + vfs + sink).
pub mod context;
// T3-3.1.8: per-tool opt-in JSON call log (port of the prior Wayland Python engine).
pub mod debug_helpers;
// T3-3.4 (sub-wave 4): env var passthrough registry — HELPER. Ported
// from the prior Wayland Python engine. Skill-declared
// `required_environment_variables` (plus host config) survive
// sandboxed sub-process env stripping in BashTool / ScriptTool.
pub mod env_passthrough;
// T3-3.1.3: delegate tool ported from the prior Wayland Python engine — bridges to wcore-types `Spawner`
// trait so the actual sub-agent dispatch stays in wcore-agent.
pub mod delegate;
pub mod dispatcher;
pub mod edit;
// T10 (v0.6.3 Tier 2B): read-only email parser — .eml (single message) and
// .mbox (mailbox) files. Reports headers + body text + attachment metadata
// (name/size, never content). Pure-Rust `mail-parser` backend.
pub mod email_parse_tool;
pub mod file_cache;
// T3-3.2.2: shared file-safety policy (write deny list + skill-hub read
// block) ported from the prior Wayland Python engine.
pub mod file_safety;
// T3-3.2.3: cross-agent file state coordination (port of the prior Wayland Python engine).
pub mod file_state;
// T3-3.2.7: 9-strategy fuzzy find-and-replace helper (port of the prior
// Wayland Python engine). Wired into EditTool as an opt-in fallback (Rank 41):
// `EditTool::with_fuzzy_fallback(true)` retries an exact-match failure
// through this chain; default OFF so the exact path is byte-identical.
pub mod fuzzy_match;
// W8b.2.A — FileWriteNotifier trait carried on ToolContext (D.4).
pub mod file_write_notifier;
// T13 (v0.6.3 Tier 2B): read-only `gcloud` CLI wrapper — read-only verb
// allowlist (list/describe/get-*/...); mutating verbs rejected before
// execution. Runs the assembled argv through the sandbox backend.
pub mod gcloud_tool;
pub mod git;
pub mod git_commit_message;
// T4 (v0.6.3 Tier 2B): GitLab REST API v4 tool — issue/MR/file read +
// note posts via a pluggable GitLabBackend (NullGitLabBackend fails
// loud, NO-STUBS). Configurable base URL for self-hosted GitLab.
// Ported from the prior Wayland Python engine.
pub mod gitlab_tool;
pub mod glob;
// T3-3.7.3 (sub-wave 7): Google Meet conferencing tool ported from
// the prior Wayland Python engine.
pub mod google_meet_tool;
pub mod grep;
// T3-3.1.5: per-thread interrupt signaling (port of the prior Wayland Python engine).
pub mod interrupt;
// T11 (Plan v2 Tier 2B): JSON Lines streaming tool — large-file-friendly
// ranged slice / count / key filter / per-line JSON validation via a
// streaming buffered reader (the file is never slurped into memory).
pub mod jsonl_tool;
// T12 (v0.6.3 Tier 2B): read-only kubectl wrapper — get/describe/logs/...
// only; every mutating verb rejected by closed allowlist. Routes through
// the sandbox backend per Tier S (mirrors BashTool).
pub mod kubectl_tool;
// T5 (v0.6.3 Tier 2B): Linear GraphQL API tool — issue/cycle/team
// queries via a pluggable LinearBackend (NullLinearBackend fails loud,
// NO-STUBS). Read-only; mirrors github_tool/gitlab_tool.
pub mod linear_tool;
// T9 (v0.6.3 Tier 2B): pure-string Markdown table format/lint tool.
pub mod markdown_tool;
// T2-C2: Mixture-of-Agents tool (Wang 2024 — proposer fan-out + aggregator synth).
pub mod moa;
// T6 (v0.6.3 Tier 2B): Notion REST API tool — page/block reads +
// block append + page create via a pluggable NotionBackend
// (NullNotionBackend fails loud, NO-STUBS). Mirrors github_tool.
pub mod notion_tool;
// T3-3.3.4 (sub-wave 4): Office-side skill enable/disable runtime (HELPER)
// ported from the prior Wayland Python engine. Manages
// materialization of optional skill bundles under the user's skills
// directory in response to Desktop-driven enable/disable toggles.
pub mod office_runtime;
// T3-3.8.4: OSV malware advisory check before launching MCP package-runner
// shims (`npx` / `uvx`) — ported from the prior Wayland Python engine (sub-wave 8).
pub mod osv_check;
// Wave SD: path validation for legacy `execute()` entry points (closes
// SECURITY MAJOR #14 — top-level Read/Write/Edit without sandbox).
pub mod path_validation;
// T3-3.2.8: V4A patch-format parser (HELPER) ported from the prior Wayland Python engine (sub-wave 2).
pub mod patch_parser;
// T15: read-only PDF text-extraction tool (port of the prior Wayland Python engine).
// Pure-Rust `pdf-extract` backend gated behind the default-on `pdf`
// cargo feature; PdfTool degrades to an honest error when the feature
// is off so the schema stays stable across build configs.
pub mod pdf_tool;
// T3-3.8 (sub-wave 8): Piper TTS voice + binary downloader (HELPER) ported
// from the prior Wayland Python engine. Pluggable
// ModelDownloader + BinaryExtractor traits (Null = fail-loud,
// Capturing = test double); not surfaced as a tool because `tts_tool`
// explicitly delegates Piper voice/binary acquisition to the backend.
pub mod piper_download;
// v0.6.3 Tier 2B (T2): postgres_schema — read-only Postgres schema
// introspection (list tables / columns / foreign keys) via a pluggable
// PostgresSchemaBackend. NullPostgresSchemaBackend fails loud (NO-STUBS).
// The live tokio-postgres backend is gated behind the optional
// `postgres` cargo feature so the native client is not a default dep.
pub mod postgres_schema_tool;
pub mod read;
// Token-opt (diff-resend): line diff between last-read and current content.
pub mod read_diff;
// Token-opt (semantic slicing): resolve a named symbol to a line window.
pub mod symbol_slice;
// Memory write tool: log a meaningful event into P2 episodic memory.
pub mod record_episode;
pub mod registry;
pub mod repomap;
// T3-3.3.2: HELPER — broad JSON-Schema sanitizer for llama.cpp / strict
// backend compat (port of the prior Wayland Python engine). Distinct from
// the Bedrock-targeted `wcore_config::compat::sanitize_json_schema`.
pub mod schema_sanitizer;
pub mod script;
// T3-3.1.4: cross-channel `send_message` tool (port of
// the prior Wayland Python engine).
pub mod send_message;
// T3-3.7 (sub-wave 7): Discord server tool — port of
// the prior Wayland Python engine. Dispatch surface only;
// host wires a `DiscordBackend` implementation for real REST I/O.
// NullDiscordBackend fails loud (NO-STUBS). Composes url_safety for
// defense-in-depth on string fields that could carry URLs.
pub mod discord_tool;
// v0.6.3 Tier 2B T3: GitHub REST API operations tool — issue/PR/file
// reads + comment/commit posts. Dispatch surface only; host wires a
// `GitHubBackend` (typically a `wcore-providers` http_client wrapper).
// NullGitHubBackend fails loud (NO-STUBS). Mirrors discord_tool.
pub mod github_tool;
// T3-3.7 (sub-wave 7): cronjob scheduled-task management tool —
// pluggable CronScheduler seam (NullCronScheduler fails loud). Ported
// from the prior Wayland Python engine.
pub mod cronjob_tools;
// T3-3.1.7: SessionSearchTool — past-session recall via MemoryApi.
pub mod session_search;
// v0.6.3 T1: read-only SQL query tool — SQLite via rusqlite, result-set
// truncation. Postgres/MySQL are out of scope (would be `sql-extra`-gated).
pub mod sql_query_tool;
// T3-3.7 (sub-wave 7): Tencent Yuanbao platform toolset
// (port of the prior Wayland Python engine). Single
// `YuanbaoTool` with an `action` discriminator dispatches all five
// operations (group_info / group_members / search_sticker /
// send_sticker / send_dm) through a host-supplied YuanbaoBackend.
pub mod yuanbao_tools;
// T3-3.3.3: Tirith pre-exec security scanner wrapper (HELPER) ported from
// the prior Wayland Python engine. Auto-installer is documented out-of-scope.
pub mod tirith_security;
// T3-3.1.2: in-memory planning/task list tool ported from the prior Wayland Python engine.
pub mod todo;
// T3-3.3.3: configurable tool-output truncation limits (HELPER) — port of
// the prior Wayland Python engine. Adds user-tunable
// `max_bytes` / `max_lines` / `max_line_length` knobs that complement the
// existing per-tool `max_result_size()` and `truncate_utf8()` primitives.
pub mod tool_output_limits;
// T3-3.3.3: tool-result persistence helper (port of
// the prior Wayland Python engine).
pub mod tool_result_storage;
pub mod tool_search;
// T3-3.3.3: SSRF / private-network URL safety helper (port of the prior
// Wayland Python engine). HELPER module — callers wire it into HTTP-client
// redirect hooks and tool pre-flight checks.
pub mod url_safety;
// W8a A.3: VirtualFs trait + RealFs / InMemoryFs / SandboxedFs (X2).
pub mod vfs;
// T3-3.5 (sub-wave 5): video_analyze tool — AI video analysis via a
// pluggable VideoAnalysisBackend (NullVideoBackend fails loud).
pub mod video_analyze_tool;
// T3-3.6 (sub-wave 6): image_generate tool — text-to-image generation
// via a pluggable ImageGenerationBackend (NullImageGenerationBackend
// fails loud). Ported from the prior Wayland Python engine.
pub mod image_generation_tool;
// T8 (v0.6.3 Tier 2B): image_inspect tool — read-only image metadata
// (dimensions / format / color type via `image`; EXIF via
// `kamadak-exif`). Pure-Rust, no native deps. Strictly read-only — the
// tool never decodes the full pixel buffer or writes any image.
pub mod image_inspect_tool;
// T3-3.6 (sub-wave 6): text_to_speech tool — multi-provider TTS via a
// pluggable TtsBackend (NullTtsBackend fails loud). Ported from
// the prior Wayland Python engine.
pub mod tts_tool;
// T3-3.3.3: website blocklist helper ported from
// the prior Wayland Python engine (sub-wave 3).
pub mod website_policy;
// T3-3.5: vision_analyze tool ported from
// the prior Wayland Python engine (sub-wave 5).
pub mod vision_tools;
// T3-3.8 (sub-wave 8): web tool — search/extract/crawl via a pluggable
// WebBackend (NullWebBackend fails loud). Ported from
// the prior Wayland Python engine. Composes url_safety +
// website_policy for SSRF + blocklist gating before backend dispatch.
pub mod web_tools;
// Wave RC (2026-05-23): simple HTTP-GET tool — `WebFetch`. The Browser
// tool requires a Camoufox / Chromium sidecar that is NOT installed on a
// fresh wayland-core, so a user asking "fetch this URL" used to watch a
// 60s spinner. WebFetch is a plain HTTP GET via a `FetchBackend` seam
// (host wires `HttpFetchBackend` in `wcore-agent`); it is what the model
// reaches for by default for read-only page fetches now.
pub mod web_fetch;
// T3-3.6: transcribe_audio tool ported from
// the prior Wayland Python engine (sub-wave 6).
// Pluggable TranscriptionBackend + AudioFetcher seams; NullBackend
// fails loud (NO-STUBS); composes url_safety + website_policy for
// URL inputs. Mirrors the vision_tools seam pattern.
pub mod transcription_tools;
// T3-3.6 (sub-wave 6): voice_mode session helper — pluggable
// AudioRecorder / TranscriptionBackend / AudioPlayer seams. Ported
// from the prior Wayland Python engine.
pub mod voice_mode;
// T3-3.7 (sub-wave 7): Spotify toolset — seven agent-facing tools
// sharing a pluggable SpotifyBackend (NullSpotifyBackend fails loud).
// Ported from the prior Wayland Python engine.
pub mod spotify_tool;
// T3-3.7 (sub-wave 7): homeassistant tool — smart-home control via a
// pluggable HomeAssistantBackend (NullHomeAssistantBackend fails loud).
// Ported from the prior Wayland Python engine.
pub mod homeassistant_tool;
// T3-3.8 (sub-wave 8): wayland self-introspection toolset
// (`wayland_status` + `wayland_telemetry_query`) ported from
// the prior Wayland Python engine.
// Pluggable WaylandIntrospectionBackend seam; NullBackend fails loud
// (NO-STUBS). Two tools share one backend so the
// `wayland_introspection` toolset disables as a unit.
pub mod wayland_introspection;
pub mod write;
// Unified workspace containment policy — filesystem + network scope for a
// session. Two presets: `trusted_local` (local CLI) and `contained` (remote
// Workspace posture). Secret predicate for `SecretDenyFs` integration.
pub mod workspace_policy;
pub use workspace_policy::{WorkspacePolicy, WorkspaceTrust};

pub use moa::{
    MoaError, MoaInput, MoaOutput, MoaTool, ProposerCaller, ProposerOutput, ProposerSpec,
};

use async_trait::async_trait;
use serde_json::Value;

use wcore_config::hooks::HooksConfig;
use wcore_protocol::events::ToolCategory;
use wcore_types::skill_types::ContextModifier;
use wcore_types::tool::{JsonSchema, ToolResult};

/// Truncate a string to at most `max_bytes`, snapping to a char boundary.
pub fn truncate_utf8(s: &str, max_bytes: usize) -> &str {
    if s.len() <= max_bytes {
        return s;
    }
    let mut end = max_bytes;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    &s[..end]
}

/// W7 F4 / W8a A.3: optional streaming sink threaded into tools that
/// implement `execute_streaming`. W8a adds `emit_progress` for tools
/// that report bounded progress (e.g. long Bash, Script DSL); the
/// default `Tool::execute_streaming` falls back to `execute()` so
/// existing tools are unaffected.
pub trait ToolOutputSink: Send + Sync {
    fn emit_chunk(&self, chunk: &str);

    /// W8a A.3: optional bounded-progress signal — percentage 0.0..=1.0
    /// paired with a human-readable message. Default is a no-op so
    /// existing sinks stay compatible.
    fn emit_progress(&self, _pct: f32, _message: &str) {}
}

/// W7 F4: pass-through no-op for tools that don't stream (or hosts that
/// haven't opted in via `streaming_tools`).
pub struct NullToolOutputSink;

impl ToolOutputSink for NullToolOutputSink {
    fn emit_chunk(&self, _chunk: &str) {}
}

/// A tool that the agent can invoke
#[async_trait]
pub trait Tool: Send + Sync {
    /// Tool name (must match API schema)
    fn name(&self) -> &str;

    /// Human-readable description for the LLM
    fn description(&self) -> &str;

    /// JSON Schema for input parameters
    fn input_schema(&self) -> JsonSchema;

    /// Whether this tool is safe to run concurrently
    fn is_concurrency_safe(&self, input: &Value) -> bool;

    /// Whether this tool has a real, callable backend wired in.
    ///
    /// External-service tools (web search, vision analysis, transcription,
    /// notion / gitlab / discord MCP, …) ship a `Null*Backend` default
    /// that fails every call with a "no backend configured" error. The
    /// host MUST upgrade these to a real backend before the model can
    /// actually use them — until then they should NOT be advertised in
    /// the tool list.
    ///
    /// Tools that override this method return `false` when their
    /// backend is null; [`ToolRegistry::register`] skips them silently
    /// so the model never sees a tool it cannot successfully call.
    ///
    /// Default: `true` (built-in tools — Bash, Read, Edit, etc. — never
    /// need a backend and are always available).
    fn is_available(&self) -> bool {
        true
    }

    /// Execute the tool
    async fn execute(&self, input: Value) -> ToolResult;

    /// W8a A.3: ctx-aware execute. Default fall-through calls
    /// `execute(input)` so the existing 26 `impl Tool for` sites stay
    /// green at this commit boundary. Tools migrate in W8a A.4 by
    /// overriding this method with vfs/cancel-aware bodies; the
    /// orchestration dispatcher routes through this entry point
    /// (replacing `execute(input)` direct calls) in the same A.4 commit
    /// so the call sites observe the per-tool context.
    async fn execute_with_ctx(&self, input: Value, _ctx: &context::ToolContext) -> ToolResult {
        self.execute(input).await
    }

    /// W7 F4: optional streaming variant. Default falls back to
    /// `execute()`. Only `BashTool` overrides this in W7. Tools that
    /// override this MUST also override `supports_streaming()` to
    /// return `true` so the engine dispatcher routes through the
    /// streaming path.
    async fn execute_streaming(&self, input: Value, _sink: &dyn ToolOutputSink) -> ToolResult {
        self.execute(input).await
    }

    /// W8a A.3: ctx + streaming-aware variant. Default delegates to
    /// `execute_streaming(input, sink)` so tools that don't yet
    /// observe `ctx.cancel` continue to work. Tools that need both
    /// the streaming sink AND cancellation override this (BashTool
    /// will in A.4).
    async fn execute_streaming_with_ctx(
        &self,
        input: Value,
        ctx: &context::ToolContext,
        sink: &dyn ToolOutputSink,
    ) -> ToolResult {
        let _ = ctx;
        self.execute_streaming(input, sink).await
    }

    /// W7 F4: whether this tool can emit `tool_chunk` events while
    /// running. Default false so existing tools opt out cleanly.
    fn supports_streaming(&self) -> bool {
        false
    }

    /// Return an optional context modifier based on the tool input.
    /// Called after execute() to collect any engine-level overrides.
    /// Only SkillTool overrides this; all other tools return None.
    fn context_modifier_for(&self, _input: &Value) -> Option<ContextModifier> {
        None
    }

    /// Return any hooks declared in the skill's frontmatter for dynamic registration.
    /// Called after a successful execute() so the orchestration layer can merge
    /// the returned hooks into the active `ShellHooks` (composed into
    /// the agent-level `HookEngine` in `wcore_agent::hooks`).
    /// Only SkillTool overrides this; all other tools return None.
    fn skill_hooks_for(&self, _input: &Value) -> Option<HooksConfig> {
        None
    }

    /// Max result size in chars before truncation
    fn max_result_size(&self) -> usize {
        50_000
    }

    /// Tool category for protocol classification
    fn category(&self) -> ToolCategory;

    /// Provenance: the MCP server this tool is sourced from, or `None` for a
    /// built-in / skill / spawn / plan tool.
    ///
    /// This is the REAL MCP-classification signal. A non-colliding MCP tool
    /// keeps its bare original name (no `mcp__{server}__` prefix — see
    /// `wcore-mcp/src/tool_proxy.rs`), so the name alone cannot tell it apart
    /// from a built-in. `McpToolProxy` overrides this to return `Some(server)`;
    /// every built-in falls through to `None`. Surfaced onto `ToolDef::server`
    /// by the registry so curation and the provider tool cap classify correctly.
    fn mcp_server(&self) -> Option<&str> {
        None
    }

    /// AUDIT B-1 follow-up — per-input category override.
    ///
    /// The dispatch-timeout (see `orchestration::tool_dispatch_timeout`)
    /// keys off `ToolCategory`. Some tools' category genuinely depends on
    /// the input — e.g. `SkillTool` is `Info` (30s) for an inline skill
    /// that just returns SKILL.md text, but `Exec` (600s) for a fork-mode
    /// skill that spawns a sub-agent and can legitimately run many turns.
    /// The orchestration dispatcher calls `category_for(&input)` (NOT the
    /// bare `category()`) so the timeout matches the actual work.
    ///
    /// Default implementation defers to `category()` so existing tools
    /// stay byte-identical. Tools that need per-input categorisation
    /// override this method and inspect `_input`.
    fn category_for(&self, _input: &Value) -> ToolCategory {
        self.category()
    }

    /// Whether this tool's schema should be deferred (sent as name-only stub).
    /// Override to `true` for tools with large schemas or infrequent use.
    fn is_deferred(&self) -> bool {
        false
    }

    /// Human-readable description of what the tool will do with the given input
    fn describe(&self, input: &Value) -> String {
        format!(
            "{}: {}",
            self.name(),
            serde_json::to_string(input).unwrap_or_default()
        )
    }
}

/// v0.6.4 Task 1.7 — blanket `Tool` impl for `Arc<T>` so an `Arc`-shared
/// tool can be registered into `ToolRegistry` (which takes `Box<dyn Tool>`)
/// via a plain `Box::new(arc)`.
///
/// Browser/cua plugin tools reify into `Arc<BrowserTool>` / `Arc<CuaTool>`
/// (they are `Arc`-shared on purpose — the cua/browser stacks hold their own
/// `Arc` handles). Without this impl, bootstrap could not hand them to
/// `ToolRegistry::register`. Every method delegates to the inner `T`.
#[async_trait]
impl<T: Tool + ?Sized> Tool for std::sync::Arc<T> {
    fn name(&self) -> &str {
        (**self).name()
    }
    fn description(&self) -> &str {
        (**self).description()
    }
    fn input_schema(&self) -> JsonSchema {
        (**self).input_schema()
    }
    fn is_concurrency_safe(&self, input: &Value) -> bool {
        (**self).is_concurrency_safe(input)
    }
    async fn execute(&self, input: Value) -> ToolResult {
        (**self).execute(input).await
    }
    async fn execute_with_ctx(&self, input: Value, ctx: &context::ToolContext) -> ToolResult {
        (**self).execute_with_ctx(input, ctx).await
    }
    async fn execute_streaming(&self, input: Value, sink: &dyn ToolOutputSink) -> ToolResult {
        (**self).execute_streaming(input, sink).await
    }
    async fn execute_streaming_with_ctx(
        &self,
        input: Value,
        ctx: &context::ToolContext,
        sink: &dyn ToolOutputSink,
    ) -> ToolResult {
        (**self).execute_streaming_with_ctx(input, ctx, sink).await
    }
    fn supports_streaming(&self) -> bool {
        (**self).supports_streaming()
    }
    fn context_modifier_for(&self, input: &Value) -> Option<ContextModifier> {
        (**self).context_modifier_for(input)
    }
    fn skill_hooks_for(&self, input: &Value) -> Option<HooksConfig> {
        (**self).skill_hooks_for(input)
    }
    fn max_result_size(&self) -> usize {
        (**self).max_result_size()
    }
    fn category(&self) -> ToolCategory {
        (**self).category()
    }
    fn mcp_server(&self) -> Option<&str> {
        (**self).mcp_server()
    }
    fn category_for(&self, input: &Value) -> ToolCategory {
        (**self).category_for(input)
    }
    fn is_deferred(&self) -> bool {
        (**self).is_deferred()
    }
    fn describe(&self, input: &Value) -> String {
        (**self).describe(input)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn truncate_utf8_ascii_within_limit() {
        assert_eq!(truncate_utf8("hello", 80), "hello");
    }

    #[test]
    fn truncate_utf8_ascii_at_boundary() {
        assert_eq!(truncate_utf8("abcde", 3), "abc");
    }

    #[test]
    fn truncate_utf8_multibyte_snaps_back() {
        // '些' is 3 bytes (E4 BA 9B) starting at index 79 would span 79..82
        let s = "# 用 script 模拟 TTY 交互来添加 DeepSeek 提供商\n# 首先看看有哪些";
        let result = truncate_utf8(s, 80);
        assert!(result.len() <= 80);
        assert!(result.is_char_boundary(result.len()));
    }

    #[test]
    fn truncate_utf8_empty() {
        assert_eq!(truncate_utf8("", 80), "");
    }

    #[test]
    fn truncate_utf8_zero_limit() {
        assert_eq!(truncate_utf8("hello", 0), "");
    }

    #[test]
    fn truncate_utf8_emoji() {
        // 🦀 is 4 bytes
        let s = "aaa🦀bbb";
        assert_eq!(truncate_utf8(s, 4), "aaa");
        assert_eq!(truncate_utf8(s, 7), "aaa🦀");
    }
}
