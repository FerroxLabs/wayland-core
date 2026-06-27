//! End-to-end: helper sets the hint → anthropic_shared::build_messages
//! translates it to cache_control on the last content block.

use serde_json::json;
use wcore_config::compat::ProviderCompat;
use wcore_observability::cache::mark_cache_boundaries;
use wcore_providers::anthropic_shared;
use wcore_types::llm::LlmRequest;
use wcore_types::message::{ContentBlock, Message, Role};

fn req_with_messages(messages: Vec<Message>) -> LlmRequest {
    LlmRequest {
        model: "claude-haiku".into(),
        system: "sys".into(),
        messages,
        tools: vec![],
        max_tokens: 1024,
        thinking: None,
        reasoning_effort: None,
        cache_tier: None,
        routing_hint: None,
        stop_sequences: Vec::new(),
        web_search: false,
        conversation_id: None,
        client_context_tokens: None,
        temperature: None,
    }
}

#[test]
fn anthropic_build_messages_places_cache_control_on_marked_message() {
    let mut req = req_with_messages(vec![
        Message::new(
            Role::User,
            vec![ContentBlock::Text {
                text: "turn1".into(),
            }],
        ),
        Message::new(
            Role::User,
            vec![ContentBlock::Text {
                text: "turn2".into(),
            }],
        ),
    ]);
    let compat = ProviderCompat::anthropic_defaults();

    mark_cache_boundaries(&mut req, &compat);
    let built = anthropic_shared::build_messages(&req.messages, &compat);

    // Last message's last content block must carry cache_control.
    let last = built.last().expect("at least one message");
    let content = last["content"].as_array().expect("content array");
    let last_block = content.last().expect("at least one content block");
    assert_eq!(
        last_block["cache_control"],
        json!({ "type": "ephemeral" }),
        "tail content block must carry ephemeral cache_control"
    );

    // First message must NOT carry cache_control.
    let first = &built[0];
    let first_content = first["content"].as_array().unwrap();
    assert!(
        first_content[0].get("cache_control").is_none(),
        "non-tail messages must not carry cache_control"
    );
}

#[test]
fn openai_compat_results_in_no_cache_control_anywhere() {
    let mut req = req_with_messages(vec![Message::new(
        Role::User,
        vec![ContentBlock::Text { text: "hi".into() }],
    )]);
    let compat = ProviderCompat::openai_defaults();

    mark_cache_boundaries(&mut req, &compat);
    let built = anthropic_shared::build_messages(&req.messages, &compat);

    for msg in built {
        let content = msg["content"].as_array().unwrap();
        for block in content {
            assert!(
                block.get("cache_control").is_none(),
                "openai compat must never set cache_control on a content block: {block:?}"
            );
        }
    }
}
