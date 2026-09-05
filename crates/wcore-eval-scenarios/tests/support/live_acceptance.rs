//! Acceptance shared by actual provider tests and adversarial fixture controls.

pub fn successful_read_answer(
    text: &str,
    sentinel: &str,
    _turns: usize,
    read_succeeded: bool,
) -> bool {
    read_succeeded && !sentinel.is_empty() && text.contains(sentinel)
}

pub fn observed_read(
    messages: &[wcore_types::message::Message],
    path: &str,
    sentinel: &str,
) -> bool {
    use wcore_types::message::ContentBlock;
    let calls: Vec<_> = messages
        .iter()
        .flat_map(|message| &message.content)
        .filter_map(|block| match block {
            ContentBlock::ToolUse {
                id, name, input, ..
            } if name == "Read"
                && input.get("file_path").and_then(serde_json::Value::as_str) == Some(path) =>
            {
                Some(id)
            }
            _ => None,
        })
        .collect();
    messages
        .iter()
        .flat_map(|message| &message.content)
        .any(|block| {
            matches!(block,
        ContentBlock::ToolResult { tool_use_id, content, is_error: false }
            if calls.contains(&tool_use_id) && content.contains(sentinel))
        })
}
