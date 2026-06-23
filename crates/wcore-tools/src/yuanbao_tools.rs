//! T3-3.7 — `yuanbao_tools`: Tencent Yuanbao platform toolset.
//!
//! Ported from an upstream MIT-licensed library (see THIRD-PARTY-NOTICES.md). The Python original registers five separate
//! tools — `yb_query_group_info`, `yb_query_group_members`,
//! `yb_search_sticker`, `yb_send_sticker`, `yb_send_dm` — against a
//! gateway-side Yuanbao adapter (`gateway.platforms.yuanbao`).
//!
//! Wayland's engine has no Yuanbao adapter and no built-in sticker
//! catalogue. This port mirrors the **dispatch surface** of all five
//! operations behind a single `YuanbaoTool` with an `action` discriminator
//! and routes them through a pluggable `YuanbaoBackend` trait that the
//! host (CLI / Electron / gateway sidecar) wires at construction time.
//!
//! With no backend wired the tool returns the same structured
//! `"Yuanbao adapter is not connected"` error the Python emits — honoring
//! the NO-STUBS contract of T3 (fails loudly rather than silently).
//!
//! Divergences from the Python original (intentional):
//!
//!   * Five separate registry entries collapse into one engine
//!     `YuanbaoTool` with an `action` field. Schema parity holds:
//!     every required parameter the Python tools declared is required
//!     here for the matching action.
//!   * No `WAYLAND_SESSION_CHAT_ID` / `WAYLAND_SESSION_PLATFORM`
//!     fallback — the engine has no session-env shim. Hosts that need
//!     it pass `chat_id` explicitly via the tool input.
//!   * Sticker catalogue lookup (`yuanbao_sticker.search_stickers` /
//!     `get_sticker_by_id` / `get_sticker_by_name`) is delegated to
//!     the backend: the engine has no opinion on what stickers exist.
//!   * `extract_media(message)` (MEDIA:<path> tag parsing) is delegated
//!     to the backend — that lives in `gateway.platforms.base` and is
//!     a Yuanbao-adapter concern, not an engine concern.

use std::sync::Arc;

use async_trait::async_trait;
use parking_lot::Mutex;
use serde_json::{Value, json};

use wcore_protocol::events::ToolCategory;
use wcore_types::tool::{JsonSchema, ToolResult};

use crate::Tool;

// ---------------------------------------------------------------------------
// Role labels (mirrors _USER_TYPE_LABEL in the Python source)
// ---------------------------------------------------------------------------

pub const MENTION_HINT: &str = "To @mention a user, you MUST use the format: \
    space + @ + nickname + space (e.g. \" @Alice \").";

/// Translate a numeric Yuanbao user_type into a role label.
///
/// Matches the Python `_USER_TYPE_LABEL` table:
/// `0 -> "unknown"`, `1 -> "user"`, `2 -> "yuanbao_ai"`, `3 -> "bot"`.
pub fn user_type_label(user_type: i64) -> &'static str {
    match user_type {
        1 => "user",
        2 => "yuanbao_ai",
        3 => "bot",
        _ => "unknown",
    }
}

// ---------------------------------------------------------------------------
// Backend types
// ---------------------------------------------------------------------------

/// Yuanbao group member record. Mirrors the fields the Python tool
/// surfaces from `adapter.get_group_member_list()`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct YuanbaoMember {
    pub user_id: String,
    pub nickname: String,
    /// Already-translated role label ("user" / "yuanbao_ai" / "bot" /
    /// "unknown") — backends may compute this from the raw numeric
    /// `user_type` via `user_type_label`.
    pub role: String,
}

/// Basic group info — name, owner, member count.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct YuanbaoGroupInfo {
    pub group_name: String,
    pub member_count: u64,
    pub owner_id: String,
    pub owner_nickname: String,
}

/// Sticker catalogue entry. Mirrors the fields the Python tool returns
/// from `yuanbao_sticker.search_stickers()`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct YuanbaoSticker {
    pub sticker_id: String,
    pub name: String,
    pub description: String,
    pub package_id: String,
}

/// Outcome of a send attempt (sticker / dm / media). Mirrors the
/// `result.success / .error / .message_id` shape used by the Python
/// `adapter.send_*` return values.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum YuanbaoSendOutcome {
    Ok { message_id: Option<String> },
    Err { message: String },
}

/// Media-file dispatch descriptor. Matches the
/// `list[tuple[str, bool]]` shape used by `send_dm(media_files=...)`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct YuanbaoMedia {
    pub path: String,
    pub is_voice: bool,
}

// ---------------------------------------------------------------------------
// YuanbaoBackend trait
// ---------------------------------------------------------------------------

/// Host-supplied Yuanbao adapter boundary. Mirrors the methods the
/// Python tool invokes against `gateway.platforms.yuanbao.get_active_adapter()`.
/// The engine never talks to Yuanbao directly; the host implements
/// this trait and binds it at registration time.
#[async_trait]
pub trait YuanbaoBackend: Send + Sync {
    /// True when a live Yuanbao session is connected. When false,
    /// every action returns the standard "adapter is not connected"
    /// error without touching the rest of the trait.
    async fn is_connected(&self) -> bool;

    /// `adapter.query_group_info(group_code)` — returns None when the
    /// group cannot be resolved.
    async fn query_group_info(&self, group_code: &str) -> Result<Option<YuanbaoGroupInfo>, String>;

    /// `adapter.get_group_member_list(group_code)` — returns the raw
    /// member list. None signals the adapter returned None (the
    /// Python `"get_group_member_list returned None"` branch).
    async fn list_group_members(
        &self,
        group_code: &str,
    ) -> Result<Option<Vec<YuanbaoMember>>, String>;

    /// `yuanbao_sticker.search_stickers(query, limit=...)`.
    async fn search_stickers(&self, query: &str, limit: u32)
    -> Result<Vec<YuanbaoSticker>, String>;

    /// `yuanbao_sticker.get_sticker_by_id(id)` — None when not found.
    async fn sticker_by_id(&self, sticker_id: &str) -> Result<Option<YuanbaoSticker>, String>;

    /// `yuanbao_sticker.get_sticker_by_name(name)` — None when not found.
    async fn sticker_by_name(&self, name: &str) -> Result<Option<YuanbaoSticker>, String>;

    /// `yuanbao_sticker.get_random_sticker()` — None when the catalogue
    /// is empty.
    async fn random_sticker(&self) -> Result<Option<YuanbaoSticker>, String>;

    /// `adapter.send_sticker(chat_id, sticker_name, reply_to)`.
    async fn send_sticker(
        &self,
        chat_id: &str,
        sticker_name: &str,
        reply_to: Option<&str>,
    ) -> YuanbaoSendOutcome;

    /// `adapter.send_dm(user_id, message, group_code=...)`.
    async fn send_dm_text(
        &self,
        user_id: &str,
        message: &str,
        group_code: &str,
    ) -> YuanbaoSendOutcome;

    /// `adapter.send_image_file(chat_id, path, group_code=...)`.
    async fn send_image_file(
        &self,
        chat_id: &str,
        path: &str,
        group_code: &str,
    ) -> YuanbaoSendOutcome;

    /// `adapter.send_document(chat_id, path, group_code=...)`.
    async fn send_document(
        &self,
        chat_id: &str,
        path: &str,
        group_code: &str,
    ) -> YuanbaoSendOutcome;
}

/// Default backend returned when the host wires nothing. Every action
/// surfaces the standard "Yuanbao adapter is not connected" error.
pub struct NullYuanbaoBackend;

#[async_trait]
impl YuanbaoBackend for NullYuanbaoBackend {
    async fn is_connected(&self) -> bool {
        false
    }
    async fn query_group_info(&self, _: &str) -> Result<Option<YuanbaoGroupInfo>, String> {
        Err("Yuanbao adapter is not connected".to_string())
    }
    async fn list_group_members(&self, _: &str) -> Result<Option<Vec<YuanbaoMember>>, String> {
        Err("Yuanbao adapter is not connected".to_string())
    }
    async fn search_stickers(&self, _: &str, _: u32) -> Result<Vec<YuanbaoSticker>, String> {
        Err("Yuanbao adapter is not connected".to_string())
    }
    async fn sticker_by_id(&self, _: &str) -> Result<Option<YuanbaoSticker>, String> {
        Err("Yuanbao adapter is not connected".to_string())
    }
    async fn sticker_by_name(&self, _: &str) -> Result<Option<YuanbaoSticker>, String> {
        Err("Yuanbao adapter is not connected".to_string())
    }
    async fn random_sticker(&self) -> Result<Option<YuanbaoSticker>, String> {
        Err("Yuanbao adapter is not connected".to_string())
    }
    async fn send_sticker(&self, _: &str, _: &str, _: Option<&str>) -> YuanbaoSendOutcome {
        YuanbaoSendOutcome::Err {
            message: "Yuanbao adapter is not connected".to_string(),
        }
    }
    async fn send_dm_text(&self, _: &str, _: &str, _: &str) -> YuanbaoSendOutcome {
        YuanbaoSendOutcome::Err {
            message: "Yuanbao adapter is not connected".to_string(),
        }
    }
    async fn send_image_file(&self, _: &str, _: &str, _: &str) -> YuanbaoSendOutcome {
        YuanbaoSendOutcome::Err {
            message: "Yuanbao adapter is not connected".to_string(),
        }
    }
    async fn send_document(&self, _: &str, _: &str, _: &str) -> YuanbaoSendOutcome {
        YuanbaoSendOutcome::Err {
            message: "Yuanbao adapter is not connected".to_string(),
        }
    }
}

// ---------------------------------------------------------------------------
// CapturingYuanbaoBackend — test harness
// ---------------------------------------------------------------------------

/// Recorded backend call. Test-only consumers inspect `captured` after
/// running the tool.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum YuanbaoCall {
    QueryGroupInfo {
        group_code: String,
    },
    ListGroupMembers {
        group_code: String,
    },
    SearchStickers {
        query: String,
        limit: u32,
    },
    StickerById {
        sticker_id: String,
    },
    StickerByName {
        name: String,
    },
    RandomSticker,
    SendSticker {
        chat_id: String,
        sticker_name: String,
        reply_to: Option<String>,
    },
    SendDmText {
        user_id: String,
        message: String,
        group_code: String,
    },
    SendImageFile {
        chat_id: String,
        path: String,
        group_code: String,
    },
    SendDocument {
        chat_id: String,
        path: String,
        group_code: String,
    },
}

/// In-memory backend that records every call and returns canned
/// fixtures. Lives in the prod module so downstream crates can reuse
/// it without depending on `#[cfg(test)]` symbols.
#[derive(Default)]
pub struct CapturingYuanbaoBackend {
    pub calls: Mutex<Vec<YuanbaoCall>>,
    pub group_info: Mutex<Option<YuanbaoGroupInfo>>,
    pub members: Mutex<Option<Vec<YuanbaoMember>>>,
    pub stickers: Mutex<Vec<YuanbaoSticker>>,
    pub connected: Mutex<bool>,
}

impl CapturingYuanbaoBackend {
    pub fn new() -> Self {
        Self {
            connected: Mutex::new(true),
            ..Self::default()
        }
    }
    pub fn snapshot(&self) -> Vec<YuanbaoCall> {
        self.calls.lock().clone()
    }
    pub fn set_group_info(&self, info: YuanbaoGroupInfo) {
        *self.group_info.lock() = Some(info);
    }
    pub fn set_members(&self, members: Vec<YuanbaoMember>) {
        *self.members.lock() = Some(members);
    }
    pub fn set_stickers(&self, stickers: Vec<YuanbaoSticker>) {
        *self.stickers.lock() = stickers;
    }
    pub fn set_connected(&self, connected: bool) {
        *self.connected.lock() = connected;
    }
}

#[async_trait]
impl YuanbaoBackend for CapturingYuanbaoBackend {
    async fn is_connected(&self) -> bool {
        *self.connected.lock()
    }
    async fn query_group_info(&self, group_code: &str) -> Result<Option<YuanbaoGroupInfo>, String> {
        self.calls.lock().push(YuanbaoCall::QueryGroupInfo {
            group_code: group_code.to_string(),
        });
        Ok(self.group_info.lock().clone())
    }
    async fn list_group_members(
        &self,
        group_code: &str,
    ) -> Result<Option<Vec<YuanbaoMember>>, String> {
        self.calls.lock().push(YuanbaoCall::ListGroupMembers {
            group_code: group_code.to_string(),
        });
        Ok(self.members.lock().clone())
    }
    async fn search_stickers(
        &self,
        query: &str,
        limit: u32,
    ) -> Result<Vec<YuanbaoSticker>, String> {
        self.calls.lock().push(YuanbaoCall::SearchStickers {
            query: query.to_string(),
            limit,
        });
        let stickers = self.stickers.lock().clone();
        let filtered: Vec<_> = if query.is_empty() {
            stickers.into_iter().take(limit as usize).collect()
        } else {
            let q = query.to_lowercase();
            stickers
                .into_iter()
                .filter(|s| {
                    s.name.to_lowercase().contains(&q) || s.description.to_lowercase().contains(&q)
                })
                .take(limit as usize)
                .collect()
        };
        Ok(filtered)
    }
    async fn sticker_by_id(&self, sticker_id: &str) -> Result<Option<YuanbaoSticker>, String> {
        self.calls.lock().push(YuanbaoCall::StickerById {
            sticker_id: sticker_id.to_string(),
        });
        Ok(self
            .stickers
            .lock()
            .iter()
            .find(|s| s.sticker_id == sticker_id)
            .cloned())
    }
    async fn sticker_by_name(&self, name: &str) -> Result<Option<YuanbaoSticker>, String> {
        self.calls.lock().push(YuanbaoCall::StickerByName {
            name: name.to_string(),
        });
        Ok(self
            .stickers
            .lock()
            .iter()
            .find(|s| s.name == name)
            .cloned())
    }
    async fn random_sticker(&self) -> Result<Option<YuanbaoSticker>, String> {
        self.calls.lock().push(YuanbaoCall::RandomSticker);
        Ok(self.stickers.lock().first().cloned())
    }
    async fn send_sticker(
        &self,
        chat_id: &str,
        sticker_name: &str,
        reply_to: Option<&str>,
    ) -> YuanbaoSendOutcome {
        self.calls.lock().push(YuanbaoCall::SendSticker {
            chat_id: chat_id.to_string(),
            sticker_name: sticker_name.to_string(),
            reply_to: reply_to.map(str::to_string),
        });
        YuanbaoSendOutcome::Ok {
            message_id: Some(format!("captured-sticker-{}", self.calls.lock().len())),
        }
    }
    async fn send_dm_text(
        &self,
        user_id: &str,
        message: &str,
        group_code: &str,
    ) -> YuanbaoSendOutcome {
        self.calls.lock().push(YuanbaoCall::SendDmText {
            user_id: user_id.to_string(),
            message: message.to_string(),
            group_code: group_code.to_string(),
        });
        YuanbaoSendOutcome::Ok {
            message_id: Some(format!("captured-dm-{}", self.calls.lock().len())),
        }
    }
    async fn send_image_file(
        &self,
        chat_id: &str,
        path: &str,
        group_code: &str,
    ) -> YuanbaoSendOutcome {
        self.calls.lock().push(YuanbaoCall::SendImageFile {
            chat_id: chat_id.to_string(),
            path: path.to_string(),
            group_code: group_code.to_string(),
        });
        YuanbaoSendOutcome::Ok {
            message_id: Some(format!("captured-image-{}", self.calls.lock().len())),
        }
    }
    async fn send_document(
        &self,
        chat_id: &str,
        path: &str,
        group_code: &str,
    ) -> YuanbaoSendOutcome {
        self.calls.lock().push(YuanbaoCall::SendDocument {
            chat_id: chat_id.to_string(),
            path: path.to_string(),
            group_code: group_code.to_string(),
        });
        YuanbaoSendOutcome::Ok {
            message_id: Some(format!("captured-document-{}", self.calls.lock().len())),
        }
    }
}

// ---------------------------------------------------------------------------
// Image extensions — mirrors Python `_IMAGE_EXTS` (MessageSender.IMAGE_EXTS)
// ---------------------------------------------------------------------------

const IMAGE_EXTS: &[&str] = &[".jpg", ".jpeg", ".png", ".gif", ".webp", ".bmp"];

fn is_image_path(path: &str) -> bool {
    let lower = path.to_ascii_lowercase();
    IMAGE_EXTS.iter().any(|e| lower.ends_with(e))
}

// ---------------------------------------------------------------------------
// Action enum
// ---------------------------------------------------------------------------

/// One of the five Yuanbao tool operations.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum YuanbaoAction {
    /// `yb_query_group_info` — basic group metadata.
    QueryGroupInfo,
    /// `yb_query_group_members` — list / search / list_bots.
    QueryGroupMembers,
    /// `yb_search_sticker` — search the built-in sticker catalogue.
    SearchSticker,
    /// `yb_send_sticker` — send a TIMFaceElem sticker.
    SendSticker,
    /// `yb_send_dm` — DM a group member (with optional media files).
    SendDm,
}

impl YuanbaoAction {
    pub fn from_name(name: &str) -> Option<Self> {
        match name {
            "query_group_info" | "yb_query_group_info" => Some(Self::QueryGroupInfo),
            "query_group_members" | "yb_query_group_members" => Some(Self::QueryGroupMembers),
            "search_sticker" | "yb_search_sticker" => Some(Self::SearchSticker),
            "send_sticker" | "yb_send_sticker" => Some(Self::SendSticker),
            "send_dm" | "yb_send_dm" => Some(Self::SendDm),
            _ => None,
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Self::QueryGroupInfo => "query_group_info",
            Self::QueryGroupMembers => "query_group_members",
            Self::SearchSticker => "search_sticker",
            Self::SendSticker => "send_sticker",
            Self::SendDm => "send_dm",
        }
    }

    pub fn all_names() -> &'static [&'static str] {
        &[
            "query_group_info",
            "query_group_members",
            "search_sticker",
            "send_sticker",
            "send_dm",
        ]
    }
}

// ---------------------------------------------------------------------------
// YuanbaoTool
// ---------------------------------------------------------------------------

/// `yuanbao` tool — Wayland engine port of the five
/// `yb_*` tools collapsed under a single `action` discriminator.
pub struct YuanbaoTool {
    backend: Arc<dyn YuanbaoBackend>,
    backend_configured: bool,
}

impl Default for YuanbaoTool {
    fn default() -> Self {
        // No real backend → not available. `ToolRegistry::register` skips
        // unavailable tools, so the model never sees a yuanbao tool that would
        // fail-loud on every call (the "running forever" guard). A real
        // backend supplied via `new()` flips this on.
        Self {
            backend: Arc::new(NullYuanbaoBackend),
            backend_configured: false,
        }
    }
}

impl YuanbaoTool {
    pub fn new(backend: Arc<dyn YuanbaoBackend>) -> Self {
        Self {
            backend,
            backend_configured: true,
        }
    }
}

fn err_result(msg: impl Into<String>) -> ToolResult {
    ToolResult {
        content: json!({ "success": false, "error": msg.into() }).to_string(),
        is_error: true,
    }
}

fn ok_result(payload: Value) -> ToolResult {
    ToolResult {
        content: payload.to_string(),
        is_error: false,
    }
}

#[async_trait]
impl Tool for YuanbaoTool {
    fn name(&self) -> &str {
        "yuanbao"
    }

    fn is_available(&self) -> bool {
        self.backend_configured
    }

    fn description(&self) -> &str {
        "Tencent Yuanbao (元宝 / Pai 派) platform toolset. \
         Use 'action' to select the operation: \
         query_group_info (group name/owner/member count), \
         query_group_members (list / find / list_bots — required before @mentioning), \
         search_sticker (find a TIM face / 贴纸 by keyword), \
         send_sticker (deliver a TIMFaceElem — prefer over inline emoji), \
         send_dm (private message a group member, with optional media)."
    }

    fn input_schema(&self) -> JsonSchema {
        json!({
            "type": "object",
            "properties": {
                "action": {
                    "type": "string",
                    "enum": YuanbaoAction::all_names(),
                    "description": "Which Yuanbao operation to invoke."
                },
                "group_code": {
                    "type": "string",
                    "description": "The unique group identifier ('派/Pai' group code). \
                                    Required for query_group_info, query_group_members, \
                                    and for send_dm when user_id is not supplied."
                },
                "members_action": {
                    "type": "string",
                    "enum": ["find", "list_bots", "list_all"],
                    "description": "query_group_members sub-action — \
                                    find (search by nickname), \
                                    list_bots (bots + Yuanbao AI), \
                                    list_all (default)."
                },
                "name": {
                    "type": "string",
                    "description": "User nickname (partial match, case-insensitive). \
                                    Required for members_action=find or send_dm without user_id."
                },
                "mention": {
                    "type": "boolean",
                    "description": "When true, attach the @mention formatting hint to \
                                    query_group_members responses."
                },
                "query": {
                    "type": "string",
                    "description": "search_sticker keyword (Chinese or English). \
                                    Empty string returns the first N stickers."
                },
                "limit": {
                    "type": "integer",
                    "description": "search_sticker max candidates (default 10, max 50)."
                },
                "sticker": {
                    "type": "string",
                    "description": "send_sticker — sticker name or numeric sticker_id. \
                                    Empty string sends a random sticker."
                },
                "chat_id": {
                    "type": "string",
                    "description": "send_sticker target chat — \
                                    'direct:<account_id>', 'group:<group_code>', or \
                                    bare account_id."
                },
                "reply_to": {
                    "type": "string",
                    "description": "Optional ref_msg_id to quote-reply (group chat only)."
                },
                "message": {
                    "type": "string",
                    "description": "send_dm message text. May be empty when media_files \
                                    are supplied."
                },
                "user_id": {
                    "type": "string",
                    "description": "send_dm target user_id — when supplied, skips \
                                    the group-member lookup."
                },
                "media_files": {
                    "type": "array",
                    "description": "send_dm optional list of media files. Images \
                                    (.jpg/.png/.gif/.webp/.bmp) ship as image messages; \
                                    other files ship as document attachments.",
                    "items": {
                        "type": "object",
                        "properties": {
                            "path": { "type": "string" },
                            "is_voice": { "type": "boolean" }
                        },
                        "required": ["path"]
                    }
                }
            },
            "required": ["action"]
        })
    }

    fn is_concurrency_safe(&self, input: &Value) -> bool {
        // Only the read-only actions are safe to run concurrently.
        matches!(
            input
                .get("action")
                .and_then(Value::as_str)
                .and_then(YuanbaoAction::from_name),
            Some(
                YuanbaoAction::QueryGroupInfo
                    | YuanbaoAction::QueryGroupMembers
                    | YuanbaoAction::SearchSticker
            )
        )
    }

    fn category(&self) -> ToolCategory {
        // Mixed: send_* paths have observable side effects. Default
        // to Exec so hosts that gate Exec tools behind approval also
        // gate Yuanbao sends. Read-only queries cost only an extra
        // round-trip to the host's gating layer, which is acceptable
        // for a platform integration tool.
        ToolCategory::Exec
    }

    async fn execute(&self, input: Value) -> ToolResult {
        let action_name = match input.get("action").and_then(Value::as_str) {
            Some(s) if !s.trim().is_empty() => s,
            _ => return err_result("Missing required parameter: 'action'"),
        };
        let action = match YuanbaoAction::from_name(action_name) {
            Some(a) => a,
            None => {
                return err_result(format!(
                    "Unknown action '{}'. Available: {}",
                    action_name,
                    YuanbaoAction::all_names().join(", ")
                ));
            }
        };

        if !self.backend.is_connected().await {
            return err_result("Yuanbao adapter is not connected");
        }

        match action {
            YuanbaoAction::QueryGroupInfo => self.do_query_group_info(&input).await,
            YuanbaoAction::QueryGroupMembers => self.do_query_group_members(&input).await,
            YuanbaoAction::SearchSticker => self.do_search_sticker(&input).await,
            YuanbaoAction::SendSticker => self.do_send_sticker(&input).await,
            YuanbaoAction::SendDm => self.do_send_dm(&input).await,
        }
    }
}

impl YuanbaoTool {
    async fn do_query_group_info(&self, input: &Value) -> ToolResult {
        let group_code = match input.get("group_code").and_then(Value::as_str) {
            Some(s) if !s.is_empty() => s,
            _ => return err_result("group_code is required"),
        };
        match self.backend.query_group_info(group_code).await {
            Ok(Some(gi)) => ok_result(json!({
                "success": true,
                "group_code": group_code,
                "group_name": gi.group_name,
                "member_count": gi.member_count,
                "owner": {
                    "user_id": gi.owner_id,
                    "nickname": gi.owner_nickname,
                },
                "note": "The group is called \"派 (Pai)\" in the app.",
            })),
            Ok(None) => err_result("query_group_info returned None"),
            Err(e) => err_result(e),
        }
    }

    async fn do_query_group_members(&self, input: &Value) -> ToolResult {
        let group_code = match input.get("group_code").and_then(Value::as_str) {
            Some(s) if !s.is_empty() => s,
            _ => return err_result("group_code is required"),
        };
        let members_action = input
            .get("members_action")
            .and_then(Value::as_str)
            .unwrap_or("list_all");
        let name = input
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        let mention = input
            .get("mention")
            .and_then(Value::as_bool)
            .unwrap_or(false);

        let all_members = match self.backend.list_group_members(group_code).await {
            Ok(Some(m)) => m,
            Ok(None) => return err_result("get_group_member_list returned None"),
            Err(e) => return err_result(e),
        };

        if all_members.is_empty() {
            return err_result("No members found in this group.");
        }

        let to_json = |m: &YuanbaoMember| {
            json!({
                "user_id": m.user_id,
                "nickname": m.nickname,
                "role": m.role,
            })
        };

        let mut hint = serde_json::Map::new();
        if mention {
            hint.insert("mention_hint".to_string(), json!(MENTION_HINT));
        }

        match members_action {
            "list_bots" => {
                let bots: Vec<_> = all_members
                    .iter()
                    .filter(|m| m.role == "yuanbao_ai" || m.role == "bot")
                    .collect();
                if bots.is_empty() {
                    return err_result("No bots found in this group.");
                }
                let mut payload = json!({
                    "success": true,
                    "msg": format!("Found {} bot(s).", bots.len()),
                    "members": bots.iter().map(|m| to_json(m)).collect::<Vec<_>>(),
                });
                if let Some(obj) = payload.as_object_mut() {
                    obj.extend(hint);
                }
                ok_result(payload)
            }
            "find" => {
                if !name.is_empty() {
                    let filt = name.trim().to_lowercase();
                    let matched: Vec<_> = all_members
                        .iter()
                        .filter(|m| m.nickname.to_lowercase().contains(&filt))
                        .collect();
                    if !matched.is_empty() {
                        let mut payload = json!({
                            "success": true,
                            "msg": format!("Found {} member(s) matching \"{}\".", matched.len(), name),
                            "members": matched.iter().map(|m| to_json(m)).collect::<Vec<_>>(),
                        });
                        if let Some(obj) = payload.as_object_mut() {
                            obj.extend(hint);
                        }
                        return ok_result(payload);
                    }
                    let mut payload = json!({
                        "success": false,
                        "msg": format!("No match for \"{}\". All members listed below.", name),
                        "members": all_members.iter().map(to_json).collect::<Vec<_>>(),
                    });
                    if let Some(obj) = payload.as_object_mut() {
                        obj.extend(hint);
                    }
                    return ToolResult {
                        content: payload.to_string(),
                        is_error: false, // Python returns success=false but as a regular dict; engine surfaces as non-error tool result so the LLM sees the candidate list.
                    };
                }
                let mut payload = json!({
                    "success": true,
                    "msg": format!("Found {} member(s).", all_members.len()),
                    "members": all_members.iter().map(to_json).collect::<Vec<_>>(),
                });
                if let Some(obj) = payload.as_object_mut() {
                    obj.extend(hint);
                }
                ok_result(payload)
            }
            _ => {
                // list_all (default) — also covers unknown sub-actions, matching Python.
                let mut payload = json!({
                    "success": true,
                    "msg": format!("Found {} member(s).", all_members.len()),
                    "members": all_members.iter().map(to_json).collect::<Vec<_>>(),
                });
                if let Some(obj) = payload.as_object_mut() {
                    obj.extend(hint);
                }
                ok_result(payload)
            }
        }
    }

    async fn do_search_sticker(&self, input: &Value) -> ToolResult {
        let query = input
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        // Mirror the Python clamp: max(1, min(50, int(limit) if limit else 10)).
        let raw_limit = input.get("limit").and_then(Value::as_i64).unwrap_or(10);
        let safe_limit = raw_limit.clamp(1, 50) as u32;

        match self.backend.search_stickers(&query, safe_limit).await {
            Ok(matches) => {
                let results: Vec<_> = matches
                    .iter()
                    .map(|s| {
                        json!({
                            "sticker_id": s.sticker_id,
                            "name": s.name,
                            "description": s.description,
                            "package_id": s.package_id,
                        })
                    })
                    .collect();
                ok_result(json!({
                    "success": true,
                    "query": query,
                    "count": results.len(),
                    "results": results,
                }))
            }
            Err(e) => err_result(e),
        }
    }

    async fn do_send_sticker(&self, input: &Value) -> ToolResult {
        let target = input
            .get("chat_id")
            .and_then(Value::as_str)
            .unwrap_or("")
            .trim()
            .to_string();
        if target.is_empty() {
            return err_result("chat_id is required (no active yuanbao session detected)");
        }
        let raw = input
            .get("sticker")
            .and_then(Value::as_str)
            .unwrap_or("")
            .trim()
            .to_string();
        let reply_to = input
            .get("reply_to")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .map(str::to_string);

        // Resolve sticker — empty -> random; numeric -> by_id; else -> by_name.
        let sticker_obj = if raw.is_empty() {
            match self.backend.random_sticker().await {
                Ok(s) => s,
                Err(e) => return err_result(e),
            }
        } else if raw.chars().all(|c| c.is_ascii_digit()) {
            match self.backend.sticker_by_id(&raw).await {
                Ok(Some(s)) => Some(s),
                Ok(None) => match self.backend.sticker_by_name(&raw).await {
                    Ok(opt) => opt,
                    Err(e) => return err_result(e),
                },
                Err(e) => return err_result(e),
            }
        } else {
            match self.backend.sticker_by_name(&raw).await {
                Ok(opt) => opt,
                Err(e) => return err_result(e),
            }
        };

        let sticker = match sticker_obj {
            Some(s) => s,
            None => {
                return err_result(format!(
                    "Sticker not found: {:?}. Use search_sticker first to discover available stickers.",
                    raw
                ));
            }
        };

        match self
            .backend
            .send_sticker(&target, &sticker.name, reply_to.as_deref())
            .await
        {
            YuanbaoSendOutcome::Ok { message_id } => ok_result(json!({
                "success": true,
                "chat_id": target,
                "sticker": {
                    "sticker_id": sticker.sticker_id,
                    "name": sticker.name,
                },
                "message_id": message_id,
                "note": "Sticker delivered to the chat. If you have additional text to say, reply now; otherwise end your turn without generating text.",
            })),
            YuanbaoSendOutcome::Err { message } => err_result(message),
        }
    }

    async fn do_send_dm(&self, input: &Value) -> ToolResult {
        // Parse inputs.
        let user_id_in = input
            .get("user_id")
            .and_then(Value::as_str)
            .unwrap_or("")
            .trim()
            .to_string();
        let name_in = input
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or("")
            .trim()
            .to_string();
        let message = input
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        let group_code = input
            .get("group_code")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        let media_files = parse_media_files(input.get("media_files"));

        if message.is_empty() && media_files.is_empty() {
            return err_result("message or media_files is required");
        }

        let mut resolved_user_id = user_id_in;
        let mut resolved_nickname = name_in.clone();

        // Step 1: Resolve user_id from group member list if not provided.
        if resolved_user_id.is_empty() {
            if group_code.is_empty() {
                return err_result("group_code is required when user_id is not provided");
            }
            if name_in.is_empty() {
                return err_result("name is required when user_id is not provided");
            }
            let members = match self.backend.list_group_members(&group_code).await {
                Ok(Some(m)) => m,
                Ok(None) => return err_result("get_group_member_list returned None"),
                Err(e) => return err_result(e),
            };
            let filt = name_in.to_lowercase();
            let matched: Vec<_> = members
                .iter()
                .filter(|m| m.nickname.to_lowercase().contains(&filt))
                .collect();
            if matched.is_empty() {
                return err_result(format!(
                    "No member matching \"{}\" found in group {}.",
                    name_in, group_code
                ));
            }
            if matched.len() > 1 {
                let candidates: Vec<_> = matched
                    .iter()
                    .map(|m| {
                        json!({
                            "user_id": m.user_id,
                            "nickname": m.nickname,
                        })
                    })
                    .collect();
                return ToolResult {
                    content: json!({
                        "success": false,
                        "error": format!(
                            "Multiple members match \"{}\". Please specify which one.",
                            name_in
                        ),
                        "candidates": candidates,
                    })
                    .to_string(),
                    is_error: true,
                };
            }
            resolved_user_id = matched[0].user_id.clone();
            resolved_nickname = matched[0].nickname.clone();
        }

        if resolved_user_id.is_empty() {
            return err_result("Could not resolve user_id");
        }

        // Step 2: send text + media. Mirror the Python control flow:
        // text first (if non-empty), then each media file by extension.
        let chat_id = format!("direct:{}", resolved_user_id);
        let mut last: Option<YuanbaoSendOutcome> = None;
        let mut errors: Vec<String> = Vec::new();

        if !message.trim().is_empty() {
            let outcome = self
                .backend
                .send_dm_text(&resolved_user_id, &message, &group_code)
                .await;
            if let YuanbaoSendOutcome::Err { message: e } = &outcome {
                errors.push(if e.is_empty() {
                    "text send failed".to_string()
                } else {
                    e.clone()
                });
            }
            last = Some(outcome);
        }

        for media in &media_files {
            let outcome = if is_image_path(&media.path) {
                self.backend
                    .send_image_file(&chat_id, &media.path, &group_code)
                    .await
            } else {
                self.backend
                    .send_document(&chat_id, &media.path, &group_code)
                    .await
            };
            if let YuanbaoSendOutcome::Err { message: e } = &outcome {
                errors.push(if e.is_empty() {
                    "media send failed".to_string()
                } else {
                    e.clone()
                });
            }
            last = Some(outcome);
        }

        let last = match last {
            Some(l) => l,
            None => return err_result("No deliverable text or media remained"),
        };

        let succeeded = matches!(last, YuanbaoSendOutcome::Ok { .. });

        if !errors.is_empty() && !succeeded {
            return err_result(errors.join("; "));
        }

        let message_id = match last {
            YuanbaoSendOutcome::Ok { message_id } => message_id,
            YuanbaoSendOutcome::Err { .. } => None,
        };
        let mut note = format!("DM sent to \"{}\" successfully.", resolved_nickname);
        if !errors.is_empty() {
            note = format!("{} (partial failure: {})", note, errors.join("; "));
        }
        ok_result(json!({
            "success": true,
            "user_id": resolved_user_id,
            "nickname": resolved_nickname,
            "message_id": message_id,
            "note": note,
        }))
    }
}

/// Parse the `media_files` parameter. Accepts either an array of
/// objects `{"path": str, "is_voice": bool}` (the Python
/// handler's primary shape) or an array of `[path, is_voice]` tuples
/// (its fallback shape).
fn parse_media_files(raw: Option<&Value>) -> Vec<YuanbaoMedia> {
    let arr = match raw.and_then(Value::as_array) {
        Some(a) => a,
        None => return Vec::new(),
    };
    let mut out = Vec::new();
    for item in arr {
        if let Some(obj) = item.as_object() {
            let path = obj
                .get("path")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string();
            if path.is_empty() {
                continue;
            }
            let is_voice = obj
                .get("is_voice")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            out.push(YuanbaoMedia { path, is_voice });
        } else if let Some(arr) = item.as_array()
            && arr.len() >= 2
        {
            let path = arr[0].as_str().unwrap_or("").to_string();
            let is_voice = arr[1].as_bool().unwrap_or(false);
            if !path.is_empty() {
                out.push(YuanbaoMedia { path, is_voice });
            }
        }
    }
    out
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn run(input: Value, tool: &YuanbaoTool) -> ToolResult {
        futures::executor::block_on(tool.execute(input))
    }

    fn parse(content: &str) -> Value {
        serde_json::from_str(content).expect("tool result must be JSON")
    }

    fn capturing_tool() -> (Arc<CapturingYuanbaoBackend>, YuanbaoTool) {
        let backend = Arc::new(CapturingYuanbaoBackend::new());
        let tool = YuanbaoTool::new(backend.clone());
        (backend, tool)
    }

    #[test]
    fn user_type_label_table() {
        assert_eq!(user_type_label(0), "unknown");
        assert_eq!(user_type_label(1), "user");
        assert_eq!(user_type_label(2), "yuanbao_ai");
        assert_eq!(user_type_label(3), "bot");
        assert_eq!(user_type_label(42), "unknown");
    }

    #[test]
    fn parse_target_action_names() {
        assert_eq!(
            YuanbaoAction::from_name("query_group_info"),
            Some(YuanbaoAction::QueryGroupInfo)
        );
        // Legacy `yb_*` registry-style names also accepted.
        assert_eq!(
            YuanbaoAction::from_name("yb_send_dm"),
            Some(YuanbaoAction::SendDm)
        );
        assert_eq!(YuanbaoAction::from_name("nope"), None);
    }

    #[test]
    fn null_backend_reports_disconnected() {
        let tool = YuanbaoTool::default();
        let r = run(
            json!({"action": "query_group_info", "group_code": "abc"}),
            &tool,
        );
        assert!(r.is_error);
        assert!(
            r.content.contains("Yuanbao adapter is not connected"),
            "got: {}",
            r.content
        );
    }

    #[test]
    fn missing_action_rejected() {
        let tool = YuanbaoTool::default();
        let r = run(json!({}), &tool);
        assert!(r.is_error);
        assert!(r.content.contains("action"));
    }

    #[test]
    fn unknown_action_rejected() {
        let tool = YuanbaoTool::default();
        let r = run(json!({"action": "destroy_world"}), &tool);
        assert!(r.is_error);
        assert!(r.content.contains("Unknown action"));
    }

    #[test]
    fn query_group_info_happy_path() {
        let (backend, tool) = capturing_tool();
        backend.set_group_info(YuanbaoGroupInfo {
            group_name: "Pai Group".to_string(),
            member_count: 42,
            owner_id: "u-1".to_string(),
            owner_nickname: "Owner".to_string(),
        });
        let r = run(
            json!({"action": "query_group_info", "group_code": "G1"}),
            &tool,
        );
        assert!(!r.is_error, "got: {}", r.content);
        let v = parse(&r.content);
        assert_eq!(v["success"], json!(true));
        assert_eq!(v["group_name"], json!("Pai Group"));
        assert_eq!(v["member_count"], json!(42));
        assert_eq!(v["owner"]["user_id"], json!("u-1"));
        assert!(v["note"].as_str().unwrap().contains("Pai"));
    }

    #[test]
    fn query_group_info_missing_group_code() {
        let (_, tool) = capturing_tool();
        let r = run(json!({"action": "query_group_info"}), &tool);
        assert!(r.is_error);
        assert!(r.content.contains("group_code"));
    }

    #[test]
    fn query_group_info_returns_none() {
        let (_, tool) = capturing_tool();
        // Backend has no group_info set → returns None.
        let r = run(
            json!({"action": "query_group_info", "group_code": "G"}),
            &tool,
        );
        assert!(r.is_error);
        assert!(r.content.contains("query_group_info returned None"));
    }

    fn sample_members() -> Vec<YuanbaoMember> {
        vec![
            YuanbaoMember {
                user_id: "u1".to_string(),
                nickname: "Alice".to_string(),
                role: "user".to_string(),
            },
            YuanbaoMember {
                user_id: "u2".to_string(),
                nickname: "Bob".to_string(),
                role: "user".to_string(),
            },
            YuanbaoMember {
                user_id: "b1".to_string(),
                nickname: "Yuanbao".to_string(),
                role: "yuanbao_ai".to_string(),
            },
        ]
    }

    #[test]
    fn query_group_members_list_all_with_mention_hint() {
        let (backend, tool) = capturing_tool();
        backend.set_members(sample_members());
        let r = run(
            json!({
                "action": "query_group_members",
                "group_code": "G",
                "members_action": "list_all",
                "mention": true,
            }),
            &tool,
        );
        assert!(!r.is_error, "got: {}", r.content);
        let v = parse(&r.content);
        assert_eq!(v["success"], json!(true));
        assert_eq!(v["members"].as_array().unwrap().len(), 3);
        assert!(v["mention_hint"].as_str().unwrap().contains("@"));
    }

    #[test]
    fn query_group_members_list_bots_only() {
        let (backend, tool) = capturing_tool();
        backend.set_members(sample_members());
        let r = run(
            json!({
                "action": "query_group_members",
                "group_code": "G",
                "members_action": "list_bots",
            }),
            &tool,
        );
        assert!(!r.is_error, "got: {}", r.content);
        let v = parse(&r.content);
        let members = v["members"].as_array().unwrap();
        assert_eq!(members.len(), 1);
        assert_eq!(members[0]["nickname"], json!("Yuanbao"));
    }

    #[test]
    fn query_group_members_find_matches() {
        let (backend, tool) = capturing_tool();
        backend.set_members(sample_members());
        let r = run(
            json!({
                "action": "query_group_members",
                "group_code": "G",
                "members_action": "find",
                "name": "ali",
            }),
            &tool,
        );
        assert!(!r.is_error);
        let v = parse(&r.content);
        assert_eq!(v["success"], json!(true));
        assert_eq!(v["members"].as_array().unwrap().len(), 1);
        assert_eq!(v["members"][0]["user_id"], json!("u1"));
    }

    #[test]
    fn query_group_members_find_no_match_returns_all() {
        let (backend, tool) = capturing_tool();
        backend.set_members(sample_members());
        let r = run(
            json!({
                "action": "query_group_members",
                "group_code": "G",
                "members_action": "find",
                "name": "zzz",
            }),
            &tool,
        );
        // Non-error because the LLM still gets a useful candidate list.
        assert!(!r.is_error);
        let v = parse(&r.content);
        assert_eq!(v["success"], json!(false));
        assert!(v["msg"].as_str().unwrap().contains("zzz"));
        assert_eq!(v["members"].as_array().unwrap().len(), 3);
    }

    #[test]
    fn query_group_members_empty_group() {
        let (backend, tool) = capturing_tool();
        backend.set_members(Vec::new());
        let r = run(
            json!({
                "action": "query_group_members",
                "group_code": "G",
                "members_action": "list_all",
            }),
            &tool,
        );
        assert!(r.is_error);
        assert!(r.content.contains("No members found"));
    }

    #[test]
    fn search_sticker_happy_path_and_limit_clamp() {
        let (backend, tool) = capturing_tool();
        backend.set_stickers(vec![
            YuanbaoSticker {
                sticker_id: "1".to_string(),
                name: "比心".to_string(),
                description: "heart".to_string(),
                package_id: "p1".to_string(),
            },
            YuanbaoSticker {
                sticker_id: "2".to_string(),
                name: "六六六".to_string(),
                description: "666 nice".to_string(),
                package_id: "p1".to_string(),
            },
        ]);
        let r = run(
            json!({"action": "search_sticker", "query": "666", "limit": 200}),
            &tool,
        );
        assert!(!r.is_error, "got: {}", r.content);
        let v = parse(&r.content);
        assert_eq!(v["success"], json!(true));
        assert_eq!(v["count"], json!(1));
        assert_eq!(v["results"][0]["sticker_id"], json!("2"));
        // Limit was clamped to 50 — capturing backend records the
        // sanitized value (not the raw 200).
        let call = backend.snapshot();
        let last = call.last().unwrap();
        assert!(matches!(
            last,
            YuanbaoCall::SearchStickers { limit: 50, .. }
        ));
    }

    #[test]
    fn send_sticker_requires_chat_id() {
        let (_, tool) = capturing_tool();
        let r = run(json!({"action": "send_sticker"}), &tool);
        assert!(r.is_error);
        assert!(r.content.contains("chat_id is required"));
    }

    #[test]
    fn send_sticker_unknown_name() {
        let (backend, tool) = capturing_tool();
        backend.set_stickers(vec![]);
        let r = run(
            json!({"action": "send_sticker", "chat_id": "group:G", "sticker": "missing"}),
            &tool,
        );
        assert!(r.is_error);
        assert!(r.content.contains("Sticker not found"));
    }

    #[test]
    fn send_sticker_by_numeric_id() {
        let (backend, tool) = capturing_tool();
        backend.set_stickers(vec![YuanbaoSticker {
            sticker_id: "278".to_string(),
            name: "六六六".to_string(),
            description: "666".to_string(),
            package_id: "p1".to_string(),
        }]);
        let r = run(
            json!({
                "action": "send_sticker",
                "chat_id": "group:G",
                "sticker": "278",
                "reply_to": "msg-42",
            }),
            &tool,
        );
        assert!(!r.is_error, "got: {}", r.content);
        let v = parse(&r.content);
        assert_eq!(v["sticker"]["sticker_id"], json!("278"));
        assert_eq!(v["sticker"]["name"], json!("六六六"));
        // Verify the backend was driven correctly.
        let calls = backend.snapshot();
        assert!(
            calls.iter().any(
                |c| matches!(c, YuanbaoCall::StickerById { sticker_id } if sticker_id == "278")
            )
        );
        assert!(calls.iter().any(|c| matches!(
            c,
            YuanbaoCall::SendSticker { chat_id, sticker_name, reply_to }
            if chat_id == "group:G" && sticker_name == "六六六" && reply_to.as_deref() == Some("msg-42")
        )));
    }

    #[test]
    fn send_sticker_empty_picks_random() {
        let (backend, tool) = capturing_tool();
        backend.set_stickers(vec![YuanbaoSticker {
            sticker_id: "r".to_string(),
            name: "random".to_string(),
            description: "".to_string(),
            package_id: "p".to_string(),
        }]);
        let r = run(
            json!({"action": "send_sticker", "chat_id": "group:G", "sticker": ""}),
            &tool,
        );
        assert!(!r.is_error);
        let calls = backend.snapshot();
        assert!(
            calls
                .iter()
                .any(|c| matches!(c, YuanbaoCall::RandomSticker))
        );
    }

    #[test]
    fn send_dm_requires_message_or_media() {
        let (_, tool) = capturing_tool();
        let r = run(json!({"action": "send_dm"}), &tool);
        assert!(r.is_error);
        assert!(r.content.contains("message or media_files is required"));
    }

    #[test]
    fn send_dm_requires_group_code_when_no_user_id() {
        let (_, tool) = capturing_tool();
        let r = run(
            json!({"action": "send_dm", "name": "Alice", "message": "hi"}),
            &tool,
        );
        assert!(r.is_error);
        assert!(r.content.contains("group_code"));
    }

    #[test]
    fn send_dm_resolves_user_by_name() {
        let (backend, tool) = capturing_tool();
        backend.set_members(sample_members());
        let r = run(
            json!({
                "action": "send_dm",
                "group_code": "G",
                "name": "ali",
                "message": "hi",
            }),
            &tool,
        );
        assert!(!r.is_error, "got: {}", r.content);
        let v = parse(&r.content);
        assert_eq!(v["user_id"], json!("u1"));
        assert_eq!(v["nickname"], json!("Alice"));
        let calls = backend.snapshot();
        assert!(calls.iter().any(|c| matches!(
            c,
            YuanbaoCall::SendDmText { user_id, message, group_code }
            if user_id == "u1" && message == "hi" && group_code == "G"
        )));
    }

    #[test]
    fn send_dm_ambiguous_name_returns_candidates() {
        let (backend, tool) = capturing_tool();
        backend.set_members(vec![
            YuanbaoMember {
                user_id: "u1".to_string(),
                nickname: "Alice".to_string(),
                role: "user".to_string(),
            },
            YuanbaoMember {
                user_id: "u2".to_string(),
                nickname: "Alicia".to_string(),
                role: "user".to_string(),
            },
        ]);
        let r = run(
            json!({
                "action": "send_dm",
                "group_code": "G",
                "name": "ali",
                "message": "hi",
            }),
            &tool,
        );
        assert!(r.is_error);
        let v = parse(&r.content);
        assert!(v["error"].as_str().unwrap().contains("Multiple"));
        assert_eq!(v["candidates"].as_array().unwrap().len(), 2);
    }

    #[test]
    fn send_dm_with_user_id_skips_member_lookup() {
        let (backend, tool) = capturing_tool();
        let r = run(
            json!({
                "action": "send_dm",
                "user_id": "u-direct",
                "message": "hi",
                "group_code": "G",
            }),
            &tool,
        );
        assert!(!r.is_error, "got: {}", r.content);
        let calls = backend.snapshot();
        // Member lookup must not have been called.
        assert!(
            !calls
                .iter()
                .any(|c| matches!(c, YuanbaoCall::ListGroupMembers { .. }))
        );
        assert!(calls.iter().any(|c| matches!(
            c,
            YuanbaoCall::SendDmText { user_id, .. } if user_id == "u-direct"
        )));
    }

    #[test]
    fn send_dm_routes_media_by_extension() {
        let (backend, tool) = capturing_tool();
        let r = run(
            json!({
                "action": "send_dm",
                "user_id": "u-direct",
                "group_code": "G",
                "message": "",
                "media_files": [
                    {"path": "/tmp/pic.PNG"},
                    {"path": "/tmp/doc.pdf", "is_voice": false},
                ],
            }),
            &tool,
        );
        assert!(!r.is_error, "got: {}", r.content);
        let calls = backend.snapshot();
        // .PNG (case-insensitive) -> image; .pdf -> document.
        assert!(calls.iter().any(|c| matches!(
            c, YuanbaoCall::SendImageFile { path, .. } if path == "/tmp/pic.PNG"
        )));
        assert!(calls.iter().any(|c| matches!(
            c, YuanbaoCall::SendDocument { path, .. } if path == "/tmp/doc.pdf"
        )));
    }

    #[test]
    fn parse_media_files_accepts_tuple_form() {
        let v = json!([["path1.png", true], ["path2.txt", false]]);
        let parsed = parse_media_files(Some(&v));
        assert_eq!(parsed.len(), 2);
        assert_eq!(parsed[0].path, "path1.png");
        assert!(parsed[0].is_voice);
        assert_eq!(parsed[1].path, "path2.txt");
        assert!(!parsed[1].is_voice);
    }

    #[test]
    fn is_image_path_extension_matrix() {
        assert!(is_image_path("foo.jpg"));
        assert!(is_image_path("foo.JPEG"));
        assert!(is_image_path("/a/b.PNG"));
        assert!(is_image_path("x.webp"));
        assert!(!is_image_path("x.pdf"));
        assert!(!is_image_path("noext"));
    }

    #[test]
    fn tool_registers_in_registry_with_required_action() {
        use crate::registry::ToolRegistry;
        let mut reg = ToolRegistry::new();
        // Use a configured backend so the tool is available and registers;
        // the default (Null) backend is intentionally unavailable + skipped.
        reg.register(Box::new(YuanbaoTool::new(Arc::new(NullYuanbaoBackend))));
        let defs = reg.to_tool_defs();
        let def = defs
            .iter()
            .find(|d| d.name == "yuanbao")
            .expect("yuanbao must be registered");
        let req = def.input_schema["required"]
            .as_array()
            .expect("required array");
        let req_strs: Vec<&str> = req.iter().filter_map(Value::as_str).collect();
        assert!(req_strs.contains(&"action"));
    }

    #[test]
    fn concurrency_safety_matches_action_category() {
        let tool = YuanbaoTool::default();
        assert!(tool.is_concurrency_safe(&json!({"action": "query_group_info"})));
        assert!(tool.is_concurrency_safe(&json!({"action": "query_group_members"})));
        assert!(tool.is_concurrency_safe(&json!({"action": "search_sticker"})));
        assert!(!tool.is_concurrency_safe(&json!({"action": "send_sticker"})));
        assert!(!tool.is_concurrency_safe(&json!({"action": "send_dm"})));
        // Unknown / missing action defaults to "not safe" (more conservative).
        assert!(!tool.is_concurrency_safe(&json!({"action": "bogus"})));
        assert!(!tool.is_concurrency_safe(&json!({})));
    }
}
