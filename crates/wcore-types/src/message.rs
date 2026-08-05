use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Unique identifier for a tool call
pub type ToolUseId = String;

/// A single content block within a message
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum ContentBlock {
    /// Plain text content
    #[serde(rename = "text")]
    Text { text: String },

    /// A tool invocation from the assistant
    #[serde(rename = "tool_use")]
    ToolUse {
        id: ToolUseId,
        name: String,
        input: Value,
        /// Opaque provider-specific metadata (e.g. Gemini thought_signature).
        /// Round-tripped verbatim so the provider can include it in follow-up requests.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        extra: Option<Value>,
    },

    /// Result of a tool execution, sent back as user message
    #[serde(rename = "tool_result")]
    ToolResult {
        tool_use_id: ToolUseId,
        content: String,
        is_error: bool,
    },

    /// Thinking / reasoning block. Serialized as `thinking` for Anthropic
    /// and as `reasoning_content` for OpenAI-compatible providers.
    #[serde(rename = "thinking")]
    Thinking { thinking: String },

    /// An inline image on a user turn (e.g. a composer-dropped local image).
    /// `data` is standard base64 (no data-URI prefix); `mime` is a sniffed
    /// image MIME such as `image/png`. The engine resolves any local path to
    /// bytes at the protocol boundary, so providers never touch the filesystem
    /// — each provider's `build_messages()` re-encodes this into its native
    /// image content shape (Anthropic `image.source.base64`, OpenAI
    /// `image_url` data URI, Gemini `inline_data`). The dedicated text-only
    /// families (Cohere, Mistral/Bedrock, Ollama) drop it and substitute a
    /// short text placeholder. The OpenAI-compatible builder always emits the
    /// image part, so a text-only OpenAI-compatible endpoint would reject it;
    /// a `ProviderCompat` vision-capability gate is a follow-up, tracked with
    /// the ingest wiring. Vision turns are kept off text-only tier models by
    /// the engine's vision-routing guard (`message_requires_vision`).
    #[serde(rename = "image")]
    Image { mime: String, data: String },
}

/// Cache-control hint placed on a `Message` by the prompt-cache discipline
/// helper (`wcore-observability::cache::mark_cache_boundaries`). The provider
/// `build_messages()` translates these into provider-specific cache markers
/// (e.g. Anthropic / Bedrock / Vertex `cache_control: {type: ephemeral}` on
/// the message's last content block). Providers that don't honour explicit
/// breakpoints (OpenAI, Gemini) ignore the hint.
///
/// `#[serde(rename_all = "snake_case")]` on this enum keeps the wire shape
/// stable: `"breakpoint"`. Future variants (e.g. a `Ttl(Duration)` hint) MUST
/// preserve the existing tag.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MessageCacheHint {
    /// Mark this message as the tail of a cacheable prompt segment. Anthropic
    /// permits up to four `cache_control` markers per request; the helper uses
    /// at most three (system + tools + this message), staying inside the cap.
    Breakpoint,
}

/// A message in the conversation
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    pub role: Role,
    pub content: Vec<ContentBlock>,
    /// When this message was created.  Used by microcompact to decide
    /// whether old tool results should be cleared.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timestamp: Option<DateTime<Utc>>,
    /// Optional cache-control hint set by the prompt-cache discipline helper.
    /// `None` for ordinary messages; providers that don't honour explicit
    /// breakpoints ignore it. `skip_serializing_if = Option::is_none` keeps
    /// the wire shape byte-identical to v0.1.21 when unset.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_breakpoint: Option<MessageCacheHint>,
}

impl Message {
    /// Create a message without a timestamp (backward-compatible default).
    pub fn new(role: Role, content: Vec<ContentBlock>) -> Self {
        Self {
            role,
            content,
            timestamp: None,
            cache_breakpoint: None,
        }
    }

    /// Create a message stamped with the current UTC time.
    pub fn now(role: Role, content: Vec<ContentBlock>) -> Self {
        Self {
            role,
            content,
            timestamp: Some(Utc::now()),
            cache_breakpoint: None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    User,
    Assistant,
    System,
    Tool,
}

/// Why the model stopped generating
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StopReason {
    /// Model finished naturally
    EndTurn,
    /// Model wants to call tools
    ToolUse,
    /// Hit max_tokens limit
    MaxTokens,
    /// Hit max_turns limit
    MaxTurns,
}

/// Protocol-level finish reason emitted in `stream_end` events.
///
/// Distinct from `StopReason` (internal): `FinishReason` is the contract
/// the JSON stream protocol exposes to host integrations (e.g. the Wayland
/// app). It coalesces stop signals into a small set of values so the host can
/// render UX consistently (e.g. show "Response was truncated" on `Length`).
///
/// Provider-native signals coalesce to `Stop` / `Length` / `Error`; unmapped
/// provider values map to `Error` rather than silently degrading. `MaxTurns`
/// is an ENGINE-level stop (never emitted by a provider) that the host must be
/// able to tell apart from a provider `Error` — see the variant docs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FinishReason {
    /// Model finished cleanly: text completion or tool call.
    Stop,
    /// Model hit the max_tokens budget before finishing — visible output
    /// may be truncated or empty (reasoning-token bug surface for Gemini Pro).
    Length,
    /// Provider returned an unrecognized stop signal, refused, or the
    /// engine never received a Done event (e.g. mid-stream error).
    Error,
    /// #457: the ENGINE stopped the run because it hit the per-turn `max_turns`
    /// cap — the model did NOT fail. Serialized as `"max_turns"`. Hosts should
    /// offer a "Continue" affordance (resume the run) rather than the provider-
    /// error UX ("use a bigger model"). Previously coalesced into `Error`, which
    /// made "out of turns" indistinguishable from a real model failure.
    MaxTurns,
}

impl FinishReason {
    /// Map an internal `StopReason` to its protocol-level `FinishReason`.
    pub fn from_stop_reason(sr: StopReason) -> Self {
        match sr {
            StopReason::EndTurn | StopReason::ToolUse => FinishReason::Stop,
            StopReason::MaxTokens => FinishReason::Length,
            // #457: keep the turn-cap distinct from a provider Error so the host
            // can surface Continue instead of a generic model-failure message.
            StopReason::MaxTurns => FinishReason::MaxTurns,
        }
    }
}

/// Provider-neutral token usage statistics.
///
/// Input categories are disjoint: `input_tokens` contains only uncached input,
/// while cache reads and cache creations are reported separately. Consumers
/// may sum the three fields when they need the total input processed.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TokenUsage {
    /// Uncached input tokens (cache misses).
    pub input_tokens: u64,
    pub output_tokens: u64,
    /// Input tokens written to the provider's prompt cache.
    #[serde(default)]
    pub cache_creation_tokens: u64,
    /// Input tokens read from the provider's prompt cache.
    #[serde(default)]
    pub cache_read_tokens: u64,
}

impl TokenUsage {
    /// Sum disaggregated provider input counters without wrapping.
    pub fn total_input_from(
        input_tokens: u64,
        cache_read_tokens: u64,
        cache_creation_tokens: u64,
    ) -> u64 {
        input_tokens
            .saturating_add(cache_read_tokens)
            .saturating_add(cache_creation_tokens)
    }

    /// Total provider input processed across the three disjoint input classes.
    ///
    /// Saturation keeps telemetry and budget accounting monotonic even when a
    /// malformed or synthetic provider response reports impossible counters.
    pub fn total_input_tokens(&self) -> u64 {
        Self::total_input_from(
            self.input_tokens,
            self.cache_read_tokens,
            self.cache_creation_tokens,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    // --- Role serialization / deserialization ---

    #[test]
    fn test_role_serialization_user() {
        // arrange
        let role = Role::User;
        // act
        let json = serde_json::to_string(&role).unwrap();
        // assert
        assert_eq!(json, "\"user\"");
    }

    #[test]
    fn test_role_serialization_assistant() {
        let role = Role::Assistant;
        let json = serde_json::to_string(&role).unwrap();
        assert_eq!(json, "\"assistant\"");
    }

    #[test]
    fn test_role_serialization_system() {
        let role = Role::System;
        let json = serde_json::to_string(&role).unwrap();
        assert_eq!(json, "\"system\"");
    }

    #[test]
    fn test_role_serialization_tool() {
        let role = Role::Tool;
        let json = serde_json::to_string(&role).unwrap();
        assert_eq!(json, "\"tool\"");
    }

    #[test]
    fn test_role_deserialization_roundtrip() {
        // arrange
        let variants = [
            (Role::User, "\"user\""),
            (Role::Assistant, "\"assistant\""),
            (Role::System, "\"system\""),
            (Role::Tool, "\"tool\""),
        ];
        // act + assert
        for (expected, raw) in &variants {
            let deserialized: Role = serde_json::from_str(raw).unwrap();
            assert_eq!(&deserialized, expected);
        }
    }

    // --- ContentBlock::Text ---

    #[test]
    fn test_content_block_text_construction() {
        // arrange + act
        let block = ContentBlock::Text {
            text: "hello".to_string(),
        };
        // assert
        match block {
            ContentBlock::Text { text } => assert_eq!(text, "hello"),
            _ => panic!("expected Text variant"),
        }
    }

    #[test]
    fn test_content_block_text_serialization() {
        // arrange
        let block = ContentBlock::Text {
            text: "hello world".to_string(),
        };
        // act
        let value = serde_json::to_value(&block).unwrap();
        // assert
        assert_eq!(value["type"], "text");
        assert_eq!(value["text"], "hello world");
    }

    #[test]
    fn test_content_block_image_serialization_roundtrip() {
        // arrange
        let block = ContentBlock::Image {
            mime: "image/png".to_string(),
            data: "QUJD".to_string(),
        };
        // act
        let value = serde_json::to_value(&block).unwrap();
        // assert wire tag + fields
        assert_eq!(value["type"], "image");
        assert_eq!(value["mime"], "image/png");
        assert_eq!(value["data"], "QUJD");
        // round-trips back to the same variant
        let back: ContentBlock = serde_json::from_value(value).unwrap();
        match back {
            ContentBlock::Image { mime, data } => {
                assert_eq!(mime, "image/png");
                assert_eq!(data, "QUJD");
            }
            other => panic!("expected Image, got {other:?}"),
        }
    }

    // --- ContentBlock::ToolUse ---

    #[test]
    fn test_content_block_tool_use_construction() {
        // arrange + act
        let block = ContentBlock::ToolUse {
            id: "call_1".to_string(),
            name: "bash".to_string(),
            input: json!({"cmd": "ls"}),
            extra: None,
        };
        // assert
        match &block {
            ContentBlock::ToolUse {
                id, name, input, ..
            } => {
                assert_eq!(id, "call_1");
                assert_eq!(name, "bash");
                assert_eq!(input["cmd"], "ls");
            }
            _ => panic!("expected ToolUse variant"),
        }
    }

    #[test]
    fn test_content_block_tool_use_serialization_type_field() {
        // arrange
        let block = ContentBlock::ToolUse {
            id: "call_1".to_string(),
            name: "bash".to_string(),
            input: json!({}),
            extra: None,
        };
        // act
        let value = serde_json::to_value(&block).unwrap();
        // assert – the discriminant must be "tool_use"
        assert_eq!(value["type"], "tool_use");
        assert_eq!(value["id"], "call_1");
        assert_eq!(value["name"], "bash");
    }

    // --- ContentBlock::ToolResult ---

    #[test]
    fn test_content_block_tool_result_construction() {
        // arrange + act
        let block = ContentBlock::ToolResult {
            tool_use_id: "call_1".to_string(),
            content: "output text".to_string(),
            is_error: false,
        };
        // assert
        match &block {
            ContentBlock::ToolResult {
                tool_use_id,
                content,
                is_error,
            } => {
                assert_eq!(tool_use_id, "call_1");
                assert_eq!(content, "output text");
                assert!(!is_error);
            }
            _ => panic!("expected ToolResult variant"),
        }
    }

    #[test]
    fn test_content_block_tool_result_serialization() {
        // arrange
        let block = ContentBlock::ToolResult {
            tool_use_id: "call_1".to_string(),
            content: "ok".to_string(),
            is_error: false,
        };
        // act
        let value = serde_json::to_value(&block).unwrap();
        // assert
        assert_eq!(value["type"], "tool_result");
        assert_eq!(value["tool_use_id"], "call_1");
        assert_eq!(value["is_error"], false);
    }

    // --- StopReason variants ---

    #[test]
    fn test_stop_reason_end_turn_variant() {
        let reason = StopReason::EndTurn;
        assert_eq!(reason, StopReason::EndTurn);
    }

    #[test]
    fn test_stop_reason_tool_use_variant() {
        let reason = StopReason::ToolUse;
        assert_eq!(reason, StopReason::ToolUse);
    }

    #[test]
    fn test_stop_reason_max_tokens_variant() {
        let reason = StopReason::MaxTokens;
        assert_eq!(reason, StopReason::MaxTokens);
    }

    // --- FinishReason::from_stop_reason (#457) ---

    #[test]
    fn finish_reason_maps_each_stop_reason() {
        assert_eq!(
            FinishReason::from_stop_reason(StopReason::EndTurn),
            FinishReason::Stop
        );
        assert_eq!(
            FinishReason::from_stop_reason(StopReason::ToolUse),
            FinishReason::Stop
        );
        assert_eq!(
            FinishReason::from_stop_reason(StopReason::MaxTokens),
            FinishReason::Length
        );
        // #457: the turn cap must NOT coalesce into Error — it is its own value
        // so the host can offer Continue instead of a model-failure message.
        assert_eq!(
            FinishReason::from_stop_reason(StopReason::MaxTurns),
            FinishReason::MaxTurns
        );
        assert_ne!(
            FinishReason::from_stop_reason(StopReason::MaxTurns),
            FinishReason::Error,
            "regression guard: max_turns must never map back to Error"
        );
    }

    #[test]
    fn finish_reason_max_turns_serializes_snake_case() {
        // The host contract: the new variant is exposed as "max_turns".
        let json = serde_json::to_value(FinishReason::MaxTurns).unwrap();
        assert_eq!(json, json!("max_turns"));
    }

    // --- TokenUsage default ---

    #[test]
    fn test_token_usage_default_all_zero() {
        // act
        let usage = TokenUsage::default();
        // assert
        assert_eq!(usage.input_tokens, 0);
        assert_eq!(usage.output_tokens, 0);
        assert_eq!(usage.cache_creation_tokens, 0);
        assert_eq!(usage.cache_read_tokens, 0);
        assert_eq!(usage.total_input_tokens(), 0);
    }

    #[test]
    fn total_input_tokens_sums_disjoint_categories_with_saturation() {
        let usage = TokenUsage {
            input_tokens: 100,
            output_tokens: 99,
            cache_creation_tokens: 20,
            cache_read_tokens: 80,
        };
        assert_eq!(usage.total_input_tokens(), 200);

        let overflow = TokenUsage {
            input_tokens: u64::MAX,
            output_tokens: 0,
            cache_creation_tokens: 1,
            cache_read_tokens: 1,
        };
        assert_eq!(overflow.total_input_tokens(), u64::MAX);
        assert_eq!(TokenUsage::total_input_from(u64::MAX, 1, 1), u64::MAX);
    }

    // --- Message construction ---

    #[test]
    fn test_message_construction_text_content() {
        let content = vec![ContentBlock::Text {
            text: "Hello".to_string(),
        }];
        let msg = Message::new(Role::User, content);
        assert_eq!(msg.role, Role::User);
        assert_eq!(msg.content.len(), 1);
        assert!(msg.timestamp.is_none());
        match &msg.content[0] {
            ContentBlock::Text { text } => assert_eq!(text, "Hello"),
            _ => panic!("expected Text block"),
        }
    }

    #[test]
    fn test_message_construction_mixed_content() {
        let content = vec![
            ContentBlock::Text {
                text: "Calling tool".to_string(),
            },
            ContentBlock::ToolUse {
                id: "call_2".to_string(),
                name: "search".to_string(),
                input: json!({"query": "rust"}),
                extra: None,
            },
        ];
        let msg = Message::new(Role::Assistant, content);
        assert_eq!(msg.role, Role::Assistant);
        assert_eq!(msg.content.len(), 2);
        assert!(msg.timestamp.is_none());
    }

    #[test]
    fn test_message_now_has_timestamp() {
        let before = Utc::now();
        let msg = Message::now(
            Role::User,
            vec![ContentBlock::Text {
                text: "hi".to_string(),
            }],
        );
        let after = Utc::now();
        let ts = msg.timestamp.expect("Message::now should set timestamp");
        assert!(ts >= before && ts <= after);
    }

    #[test]
    fn test_message_timestamp_serialization_roundtrip() {
        let msg = Message::now(
            Role::User,
            vec![ContentBlock::Text {
                text: "hello".to_string(),
            }],
        );
        let json = serde_json::to_string(&msg).unwrap();
        assert!(json.contains("timestamp"));

        let back: Message = serde_json::from_str(&json).unwrap();
        assert_eq!(back.timestamp, msg.timestamp);
    }

    #[test]
    fn test_message_timestamp_backward_compat_deserialization() {
        // Old JSON without timestamp field should deserialize with timestamp = None
        let json = r#"{"role":"user","content":[{"type":"text","text":"hi"}]}"#;
        let msg: Message = serde_json::from_str(json).unwrap();
        assert!(msg.timestamp.is_none());
    }

    #[test]
    fn test_message_new_skips_timestamp_in_json() {
        let msg = Message::new(
            Role::User,
            vec![ContentBlock::Text {
                text: "hi".to_string(),
            }],
        );
        let json = serde_json::to_string(&msg).unwrap();
        assert!(
            !json.contains("timestamp"),
            "None timestamp should be omitted via skip_serializing_if"
        );
    }

    // --- W1 Task 2: cache_breakpoint field ---

    #[test]
    fn message_new_initializes_cache_breakpoint_to_none() {
        let msg = Message::new(Role::User, vec![]);
        assert!(msg.cache_breakpoint.is_none());
    }

    #[test]
    fn message_now_initializes_cache_breakpoint_to_none() {
        let msg = Message::now(
            Role::Assistant,
            vec![ContentBlock::Text { text: "x".into() }],
        );
        assert!(msg.cache_breakpoint.is_none());
    }

    #[test]
    fn message_cache_breakpoint_round_trips_through_serde() {
        let mut msg = Message::new(Role::User, vec![ContentBlock::Text { text: "hi".into() }]);
        msg.cache_breakpoint = Some(MessageCacheHint::Breakpoint);
        let json = serde_json::to_value(&msg).unwrap();
        assert_eq!(json["cache_breakpoint"], "breakpoint");

        let parsed: Message = serde_json::from_value(json).unwrap();
        assert!(matches!(
            parsed.cache_breakpoint,
            Some(MessageCacheHint::Breakpoint)
        ));
    }

    #[test]
    fn message_cache_breakpoint_none_is_skipped_in_serde() {
        let msg = Message::new(Role::User, vec![]);
        let json = serde_json::to_value(&msg).unwrap();
        assert!(
            json.get("cache_breakpoint").is_none(),
            "cache_breakpoint must be absent when None to preserve wire compatibility"
        );
    }
}
