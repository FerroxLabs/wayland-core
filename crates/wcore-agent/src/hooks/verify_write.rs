//! F15 verification loop hook (Write only).
//!
//! Subscribes to `post_tool_use`. For successful `Write` tool calls, the
//! hook re-reads the touched `file_path` and compares its contents to the
//! `content` field of the tool input. On mismatch, returns
//! `HookAction::InjectMessage` so the next turn can correct.
//!
//! Cheap (one fs read per Write) but not free; default off in `AgentConfig`.
//! Useful in long autonomous loops where a tool-side bug or external editor
//! race could silently desync the agent's mental model from disk.
//!
//! Edit verification is deferred to W6.1 (audit rev-2 finding 7): Edit's
//! tool output is "Edited {path}: replaced N occurrence(s)" — a status
//! string, not a recoverable post-state — so we cannot reconstruct the
//! expected content from `output` alone, and the Edit input doesn't carry
//! a full post-state field. A future hardening pass can re-derive expected
//! content from `input.old_string`/`input.new_string` + the cached pre-edit
//! content; that pass is W6.1.

use async_trait::async_trait;
use serde_json::Value;
use wcore_types::message::{ContentBlock, Message, Role};

use crate::hooks::{Hook, HookAction};

pub struct VerifyWriteHook;

impl VerifyWriteHook {
    pub fn new() -> Self {
        Self
    }
}

impl Default for VerifyWriteHook {
    fn default() -> Self {
        Self::new()
    }
}

fn inject(text: String) -> HookAction {
    HookAction::InjectMessage(Message::new(Role::User, vec![ContentBlock::Text { text }]))
}

#[async_trait]
impl Hook for VerifyWriteHook {
    fn name(&self) -> &str {
        "verify_write"
    }

    async fn post_tool_use(
        &self,
        tool: &str,
        _call_id: &str,
        input: &Value,
        _output: &str,
        is_error: bool,
    ) -> HookAction {
        if is_error {
            return HookAction::Continue;
        }
        if tool != "Write" {
            // Edit verification is W6.1 (audit rev-2 finding 7).
            return HookAction::Continue;
        }
        let Some(file_path) = input.get("file_path").and_then(|v| v.as_str()) else {
            return HookAction::Continue;
        };
        let Some(expected) = input.get("content").and_then(|v| v.as_str()) else {
            return HookAction::Continue;
        };
        let actual = match tokio::fs::read_to_string(file_path).await {
            Ok(s) => s,
            Err(_) => {
                return inject(format!(
                    "[verification] Write on `{file_path}` reported success but the file could not be re-read"
                ));
            }
        };
        if actual != expected {
            return inject(format!(
                "[verification] Write on `{file_path}` did not produce the expected content on disk. \
                 Re-read the file and reconcile before continuing."
            ));
        }
        HookAction::Continue
    }
}
