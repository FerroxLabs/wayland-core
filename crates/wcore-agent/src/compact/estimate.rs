use wcore_types::message::{ContentBlock, Message};
use wcore_types::tool::ToolDef;

const CHARS_PER_TOKEN_TEXT: usize = 4;

const CHARS_PER_TOKEN_JSON: usize = 3;

/// Flat per-image token charge. Images are not char-countable, so decoding
/// them in this hot estimator is avoided; instead each inline image is charged
/// a constant at the HIGH end of provider vision-token accounting. Anthropic's
/// patch-based cost reaches ~4784 tokens for a large image (≈3888 at 2000x1500,
/// ≈2691 at 1920x1080); OpenAI/Gemini high-detail costs sit below that. The
/// EMERGENCY compaction watermark must never undercount, so this deliberately
/// over-estimates: firing compaction early is safe, overflowing the window is
/// not. A composer-dropped image is typically a full-resolution screenshot, so
/// the ceiling — not an average — is the correct charge.
const TOKENS_PER_IMAGE: usize = 4800;

/// Estimate the total token count for a slice of messages.
///
/// Intentionally conservative (slightly over-estimates) to ensure
/// compaction triggers rather than being skipped. Counts historical
/// thinking blocks — use this for the EMERGENCY hard-stop watermark,
/// which must never undercount.
pub fn estimate_tokens_from_messages(messages: &[Message]) -> u64 {
    estimate_tokens_from_messages_inner(messages, true)
}

/// Finding #174 — estimate token count, excluding historical thinking
/// blocks when the provider does NOT replay them in history
/// (`count_thinking = false`).
///
/// Anthropic/Bedrock/Vertex drop historical thinking at the wire
/// (`ContentBlock::Thinking => None`), so those tokens cost zero real
/// input on the next turn. Counting them inflates the estimate and
/// fires the AUTO-compaction trigger earlier than real token pressure
/// warrants. Pass `count_thinking = compat.replays_thinking_in_history()`
/// so the AUTO watermark tracks real billing. The EMERGENCY path still
/// uses `estimate_tokens_from_messages` (thinking counted, conservative).
pub fn estimate_tokens_from_messages_with_thinking(
    messages: &[Message],
    count_thinking: bool,
) -> u64 {
    estimate_tokens_from_messages_inner(messages, count_thinking)
}

fn estimate_tokens_from_messages_inner(messages: &[Message], count_thinking: bool) -> u64 {
    let mut total_chars: usize = 0;
    let mut json_chars: usize = 0;
    let mut image_tokens: usize = 0;

    for msg in messages {
        for block in &msg.content {
            match block {
                ContentBlock::Text { text } => {
                    total_chars += text.len();
                }
                ContentBlock::Thinking { thinking, .. } => {
                    if count_thinking {
                        total_chars += thinking.len();
                    }
                }
                ContentBlock::ToolUse { name, input, .. } => {
                    let input_str = input.to_string();
                    json_chars += name.len() + input_str.len();
                }
                ContentBlock::ToolResult { content, .. } => {
                    total_chars += content.len();
                }
                // Charge a flat conservative constant per inline image rather
                // than counting the base64 payload (which would wildly
                // over-count against the model's real vision token cost).
                ContentBlock::Image { .. } => {
                    image_tokens += TOKENS_PER_IMAGE;
                }
            }
        }
    }

    let text_tokens = total_chars / CHARS_PER_TOKEN_TEXT;
    let json_tokens = json_chars / CHARS_PER_TOKEN_JSON;

    (text_tokens + json_tokens + image_tokens) as u64
}

/// AUDIT A5 — estimate the token count of a FULL request: messages
/// plus the system prompt plus the serialized tool definitions.
///
/// `estimate_tokens_from_messages` counts only message content, so on
/// turn 1 (when the provider has not yet reported `input_tokens`) the
/// compaction watermark undercounts by the size of the system prompt
/// and the tool schema — which, for MCP-heavy configs, can be tens of
/// thousands of tokens. This helper adds both so the turn-1 watermark
/// and the context-ceiling guard reflect what is actually sent.
pub fn estimate_request_tokens(messages: &[Message], system: &str, tools: &[ToolDef]) -> u64 {
    let messages_tokens = estimate_tokens_from_messages(messages);
    let system_tokens = (system.len() / CHARS_PER_TOKEN_TEXT) as u64;
    let mut tool_chars: usize = 0;
    for tool in tools {
        tool_chars += tool.name.len();
        tool_chars += tool.description.len();
        // Tool input schemas are JSON; charge them at the JSON ratio.
        tool_chars += tool.input_schema.to_string().len();
    }
    let tool_tokens = (tool_chars / CHARS_PER_TOKEN_JSON) as u64;
    messages_tokens + system_tokens + tool_tokens
}

/// The UN-COMPACTABLE FLOOR of a turn: the system prompt plus the tool
/// schemas, computed from THIS assembled request rather than assumed --
/// FerroxLabs/wayland#1230 c1.
///
/// # What un-compactable means, precisely
///
/// The degradation rungs the engine can pull shrink the CONVERSATION and
/// nothing else: rung 1 sheds tool RESULTS, rung 2 truncates or drops
/// MESSAGES. Neither can touch the system prompt or the tool array, so
/// whatever those two cost is charged on every request the session will ever
/// send -- including the one before the user has typed anything. That is the
/// floor, and it is exactly [`estimate_request_tokens`] evaluated at an empty
/// message list. The identity
///
/// ```text
/// estimate_request_tokens(msgs, sys, tools) >= uncompactable_floor_tokens(sys, tools)
/// ```
///
/// holds for EVERY `msgs`, which is what makes this a floor rather than an
/// estimate of one; `floor_is_a_floor_for_every_message_list` pins it.
///
/// # Why it is computed and not read off a constant
///
/// [`wcore_config::compact::BASELINE_TURN_TOKENS`] is a MEASURED SNAPSHOT of
/// this quantity (3,118 real prompt tokens; #1172 drove a real qwen3:8b
/// through a logging proxy and read it off the usage block the endpoint
/// returned). It moves whenever the system prompt is edited, a built-in tool
/// is added, an MCP server registers, or a skill lands in the index -- and
/// nothing regrades it. #1230 c2 pins that drift with a test; this function
/// is what a caller uses when it needs the floor of the session in hand
/// rather than a snapshot of some other one.
///
/// # Live figure
///
/// Measured 2026-08-31 on hetzner-dsm, against a private Ollama on port 21434
/// serving qwen3:8b at CONTEXT 4096: this function returned 3,619 tokens for
/// the default turn, against 3,207 real prompt tokens reported for the same
/// turn on the wire. The char/4 estimator therefore runs about 13% HIGH, i.e.
/// conservative for a floor -- it refuses slightly early rather than slightly
/// late, which is the correct direction for a guard whose failure mode is
/// silent truncation.
pub fn uncompactable_floor_tokens(system: &str, tools: &[ToolDef]) -> u64 {
    estimate_request_tokens(&[], system, tools)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use wcore_types::message::{Message, Role};

    fn tool(name: &str, description: &str) -> ToolDef {
        ToolDef {
            name: name.to_string(),
            description: description.to_string(),
            input_schema: json!({"type": "object", "properties": {"path": {"type": "string"}}}),
            deferred: false,
            server: None,
        }
    }

    /// #1230 c1 -- the floor is the part of a request no degradation rung can
    /// reach, so it must be a LOWER BOUND on every request the session sends,
    /// not merely the size of one particular turn.
    ///
    /// The failure this catches is the substituted property: a floor that is
    /// really the estimate of turn 1 passes any test written against turn 1
    /// and is wrong on turn 2. Sweeping message lists -- empty, one small user
    /// turn, and a 26,054-character tool result (the exact payload #1230
    /// measured Ollama discarding) -- is what separates them.
    #[test]
    fn floor_is_a_floor_for_every_message_list() {
        let system = "You are a helpful agent.".repeat(40);
        let tools = [tool("Read", "Read a file"), tool("Bash", "Run a command")];
        let floor = uncompactable_floor_tokens(&system, &tools);
        assert!(
            floor > 0,
            "a real system prompt plus tools cannot cost nothing"
        );

        let lists: Vec<Vec<Message>> = vec![
            vec![],
            vec![Message::new(
                Role::User,
                vec![ContentBlock::Text { text: "hi".into() }],
            )],
            vec![Message::new(
                Role::User,
                vec![ContentBlock::ToolResult {
                    tool_use_id: "t1".to_string(),
                    content: "x".repeat(26_054),
                    is_error: false,
                }],
            )],
        ];
        for messages in &lists {
            let full = estimate_request_tokens(messages, &system, &tools);
            assert!(
                full >= floor,
                "floor {floor} exceeded the full estimate {full} for a {}-message \
                 request -- then it is not a floor",
                messages.len()
            );
        }
    }

    /// The floor must move when the TOOL SET moves, which is the whole reason
    /// #1230 c2 refuses to trust a constant. A floor that ignored its tools
    /// argument would still pass the bound test above and be useless.
    #[test]
    fn floor_tracks_the_tool_schemas_it_is_given() {
        let system = "sys";
        let lean = uncompactable_floor_tokens(system, &[tool("Read", "Read a file")]);
        let fat = uncompactable_floor_tokens(
            system,
            &[
                tool("Read", "Read a file"),
                tool("Bash", &"Run a shell command. ".repeat(50)),
            ],
        );
        assert!(
            fat > lean,
            "adding a tool schema left the floor at {lean} -- the floor is not \
             reading the tool array"
        );
    }

    #[test]
    fn empty_messages_returns_zero() {
        assert_eq!(estimate_tokens_from_messages(&[]), 0);
    }

    #[test]
    fn text_only_message() {
        let text = "a".repeat(400);
        let msg = Message::new(Role::User, vec![ContentBlock::Text { text }]);
        assert_eq!(estimate_tokens_from_messages(&[msg]), 100);
    }

    #[test]
    fn image_block_charges_flat_constant() {
        // An inline image is charged the flat per-image constant regardless of
        // its (tiny) base64 payload length — not counted as text chars.
        let msg = Message::new(
            Role::User,
            vec![ContentBlock::Image {
                mime: "image/png".into(),
                data: "QUJD".into(),
            }],
        );
        assert_eq!(
            estimate_tokens_from_messages(&[msg]),
            TOKENS_PER_IMAGE as u64
        );
    }

    #[test]
    fn tool_use_message_uses_json_ratio() {
        let input = json!({"cmd": "ls -la"});
        let input_len = "Bash".len() + input.to_string().len();
        let msg = Message::new(
            Role::Assistant,
            vec![ContentBlock::ToolUse {
                id: "call_1".into(),
                name: "Bash".into(),
                input,
                extra: None,
            }],
        );
        let result = estimate_tokens_from_messages(&[msg]);
        assert_eq!(result, (input_len / CHARS_PER_TOKEN_JSON) as u64);
    }

    #[test]
    fn tool_result_uses_text_ratio() {
        let content = "x".repeat(800);
        let msg = Message::new(
            Role::User,
            vec![ContentBlock::ToolResult {
                tool_use_id: "call_1".into(),
                content,
                is_error: false,
            }],
        );
        assert_eq!(estimate_tokens_from_messages(&[msg]), 200);
    }

    #[test]
    fn mixed_conversation_accumulates() {
        let messages = vec![
            Message::new(
                Role::User,
                vec![ContentBlock::Text {
                    text: "a".repeat(400),
                }],
            ),
            Message::new(
                Role::Assistant,
                vec![
                    ContentBlock::Text {
                        text: "b".repeat(200),
                    },
                    ContentBlock::ToolUse {
                        id: "c1".into(),
                        name: "Read".into(),
                        input: json!({"path": "/foo/bar.rs"}),
                        extra: None,
                    },
                ],
            ),
            Message::new(
                Role::User,
                vec![ContentBlock::ToolResult {
                    tool_use_id: "c1".into(),
                    content: "c".repeat(1200),
                    is_error: false,
                }],
            ),
        ];
        let estimate = estimate_tokens_from_messages(&messages);
        // text_tokens = (400 + 200 + 1200) / 4 = 450
        // json_tokens = ("Read".len() + json_string.len()) / 3
        assert!(estimate > 450);
        assert!(estimate < 600);
    }

    #[test]
    fn thinking_block_counted() {
        let thinking = "t".repeat(4000);
        let msg = Message::new(
            Role::Assistant,
            vec![ContentBlock::Thinking {
                thinking,
                extra: None,
            }],
        );
        assert_eq!(estimate_tokens_from_messages(&[msg]), 1000);
    }

    // Finding #174 — for providers that DROP historical thinking at the
    // wire (`count_thinking = false`), the estimate must NOT charge for it.
    // Without the exclusion the estimate over-counts by the thinking size,
    // which is what fired the AUTO trigger prematurely.
    #[test]
    fn thinking_excluded_when_not_replayed() {
        let thinking = "t".repeat(40_000); // 10k tokens if counted
        let text = "x".repeat(400); // 100 real text tokens
        let msg = Message::new(
            Role::Assistant,
            vec![
                ContentBlock::Thinking {
                    thinking,
                    extra: None,
                },
                ContentBlock::Text { text },
            ],
        );

        // count_thinking=false (Anthropic/Bedrock/Vertex): only the text counts.
        assert_eq!(
            estimate_tokens_from_messages_with_thinking(std::slice::from_ref(&msg), false),
            100,
            "wire-dropped thinking must not inflate the estimate"
        );
        // count_thinking=true (DeepSeek/Moonshot replay): thinking IS billed.
        assert_eq!(
            estimate_tokens_from_messages_with_thinking(std::slice::from_ref(&msg), true),
            100 + 10_000,
            "replaying providers must still count thinking (no regression)"
        );
        // The conservative emergency-path entry point always counts thinking.
        assert_eq!(
            estimate_tokens_from_messages(std::slice::from_ref(&msg)),
            100 + 10_000,
        );
    }

    #[test]
    fn large_conversation_realistic_estimate() {
        let big_result = "x".repeat(400_000);
        let messages = vec![Message::new(
            Role::User,
            vec![ContentBlock::ToolResult {
                tool_use_id: "c1".into(),
                content: big_result,
                is_error: false,
            }],
        )];
        let estimate = estimate_tokens_from_messages(&messages);
        assert_eq!(estimate, 100_000);
    }

    #[test]
    fn effective_watermark_uses_max() {
        let provider_reported: u64 = 500;
        let messages = vec![Message::new(
            Role::User,
            vec![ContentBlock::ToolResult {
                tool_use_id: "c1".into(),
                content: "x".repeat(400_000),
                is_error: false,
            }],
        )];
        let local_estimate = estimate_tokens_from_messages(&messages);
        let effective = provider_reported.max(local_estimate);

        assert_eq!(effective, 100_000);
        assert!(effective > provider_reported);
    }

    // AUDIT A5 — estimate_request_tokens must include system + tools.

    #[test]
    fn request_tokens_includes_system_and_tools() {
        use wcore_types::tool::ToolDef;
        let messages = vec![Message::new(
            Role::User,
            vec![ContentBlock::Text {
                text: "a".repeat(400), // 100 text tokens
            }],
        )];
        let system = "s".repeat(400); // 100 system tokens
        let tools = vec![ToolDef {
            name: "Read".into(),
            description: "reads".into(),
            input_schema: json!({"type": "object"}),
            deferred: false,
            server: None,
        }];

        let messages_only = estimate_tokens_from_messages(&messages);
        let full = estimate_request_tokens(&messages, &system, &tools);

        assert_eq!(messages_only, 100);
        assert!(
            full > messages_only,
            "the full estimate must add system + tool tokens on top of \
             the message estimate (full={full}, messages_only={messages_only})"
        );
        // System prompt alone contributes 100 tokens; tools contribute
        // at least a few. The full estimate must clear messages+system.
        assert!(full >= messages_only + 100);
    }

    #[test]
    fn request_tokens_with_no_system_no_tools_equals_messages() {
        let messages = vec![Message::new(
            Role::User,
            vec![ContentBlock::Text {
                text: "b".repeat(800),
            }],
        )];
        assert_eq!(
            estimate_request_tokens(&messages, "", &[]),
            estimate_tokens_from_messages(&messages),
        );
    }
}
