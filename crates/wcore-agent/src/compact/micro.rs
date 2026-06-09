//! Microcompact: clear old tool result content without any LLM call.
//!
//! This is the lightest compaction level.  It walks the conversation,
//! identifies tool results from compactable tools, and replaces the
//! content of all but the N most recent with a short placeholder.

use std::collections::{HashMap, HashSet};

use chrono::Utc;
use wcore_config::compact::CompactConfig;
use wcore_types::message::{ContentBlock, Message, Role};

/// Placeholder that replaces cleared tool result content.
pub const CLEARED_TOOL_RESULT: &str = "[Tool result cleared]";

/// Constant PREFIX for a Read result superseded by a later edit to the same
/// file (token-opt trajectory-pruning). A PREFIX, not an exact constant, so the
/// stub can name the file while staying idempotent: any body starting with it
/// is treated as already-cleared, so a second pass never re-mutates it.
pub const SUPERSEDED_TOOL_RESULT_PREFIX: &str = "[Stale read superseded by a later edit]";

/// Tools whose result mutates a file, invalidating earlier full reads of it.
const MUTATION_TOOLS: &[&str] = &["Edit", "Write", "MultiEdit", "NotebookEdit"];

/// Statistics returned after a microcompact pass.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MicrocompactResult {
    /// Number of tool results whose content was cleared.
    pub cleared_count: usize,
    /// Rough estimate of tokens freed (content bytes / 4).
    pub estimated_tokens_freed: usize,
}

// ── Trigger checks ──────────────────────────────────────────────────────────

/// Decide whether microcompact should run.
///
/// Returns `true` if **either** trigger fires:
/// - **Time**: the most recent assistant message is older than
///   `config.micro_gap_seconds`.
/// - **Count**: total compactable (non-cleared) tool results exceed
///   `config.micro_keep_recent * 2`.
pub fn should_microcompact(messages: &[Message], config: &CompactConfig) -> bool {
    if !config.enabled {
        return false;
    }
    time_trigger(messages, config) || count_trigger(messages, config)
}

/// Time-based trigger: last assistant timestamp older than gap threshold.
fn time_trigger(messages: &[Message], config: &CompactConfig) -> bool {
    let last_assistant_ts = messages
        .iter()
        .rev()
        .filter(|m| m.role == Role::Assistant)
        .find_map(|m| m.timestamp);

    let Some(ts) = last_assistant_ts else {
        return false;
    };

    let gap = Utc::now().signed_duration_since(ts);
    gap.num_seconds() >= config.micro_gap_seconds as i64
}

/// Count-based trigger: compactable tool results > keep_recent * 2.
fn count_trigger(messages: &[Message], config: &CompactConfig) -> bool {
    let tool_names = build_tool_name_map(messages);
    let compactable_set: HashSet<&str> = config
        .compactable_tools
        .iter()
        .map(String::as_str)
        .collect();

    let count = count_compactable_results(messages, &tool_names, &compactable_set);
    count > config.micro_keep_recent * 2
}

// ── Core compaction ─────────────────────────────────────────────────────────

/// Clear old tool result content in-place.
///
/// Keeps the `config.micro_keep_recent` most recent compactable results
/// (minimum 1) and replaces older ones with [`CLEARED_TOOL_RESULT`].
/// Already-cleared results are left untouched and do not count toward
/// the keep budget.
pub fn microcompact(messages: &mut [Message], config: &CompactConfig) -> MicrocompactResult {
    // Supersession pre-pass (token-opt trajectory-pruning): a full Read result
    // is stale once a later Edit/Write to the same file appears AND a newer read
    // of that file exists. Stub those bodies before the recency pass runs.
    let (superseded_count, superseded_tokens) = prune_superseded_reads(messages, config);

    let tool_names = build_tool_name_map(messages);
    let compactable_set: HashSet<&str> = config
        .compactable_tools
        .iter()
        .map(String::as_str)
        .collect();

    // Collect (message_index, block_index) of all compactable, non-cleared
    // tool results, in conversation order.
    let targets = collect_compactable_locations(messages, &tool_names, &compactable_set);

    let keep = config.micro_keep_recent.max(1);
    if targets.len() <= keep {
        return MicrocompactResult {
            cleared_count: superseded_count,
            estimated_tokens_freed: superseded_tokens,
        };
    }

    let to_clear = &targets[..targets.len() - keep];

    let mut cleared_count = 0usize;
    let mut tokens_freed = 0usize;

    for &(mi, bi) in to_clear {
        if let ContentBlock::ToolResult { content, .. } = &mut messages[mi].content[bi] {
            // Rough token estimate: ~4 chars per token.
            tokens_freed += content.len() / 4;
            *content = CLEARED_TOOL_RESULT.to_string();
            cleared_count += 1;
        }
    }

    MicrocompactResult {
        cleared_count: cleared_count + superseded_count,
        estimated_tokens_freed: tokens_freed + superseded_tokens,
    }
}

/// Normalize a tool-input file path for supersession matching. Conservative:
/// trims and strips a single leading `./`. Exact-match only — a missed match
/// merely keeps the read (safe); we never want a false match across files.
fn normalize_path(p: &str) -> String {
    let t = p.trim();
    t.strip_prefix("./").unwrap_or(t).to_string()
}

/// Supersession pre-pass (token-opt trajectory-pruning).
///
/// A *full* `Read` result of file P is stale once (a) a later `Edit`/`Write` to
/// P appears in history and (b) a newer `Read` result of P also exists — the
/// model holds both the edit and the fresher read, so the old full body is dead
/// weight. Replace such bodies with a constant-prefixed stub naming the file.
///
/// Conservative and idempotent:
/// - only full reads (no `offset`/`limit`) are touched;
/// - errored reads are skipped;
/// - the *freshest* read of every path is always kept;
/// - already-stubbed/cleared bodies are skipped, so a second pass is a no-op;
/// - `Read` must be in `compactable_tools` (respects the user's allow-list);
/// - exact (normalized) path match only — a missed match keeps the read.
///
/// Returns `(stubbed_count, estimated_tokens_freed)`.
fn prune_superseded_reads(messages: &mut [Message], config: &CompactConfig) -> (usize, usize) {
    if !config.enabled || !config.compactable_tools.iter().any(|t| t == "Read") {
        return (0, 0);
    }

    // tool_use_id -> (tool name, normalized file path, is a windowed/partial read)
    let mut meta: HashMap<String, (String, Option<String>, bool)> = HashMap::new();
    // path -> latest message index of a mutation (Edit/Write/...) to it.
    let mut latest_mutation: HashMap<String, usize> = HashMap::new();
    for (mi, msg) in messages.iter().enumerate() {
        for block in &msg.content {
            if let ContentBlock::ToolUse {
                id, name, input, ..
            } = block
            {
                let path = input
                    .get("file_path")
                    .and_then(|v| v.as_str())
                    .map(normalize_path);
                let partial = input.get("offset").is_some() || input.get("limit").is_some();
                if MUTATION_TOOLS.contains(&name.as_str())
                    && let Some(p) = path.clone()
                {
                    let e = latest_mutation.entry(p).or_insert(mi);
                    *e = (*e).max(mi);
                }
                meta.insert(id.clone(), (name.clone(), path, partial));
            }
        }
    }

    // path -> latest message index of a live full Read *result* of it.
    let mut freshest_read: HashMap<String, usize> = HashMap::new();
    for (mi, msg) in messages.iter().enumerate() {
        for block in &msg.content {
            if let ContentBlock::ToolResult {
                tool_use_id,
                content,
                is_error,
            } = block
                && !*is_error
                && content != CLEARED_TOOL_RESULT
                && !content.starts_with(SUPERSEDED_TOOL_RESULT_PREFIX)
                && let Some((name, Some(path), partial)) = meta.get(tool_use_id)
                && name.as_str() == "Read"
                && !*partial
            {
                let e = freshest_read.entry(path.clone()).or_insert(mi);
                *e = (*e).max(mi);
            }
        }
    }

    // Stub stale reads (read-then-write to satisfy the borrow checker).
    let mut count = 0usize;
    let mut tokens = 0usize;
    for (mi, msg) in messages.iter_mut().enumerate() {
        for bi in 0..msg.content.len() {
            let stale: Option<(String, usize)> = if let ContentBlock::ToolResult {
                tool_use_id,
                content,
                is_error,
            } = &msg.content[bi]
            {
                if *is_error
                    || content == CLEARED_TOOL_RESULT
                    || content.starts_with(SUPERSEDED_TOOL_RESULT_PREFIX)
                {
                    None
                } else if let Some((name, Some(path), partial)) = meta.get(tool_use_id) {
                    let later_edit = latest_mutation.get(path).is_some_and(|&j| j > mi);
                    let newer_read = freshest_read.get(path).is_some_and(|&f| f > mi);
                    if name.as_str() == "Read" && !*partial && later_edit && newer_read {
                        Some((path.clone(), content.len()))
                    } else {
                        None
                    }
                } else {
                    None
                }
            } else {
                None
            };

            if let Some((path, len)) = stale
                && let ContentBlock::ToolResult { content, .. } = &mut msg.content[bi]
            {
                tokens += len / 4;
                *content = format!(
                    "{SUPERSEDED_TOOL_RESULT_PREFIX} {path} — re-read if you need the current contents."
                );
                count += 1;
            }
        }
    }
    (count, tokens)
}

// ── Helpers ─────────────────────────────────────────────────────────────────

/// Build a map from tool_use_id → tool name by scanning ToolUse blocks
/// across all messages.
fn build_tool_name_map(messages: &[Message]) -> HashMap<String, String> {
    let mut map = HashMap::new();
    for msg in messages {
        for block in &msg.content {
            if let ContentBlock::ToolUse { id, name, .. } = block {
                map.insert(id.clone(), name.clone());
            }
        }
    }
    map
}

/// Count compactable, non-cleared tool results.
fn count_compactable_results(
    messages: &[Message],
    tool_names: &HashMap<String, String>,
    compactable_set: &HashSet<&str>,
) -> usize {
    messages
        .iter()
        .flat_map(|m| &m.content)
        .filter(|b| is_compactable_and_live(b, tool_names, compactable_set))
        .count()
}

/// Collect `(message_index, block_index)` of every compactable, non-cleared
/// tool result in conversation order.
fn collect_compactable_locations(
    messages: &[Message],
    tool_names: &HashMap<String, String>,
    compactable_set: &HashSet<&str>,
) -> Vec<(usize, usize)> {
    let mut locations = Vec::new();
    for (mi, msg) in messages.iter().enumerate() {
        for (bi, block) in msg.content.iter().enumerate() {
            if is_compactable_and_live(block, tool_names, compactable_set) {
                locations.push((mi, bi));
            }
        }
    }
    locations
}

/// A tool result is "compactable and live" when:
/// 1. It is a `ToolResult` variant.
/// 2. Its corresponding tool name is in the compactable set.
/// 3. Its content has not already been cleared.
fn is_compactable_and_live(
    block: &ContentBlock,
    tool_names: &HashMap<String, String>,
    compactable_set: &HashSet<&str>,
) -> bool {
    if let ContentBlock::ToolResult {
        tool_use_id,
        content,
        ..
    } = block
    {
        if content == CLEARED_TOOL_RESULT || content.starts_with(SUPERSEDED_TOOL_RESULT_PREFIX) {
            return false;
        }
        if let Some(name) = tool_names.get(tool_use_id) {
            return compactable_set.contains(name.as_str());
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Duration;
    use serde_json::json;

    // ── Test helpers ────────────────────────────────────────────────────

    fn tool_use_block(id: &str, name: &str) -> ContentBlock {
        ContentBlock::ToolUse {
            id: id.to_string(),
            name: name.to_string(),
            input: json!({}),
            extra: None,
        }
    }

    fn tool_result_block(id: &str, content: &str) -> ContentBlock {
        ContentBlock::ToolResult {
            tool_use_id: id.to_string(),
            content: content.to_string(),
            is_error: false,
        }
    }

    fn text_block(text: &str) -> ContentBlock {
        ContentBlock::Text {
            text: text.to_string(),
        }
    }

    fn assistant_msg(blocks: Vec<ContentBlock>) -> Message {
        Message::new(Role::Assistant, blocks)
    }

    fn user_msg(blocks: Vec<ContentBlock>) -> Message {
        Message::new(Role::User, blocks)
    }

    fn assistant_msg_at(blocks: Vec<ContentBlock>, ts: chrono::DateTime<Utc>) -> Message {
        Message {
            role: Role::Assistant,
            content: blocks,
            timestamp: Some(ts),
            cache_breakpoint: None,
        }
    }

    fn default_config() -> CompactConfig {
        CompactConfig::default()
    }

    // ── build_tool_name_map ─────────────────────────────────────────────

    #[test]
    fn tool_name_map_from_single_assistant() {
        let msgs = vec![assistant_msg(vec![
            tool_use_block("t1", "Read"),
            tool_use_block("t2", "Bash"),
        ])];
        let map = build_tool_name_map(&msgs);
        assert_eq!(map.get("t1").unwrap(), "Read");
        assert_eq!(map.get("t2").unwrap(), "Bash");
    }

    #[test]
    fn tool_name_map_ignores_non_tool_use() {
        let msgs = vec![
            user_msg(vec![text_block("hello")]),
            user_msg(vec![tool_result_block("t1", "output")]),
        ];
        let map = build_tool_name_map(&msgs);
        assert!(map.is_empty());
    }

    // ── is_compactable_and_live ─────────────────────────────────────────

    #[test]
    fn live_compactable_result_returns_true() {
        let tool_names: HashMap<String, String> =
            [("t1".into(), "Read".into())].into_iter().collect();
        let set: HashSet<&str> = ["Read"].into_iter().collect();
        let block = tool_result_block("t1", "file content here");
        assert!(is_compactable_and_live(&block, &tool_names, &set));
    }

    #[test]
    fn already_cleared_result_returns_false() {
        let tool_names: HashMap<String, String> =
            [("t1".into(), "Read".into())].into_iter().collect();
        let set: HashSet<&str> = ["Read"].into_iter().collect();
        let block = tool_result_block("t1", CLEARED_TOOL_RESULT);
        assert!(!is_compactable_and_live(&block, &tool_names, &set));
    }

    #[test]
    fn non_compactable_tool_returns_false() {
        let tool_names: HashMap<String, String> =
            [("t1".into(), "Skill".into())].into_iter().collect();
        let set: HashSet<&str> = ["Read", "Bash"].into_iter().collect();
        let block = tool_result_block("t1", "result");
        assert!(!is_compactable_and_live(&block, &tool_names, &set));
    }

    #[test]
    fn text_block_returns_false() {
        let tool_names = HashMap::new();
        let set: HashSet<&str> = ["Read"].into_iter().collect();
        let block = text_block("hello");
        assert!(!is_compactable_and_live(&block, &tool_names, &set));
    }

    #[test]
    fn unknown_tool_use_id_returns_false() {
        let tool_names = HashMap::new(); // no ToolUse registered
        let set: HashSet<&str> = ["Read"].into_iter().collect();
        let block = tool_result_block("orphan", "data");
        assert!(!is_compactable_and_live(&block, &tool_names, &set));
    }

    // ── time_trigger ────────────────────────────────────────────────────

    #[test]
    fn time_trigger_fires_when_gap_exceeded() {
        let old_ts = Utc::now() - Duration::seconds(3700);
        let msgs = vec![assistant_msg_at(vec![text_block("hi")], old_ts)];
        let config = CompactConfig {
            micro_gap_seconds: 3600,
            ..default_config()
        };
        assert!(time_trigger(&msgs, &config));
    }

    #[test]
    fn time_trigger_silent_when_within_gap() {
        let recent_ts = Utc::now() - Duration::seconds(1800);
        let msgs = vec![assistant_msg_at(vec![text_block("hi")], recent_ts)];
        let config = CompactConfig {
            micro_gap_seconds: 3600,
            ..default_config()
        };
        assert!(!time_trigger(&msgs, &config));
    }

    #[test]
    fn time_trigger_silent_when_no_timestamp() {
        let msgs = vec![assistant_msg(vec![text_block("hi")])];
        let config = default_config();
        assert!(!time_trigger(&msgs, &config));
    }

    #[test]
    fn time_trigger_uses_latest_assistant() {
        let old_ts = Utc::now() - Duration::seconds(7200);
        let recent_ts = Utc::now() - Duration::seconds(100);
        let msgs = vec![
            assistant_msg_at(vec![text_block("first")], old_ts),
            assistant_msg_at(vec![text_block("second")], recent_ts),
        ];
        let config = CompactConfig {
            micro_gap_seconds: 3600,
            ..default_config()
        };
        // The most recent assistant (100s ago) is within the gap.
        assert!(!time_trigger(&msgs, &config));
    }

    // ── count_trigger ───────────────────────────────────────────────────

    #[test]
    fn count_trigger_fires_above_threshold() {
        // keep_recent=3, threshold=6.  Create 7 compactable results.
        let mut msgs = Vec::new();
        for i in 0..7 {
            let id = format!("t{i}");
            msgs.push(assistant_msg(vec![tool_use_block(&id, "Read")]));
            msgs.push(user_msg(vec![tool_result_block(&id, "data")]));
        }
        let config = CompactConfig {
            micro_keep_recent: 3,
            ..default_config()
        };
        assert!(count_trigger(&msgs, &config));
    }

    #[test]
    fn count_trigger_silent_at_threshold() {
        // keep_recent=3, threshold=6.  Create exactly 6 results.
        let mut msgs = Vec::new();
        for i in 0..6 {
            let id = format!("t{i}");
            msgs.push(assistant_msg(vec![tool_use_block(&id, "Read")]));
            msgs.push(user_msg(vec![tool_result_block(&id, "data")]));
        }
        let config = CompactConfig {
            micro_keep_recent: 3,
            ..default_config()
        };
        assert!(!count_trigger(&msgs, &config));
    }

    // ── microcompact ────────────────────────────────────────────────────

    #[test]
    fn clears_oldest_keeps_recent() {
        // 5 tool results, keep_recent=2  →  clear 3.
        let mut msgs = Vec::new();
        for i in 0..5 {
            let id = format!("t{i}");
            msgs.push(assistant_msg(vec![tool_use_block(&id, "Read")]));
            msgs.push(user_msg(vec![tool_result_block(&id, &format!("data-{i}"))]));
        }
        let config = CompactConfig {
            micro_keep_recent: 2,
            ..default_config()
        };

        let result = microcompact(&mut msgs, &config);
        assert_eq!(result.cleared_count, 3);
        assert!(result.estimated_tokens_freed > 0);

        // First 3 user msgs (indices 1,3,5) should be cleared.
        for idx in [1, 3, 5] {
            let content = match &msgs[idx].content[0] {
                ContentBlock::ToolResult { content, .. } => content.as_str(),
                _ => panic!("expected ToolResult"),
            };
            assert_eq!(content, CLEARED_TOOL_RESULT);
        }
        // Last 2 user msgs (indices 7,9) should retain original content.
        for (idx, expected) in [(7, "data-3"), (9, "data-4")] {
            let content = match &msgs[idx].content[0] {
                ContentBlock::ToolResult { content, .. } => content.as_str(),
                _ => panic!("expected ToolResult"),
            };
            assert_eq!(content, expected);
        }
    }

    #[test]
    fn no_clear_when_below_keep_recent() {
        let mut msgs = vec![
            assistant_msg(vec![tool_use_block("t1", "Read")]),
            user_msg(vec![tool_result_block("t1", "data")]),
        ];
        let config = CompactConfig {
            micro_keep_recent: 5,
            ..default_config()
        };
        let result = microcompact(&mut msgs, &config);
        assert_eq!(result.cleared_count, 0);
        assert_eq!(result.estimated_tokens_freed, 0);
    }

    #[test]
    fn skips_non_compactable_tools() {
        let mut msgs = vec![
            assistant_msg(vec![tool_use_block("t1", "Read")]),
            user_msg(vec![tool_result_block("t1", "file-data")]),
            assistant_msg(vec![tool_use_block("t2", "Skill")]),
            user_msg(vec![tool_result_block("t2", "skill-output")]),
            assistant_msg(vec![tool_use_block("t3", "Bash")]),
            user_msg(vec![tool_result_block("t3", "bash-output")]),
        ];
        // compactable_tools does NOT include Skill.
        let config = CompactConfig {
            micro_keep_recent: 1,
            compactable_tools: vec!["Read".into(), "Bash".into()],
            ..default_config()
        };

        let result = microcompact(&mut msgs, &config);
        // Only Read(t1) should be cleared; Bash(t3) kept as most recent.
        assert_eq!(result.cleared_count, 1);

        // Skill result untouched.
        match &msgs[3].content[0] {
            ContentBlock::ToolResult { content, .. } => {
                assert_eq!(content, "skill-output");
            }
            _ => panic!("expected ToolResult"),
        }
    }

    #[test]
    fn does_not_recleared_already_cleared() {
        let mut msgs = vec![
            assistant_msg(vec![tool_use_block("t1", "Read")]),
            user_msg(vec![tool_result_block("t1", CLEARED_TOOL_RESULT)]),
            assistant_msg(vec![tool_use_block("t2", "Read")]),
            user_msg(vec![tool_result_block("t2", "live-data")]),
        ];
        let config = CompactConfig {
            micro_keep_recent: 1,
            ..default_config()
        };
        let result = microcompact(&mut msgs, &config);
        // t1 already cleared → not in compactable list.
        // Only t2 is compactable, and it's the most recent → keep it.
        assert_eq!(result.cleared_count, 0);
    }

    #[test]
    fn empty_messages_returns_zero() {
        let mut msgs: Vec<Message> = Vec::new();
        let result = microcompact(&mut msgs, &default_config());
        assert_eq!(result.cleared_count, 0);
        assert_eq!(result.estimated_tokens_freed, 0);
    }

    #[test]
    fn message_count_and_order_preserved() {
        let mut msgs = vec![
            assistant_msg(vec![tool_use_block("t1", "Read")]),
            user_msg(vec![tool_result_block("t1", &"a".repeat(100))]),
            assistant_msg(vec![tool_use_block("t2", "Read")]),
            user_msg(vec![tool_result_block("t2", &"b".repeat(100))]),
            assistant_msg(vec![tool_use_block("t3", "Read")]),
            user_msg(vec![tool_result_block("t3", &"c".repeat(100))]),
        ];
        let original_len = msgs.len();
        let config = CompactConfig {
            micro_keep_recent: 1,
            ..default_config()
        };
        microcompact(&mut msgs, &config);

        assert_eq!(msgs.len(), original_len);
        // Roles alternate: Assistant, User, Assistant, User, ...
        for (i, msg) in msgs.iter().enumerate() {
            let expected = if i % 2 == 0 {
                Role::Assistant
            } else {
                Role::User
            };
            assert_eq!(msg.role, expected);
        }
    }

    #[test]
    fn token_estimate_proportional_to_content() {
        let long_content = "x".repeat(400); // ~100 tokens
        let mut msgs = vec![
            assistant_msg(vec![tool_use_block("t1", "Read")]),
            user_msg(vec![tool_result_block("t1", &long_content)]),
            assistant_msg(vec![tool_use_block("t2", "Read")]),
            user_msg(vec![tool_result_block("t2", "keep")]),
        ];
        let config = CompactConfig {
            micro_keep_recent: 1,
            ..default_config()
        };
        let result = microcompact(&mut msgs, &config);
        assert_eq!(result.cleared_count, 1);
        assert_eq!(result.estimated_tokens_freed, 100); // 400 / 4
    }

    // ── should_microcompact ─────────────────────────────────────────────

    #[test]
    fn should_returns_false_when_disabled() {
        let old_ts = Utc::now() - Duration::seconds(7200);
        let msgs = vec![assistant_msg_at(vec![text_block("hi")], old_ts)];
        let config = CompactConfig {
            enabled: false,
            micro_gap_seconds: 3600,
            ..default_config()
        };
        assert!(!should_microcompact(&msgs, &config));
    }

    #[test]
    fn keep_recent_floored_at_one() {
        // Even with keep_recent=0, we never clear everything.
        let mut msgs = vec![
            assistant_msg(vec![tool_use_block("t1", "Read")]),
            user_msg(vec![tool_result_block("t1", "data-1")]),
            assistant_msg(vec![tool_use_block("t2", "Read")]),
            user_msg(vec![tool_result_block("t2", "data-2")]),
        ];
        let config = CompactConfig {
            micro_keep_recent: 0,
            ..default_config()
        };
        let result = microcompact(&mut msgs, &config);
        // 2 compactable, keep at least 1 → clear 1.
        assert_eq!(result.cleared_count, 1);
        // The most recent (t2) must survive.
        match &msgs[3].content[0] {
            ContentBlock::ToolResult { content, .. } => {
                assert_eq!(content, "data-2");
            }
            _ => panic!("expected ToolResult"),
        }
    }

    // ── trajectory-pruning: supersession pre-pass ───────────────────────

    fn read_use(id: &str, path: &str) -> ContentBlock {
        ContentBlock::ToolUse {
            id: id.to_string(),
            name: "Read".to_string(),
            input: json!({ "file_path": path }),
            extra: None,
        }
    }
    fn read_use_window(id: &str, path: &str) -> ContentBlock {
        ContentBlock::ToolUse {
            id: id.to_string(),
            name: "Read".to_string(),
            input: json!({ "file_path": path, "offset": 1, "limit": 20 }),
            extra: None,
        }
    }
    fn edit_use(id: &str, path: &str) -> ContentBlock {
        ContentBlock::ToolUse {
            id: id.to_string(),
            name: "Edit".to_string(),
            input: json!({ "file_path": path }),
            extra: None,
        }
    }
    fn read_only_cfg() -> CompactConfig {
        CompactConfig {
            compactable_tools: vec!["Read".into()],
            ..default_config()
        }
    }

    #[test]
    fn supersedes_stale_read_after_edit_and_reread() {
        let mut msgs = vec![
            assistant_msg(vec![read_use("r1", "src/x.rs")]),
            user_msg(vec![tool_result_block(
                "r1",
                &"old contents v1 ".repeat(20),
            )]),
            assistant_msg(vec![edit_use("e1", "src/x.rs")]),
            user_msg(vec![tool_result_block("e1", "edit applied")]),
            assistant_msg(vec![read_use("r2", "src/x.rs")]),
            user_msg(vec![tool_result_block(
                "r2",
                &"new contents v2 ".repeat(20),
            )]),
        ];
        let (count, tokens) = prune_superseded_reads(&mut msgs, &read_only_cfg());
        assert_eq!(count, 1, "the pre-edit read must be stubbed");
        assert!(tokens > 0);
        // r1 result (index 1) is now a superseded stub naming the file.
        match &msgs[1].content[0] {
            ContentBlock::ToolResult { content, .. } => {
                assert!(content.starts_with(SUPERSEDED_TOOL_RESULT_PREFIX));
                assert!(content.contains("src/x.rs"));
            }
            _ => panic!("expected ToolResult"),
        }
        // r2 result (index 5, the fresh post-edit read) is untouched.
        match &msgs[5].content[0] {
            ContentBlock::ToolResult { content, .. } => assert!(content.contains("v2")),
            _ => panic!("expected ToolResult"),
        }
    }

    #[test]
    fn supersession_pass_is_idempotent() {
        let mut msgs = vec![
            assistant_msg(vec![read_use("r1", "a.rs")]),
            user_msg(vec![tool_result_block("r1", &"x".repeat(80))]),
            assistant_msg(vec![edit_use("e1", "a.rs")]),
            user_msg(vec![tool_result_block("e1", "ok")]),
            assistant_msg(vec![read_use("r2", "a.rs")]),
            user_msg(vec![tool_result_block("r2", &"y".repeat(80))]),
        ];
        let (first, _) = prune_superseded_reads(&mut msgs, &read_only_cfg());
        assert_eq!(first, 1);
        let stub_after_first = match &msgs[1].content[0] {
            ContentBlock::ToolResult { content, .. } => content.clone(),
            _ => panic!("expected ToolResult"),
        };
        let (second, second_tokens) = prune_superseded_reads(&mut msgs, &read_only_cfg());
        assert_eq!(second, 0, "a second pass must be a no-op");
        assert_eq!(second_tokens, 0);
        let stub_after_second = match &msgs[1].content[0] {
            ContentBlock::ToolResult { content, .. } => content.clone(),
            _ => panic!("expected ToolResult"),
        };
        assert_eq!(
            stub_after_first, stub_after_second,
            "the stub must not be re-mutated on a second pass"
        );
    }

    #[test]
    fn never_supersedes_a_partial_read() {
        let mut msgs = vec![
            assistant_msg(vec![read_use_window("r1", "p.rs")]),
            user_msg(vec![tool_result_block("r1", &"windowed slice ".repeat(20))]),
            assistant_msg(vec![edit_use("e1", "p.rs")]),
            user_msg(vec![tool_result_block("e1", "ok")]),
            assistant_msg(vec![read_use("r2", "p.rs")]),
            user_msg(vec![tool_result_block("r2", &"full ".repeat(20))]),
        ];
        let (count, _) = prune_superseded_reads(&mut msgs, &read_only_cfg());
        assert_eq!(count, 0, "partial reads are never superseded");
    }

    #[test]
    fn keeps_the_freshest_read_even_with_a_later_edit() {
        // Read then Edit with NO verify-read: the single read is the freshest
        // view of the file, so it is conservatively kept.
        let mut msgs = vec![
            assistant_msg(vec![read_use("r1", "z.rs")]),
            user_msg(vec![tool_result_block("r1", &"only read ".repeat(20))]),
            assistant_msg(vec![edit_use("e1", "z.rs")]),
            user_msg(vec![tool_result_block("e1", "ok")]),
        ];
        let (count, _) = prune_superseded_reads(&mut msgs, &read_only_cfg());
        assert_eq!(count, 0, "the only/freshest read of a file is always kept");
    }

    #[test]
    fn no_edit_means_no_supersession() {
        let mut msgs = vec![
            assistant_msg(vec![read_use("r1", "q.rs")]),
            user_msg(vec![tool_result_block("r1", &"v ".repeat(20))]),
            assistant_msg(vec![read_use("r2", "q.rs")]),
            user_msg(vec![tool_result_block("r2", &"v ".repeat(20))]),
        ];
        let (count, _) = prune_superseded_reads(&mut msgs, &read_only_cfg());
        assert_eq!(count, 0, "supersession requires an intervening edit");
    }

    #[test]
    fn errored_read_is_not_superseded() {
        let mut msgs = vec![
            assistant_msg(vec![read_use("r1", "e.rs")]),
            user_msg(vec![ContentBlock::ToolResult {
                tool_use_id: "r1".into(),
                content: "permission denied ".repeat(5),
                is_error: true,
            }]),
            assistant_msg(vec![edit_use("e1", "e.rs")]),
            user_msg(vec![tool_result_block("e1", "ok")]),
            assistant_msg(vec![read_use("r2", "e.rs")]),
            user_msg(vec![tool_result_block("r2", &"full ".repeat(20))]),
        ];
        let (count, _) = prune_superseded_reads(&mut msgs, &read_only_cfg());
        assert_eq!(count, 0, "errored reads carry signal and are never stubbed");
    }
}
