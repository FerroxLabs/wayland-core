//! T3-3.7 — Discord server introspection / management tool.
//!
//! Ported from an upstream MIT-licensed library (see THIRD-PARTY-NOTICES.md).
//! The Python original talks to Discord's REST API
//! directly (urllib + bot token). Wayland's engine MUST NOT initiate
//! HTTP from inside `wcore-tools` (HTTP is a `wcore-providers` /
//! plugin / host concern), so this port covers the **dispatch surface
//! only**: schema, action manifest, per-action required-parameter
//! validation, 403 → human-readable enrichment, and a pluggable
//! `DiscordBackend` boundary that the host wires to a real REST
//! client (or any other transport) at construction time.
//!
//! Without a backend bound, `execute()` returns a structured error
//! ("no discord backend configured for action <name>") rather than a
//! silent stub — honoring the NO-STUBS contract of T3.
//!
//! Divergences from the Python original (intentional):
//! * No live capability detection (`GET /applications/@me` for
//!   privileged intents) — that's a host concern. Hosts that filter
//!   actions by detected intents can supply a custom allowlist via
//!   `DiscordTool::with_allowed_actions`.
//! * No user-config YAML reading — the engine receives a pre-resolved
//!   allowlist from the host.
//! * No registry side-effect / `requires_env` declaration — that
//!   wiring lives in the host's tool-loading layer.
//! * Pure-data 403 enrichment: the backend returns
//!   `DiscordOutcome::Forbidden { body }` and this module maps it to
//!   the same human-readable hints the Python `_enrich_403` produces.

use std::collections::BTreeSet;
use std::sync::Arc;

use async_trait::async_trait;
use serde_json::{Value, json};

use wcore_protocol::events::ToolCategory;
use wcore_types::tool::{JsonSchema, ToolResult};

use crate::Tool;
use crate::url_safety::is_safe_url;

/// Canonical Discord action set. Order is preserved in the schema
/// description and the `_available_actions` filter so the model sees
/// a stable manifest.
pub const DISCORD_ACTIONS: &[&str] = &[
    "list_guilds",
    "server_info",
    "list_channels",
    "channel_info",
    "list_roles",
    "member_info",
    "search_members",
    "fetch_messages",
    "list_pins",
    "pin_message",
    "unpin_message",
    "create_thread",
    "add_role",
    "remove_role",
];

/// Action manifest entry — `(name, signature, one-line description)`.
/// Lifted verbatim from the Python `_ACTION_MANIFEST` so prompts and
/// examples that reference this surface stay accurate.
pub const DISCORD_ACTION_MANIFEST: &[(&str, &str, &str)] = &[
    ("list_guilds", "()", "list servers the bot is in"),
    (
        "server_info",
        "(guild_id)",
        "server details + member counts",
    ),
    (
        "list_channels",
        "(guild_id)",
        "all channels grouped by category",
    ),
    ("channel_info", "(channel_id)", "single channel details"),
    ("list_roles", "(guild_id)", "roles sorted by position"),
    (
        "member_info",
        "(guild_id, user_id)",
        "lookup a specific member",
    ),
    (
        "search_members",
        "(guild_id, query)",
        "find members by name prefix",
    ),
    (
        "fetch_messages",
        "(channel_id)",
        "recent messages; optional before/after snowflakes",
    ),
    ("list_pins", "(channel_id)", "pinned messages in a channel"),
    ("pin_message", "(channel_id, message_id)", "pin a message"),
    (
        "unpin_message",
        "(channel_id, message_id)",
        "unpin a message",
    ),
    (
        "create_thread",
        "(channel_id, name)",
        "create a public thread; optional message_id anchor",
    ),
    ("add_role", "(guild_id, user_id, role_id)", "assign a role"),
    (
        "remove_role",
        "(guild_id, user_id, role_id)",
        "remove a role",
    ),
];

/// Required-parameter manifest. Mirrors the Python `_REQUIRED_PARAMS`
/// table; missing values trigger a structured error before the
/// backend is invoked.
fn required_params_for(action: &str) -> &'static [&'static str] {
    match action {
        "server_info" => &["guild_id"],
        "list_channels" => &["guild_id"],
        "list_roles" => &["guild_id"],
        "member_info" => &["guild_id", "user_id"],
        "search_members" => &["guild_id", "query"],
        "channel_info" => &["channel_id"],
        "fetch_messages" => &["channel_id"],
        "list_pins" => &["channel_id"],
        "pin_message" => &["channel_id", "message_id"],
        "unpin_message" => &["channel_id", "message_id"],
        "create_thread" => &["channel_id", "name"],
        "add_role" => &["guild_id", "user_id", "role_id"],
        "remove_role" => &["guild_id", "user_id", "role_id"],
        _ => &[],
    }
}

/// 403-hint table ported 1:1 from the Python `_ACTION_403_HINT`.
fn forbidden_hint(action: &str) -> Option<&'static str> {
    match action {
        "pin_message" => Some(
            "Bot lacks MANAGE_MESSAGES permission in this channel. \
             Ask the server admin to grant the bot a role that has MANAGE_MESSAGES, \
             or a per-channel overwrite.",
        ),
        "unpin_message" => Some("Bot lacks MANAGE_MESSAGES permission in this channel."),
        "create_thread" => {
            Some("Bot lacks CREATE_PUBLIC_THREADS in this channel, or cannot view it.")
        }
        "add_role" => Some(
            "Either the bot lacks MANAGE_ROLES, or the target role sits higher \
             than the bot's highest role. Roles can only be assigned below the \
             bot's own position in the role hierarchy.",
        ),
        "remove_role" => Some(
            "Either the bot lacks MANAGE_ROLES, or the target role sits higher \
             than the bot's highest role.",
        ),
        "fetch_messages" => {
            Some("Bot cannot view this channel (missing VIEW_CHANNEL or READ_MESSAGE_HISTORY).")
        }
        "list_pins" => {
            Some("Bot cannot view this channel (missing VIEW_CHANNEL or READ_MESSAGE_HISTORY).")
        }
        "channel_info" => Some("Bot cannot view this channel (missing VIEW_CHANNEL)."),
        "search_members" => Some(
            "Likely missing the Server Members privileged intent — enable it in the \
             Discord Developer Portal under your bot's settings.",
        ),
        "member_info" => Some(
            "Bot cannot see this guild member (missing Server Members intent or \
             insufficient permissions).",
        ),
        _ => None,
    }
}

/// Map a Discord 403 body to the same enriched guidance the Python
/// `_enrich_403` returns.
pub fn enrich_forbidden(action: &str, body: &str) -> String {
    let base = format!("Discord API 403 (forbidden) on '{action}'.");
    match forbidden_hint(action) {
        Some(hint) => format!("{base} {hint} (Raw: {body})"),
        None => format!("{base} (Raw: {body})"),
    }
}

/// Parsed call into the backend — exactly the fields the Python
/// dispatch passes down, normalized into a struct so backends see a
/// stable, typed surface instead of free-form JSON.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiscordCall {
    pub action: String,
    pub guild_id: String,
    pub channel_id: String,
    pub user_id: String,
    pub role_id: String,
    pub message_id: String,
    pub query: String,
    pub name: String,
    pub limit: u32,
    pub before: String,
    pub after: String,
    pub auto_archive_duration: u32,
}

impl DiscordCall {
    /// Parse a JSON args object into a `DiscordCall`. Returns the
    /// rejection reason on missing/invalid `action`.
    pub fn parse(input: &Value) -> Result<Self, String> {
        let action = match input.get("action").and_then(Value::as_str) {
            Some(s) if !s.trim().is_empty() => s.trim().to_string(),
            _ => return Err("Missing required parameter: 'action'".to_string()),
        };
        Ok(Self {
            action,
            guild_id: str_field(input, "guild_id"),
            channel_id: str_field(input, "channel_id"),
            user_id: str_field(input, "user_id"),
            role_id: str_field(input, "role_id"),
            message_id: str_field(input, "message_id"),
            query: str_field(input, "query"),
            name: str_field(input, "name"),
            limit: input
                .get("limit")
                .and_then(Value::as_u64)
                .map(|n| n.min(100) as u32)
                .unwrap_or(50),
            before: str_field(input, "before"),
            after: str_field(input, "after"),
            auto_archive_duration: input
                .get("auto_archive_duration")
                .and_then(Value::as_u64)
                .map(|n| n as u32)
                .unwrap_or(1440),
        })
    }
}

fn str_field(input: &Value, key: &str) -> String {
    input
        .get(key)
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string()
}

/// Outcome of a backend call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DiscordOutcome {
    /// Success — `payload` is the JSON the engine returns to the model
    /// verbatim. Backends should preserve the field shape used by the
    /// Python original (`{"guilds": [...], "count": N}` etc.) so prompts
    /// continue to work unchanged.
    Ok { payload: Value },
    /// Discord returned 403 Forbidden — `body` is the raw response body
    /// and the engine will run it through `enrich_forbidden`.
    Forbidden { body: String },
    /// Any other failure path (network, 4xx other than 403, 5xx,
    /// auth missing). The message is surfaced to the model verbatim
    /// inside an `{"error": ...}` envelope.
    Err { message: String },
}

/// Host-supplied Discord backend. The engine never speaks HTTP; the
/// host implements this trait (typically wrapping a `reqwest` or
/// `hyper` client + bot token) and binds it at registration time.
#[async_trait]
pub trait DiscordBackend: Send + Sync {
    /// Execute `call` against Discord. The backend is responsible for
    /// token resolution, request signing, retries, and translating
    /// REST responses into the `DiscordOutcome` variants.
    async fn dispatch(&self, call: &DiscordCall) -> DiscordOutcome;
}

/// Default backend returned when the host wires nothing — every
/// `dispatch()` fails loudly with a "no backend configured" error so
/// the tool never appears to succeed silently.
pub struct NullDiscordBackend;

#[async_trait]
impl DiscordBackend for NullDiscordBackend {
    async fn dispatch(&self, call: &DiscordCall) -> DiscordOutcome {
        DiscordOutcome::Err {
            message: format!(
                "No discord backend configured for action '{}'. Wire a DiscordBackend \
                 implementation when constructing DiscordTool.",
                call.action
            ),
        }
    }
}

/// In-memory backend that records every dispatch for test assertions.
/// Returns deterministic synthetic JSON shaped to mimic the real
/// Discord REST responses for the most common actions, so tests can
/// exercise the full pipeline (parse → validate → dispatch → render).
#[derive(Default)]
pub struct CapturingDiscordBackend {
    pub captured: parking_lot::Mutex<Vec<DiscordCall>>,
}

impl CapturingDiscordBackend {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn snapshot(&self) -> Vec<DiscordCall> {
        self.captured.lock().clone()
    }
}

#[async_trait]
impl DiscordBackend for CapturingDiscordBackend {
    async fn dispatch(&self, call: &DiscordCall) -> DiscordOutcome {
        self.captured.lock().push(call.clone());
        let payload = match call.action.as_str() {
            "list_guilds" => json!({"guilds": [], "count": 0}),
            "server_info" => json!({"id": call.guild_id, "name": "synthetic"}),
            "list_channels" => json!({"channel_groups": [], "total_channels": 0}),
            "list_pins" => json!({"pinned_messages": [], "count": 0}),
            "pin_message" => json!({"success": true, "message": "pinned"}),
            "unpin_message" => json!({"success": true, "message": "unpinned"}),
            "create_thread" => json!({
                "success": true,
                "thread_id": "1",
                "name": call.name,
            }),
            _ => json!({"ok": true, "action": call.action}),
        };
        DiscordOutcome::Ok { payload }
    }
}

/// `discord_server` tool — Wayland engine port of `discord_tool.py`.
pub struct DiscordTool {
    backend: Arc<dyn DiscordBackend>,
    /// Optional pre-resolved allowlist of action names. `None` =
    /// every canonical action is exposed. `Some(set)` (possibly empty)
    /// restricts dispatch and schema enumeration to that set.
    allowed: Option<BTreeSet<String>>,
    description: String,
    schema: JsonSchema,
    /// v0.9.0 W1 B4: defaults `false` so `Tool::is_available()` hides
    /// the tool when no real backend wired. `new(backend)` flips it on.
    backend_configured: bool,
}

impl Default for DiscordTool {
    fn default() -> Self {
        let actions: Vec<&'static str> = DISCORD_ACTIONS.to_vec();
        let description = build_description(&actions);
        let schema = build_schema(&actions);
        Self {
            backend: Arc::new(NullDiscordBackend),
            allowed: None,
            description,
            schema,
            backend_configured: false,
        }
    }
}

impl DiscordTool {
    pub fn new(backend: Arc<dyn DiscordBackend>) -> Self {
        let actions: Vec<&'static str> = DISCORD_ACTIONS.to_vec();
        let description = build_description(&actions);
        let schema = build_schema(&actions);
        Self {
            backend,
            allowed: None,
            description,
            schema,
            backend_configured: true,
        }
    }

    /// Restrict the visible/dispatchable action set. Unknown action
    /// names are silently dropped (matches Python's
    /// `_load_allowed_actions_config` behaviour where invalid entries
    /// are logged and ignored).
    pub fn with_allowed_actions<I, S>(mut self, actions: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        let canonical: BTreeSet<&'static str> = DISCORD_ACTIONS.iter().copied().collect();
        let allowed: BTreeSet<String> = actions
            .into_iter()
            .filter_map(|s| {
                let s = s.as_ref().to_string();
                if canonical.contains(s.as_str()) {
                    Some(s)
                } else {
                    None
                }
            })
            .collect();
        // Schema/description preserves canonical order, filtered to allowed.
        let filtered: Vec<&'static str> = DISCORD_ACTIONS
            .iter()
            .copied()
            .filter(|a| allowed.contains(*a))
            .collect();
        self.description = build_description(&filtered);
        self.schema = build_schema(&filtered);
        self.allowed = Some(allowed);
        self
    }

    fn is_allowed(&self, action: &str) -> bool {
        match &self.allowed {
            None => DISCORD_ACTIONS.contains(&action),
            Some(set) => set.contains(action),
        }
    }
}

fn build_description(actions: &[&str]) -> String {
    let manifest: Vec<String> = DISCORD_ACTION_MANIFEST
        .iter()
        .filter(|(name, _, _)| actions.contains(name))
        .map(|(name, sig, desc)| format!("  {name}{sig}  — {desc}"))
        .collect();
    format!(
        "Query and manage a Discord server via the REST API.\n\n\
         Available actions:\n{}\n\n\
         Call list_guilds first to discover guild_ids, then list_channels for \
         channel_ids. Runtime errors will tell you if the bot lacks a specific \
         per-guild permission (e.g. MANAGE_ROLES for add_role).",
        manifest.join("\n")
    )
}

fn build_schema(actions: &[&str]) -> JsonSchema {
    // If the caller filtered down to zero, still expose the canonical
    // enum so the schema is valid JSON Schema (an empty enum is
    // rejected by some providers). The dispatcher will reject any
    // call with a structured error anyway.
    let enum_actions: Vec<&str> = if actions.is_empty() {
        DISCORD_ACTIONS.to_vec()
    } else {
        actions.to_vec()
    };
    json!({
        "type": "object",
        "properties": {
            "action": {
                "type": "string",
                "enum": enum_actions,
            },
            "guild_id": {"type": "string", "description": "Discord server (guild) ID."},
            "channel_id": {"type": "string", "description": "Discord channel ID."},
            "user_id": {"type": "string", "description": "Discord user ID."},
            "role_id": {"type": "string", "description": "Discord role ID."},
            "message_id": {"type": "string", "description": "Discord message ID."},
            "query": {
                "type": "string",
                "description": "Member name prefix to search for (search_members)."
            },
            "name": {
                "type": "string",
                "description": "New thread name (create_thread)."
            },
            "limit": {
                "type": "integer",
                "minimum": 1,
                "maximum": 100,
                "description": "Max results (default 50). Applies to fetch_messages, search_members."
            },
            "before": {
                "type": "string",
                "description": "Snowflake ID for reverse pagination (fetch_messages)."
            },
            "after": {
                "type": "string",
                "description": "Snowflake ID for forward pagination (fetch_messages)."
            },
            "auto_archive_duration": {
                "type": "integer",
                "enum": [60, 1440, 4320, 10080],
                "description": "Thread archive duration in minutes (create_thread, default 1440)."
            }
        },
        "required": ["action"]
    })
}

#[async_trait]
impl Tool for DiscordTool {
    fn name(&self) -> &str {
        "discord_server"
    }

    /// v0.9.0 W1 B4: hidden when no real `DiscordBackend` is wired.
    /// `Default::default()` yields `backend_configured == false`, so
    /// `ToolRegistry::register` drops the tool before the model sees it.
    fn is_available(&self) -> bool {
        self.backend_configured
    }

    fn description(&self) -> &str {
        &self.description
    }

    fn input_schema(&self) -> JsonSchema {
        self.schema.clone()
    }

    fn is_concurrency_safe(&self, input: &Value) -> bool {
        // Read-only inspection actions are concurrency-safe; mutation
        // actions (pin/unpin/create_thread/add_role/remove_role) are not.
        let action = input.get("action").and_then(Value::as_str).unwrap_or("");
        matches!(
            action,
            "list_guilds"
                | "server_info"
                | "list_channels"
                | "channel_info"
                | "list_roles"
                | "member_info"
                | "search_members"
                | "fetch_messages"
                | "list_pins"
        )
    }

    fn category(&self) -> ToolCategory {
        // Includes mutating actions (pin/role/thread). Categorize as
        // Exec so hosts that gate side-effecting tools behind approval
        // catch this tool too. The dispatcher honours per-action
        // concurrency via `is_concurrency_safe`.
        ToolCategory::Exec
    }

    async fn execute(&self, input: Value) -> ToolResult {
        let call = match DiscordCall::parse(&input) {
            Ok(c) => c,
            Err(e) => {
                return ToolResult {
                    content: json!({"error": e}).to_string(),
                    is_error: true,
                };
            }
        };

        if !self.is_allowed(&call.action) {
            // Distinguish "not a real action" from "disabled by host".
            let known = DISCORD_ACTIONS.contains(&call.action.as_str());
            let payload = if known {
                json!({
                    "error": format!(
                        "Action '{}' is disabled by host allowlist. Allowed: {}",
                        call.action,
                        allowed_list_display(&self.allowed),
                    ),
                })
            } else {
                json!({
                    "error": format!("Unknown action: {}", call.action),
                    "available_actions": DISCORD_ACTIONS,
                })
            };
            return ToolResult {
                content: payload.to_string(),
                is_error: true,
            };
        }

        let missing: Vec<&str> = required_params_for(&call.action)
            .iter()
            .copied()
            .filter(|p| field_value(&call, p).is_empty())
            .collect();
        if !missing.is_empty() {
            return ToolResult {
                content: json!({
                    "error": format!(
                        "Missing required parameters for '{}': {}",
                        call.action,
                        missing.join(", ")
                    )
                })
                .to_string(),
                is_error: true,
            };
        }

        // Defensive URL safety check: if a future caller smuggles a
        // URL into `name` or `query`, refuse to forward it to the
        // backend when it points at private/metadata space. (Snowflake
        // IDs are not URLs, so safe fields pass through trivially.)
        for field in ["name", "query"] {
            let v = field_value(&call, field);
            let is_url = v.starts_with("http://") || v.starts_with("https://");
            if is_url && !is_safe_url(v) {
                return ToolResult {
                    content: json!({
                        "error": format!(
                            "Rejected unsafe URL in '{}' (private/metadata target).",
                            field
                        )
                    })
                    .to_string(),
                    is_error: true,
                };
            }
        }

        match self.backend.dispatch(&call).await {
            DiscordOutcome::Ok { payload } => ToolResult {
                content: payload.to_string(),
                is_error: false,
            },
            DiscordOutcome::Forbidden { body } => ToolResult {
                content: json!({"error": enrich_forbidden(&call.action, &body)}).to_string(),
                is_error: true,
            },
            DiscordOutcome::Err { message } => ToolResult {
                content: json!({"error": message}).to_string(),
                is_error: true,
            },
        }
    }
}

fn field_value<'a>(call: &'a DiscordCall, field: &str) -> &'a str {
    match field {
        "guild_id" => &call.guild_id,
        "channel_id" => &call.channel_id,
        "user_id" => &call.user_id,
        "role_id" => &call.role_id,
        "message_id" => &call.message_id,
        "query" => &call.query,
        "name" => &call.name,
        _ => "",
    }
}

fn allowed_list_display(allowed: &Option<BTreeSet<String>>) -> String {
    match allowed {
        None => "<all>".to_string(),
        Some(set) if set.is_empty() => "<none>".to_string(),
        Some(set) => set.iter().cloned().collect::<Vec<_>>().join(", "),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(t: &DiscordTool, args: Value) -> ToolResult {
        futures::executor::block_on(t.execute(args))
    }

    #[test]
    fn dispatch_records_call_and_returns_payload() {
        let backend = Arc::new(CapturingDiscordBackend::new());
        let tool = DiscordTool::new(backend.clone());
        let res = run(&tool, json!({"action": "list_guilds"}));
        assert!(!res.is_error, "expected ok, got: {}", res.content);
        let calls = backend.snapshot();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].action, "list_guilds");
        assert!(res.content.contains("\"guilds\""));
    }

    #[test]
    fn missing_required_params_short_circuits_before_backend() {
        let backend = Arc::new(CapturingDiscordBackend::new());
        let tool = DiscordTool::new(backend.clone());
        // pin_message requires channel_id + message_id.
        let res = run(&tool, json!({"action": "pin_message"}));
        assert!(res.is_error);
        assert!(res.content.contains("Missing required parameters"));
        assert!(res.content.contains("channel_id"));
        assert!(res.content.contains("message_id"));
        assert!(
            backend.snapshot().is_empty(),
            "backend must not be called when params are missing"
        );
    }

    #[test]
    fn unknown_action_returns_structured_error() {
        let tool = DiscordTool::new(Arc::new(CapturingDiscordBackend::new()));
        let res = run(&tool, json!({"action": "delete_server"}));
        assert!(res.is_error);
        assert!(res.content.contains("Unknown action"));
        assert!(res.content.contains("list_guilds"));
    }

    #[test]
    fn null_backend_fails_loud_no_silent_stub() {
        let tool = DiscordTool::default();
        let res = run(&tool, json!({"action": "list_guilds"}));
        assert!(res.is_error);
        assert!(
            res.content.contains("No discord backend configured"),
            "expected fail-loud, got: {}",
            res.content
        );
    }

    #[test]
    fn forbidden_outcome_is_enriched_with_actionable_hint() {
        struct ForbiddenBackend;
        #[async_trait]
        impl DiscordBackend for ForbiddenBackend {
            async fn dispatch(&self, _call: &DiscordCall) -> DiscordOutcome {
                DiscordOutcome::Forbidden {
                    body: r#"{"message":"Missing Permissions","code":50013}"#.to_string(),
                }
            }
        }
        let tool = DiscordTool::new(Arc::new(ForbiddenBackend));
        let res = run(
            &tool,
            json!({
                "action": "add_role",
                "guild_id": "1",
                "user_id": "2",
                "role_id": "3"
            }),
        );
        assert!(res.is_error);
        assert!(res.content.contains("Discord API 403"));
        assert!(res.content.contains("MANAGE_ROLES"));
        assert!(res.content.contains("Missing Permissions"));
    }

    #[test]
    fn host_allowlist_blocks_disabled_actions() {
        let backend = Arc::new(CapturingDiscordBackend::new());
        let tool =
            DiscordTool::new(backend.clone()).with_allowed_actions(["list_guilds", "server_info"]);
        // Allowed action passes through.
        let ok = run(&tool, json!({"action": "list_guilds"}));
        assert!(!ok.is_error);
        // Mutating action is blocked.
        let res = run(
            &tool,
            json!({
                "action": "pin_message",
                "channel_id": "1",
                "message_id": "2"
            }),
        );
        assert!(res.is_error);
        assert!(res.content.contains("disabled by host allowlist"));
        assert!(res.content.contains("list_guilds"));
        // Only the allowed call reached the backend.
        let calls = backend.snapshot();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].action, "list_guilds");
    }

    #[test]
    fn concurrency_safety_distinguishes_read_vs_mutate() {
        let tool = DiscordTool::default();
        assert!(tool.is_concurrency_safe(&json!({"action": "list_guilds"})));
        assert!(tool.is_concurrency_safe(&json!({"action": "fetch_messages"})));
        assert!(!tool.is_concurrency_safe(&json!({"action": "pin_message"})));
        assert!(!tool.is_concurrency_safe(&json!({"action": "create_thread"})));
        assert!(!tool.is_concurrency_safe(&json!({"action": "add_role"})));
    }

    #[test]
    fn unsafe_url_in_query_field_is_rejected_before_backend() {
        let backend = Arc::new(CapturingDiscordBackend::new());
        let tool = DiscordTool::new(backend.clone());
        // 169.254.169.254 is the AWS instance-metadata address — must
        // never be forwarded to a backend even via a `query` field.
        let res = run(
            &tool,
            json!({
                "action": "search_members",
                "guild_id": "1",
                "query": "http://169.254.169.254/latest/meta-data/"
            }),
        );
        assert!(res.is_error, "expected rejection, got: {}", res.content);
        assert!(res.content.contains("Rejected unsafe URL"));
        assert!(
            backend.snapshot().is_empty(),
            "backend must not see the unsafe URL"
        );
    }

    #[test]
    fn enrich_forbidden_falls_back_when_no_hint() {
        let s = enrich_forbidden("list_guilds", "raw-body");
        assert!(s.contains("Discord API 403"));
        assert!(s.contains("raw-body"));
        assert!(!s.contains("MANAGE_"));
    }

    #[test]
    fn schema_filters_actions_under_allowlist() {
        let tool = DiscordTool::default().with_allowed_actions(["list_guilds", "server_info"]);
        let schema = tool.input_schema();
        let actions = schema
            .pointer("/properties/action/enum")
            .and_then(Value::as_array)
            .expect("enum present");
        let names: Vec<&str> = actions.iter().filter_map(Value::as_str).collect();
        assert_eq!(names, ["list_guilds", "server_info"]);
        // Description manifest is filtered too.
        assert!(tool.description().contains("list_guilds"));
        assert!(!tool.description().contains("pin_message"));
    }
}
