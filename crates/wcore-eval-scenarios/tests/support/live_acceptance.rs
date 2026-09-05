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
    let requested_file = std::fs::canonicalize(path).ok();
    let calls: Vec<_> = messages
        .iter()
        .flat_map(|message| &message.content)
        .filter_map(|block| match block {
            ContentBlock::ToolUse {
                id, name, input, ..
            } if name == "Read" => {
                let candidate = input.get("file_path")?.as_str()?;
                // The file stays alive throughout the provider test. Compare
                // its identity across platform/alias spellings, not just text.
                let same_file = candidate == path
                    || requested_file.as_ref().is_some_and(|expected| {
                        std::fs::canonicalize(candidate).is_ok_and(|actual| actual == *expected)
                    });
                same_file.then_some(id)
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
