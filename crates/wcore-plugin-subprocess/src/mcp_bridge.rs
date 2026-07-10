//! v0.6.5 Task 3.4 — MCP-server-as-plugin auto-adapter.
//!
//! Spawns a Model Context Protocol (MCP) server binary as a subprocess and
//! synthesizes one [`PluginTool`] per discovered MCP tool. Each synthesized
//! tool's execute closure forwards `tools/call` back into the MCP server
//! over the same stdio transport.
//!
//! ## Why this exists
//!
//! v0.6.5 v0 — "instant-catalog of the MCP ecosystem". When operators drop
//! an MCP-bridge plugin manifest in `~/.local/share/wayland/plugins/`, the
//! engine spawns the configured MCP server, lists its tools, and registers
//! them as first-class Wayland plugin tools — no per-server adapter code
//! required.
//!
//! ## Wire protocol
//!
//! JSON-RPC 2.0 over JSON-Lines on stdin/stdout (this is canonical MCP-stdio
//! framing — every JSON-RPC message ends with `\n`). We reuse
//! [`wcore_mcp::protocol::JsonRpcRequest`] / [`JsonRpcResponse`] verbatim
//! rather than the subprocess-SDK's custom [`crate::rpc::SubprocessVerb`]
//! envelope; the bridge speaks MCP all the way down so any conformant MCP
//! server works without modification.
//!
//! ## Permission model — cross-audit M9
//!
//! The original plan text claimed MCP-bridge plugins inherit "the engine's
//! existing MCP permission gate." That gate, as a per-call thing, **does
//! not exist** inside `wcore-mcp/src/{manager,tool_proxy}.rs`. The actual
//! gating that MCP-bridge plugins reuse is:
//!
//! 1. [`PluginPermissions::register_mcp_server`] — manifest-level boolean
//!    that decides whether the plugin is *allowed* to expose an MCP server
//!    at all. The host's plugin loader (Task 2.7) refuses to launch this
//!    runner if the manifest does not set it.
//! 2. [`PluginPermissions::tool_namespace`] — the `"<namespace>::<tool>"`
//!    prefix every synthesized tool is registered under. Forced by
//!    [`PluginManifest::validate`] when `register_tools = true`.
//! 3. `ScopedToolRegistry` namespace ledger — registered host-side on
//!    plugin load; prevents two plugins from claiming the same
//!    `"<ns>::<name>"` pair.
//!
//! Subprocess privilege inheritance from Task 3.2 still applies: the MCP
//! server runs with the engine's uid / env / filesystem reach (cross-audit
//! A7). True isolation is a containerization concern, out of scope here.
//!
//! ## Loader boundary
//!
//! This module **does not** wire itself into the engine's manifest
//! dispatch. When the plugin loader (Task 2.7's worktree) sees
//! `[runtime] kind = "mcp-bridge"`, it dispatches to
//! [`McpBridgePluginRunner::load`] (or [`McpBridgePluginRunner::load_with_transport`]
//! in tests) and folds the resulting [`LoadedMcpBridgePlugin::tools`] into
//! the engine's `InitializeOutcome.tools` surface alongside any
//! statically-linked plugin tools. The two surfaces are interchangeable —
//! a host-delegated MCP-bridge tool looks identical to an in-process
//! `PluginTool` from the `apply.rs` carrier's perspective.
//!
//! ## Testing
//!
//! `tests/mcp_bridge_e2e.rs` drives [`McpBridgePluginRunner::load_with_transport`]
//! through a `tokio::io::duplex` pair. The fixture half answers `initialize`,
//! `tools/list`, and `tools/call` requests with canonical MCP JSON-RPC
//! envelopes. No real MCP-server binary is required.

use std::collections::HashMap;
use std::path::Path;
use std::process::Stdio;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, BufReader};
use tokio::process::{Child, Command};
use tokio::sync::{Mutex, oneshot};
use tokio::task::JoinHandle;
use tokio::time::{Duration, timeout};
use tracing::{debug, error, info, warn};
use wcore_mcp::protocol::{
    ClientCapabilities, ClientInfo, InitializeParams, JsonRpcRequest, JsonRpcResponse, McpToolDef,
    ToolsListResult,
};
use wcore_plugin_api::access_gate::PluginAccessGate;
use wcore_plugin_api::manifest::PluginManifest;
use wcore_plugin_api::tool::{PluginTool, PluginToolInvocation};
use wcore_protocol::events::ToolCategory;
use wcore_types::tool::ToolResult;

use crate::error::{Result, SubprocessPluginError};

/// Default per-RPC timeout for MCP requests routed through the bridge.
/// Mirrors `runner.rs`'s budget so users see consistent semantics.
const DEFAULT_RPC_TIMEOUT: Duration = Duration::from_secs(30);

/// Grace period for the MCP server to exit cleanly after stdin close.
const SHUTDOWN_GRACE: Duration = Duration::from_secs(5);

/// Security/reliability cap on a single newline-delimited line read from an
/// untrusted MCP server's stdout (audit `rel-panic-68`/M-21). A third-party
/// MCP server streaming an endless newline-free payload would otherwise grow
/// the read buffer until the host OOMs. On overflow the reader drops the
/// connection (drains pending callers → `WorkerTerminated`). 8 MiB is far
/// above any legitimate JSON-RPC envelope.
const MAX_LINE_BYTES: u64 = 8 * 1024 * 1024;

/// MCP protocol version we advertise during `initialize`. Matches what
/// `wcore-mcp`'s in-engine `McpManager` already sends to upstream servers.
const MCP_PROTOCOL_VERSION: &str = "2025-03-26";

/// Minimal env vars forwarded to MCP-bridge child processes after
/// [`std::process::Command::env_clear`]. Everything else — including
/// `OPENAI_API_KEY`, `WAYLAND_VAULT_*`, `ANTHROPIC_*`, etc. — is withheld
/// (`WAYLAND_HOME` is an intentional exception below, forwarded so the child
/// resolves the same isolated profile — C3; the vault secret stays withheld).
/// Kept minimal: just enough for CLI tools to locate executables and
/// behave correctly under different locales on every supported OS.
///
/// Windows entries are mandatory — without `SYSTEMROOT`/`WINDIR`/etc. the
/// spawned child cannot initialise (CreateProcess returns
/// `ERROR_ENVVAR_NOT_FOUND` / 0xcb, observed v0.8.6 round 17). Mirrors
/// the same list in `wcore_mcp::transport::stdio::FORWARDED_ENV_VARS` and
/// `wcore_plugin_subprocess::runner::FORWARDED_ENV_VARS` — keep all three
/// in sync when adding vars.
const FORWARDED_ENV_VARS: &[&str] = &[
    // Unix essentials
    "PATH",
    "HOME",
    "USER",
    "LANG",
    "TZ",
    "LC_ALL",
    "LC_CTYPE",
    "LC_MESSAGES",
    "LC_MONETARY",
    "LC_NUMERIC",
    "LC_TIME",
    "TMPDIR",
    // C3: the isolated-profile home. Engine-controlled children must resolve the
    // SAME profile as the parent — without this they fall back to the default
    // ~/.wayland (cross-profile leak). Non-secret path; the vault passphrase
    // (WAYLAND_VAULT_*) is never forwarded.
    "WAYLAND_HOME",
    // Windows essentials
    "SYSTEMROOT",
    "WINDIR",
    "COMSPEC",
    "PATHEXT",
    "PROCESSOR_ARCHITECTURE",
    "USERPROFILE",
    "APPDATA",
    "LOCALAPPDATA",
    "PROGRAMFILES",
    "PROGRAMFILES(X86)",
    "PSMODULEPATH",
    "TEMP",
    "TMP",
];

/// Result of [`McpBridgePluginRunner::load`] — the runner plus the
/// synthesized [`PluginTool`] list the host's apply pipeline registers.
pub struct LoadedMcpBridgePlugin {
    runner: Arc<McpBridgePluginRunner>,
    /// One [`PluginTool`] per MCP tool the upstream server advertised.
    tools: Vec<PluginTool>,
}

impl LoadedMcpBridgePlugin {
    /// Synthesized [`PluginTool`] entries. The host's apply pipeline
    /// (`wcore-agent::plugins::apply`) registers these alongside any
    /// statically-linked plugin tools — there is no MCP-bridge-specific
    /// branch in the apply path.
    pub fn tools(&self) -> &[PluginTool] {
        &self.tools
    }

    /// Number of synthesized tools — convenience for the host loader.
    pub fn tool_count(&self) -> usize {
        self.tools.len()
    }

    /// Owning handle to the underlying runner. Held by the host loader for
    /// the plugin's lifetime; dropping it kills the MCP server subprocess.
    pub fn runner(&self) -> Arc<McpBridgePluginRunner> {
        Arc::clone(&self.runner)
    }

    /// Test seam — pull the runner + tools apart so tests can call
    /// [`McpBridgePluginRunner::shutdown`] explicitly.
    #[doc(hidden)]
    pub fn into_parts(self) -> (Arc<McpBridgePluginRunner>, Vec<PluginTool>) {
        (self.runner, self.tools)
    }
}

/// Channel-based async handle to a spawned MCP server.
///
/// Architectural twin of [`crate::runner::SubprocessPluginRunner`] but
/// speaks JSON-RPC (MCP) instead of the subprocess SDK's
/// [`crate::rpc::SubprocessVerb`] envelope. Both runners share the same
/// reader-task / pending-map / stdin-mutex shape — see `runner.rs` for the
/// rationale (single in-flight map, oneshot per request, drop on EOF).
pub struct McpBridgePluginRunner {
    next_id: AtomicU64,
    pending: Arc<Mutex<HashMap<u64, oneshot::Sender<JsonRpcResponse>>>>,
    stdin: Mutex<Box<dyn AsyncWrite + Send + Unpin>>,
    reader_task: Mutex<Option<JoinHandle<()>>>,
    child: Mutex<Option<Child>>,
    _gate: Arc<PluginAccessGate>,
}

impl McpBridgePluginRunner {
    /// Spawn the MCP server binary declared at
    /// `manifest.runtime.subprocess.binary_path` (resolved against the
    /// directory containing `manifest_path`), perform the canonical MCP
    /// initialize → notifications/initialized → tools/list handshake, and
    /// return a [`LoadedMcpBridgePlugin`] carrying one synthesized
    /// [`PluginTool`] per discovered MCP tool.
    ///
    /// Note: the manifest re-uses the `[runtime.subprocess]` block for
    /// binary + args — declaring a second redundant
    /// `[runtime.mcp-bridge]` field would double-source the spawn config.
    /// `PluginRuntimeMcpBridge` (currently `server_url`) is reserved for
    /// future remote-MCP-bridge variants; this implementation is the
    /// stdio-bridge variant.
    ///
    /// Aud-18 (verify-vs-execute TOCTOU): see
    /// [`crate::runner::resolve_verified_binary`]. When the loader passes a
    /// signature-verified `verified_binary`, this runner spawns that exact path
    /// (after asserting the manifest-derived path still canonicalizes to it)
    /// instead of independently re-deriving and spawning, closing the gap where
    /// the spawned artifact could differ from the signed one.
    pub async fn load(
        manifest_path: &Path,
        manifest: &PluginManifest,
        gate: Arc<PluginAccessGate>,
        verified_binary: Option<&Path>,
    ) -> Result<LoadedMcpBridgePlugin> {
        let binary_rel = manifest
            .runtime
            .as_ref()
            .and_then(|r| r.subprocess.as_ref())
            .and_then(|s| s.binary_path.as_ref())
            .ok_or_else(|| {
                SubprocessPluginError::RpcParse(format!(
                    "plugin {}: mcp-bridge manifest is missing \
                     [runtime.subprocess].binary_path",
                    manifest.plugin.name
                ))
            })?;
        let plugin_dir = manifest_path.parent().ok_or_else(|| {
            SubprocessPluginError::RpcParse(format!(
                "plugin {}: manifest_path has no parent directory",
                manifest.plugin.name
            ))
        })?;
        let binary = crate::runner::resolve_verified_binary(
            &manifest.plugin.name,
            plugin_dir,
            binary_rel,
            verified_binary,
        )?;
        let args: Vec<String> = manifest
            .runtime
            .as_ref()
            .and_then(|r| r.subprocess.as_ref())
            .map(|s| s.args.clone())
            .unwrap_or_default();

        let mut cmd = Command::new(&binary);
        cmd.args(&args)
            .env_clear()
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .kill_on_drop(true);
        for var in FORWARDED_ENV_VARS {
            if let Ok(val) = std::env::var(var) {
                cmd.env(var, val);
            }
        }
        info!(
            plugin = %manifest.plugin.name,
            binary = %binary.display(),
            forwarded_env = ?FORWARDED_ENV_VARS,
            "mcp-bridge subprocess spawning with cleared env"
        );
        let mut child = cmd.spawn().map_err(SubprocessPluginError::SpawnFailed)?;

        let stdin = child
            .stdin
            .take()
            .ok_or(SubprocessPluginError::BrokenPipe)?;
        let stdout = child
            .stdout
            .take()
            .ok_or(SubprocessPluginError::BrokenPipe)?;

        Self::handshake_over_transport(
            Box::new(stdin),
            Box::new(stdout),
            gate,
            Some(child),
            &binary.display().to_string(),
        )
        .await
    }

    /// Test seam — drive the runner over an arbitrary async transport
    /// (`tokio::io::duplex()`). Used by `tests/mcp_bridge_e2e.rs` to
    /// exercise the JSON-RPC handshake / call lifecycle without a real
    /// MCP-server binary.
    #[doc(hidden)]
    pub async fn load_with_transport<W, R>(
        stdin: W,
        stdout: R,
        gate: Arc<PluginAccessGate>,
    ) -> Result<LoadedMcpBridgePlugin>
    where
        W: AsyncWrite + Send + Unpin + 'static,
        R: AsyncRead + Send + Unpin + 'static,
    {
        Self::handshake_over_transport(Box::new(stdin), Box::new(stdout), gate, None, "<duplex>")
            .await
    }

    async fn handshake_over_transport(
        stdin: Box<dyn AsyncWrite + Send + Unpin>,
        stdout: Box<dyn AsyncRead + Send + Unpin>,
        gate: Arc<PluginAccessGate>,
        child: Option<Child>,
        binary_for_logs: &str,
    ) -> Result<LoadedMcpBridgePlugin> {
        let pending: Arc<Mutex<HashMap<u64, oneshot::Sender<JsonRpcResponse>>>> =
            Arc::new(Mutex::new(HashMap::new()));
        let reader_pending = Arc::clone(&pending);
        let binary_display = binary_for_logs.to_string();

        let reader_task = tokio::spawn(async move {
            let mut reader = BufReader::new(stdout);
            let mut line: Vec<u8> = Vec::new();
            loop {
                line.clear();
                // Byte-capped read (audit rel-panic-68/M-21): a hostile/buggy
                // MCP server streaming an endless newline-free payload would
                // grow an unbounded `read_line` buffer until the host OOMs.
                // Cap the read at MAX_LINE_BYTES; on an over-length line (cap
                // hit with no terminating newline) drop the transport so
                // pending callers get WorkerTerminated.
                match (&mut reader)
                    .take(MAX_LINE_BYTES)
                    .read_until(b'\n', &mut line)
                    .await
                {
                    Ok(0) => {
                        debug!(binary = %binary_display, "mcp-bridge stdout closed");
                        break;
                    }
                    Ok(_) => {
                        let over_length =
                            line.len() as u64 >= MAX_LINE_BYTES && line.last() != Some(&b'\n');
                        if over_length {
                            error!(
                                binary = %binary_display,
                                cap_bytes = MAX_LINE_BYTES,
                                "mcp-bridge server sent over-length line (no newline within cap) — \
                                 dropping transport"
                            );
                            break;
                        }
                        let line_str = String::from_utf8_lossy(&line);
                        let trimmed = line_str.trim_end_matches(['\n', '\r']);
                        if trimmed.is_empty() {
                            continue;
                        }
                        match serde_json::from_str::<JsonRpcResponse>(trimmed) {
                            Ok(resp) => match resp.id {
                                Some(id) => {
                                    let mut guard = reader_pending.lock().await;
                                    match guard.remove(&id) {
                                        Some(tx) => {
                                            let _ = tx.send(resp);
                                        }
                                        None => {
                                            warn!(
                                                id,
                                                "mcp-bridge response for unknown id (dropping)"
                                            );
                                        }
                                    }
                                }
                                None => {
                                    // Notification — MCP server side-channel
                                    // chatter. Out-of-scope for v0.6.5; log
                                    // and drop. Future: route to a
                                    // per-server notification handler.
                                    debug!(
                                        line = %trimmed,
                                        "mcp-bridge notification ignored (no id)"
                                    );
                                }
                            },
                            Err(e) => {
                                error!(
                                    error = %e, line = %trimmed,
                                    "mcp-bridge server sent unparseable line"
                                );
                            }
                        }
                    }
                    Err(e) => {
                        error!(error = %e, "mcp-bridge stdout read error");
                        break;
                    }
                }
            }
            // On EOF / error: drain pending senders so callers get
            // a typed WorkerTerminated error.
            let mut guard = reader_pending.lock().await;
            guard.clear();
        });

        let runner = Arc::new(McpBridgePluginRunner {
            next_id: AtomicU64::new(1),
            pending,
            stdin: Mutex::new(stdin),
            reader_task: Mutex::new(Some(reader_task)),
            child: Mutex::new(child),
            _gate: gate,
        });

        // 1. initialize handshake.
        let init_params = InitializeParams {
            protocol_version: MCP_PROTOCOL_VERSION.to_string(),
            capabilities: ClientCapabilities {
                tools: Some(json!({})),
            },
            client_info: ClientInfo {
                name: "wayland-mcp-bridge".to_string(),
                version: env!("CARGO_PKG_VERSION").to_string(),
            },
        };
        let init_params_value = serde_json::to_value(&init_params).map_err(|e| {
            SubprocessPluginError::RpcParse(format!("serialize InitializeParams: {e}"))
        })?;
        let init_resp = runner
            .send_request("initialize", Some(init_params_value))
            .await?;
        // Result presence is enough — we don't currently consume any
        // capability flag from the InitializeResult here. The engine-level
        // `McpManager` parses `capabilities.resources` for skill bridges;
        // synthesized PluginTools don't expose resources, so this bridge
        // intentionally ignores that field.
        if init_resp.result.is_none() {
            return Err(SubprocessPluginError::ProtocolError {
                code: "mcp_initialize_no_result".to_string(),
                message: "MCP server initialize returned no result".to_string(),
            });
        }

        // 2. notifications/initialized — fire-and-forget per JSON-RPC notif
        // semantics. The MCP spec requires this before any further request.
        runner
            .send_notification("notifications/initialized", None)
            .await?;

        // 3. tools/list.
        let list_resp = runner.send_request("tools/list", None).await?;
        let list_result_value =
            list_resp
                .result
                .ok_or_else(|| SubprocessPluginError::ProtocolError {
                    code: "mcp_tools_list_no_result".to_string(),
                    message: "MCP server tools/list returned no result".to_string(),
                })?;
        let tools_list: ToolsListResult = serde_json::from_value(list_result_value)
            .map_err(|e| SubprocessPluginError::RpcParse(format!("parse ToolsListResult: {e}")))?;

        // 4. Synthesize one PluginTool per discovered MCP tool. Each tool's
        // execute closure captures an `Arc<McpBridgePluginRunner>` so the
        // closure outlives the load call and can fire `tools/call` back
        // through the same transport for the plugin's whole lifetime.
        let tools = tools_list
            .tools
            .into_iter()
            .map(|def| synthesize_plugin_tool(def, Arc::clone(&runner)))
            .collect::<Vec<_>>();

        Ok(LoadedMcpBridgePlugin { runner, tools })
    }

    /// Send a JSON-RPC request line, await the response matching its id.
    async fn send_request(&self, method: &str, params: Option<Value>) -> Result<JsonRpcResponse> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let req = JsonRpcRequest::new(id, method, params);
        let line = serde_json::to_string(&req)
            .map_err(|e| SubprocessPluginError::RpcParse(e.to_string()))?;

        let (tx, rx) = oneshot::channel();
        {
            let mut guard = self.pending.lock().await;
            guard.insert(id, tx);
        }

        {
            let mut stdin = self.stdin.lock().await;
            if let Err(e) = stdin.write_all(line.as_bytes()).await {
                self.pending.lock().await.remove(&id);
                return Err(map_io_err(e));
            }
            if let Err(e) = stdin.write_all(b"\n").await {
                self.pending.lock().await.remove(&id);
                return Err(map_io_err(e));
            }
            if let Err(e) = stdin.flush().await {
                self.pending.lock().await.remove(&id);
                return Err(map_io_err(e));
            }
        }

        match timeout(DEFAULT_RPC_TIMEOUT, rx).await {
            Ok(Ok(resp)) => {
                if let Some(err) = resp.error {
                    return Err(SubprocessPluginError::ProtocolError {
                        code: err.code.to_string(),
                        message: err.message,
                    });
                }
                Ok(resp)
            }
            Ok(Err(_)) => Err(SubprocessPluginError::WorkerTerminated),
            Err(_) => {
                self.pending.lock().await.remove(&id);
                Err(SubprocessPluginError::Timeout)
            }
        }
    }

    /// Send a JSON-RPC notification line (no `id`, no response expected).
    async fn send_notification(&self, method: &str, params: Option<Value>) -> Result<()> {
        let req = JsonRpcRequest::notification(method, params);
        let line = serde_json::to_string(&req)
            .map_err(|e| SubprocessPluginError::RpcParse(e.to_string()))?;
        let mut stdin = self.stdin.lock().await;
        stdin.write_all(line.as_bytes()).await.map_err(map_io_err)?;
        stdin.write_all(b"\n").await.map_err(map_io_err)?;
        stdin.flush().await.map_err(map_io_err)?;
        Ok(())
    }

    /// Invoke an MCP tool by name through the bridge. Synthesized
    /// `PluginTool::execute` closures dispatch through here.
    pub async fn call_mcp_tool(&self, name: &str, input: Value) -> Result<ToolOutput> {
        let params = json!({
            "name": name,
            "arguments": input,
        });
        let resp = self.send_request("tools/call", Some(params)).await?;
        let result_value = resp
            .result
            .ok_or_else(|| SubprocessPluginError::ProtocolError {
                code: "mcp_tools_call_no_result".to_string(),
                message: format!("MCP server tools/call for '{name}' returned no result"),
            })?;

        // Concatenate text content; flag non-text content shapes for
        // operator visibility but keep them in the stream so the model can
        // still see them. is_error is read from the optional MCP
        // `isError` field — newer MCP servers set it on domain-level failures.
        let is_error = result_value
            .get("isError")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        let content = result_value
            .get("content")
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default();
        let mut text_parts: Vec<String> = Vec::with_capacity(content.len());
        for item in &content {
            match item.get("type").and_then(|t| t.as_str()) {
                Some("text") => {
                    if let Some(t) = item.get("text").and_then(|t| t.as_str()) {
                        text_parts.push(t.to_string());
                    }
                }
                Some("image") => {
                    let mime = item
                        .get("mimeType")
                        .and_then(|m| m.as_str())
                        .unwrap_or("application/octet-stream");
                    text_parts.push(format!("[image: {mime}]"));
                }
                Some("resource") => {
                    text_parts.push("[resource]".to_string());
                }
                Some(other) => {
                    text_parts.push(format!("[unknown content type: {other}]"));
                }
                None => {}
            }
        }

        Ok(ToolOutput {
            stdout: text_parts.join("\n"),
            structured: Some(result_value),
            is_error,
        })
    }

    /// Best-effort shutdown — closes stdin, waits up to [`SHUTDOWN_GRACE`]
    /// for the child to exit, then SIGKILL. Idempotent.
    pub async fn shutdown(&self) -> Result<()> {
        // Closing stdin signals MCP servers to exit per the spec.
        // We don't have a way to drop just the inner Box here without
        // taking ownership; instead we reap the child with grace.
        let mut child_guard = self.child.lock().await;
        if let Some(mut child) = child_guard.take() {
            // Try a graceful wait first — the server may exit on its own.
            match timeout(SHUTDOWN_GRACE, child.wait()).await {
                Ok(Ok(_)) => {}
                Ok(Err(e)) => warn!(error = %e, "mcp-bridge child wait failed"),
                Err(_) => {
                    warn!("mcp-bridge child did not exit within grace — SIGKILL");
                    let _ = child.kill().await;
                    let _ = child.wait().await;
                }
            }
        }

        let mut reader_guard = self.reader_task.lock().await;
        if let Some(handle) = reader_guard.take() {
            match timeout(Duration::from_secs(1), handle).await {
                Ok(Ok(_)) => {}
                Ok(Err(e)) => warn!(error = %e, "mcp-bridge reader join error"),
                Err(_) => warn!("mcp-bridge reader did not finish — leaking"),
            }
        }
        Ok(())
    }
}

/// Output of a synthesized [`PluginTool`] backed by an MCP tool. Mirrors
/// the shape of [`crate::runner::ToolOutput`] so the host loader can treat
/// MCP-bridge and subprocess-SDK plugins identically.
#[derive(Debug, Clone, PartialEq)]
pub struct ToolOutput {
    pub stdout: String,
    pub structured: Option<Value>,
    pub is_error: bool,
}

impl Drop for McpBridgePluginRunner {
    fn drop(&mut self) {
        // `kill_on_drop(true)` on the spawned Command reaps the MCP
        // server process; the reader task exits naturally on stdout EOF.
    }
}

/// Build a [`PluginTool`] that forwards execution into the given runner's
/// MCP `tools/call` channel.
fn synthesize_plugin_tool(def: McpToolDef, runner: Arc<McpBridgePluginRunner>) -> PluginTool {
    let tool_name = def.name.clone();
    let description = def
        .description
        .clone()
        .unwrap_or_else(|| format!("MCP tool `{}` (synthesized by mcp-bridge plugin)", def.name));
    let input_schema = def.input_schema.clone();

    PluginTool {
        name: def.name,
        description,
        input_schema,
        category: ToolCategory::Mcp,
        is_deferred: false,
        max_result_size: 50_000,
        execute: Arc::new(move |inv: PluginToolInvocation| {
            let runner = Arc::clone(&runner);
            let name = tool_name.clone();
            Box::pin(async move {
                match runner.call_mcp_tool(&name, inv.input).await {
                    Ok(out) => ToolResult {
                        content: out.stdout,
                        is_error: out.is_error,
                    },
                    Err(e) => ToolResult {
                        content: format!("mcp-bridge tool `{name}` failed: {e}"),
                        is_error: true,
                    },
                }
            })
        }),
    }
}

fn map_io_err(e: std::io::Error) -> SubprocessPluginError {
    match e.kind() {
        std::io::ErrorKind::BrokenPipe | std::io::ErrorKind::UnexpectedEof => {
            SubprocessPluginError::BrokenPipe
        }
        _ => SubprocessPluginError::RpcParse(e.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::FORWARDED_ENV_VARS;

    /// env_clear() — secret vars must NOT reach the MCP-bridge child process.
    ///
    /// The `load()` path calls `env_clear()` then re-forwards only the
    /// `FORWARDED_ENV_VARS` allowlist. This test directly exercises the
    /// Command builder logic by constructing an equivalent command and
    /// asserting the child does not inherit secret vars.
    #[cfg(unix)]
    #[tokio::test]
    #[serial_test::serial]
    async fn mcp_bridge_env_clear_blocks_secret_vars() {
        use tokio::io::AsyncReadExt;
        use tokio::process::Command;

        let secret_var = "WAYLAND_MCP_TEST_SECRET";
        let secret_val = "mcp-should-not-leak";
        // The environ table is process-global; under `cargo test` all tests in
        // this binary share one process, so mutating it while another test
        // reads/writes env is a data race (unsafe in edition 2024). `#[serial]`
        // serializes every env-mutating test in this binary.
        // SAFETY: `#[serial]` guarantees no other `#[serial]` test mutates the
        // environment concurrently.
        unsafe { std::env::set_var(secret_var, secret_val) };

        let mut cmd = Command::new("/bin/sh");
        cmd.args(["-c", &format!("printf '%s' \"${secret_var}\"")])
            .env_clear()
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .kill_on_drop(true);
        for var in FORWARDED_ENV_VARS {
            if let Ok(val) = std::env::var(var) {
                cmd.env(var, val);
            }
        }

        let mut child = cmd.spawn().expect("spawn /bin/sh failed");
        let mut stdout_buf = String::new();
        if let Some(mut out) = child.stdout.take() {
            out.read_to_string(&mut stdout_buf).await.unwrap();
        }
        let _ = child.wait().await;

        assert!(
            stdout_buf.is_empty(),
            "mcp-bridge child saw secret var: {stdout_buf:?}"
        );

        // SAFETY: single-threaded test context; no concurrent env mutation.
        unsafe { std::env::remove_var(secret_var) };
    }

    #[test]
    fn mcp_bridge_forwarded_env_vars_contains_expected_minimum() {
        assert!(FORWARDED_ENV_VARS.contains(&"PATH"));
        assert!(FORWARDED_ENV_VARS.contains(&"HOME"));
        assert!(FORWARDED_ENV_VARS.contains(&"USER"));
        assert!(FORWARDED_ENV_VARS.contains(&"LANG"));
        // C3 profile propagation.
        assert!(FORWARDED_ENV_VARS.contains(&"WAYLAND_HOME"));
    }

    /// Audit rel-panic-68/M-21: a hostile MCP server that streams an endless
    /// newline-free payload must NOT OOM the host. The reader caps each line
    /// at `MAX_LINE_BYTES`; on an over-length line it drops the transport,
    /// which drains pending senders so the in-flight `initialize` handshake
    /// fails with `WorkerTerminated` instead of buffering forever.
    #[tokio::test]
    async fn mcp_bridge_overlong_line_fails_handshake_not_oom() {
        use std::sync::Arc;
        use tokio::io::{AsyncWriteExt, duplex};
        use wcore_plugin_api::access_gate::PluginAccessGate;

        use crate::error::SubprocessPluginError;
        use crate::mcp_bridge::McpBridgePluginRunner;

        // host -> plugin (stdin); plugin -> host (stdout).
        let (host_to_plugin_w, _host_to_plugin_r) = duplex(8192);
        let (mut plugin_to_host_w, plugin_to_host_r) = duplex(64 * 1024);

        // Fake plugin: stream > MAX_LINE_BYTES of newline-free bytes as the
        // first "response", never emitting a complete line.
        let fixture = tokio::spawn(async move {
            let chunk = vec![b'a'; 256 * 1024];
            let mut written: u64 = 0;
            while written <= super::MAX_LINE_BYTES {
                if plugin_to_host_w.write_all(&chunk).await.is_err() {
                    break;
                }
                written += chunk.len() as u64;
            }
            let _ = plugin_to_host_w.flush().await;
        });

        let gate = Arc::new(PluginAccessGate);
        let result = tokio::time::timeout(
            std::time::Duration::from_secs(10),
            McpBridgePluginRunner::load_with_transport(host_to_plugin_w, plugin_to_host_r, gate),
        )
        .await
        .expect("load_with_transport hung — over-length line was not capped");

        match result {
            Err(SubprocessPluginError::WorkerTerminated) => {}
            // Note: the Ok arm is matched separately so this assertion does
            // not require `LoadedMcpBridgePlugin: Debug` (its `Arc<Runner>`
            // wraps non-Debug subprocess handles).
            Ok(_) => panic!(
                "expected WorkerTerminated on over-length line, got Ok(LoadedMcpBridgePlugin)"
            ),
            Err(other) => {
                panic!("expected WorkerTerminated on over-length line, got {other:?}")
            }
        }
        let _ = fixture.await;
    }
}
