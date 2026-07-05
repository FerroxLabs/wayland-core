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

/// Marker key stamped into a `ToolUse.input` object whose arguments were
/// compacted by [`compact_tool_call_args`] (parity gap 2). Presence of this
/// key means "already compacted — never re-mutate", which is what makes the
/// pass monotonic: once a call's arguments are stubbed at turn K, the block
/// serializes byte-identically at every later turn (prompt-cache safety).
pub const COMPACTED_ARGS_KEY: &str = "_args_cleared";

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

// ── Tool-call-argument compaction (parity gap 2) ────────────────────────────

/// Compact HISTORICAL assistant tool-call ARGUMENTS in place.
///
/// The mirror image of the tool-RESULT clearing above: on real workloads the
/// biggest resent-history slice is the assistant's own `tool_calls` argument
/// payloads (Write file bodies up to ~19 KB each) riding in every subsequent
/// request forever. This pass replaces such payloads with a small valid-JSON
/// stub once the call is older than the last `keep_recent_turns` assistant
/// turns (the model may still reference recent args; older ones are dead
/// weight — the outcome is in the tool result and, for file writes, on disk)
/// AND the epoch boundary below has ticked past it.
///
/// Modeled on hermes-agent's `_truncate_tool_call_args_json`
/// (context_compressor.py:1130-1150), but run CONTINUOUSLY on every
/// compaction pipeline pass rather than at a compression threshold.
///
/// CACHE-SAFETY invariants (prompt-cache prefix stability):
/// - **Deterministic**: the stub is a pure function of the original
///   arguments (serialized size, sorted top-level keys, `file_path`), built
///   as a `serde_json` object — BTreeMap-backed, so keys serialize in sorted
///   order every time.
/// - **Monotonic**: a compacted input carries [`COMPACTED_ARGS_KEY`]; the
///   pass skips it forever after, so a message compacts exactly once and its
///   bytes never change again. Growth of history only moves the protected
///   tail forward — it can newly compact a message, never un-compact one.
/// - **Epoch-quantized boundary** (cache economics, GLM byte-walk audit):
///   monotonicity alone is not enough. If the stub boundary advanced every
///   turn, exactly one message would flip verbatim→stub INSIDE the
///   previously-cached prefix each turn; provider prefix-matching is
///   contiguous, so the byte-identical protected tail AFTER the flip would
///   re-bill at full price EVERY turn (steady-state cache miss ≈ 2x the
///   no-compaction baseline; break-even only past ~11 turns). Instead the
///   boundary advances only in steps of `epoch_turns` (default 4): the count
///   of eligible assistant turns is `floor((A - keep) / E) * E` where `A` is
///   the total number of assistant messages. Between ticks the boundary is
///   FROZEN — zero mid-prefix byte changes, full prefix reuse; at a tick one
///   batch of `E` turns flips at the deepest possible point, one prefix
///   rewrite per `E` turns instead of per turn. The formula is a pure
///   function of the message list (no persisted state), so it is stable
///   across retries, restarts and session rehydration, and monotone because
///   `A` only grows. `epoch_turns = 1` degenerates to the per-turn boundary.
///
/// The stub keeps the block's `id`/`name`/`extra` untouched (provider
/// round-trip metadata such as thought signatures survives) and preserves a
/// top-level `file_path` key when present — the supersession pre-pass above
/// keys its mutation map off `input.file_path`, and the model keeps the
/// recovery handle (it can Read the file if it needs the content).
///
/// Returns a [`MicrocompactResult`] with the number of argument payloads
/// stubbed and a rough token estimate freed.
pub fn compact_tool_call_args(
    messages: &mut [Message],
    config: &CompactConfig,
) -> MicrocompactResult {
    let tca = &config.tool_call_args;
    if !config.enabled || !tca.enabled {
        return MicrocompactResult {
            cleared_count: 0,
            estimated_tokens_freed: 0,
        };
    }

    // Epoch-quantized boundary (see the doc comment): stub the oldest
    // `floor((A - keep) / E) * E` assistant turns. Always ≤ A - keep, so
    // nothing newer than the last `keep` assistant turns is ever touched;
    // between epoch ticks the count is constant, so the pass changes zero
    // bytes and the provider's contiguous prefix cache holds end-to-end.
    let keep = tca.keep_recent_turns.max(1);
    let epoch = tca.epoch_turns.max(1);
    let total_assistant = messages
        .iter()
        .filter(|m| m.role == Role::Assistant)
        .count();
    let eligible = (total_assistant.saturating_sub(keep) / epoch) * epoch;
    if eligible == 0 {
        return MicrocompactResult {
            cleared_count: 0,
            estimated_tokens_freed: 0,
        };
    }

    let mut cleared_count = 0usize;
    let mut tokens_freed = 0usize;
    let mut ordinal = 0usize;
    for msg in messages.iter_mut() {
        if msg.role != Role::Assistant {
            continue;
        }
        if ordinal >= eligible {
            break;
        }
        ordinal += 1;
        for block in &mut msg.content {
            let ContentBlock::ToolUse { name, input, .. } = block else {
                continue;
            };
            // Monotonicity: never re-mutate an already-compacted input.
            if input.get(COMPACTED_ARGS_KEY).is_some() {
                continue;
            }
            let serialized_len = serde_json::to_string(&*input).map(|s| s.len()).unwrap_or(0);
            if serialized_len < tca.min_args_bytes {
                continue;
            }
            let stub = args_stub(name, input, serialized_len);
            let stub_len = serde_json::to_string(&stub).map(|s| s.len()).unwrap_or(0);
            tokens_freed += serialized_len.saturating_sub(stub_len) / 4;
            *input = stub;
            cleared_count += 1;
        }
    }

    MicrocompactResult {
        cleared_count,
        estimated_tokens_freed: tokens_freed,
    }
}

/// Build the deterministic replacement `input` for a compacted tool call.
///
/// Shape (keys serialize sorted — serde_json maps are BTreeMap-backed):
/// - `file_path` (when the original args carried one, e.g. Write/Edit):
///   preserved verbatim as the recovery handle;
/// - [`COMPACTED_ARGS_KEY`]: a one-line summary — original byte count plus,
///   generically, the sorted top-level argument keys — telling the model how
///   to recover (Read the file / the outcome is in the tool result).
fn args_stub(name: &str, input: &serde_json::Value, original_bytes: usize) -> serde_json::Value {
    let mut obj = serde_json::Map::new();
    let summary = if let Some(path) = input.get("file_path").and_then(|v| v.as_str()) {
        obj.insert(
            "file_path".to_string(),
            serde_json::Value::String(path.to_string()),
        );
        format!(
            "[Old tool arguments cleared: {name} {path} ({original_bytes} bytes) — \
             the content is on disk; Read the file if you need it.]"
        )
    } else {
        let mut keys: Vec<&str> = input
            .as_object()
            .map(|o| o.keys().map(String::as_str).collect())
            .unwrap_or_default();
        keys.sort_unstable();
        format!(
            "[Old tool arguments cleared: {name} keys [{}] ({original_bytes} bytes) — \
             the outcome is in the tool result; re-run the tool if you need them.]",
            keys.join(", ")
        )
    };
    obj.insert(
        COMPACTED_ARGS_KEY.to_string(),
        serde_json::Value::String(summary),
    );
    serde_json::Value::Object(obj)
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

    // ── parity gap 2: tool-call-argument compaction ─────────────────────

    use wcore_config::compact::ToolCallArgsConfig;

    fn write_use(id: &str, path: &str, body: &str) -> ContentBlock {
        ContentBlock::ToolUse {
            id: id.to_string(),
            name: "Write".to_string(),
            input: json!({ "file_path": path, "content": body }),
            extra: None,
        }
    }

    fn bash_use(id: &str, command: &str) -> ContentBlock {
        ContentBlock::ToolUse {
            id: id.to_string(),
            name: "Bash".to_string(),
            input: json!({ "command": command, "description": "run" }),
            extra: None,
        }
    }

    /// Boundary-semantics test config: `epoch_turns = 1` (no quantization) so
    /// the keep/threshold/stub behavior is exercised at every turn. Epoch
    /// quantization has its own dedicated tests below.
    fn tca_cfg(keep_recent_turns: usize, min_args_bytes: usize) -> CompactConfig {
        tca_cfg_epoch(keep_recent_turns, min_args_bytes, 1)
    }

    fn tca_cfg_epoch(
        keep_recent_turns: usize,
        min_args_bytes: usize,
        epoch_turns: usize,
    ) -> CompactConfig {
        CompactConfig {
            tool_call_args: ToolCallArgsConfig {
                enabled: true,
                keep_recent_turns,
                min_args_bytes,
                epoch_turns,
            },
            ..default_config()
        }
    }

    /// A Write turn (assistant tool call + tool result) followed by nothing.
    fn write_turn(msgs: &mut Vec<Message>, id: &str, path: &str, body: &str) {
        msgs.push(assistant_msg(vec![write_use(id, path, body)]));
        msgs.push(user_msg(vec![tool_result_block(id, "File written")]));
    }

    fn tool_use_input(msg: &Message) -> &serde_json::Value {
        match &msg.content[0] {
            ContentBlock::ToolUse { input, .. } => input,
            _ => panic!("expected ToolUse"),
        }
    }

    #[test]
    fn old_write_args_stubbed_recent_verbatim() {
        let body = "x".repeat(4000);
        let mut msgs = Vec::new();
        write_turn(&mut msgs, "w1", "src/old.rs", &body); // oldest assistant turn
        write_turn(&mut msgs, "w2", "src/mid.rs", &body); // 2nd-from-last: protected
        write_turn(&mut msgs, "w3", "src/new.rs", &body); // last: protected
        let cfg = tca_cfg(2, 768);

        let result = compact_tool_call_args(&mut msgs, &cfg);
        assert_eq!(result.cleared_count, 1, "only the pre-tail Write compacts");
        assert!(result.estimated_tokens_freed > 500);

        // w1 (index 0): stubbed — marker present, file_path + byte count kept,
        // recovery text tells the model to Read the file.
        let stubbed = tool_use_input(&msgs[0]);
        let summary = stubbed[COMPACTED_ARGS_KEY].as_str().unwrap();
        assert_eq!(stubbed["file_path"], "src/old.rs");
        assert!(summary.contains("src/old.rs"));
        assert!(summary.contains("bytes"));
        assert!(summary.contains("Read the file"));
        assert!(stubbed.get("content").is_none(), "the body must be gone");

        // w2 + w3: verbatim.
        for idx in [2, 4] {
            let input = tool_use_input(&msgs[idx]);
            assert_eq!(input["content"].as_str().unwrap(), body);
            assert!(input.get(COMPACTED_ARGS_KEY).is_none());
        }
    }

    #[test]
    fn generic_args_stub_lists_top_level_keys() {
        let long_cmd = "echo ".to_string() + &"y".repeat(2000);
        let mut msgs = vec![
            assistant_msg(vec![bash_use("b1", &long_cmd)]),
            user_msg(vec![tool_result_block("b1", "ok")]),
        ];
        write_turn(&mut msgs, "w1", "a.rs", "small");
        write_turn(&mut msgs, "w2", "b.rs", "small");
        let cfg = tca_cfg(2, 768);

        let result = compact_tool_call_args(&mut msgs, &cfg);
        assert_eq!(result.cleared_count, 1);
        let stubbed = tool_use_input(&msgs[0]);
        let summary = stubbed[COMPACTED_ARGS_KEY].as_str().unwrap();
        assert!(
            summary.contains("[command, description]"),
            "sorted top-level keys must be listed: {summary}"
        );
        assert!(summary.contains("bytes"));
    }

    #[test]
    fn determinism_same_history_serializes_byte_identically() {
        let body = "d".repeat(3000);
        let mut msgs = Vec::new();
        for (i, p) in ["one.rs", "two.rs", "three.rs", "four.rs"]
            .iter()
            .enumerate()
        {
            write_turn(&mut msgs, &format!("w{i}"), p, &body);
        }
        let cfg = tca_cfg(2, 768);

        compact_tool_call_args(&mut msgs, &cfg);
        let first = serde_json::to_string(&msgs).unwrap();
        // A second pass over the same history (e.g. a retry rebuilding the
        // same turn) must be a byte-level no-op.
        let again = compact_tool_call_args(&mut msgs, &cfg);
        assert_eq!(again.cleared_count, 0);
        assert_eq!(again.estimated_tokens_freed, 0);
        let second = serde_json::to_string(&msgs).unwrap();
        assert_eq!(first, second, "serialization must be byte-identical");
    }

    #[test]
    fn monotonic_adding_turns_never_changes_stubbed_bytes() {
        let body = "m".repeat(3000);
        let mut msgs = Vec::new();
        write_turn(&mut msgs, "w0", "zero.rs", &body);
        write_turn(&mut msgs, "w1", "one.rs", &body);
        write_turn(&mut msgs, "w2", "two.rs", &body);
        let cfg = tca_cfg(2, 768);

        compact_tool_call_args(&mut msgs, &cfg);
        let stub_at_k = serde_json::to_string(&msgs[0]).unwrap();

        // Two more turns land; w1 crosses the boundary and newly compacts,
        // but w0's already-compacted bytes must not move.
        write_turn(&mut msgs, "w3", "three.rs", &body);
        write_turn(&mut msgs, "w4", "four.rs", &body);
        let result = compact_tool_call_args(&mut msgs, &cfg);
        assert!(result.cleared_count >= 1, "w1/w2 newly cross the boundary");
        let stub_later = serde_json::to_string(&msgs[0]).unwrap();
        assert_eq!(
            stub_at_k, stub_later,
            "a compacted message must serialize byte-identically at every later turn"
        );
    }

    #[test]
    fn below_threshold_args_untouched() {
        let mut msgs = Vec::new();
        write_turn(&mut msgs, "w0", "tiny.rs", "short body"); // well under 768 B
        write_turn(&mut msgs, "w1", "a.rs", "x");
        write_turn(&mut msgs, "w2", "b.rs", "x");
        let cfg = tca_cfg(2, 768);

        let result = compact_tool_call_args(&mut msgs, &cfg);
        assert_eq!(result.cleared_count, 0);
        let input = tool_use_input(&msgs[0]);
        assert_eq!(input["content"], "short body");
        assert!(input.get(COMPACTED_ARGS_KEY).is_none());
    }

    #[test]
    fn config_off_is_byte_identical_to_today() {
        let body = "o".repeat(5000);
        let mut msgs = Vec::new();
        for i in 0..4 {
            write_turn(&mut msgs, &format!("w{i}"), &format!("f{i}.rs"), &body);
        }
        let before = serde_json::to_string(&msgs).unwrap();

        // Feature gate off.
        let mut off = tca_cfg(2, 768);
        off.tool_call_args.enabled = false;
        let result = compact_tool_call_args(&mut msgs, &off);
        assert_eq!(result.cleared_count, 0);
        assert_eq!(before, serde_json::to_string(&msgs).unwrap());

        // Master compaction gate off.
        let mut master_off = tca_cfg(2, 768);
        master_off.enabled = false;
        let result = compact_tool_call_args(&mut msgs, &master_off);
        assert_eq!(result.cleared_count, 0);
        assert_eq!(before, serde_json::to_string(&msgs).unwrap());
    }

    #[test]
    fn keep_recent_turns_floored_at_one() {
        let body = "f".repeat(2000);
        let mut msgs = Vec::new();
        write_turn(&mut msgs, "w0", "a.rs", &body);
        write_turn(&mut msgs, "w1", "b.rs", &body);
        let cfg = tca_cfg(0, 768); // 0 → floored to 1

        let result = compact_tool_call_args(&mut msgs, &cfg);
        assert_eq!(
            result.cleared_count, 1,
            "only w0; the last turn is protected"
        );
        let last = tool_use_input(&msgs[2]);
        assert_eq!(last["content"].as_str().unwrap(), body);
    }

    #[test]
    fn file_path_survives_for_supersession_matching() {
        // The supersession pre-pass keys latest_mutation off Write/Edit
        // `input.file_path`. A compacted old Write must still register as a
        // mutation of its path so even-older reads keep getting pruned.
        let body = "s".repeat(3000);
        let mut msgs = vec![
            assistant_msg(vec![read_use("r1", "src/x.rs")]),
            user_msg(vec![tool_result_block("r1", &"old v1 ".repeat(50))]),
        ];
        write_turn(&mut msgs, "w1", "src/x.rs", &body);
        msgs.push(assistant_msg(vec![read_use("r2", "src/x.rs")]));
        msgs.push(user_msg(vec![tool_result_block(
            "r2",
            &"new v2 ".repeat(50),
        )]));
        write_turn(&mut msgs, "w2", "other.rs", &body);
        write_turn(&mut msgs, "w3", "other2.rs", &body);

        let mut cfg = tca_cfg(2, 768);
        cfg.compactable_tools = vec!["Read".into()];
        compact_tool_call_args(&mut msgs, &cfg);
        // w1's args are now stubbed but its file_path survives...
        let w1 = tool_use_input(&msgs[2]);
        assert!(w1.get(COMPACTED_ARGS_KEY).is_some());
        assert_eq!(w1["file_path"], "src/x.rs");
        // ...so supersession still prunes the pre-write read of src/x.rs.
        let (count, _) = prune_superseded_reads(&mut msgs, &cfg);
        assert_eq!(count, 1, "stale pre-write read must still be superseded");
        match &msgs[1].content[0] {
            ContentBlock::ToolResult { content, .. } => {
                assert!(content.starts_with(SUPERSEDED_TOOL_RESULT_PREFIX));
            }
            _ => panic!("expected ToolResult"),
        }
    }

    // ── epoch quantization (cache-economics fix, GLM byte-walk audit) ───

    #[test]
    fn epoch_boundary_frozen_between_ticks() {
        // keep=2, epoch=4. Eligible count = floor((A - 2) / 4) * 4:
        // frozen between ticks (zero mid-prefix byte changes = the provider's
        // contiguous prefix cache holds), one batch flip per epoch tick.
        let body = "e".repeat(2000);
        let cfg = tca_cfg_epoch(2, 768, 4);
        let mut msgs = Vec::new();
        for i in 0..5 {
            write_turn(&mut msgs, &format!("w{i}"), &format!("f{i}.rs"), &body);
        }

        // A=5 → floor(3/4)*4 = 0: below the first tick, nothing stubbed even
        // though w0..w2 are older than the last 2 assistant turns.
        let r = compact_tool_call_args(&mut msgs, &cfg);
        assert_eq!(r.cleared_count, 0, "pre-tick: boundary has not advanced");

        // A=6 → floor(4/4)*4 = 4: first tick stubs w0..w3 in ONE batch.
        write_turn(&mut msgs, "w5", "f5.rs", &body);
        let r = compact_tool_call_args(&mut msgs, &cfg);
        assert_eq!(r.cleared_count, 4, "epoch tick stubs a batch of 4");
        for idx in [8, 10] {
            // w4, w5 verbatim (protected tail is >= keep at the tick).
            assert!(tool_use_input(&msgs[idx]).get(COMPACTED_ARGS_KEY).is_none());
        }
        let frozen = serde_json::to_string(&msgs).unwrap();
        let first_batch = serde_json::to_string(&msgs[..8]).unwrap();

        // A=7,8,9 → still 4 eligible: the pass changes ZERO bytes of the
        // existing messages — the whole previous request stays a cache hit.
        let pre_len = msgs.len();
        for i in 6..9 {
            write_turn(&mut msgs, &format!("w{i}"), &format!("f{i}.rs"), &body);
            let r = compact_tool_call_args(&mut msgs, &cfg);
            assert_eq!(r.cleared_count, 0, "between ticks the boundary is frozen");
        }
        let existing = serde_json::to_string(&msgs[..pre_len]).unwrap();
        assert_eq!(
            frozen, existing,
            "between ticks, previously-sent messages must be byte-identical"
        );

        // A=10 → floor(8/4)*4 = 8: second tick stubs w4..w7 (next batch of 4)
        // and the first batch's bytes still never move.
        write_turn(&mut msgs, "w9", "f9.rs", &body);
        let r = compact_tool_call_args(&mut msgs, &cfg);
        assert_eq!(r.cleared_count, 4, "second tick stubs the next batch of 4");
        assert_eq!(
            first_batch,
            serde_json::to_string(&msgs[..8]).unwrap(),
            "first-batch stubs are byte-stable across ticks"
        );
    }

    #[test]
    fn epoch_never_reaches_the_keep_window() {
        // floor((A - keep) / E) * E <= A - keep for all A: even exactly at a
        // tick, the last `keep` assistant turns stay verbatim.
        let body = "k".repeat(2000);
        let cfg = tca_cfg_epoch(2, 768, 4);
        let mut msgs = Vec::new();
        for i in 0..6 {
            write_turn(&mut msgs, &format!("w{i}"), &format!("g{i}.rs"), &body);
        }
        compact_tool_call_args(&mut msgs, &cfg); // tick: A=6, eligible=4
        for idx in [8, 10] {
            let input = tool_use_input(&msgs[idx]);
            assert_eq!(input["content"].as_str().unwrap(), body);
        }
    }

    #[test]
    fn synthetic_19_turn_capture_shape_byte_reduction() {
        // Mimics the reference capture: 9 user prompts, Write bodies from
        // ~2 KB to ~19 KB (45 KB of args in the biggest turn), a serial-Bash
        // tail — then measures serialized history bytes before/after as seen
        // by the final (19th) request.
        let mut msgs = Vec::new();
        let mut w = 0usize;
        let mut write_sizes = vec![18_895usize, 9_200, 6_400, 4_100, 2_300, 15_700, 11_280];
        for stage in 0..9 {
            msgs.push(user_msg(vec![text_block(&format!(
                "Stage {stage}: build the next piece. {}",
                "prompt prose ".repeat(120)
            ))]));
            if let Some(size) = write_sizes.pop() {
                let body = "fn main() { /* generated */ }\n".repeat(size / 30);
                write_turn(
                    &mut msgs,
                    &format!("w{w}"),
                    &format!("src/gen{w}.rs"),
                    &body,
                );
                w += 1;
            }
            msgs.push(assistant_msg(vec![text_block("Done with this stage.")]));
        }
        // 7-round serial Bash deploy/verify tail (small args, stays verbatim).
        for i in 0..7 {
            msgs.push(assistant_msg(vec![bash_use(
                &format!("b{i}"),
                "npm run verify && git status",
            )]));
            msgs.push(user_msg(vec![tool_result_block(&format!("b{i}"), "ok")]));
        }

        let before = serde_json::to_string(&msgs).unwrap().len();
        // Ship defaults: keep=2, min=768, epoch=4. A=23 assistant messages →
        // eligible = floor(21/4)*4 = 20; all 7 Writes (ordinals ≤ 12) are past
        // the epoch boundary at the final request.
        let cfg = tca_cfg_epoch(2, 768, 4);
        let result = compact_tool_call_args(&mut msgs, &cfg);
        let after = serde_json::to_string(&msgs).unwrap().len();

        assert_eq!(result.cleared_count, 7, "all 7 Write bodies are historical");
        let freed = before - after;
        println!(
            "synthetic 19-turn history: {before} B -> {after} B \
             (freed {freed} B, {:.1}% of history; ~{} tokens)",
            100.0 * freed as f64 / before as f64,
            freed / 4
        );
        assert!(
            freed > 60_000,
            "expected >60 KB freed from the Write bodies, got {freed}"
        );
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
