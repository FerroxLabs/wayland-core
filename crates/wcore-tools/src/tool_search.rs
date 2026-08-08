use std::collections::HashSet;

use async_trait::async_trait;
use parking_lot::Mutex;
use serde_json::{Value, json};

use wcore_protocol::events::ToolCategory;
use wcore_types::tool::{JsonSchema, ToolDef, ToolResult};

use crate::Tool;
use crate::context::ToolContext;

/// Wave RA — check `ctx.cancel` every N items so a `ToolSearch` against a
/// huge registry returns within ~500ms of a cancel signal instead of
/// running to completion. 100 is a balance between cancel-latency and the
/// overhead of repeated `is_cancelled()` calls on a `CancellationToken`.
const CANCEL_CHECK_INTERVAL: usize = 100;

/// A token found in a tool's NAME is worth this many times the same token
/// found in its description. The name is what the caller has to type to call
/// the tool; prose is a hint about it.
const NAME_WEIGHT: usize = 2;

/// Upper bound on returned matches. Each match carries the tool's FULL input
/// schema, so an unbounded relaxed match could pour an entire MCP catalogue
/// into one tool result. Matches are RANKED before the cut, so this is a
/// relevance cut, not an arbitrary one.
const MAX_MATCHES: usize = 10;

/// Status line on a tool this instance has not returned before.
const STATUS_FIRST_LOAD: &str = "LOADED — this tool is now callable by name. \
     Call it directly on your next step; searching for it again returns this \
     same result and makes no progress.";

/// Status line on a tool this instance has already returned. See the
/// repeat-search note in [`ToolSearchTool::execute_with_ctx`].
const STATUS_ALREADY_LOADED: &str = "ALREADY LOADED — an earlier ToolSearch \
     in this session already returned this tool, and it has been callable by \
     name ever since. Searching again cannot change that. Call it directly, \
     now.";

/// Built-in tool that searches for deferred tools and loads their full schema.
/// Core tool (never deferred itself) — always available to the LLM.
pub struct ToolSearchTool {
    /// Snapshot of all tool definitions (taken at construction time).
    tool_defs: Vec<ToolDef>,
    /// Names this instance has already returned as loaded.
    ///
    /// The snapshot above cannot tell a hydrated tool from a still-deferred
    /// one — that state lives on the engine — so remembering what we handed
    /// out is the only thing this tool can do to stop the second answer being
    /// the same bytes as the first. See the repeat-search note in
    /// [`Self::execute_with_ctx`] for the measured loop that motivates it.
    already_returned: Mutex<HashSet<String>>,
}

impl ToolSearchTool {
    pub fn new(tool_defs: Vec<ToolDef>) -> Self {
        Self {
            tool_defs,
            already_returned: Mutex::new(HashSet::new()),
        }
    }
}

#[async_trait]
impl Tool for ToolSearchTool {
    fn name(&self) -> &str {
        "ToolSearch"
    }

    fn description(&self) -> &str {
        "Search for deferred tools and load their full schema. Returns the \
         best-matching tools, most relevant first (up to 10). \
         Use this ONCE before calling a deferred tool: a match makes that tool \
         immediately callable by name on your next step. Call it directly — do \
         NOT search for the same tool again, because repeating the search \
         makes no further progress."
    }

    fn input_schema(&self) -> JsonSchema {
        json!({
            "type": "object",
            "properties": {
                "query": {
                    "type": "string",
                    "description": "Tool name or keyword to search for"
                }
            },
            "required": ["query"]
        })
    }

    fn is_concurrency_safe(&self, _input: &Value) -> bool {
        true
    }

    async fn execute(&self, input: Value) -> ToolResult {
        self.execute_with_ctx(input, &ToolContext::test_default())
            .await
    }

    /// Wave RA RELIABILITY MAJOR #3 — periodic cancel check so a search
    /// against a large tool registry doesn't run to completion after the
    /// agent cancelled. Iterates `tool_defs` manually so we can poll
    /// `ctx.cancel.is_cancelled()` every [`CANCEL_CHECK_INTERVAL`] items.
    /// Cancel returns an `is_error: true` ToolResult with a cancellation
    /// message — matches BashTool / McpToolProxy / BrowserTool shape.
    async fn execute_with_ctx(&self, input: Value, ctx: &ToolContext) -> ToolResult {
        let query = input["query"].as_str().unwrap_or("");
        if query.is_empty() {
            return ToolResult {
                content: "Error: query is required".to_string(),
                is_error: true,
            };
        }

        let query_lower = query.to_lowercase();

        // TOKENS, not one substring. `contains(&query_lower)` required the
        // ENTIRE query to appear verbatim in a single name or description, so
        // any natural multi-word query could only ever miss: "tvcontrol
        // TradingView chart" has to be present literally, and no tool
        // description will ever contain it. Measured in this repo's own live
        // runs: `tiny_ping` matched, `tiny_ping tool` returned "No deferred
        // tools matching" — same tool, one extra word.
        //
        // Credited to the Wayland Desktop lane, which found and proved this
        // independently.
        //
        // Punctuation is trimmed off each token's EDGES. `split_whitespace()`
        // leaves it glued on, so `aion_list_models,` — a verbatim query from a
        // captured session, because models write tool names into prose lists —
        // could never be a substring of `aion_list_models`. Edges only: `_`,
        // `-` and `.` INSIDE a name are part of the name, and splitting on
        // them would turn a search for `synthetic_tool_7` into a search for
        // the token `tool`, which matches most of any catalogue.
        let mut tokens: Vec<&str> = query_lower
            .split_whitespace()
            .map(|t| t.trim_matches(|c: char| !c.is_alphanumeric() && c != '_'))
            .filter(|t| !t.is_empty())
            .collect();

        // A one-character token is a substring of a large share of any
        // catalogue ("a" is inside `fs_cat`), so it contributes noise to the
        // ranking below and can outrank the tool the caller actually named.
        // Kept only when it is all the caller gave us.
        if tokens.iter().any(|t| t.chars().count() > 1) {
            tokens.retain(|t| t.chars().count() > 1);
        }
        // Repeating a word must not multiply its weight.
        let mut seen: HashSet<&str> = HashSet::new();
        tokens.retain(|t| seen.insert(*t));

        if tokens.is_empty() {
            // Whitespace- or punctuation-only: non-empty by the guard above,
            // but no token to match on. Matching everything here would be
            // worse than useless.
            return ToolResult {
                content: "Error: query is required".to_string(),
                is_error: true,
            };
        }

        // RANK, do not gate. Requiring EVERY token to match (the previous
        // rule) made a longer, more descriptive query strictly LESS likely to
        // succeed than a terser one, which is backwards: "wld_probe_secret
        // tool schema parameters" names the tool exactly and then says what
        // the caller wants from it, and "schema"/"parameters" are words ABOUT
        // a tool that appear nowhere in a real one's prose. Measured cost of
        // that rule: one captured claude-sonnet-5 session, 28 tool calls,
        // every one of them ToolSearch, 19 of them returning no match.
        //
        // The reason the old rule was AND rather than OR still stands — a
        // chatty query OR-matched against a big catalogue buries the tool the
        // caller meant. Ranking answers that directly instead of by exclusion:
        // score each tool, best first, and cut at MAX_MATCHES.
        //
        // Score = Σ over matched tokens of the token's LENGTH, doubled when
        // the token is in the NAME. Length is the specificity proxy that makes
        // this work: on the query above, `aion_describe_tool` matches four of
        // the query's words to `wld_probe_secret`'s one, so a plain
        // matched-token COUNT ranks the decoy first and the fix would have
        // traded "no match" for "buried" — which reads the same to a model.
        // One long, specific token outweighs a handful of short generic ones.
        let mut scored: Vec<(usize, usize)> = Vec::new(); // (score, def index)

        for (idx, def) in self.tool_defs.iter().enumerate() {
            if idx % CANCEL_CHECK_INTERVAL == 0 && ctx.cancel.is_cancelled() {
                return ToolResult {
                    content: "ToolSearch cancelled by cancellation token".to_string(),
                    is_error: true,
                };
            }
            if !def.deferred {
                continue;
            }
            let name_l = def.name.to_lowercase();
            let desc_l = def.description.to_lowercase();
            let score: usize = tokens
                .iter()
                .map(|t| {
                    if name_l.contains(t) {
                        t.len() * NAME_WEIGHT
                    } else if desc_l.contains(t) {
                        t.len()
                    } else {
                        0
                    }
                })
                .sum();
            // The floor. Zero matched tokens is still NO MATCH — without it,
            // "rank instead of require-all" degrades into "match everything",
            // and every positive case above would still look fixed.
            if score > 0 {
                scored.push((score, idx));
            }
        }

        // Stable sort: tools with equal scores keep registry order.
        scored.sort_by_key(|&(score, _)| std::cmp::Reverse(score));
        scored.truncate(MAX_MATCHES);

        // Final cancel check before producing output; lets a cancel that
        // fired in the tail of a fast iteration still be observed.
        if ctx.cancel.is_cancelled() {
            return ToolResult {
                content: "ToolSearch cancelled by cancellation token".to_string(),
                is_error: true,
            };
        }

        // `status` is the repair for a MEASURED no-progress loop, not
        // decoration. A match hydrates the tool engine-side and it is
        // genuinely callable on the next turn (measured: after one search,
        // `tiny_ping` appears in the model's own callable list). But this tool
        // answers from a CONSTRUCTION-TIME snapshot that still marks it
        // deferred, so a second search used to return the byte-identical
        // result — which the model reads as "the schema still has not loaded"
        // and searches again. Observed against a real MCP server: ten
        // identical searches, no call ever attempted, and the run killed by
        // the engine's own repeated-tool-call guard. Every MCP tool was
        // unreachable this way, not merely a large server's.
        //
        // The snapshot still cannot express hydration — the hydrated set lives
        // on the engine (`AgentEngine::hydrated_tool_names`), not here. What
        // this tool CAN do is remember what it already handed out, so a repeat
        // search is not the same bytes and says so in words. A rebuild via
        // `refresh_tool_search_catalog` resets that memory, which is correct:
        // a rebuilt catalog is a new snapshot and a re-search of it is a
        // genuine first load.
        let mut already_returned = self.already_returned.lock();
        let matches: Vec<Value> = scored
            .iter()
            .map(|&(_, idx)| {
                let def = &self.tool_defs[idx];
                let first_load = already_returned.insert(def.name.clone());
                json!({
                    "name": def.name,
                    "description": def.description,
                    "parameters": def.input_schema,
                    "status": if first_load { STATUS_FIRST_LOAD } else { STATUS_ALREADY_LOADED },
                })
            })
            .collect();
        drop(already_returned);

        if matches.is_empty() {
            return ToolResult {
                content: format!("No deferred tools matching \"{}\" found.", query),
                is_error: false,
            };
        }

        ToolResult {
            content: serde_json::to_string_pretty(&matches).unwrap_or_default(),
            is_error: false,
        }
    }

    fn category(&self) -> ToolCategory {
        ToolCategory::Info
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn build_tool_defs() -> Vec<ToolDef> {
        vec![
            ToolDef {
                name: "Read".into(),
                description: "Read a file".into(),
                input_schema: json!({"type": "object", "properties": {"path": {"type": "string"}}}),
                deferred: false,
                server: None,
            },
            ToolDef {
                name: "SpawnTool".into(),
                description: "Spawn sub-agents".into(),
                input_schema: json!({"type": "object", "properties": {"agents": {"type": "array"}}}),
                deferred: true,
                server: None,
            },
            ToolDef {
                name: "EnterPlanMode".into(),
                description: "Enter plan mode".into(),
                input_schema: json!({"type": "object", "properties": {}}),
                deferred: true,
                server: None,
            },
        ]
    }

    fn deferred_def(name: &str, description: &str) -> ToolDef {
        ToolDef {
            name: name.into(),
            description: description.into(),
            input_schema: json!({"type": "object", "properties": {}}),
            deferred: true,
            server: None,
        }
    }

    /// The catalogue SHAPE from the captured claude-sonnet-5 session that
    /// motivated finding C-5: MCP tools with long snake_case names and terse
    /// descriptions, sitting next to tools whose PROSE carries the generic
    /// words a model sprinkles into a query — "tool", "schema", "parameters".
    ///
    /// `aion_describe_tool` and `wld_render_report` are the decoys. They exist
    /// so a ranking rule that merely counts matched tokens is not good enough:
    /// on the measured query below `aion_describe_tool` matches FOUR of the
    /// query's words (one in its name, three in its prose) while the tool the
    /// caller actually named matches exactly one. Counting alone buries it.
    fn build_measured_defs() -> Vec<ToolDef> {
        vec![
            ToolDef {
                name: "Read".into(),
                description: "Read a file".into(),
                input_schema: json!({"type": "object"}),
                deferred: false,
                server: None,
            },
            deferred_def("wld_probe_secret", "Probe a stored secret value"),
            deferred_def(
                "aion_list_models",
                "List the inference models this account can use",
            ),
            deferred_def(
                "aion_describe_tool",
                "Describe a tool: its schema and its parameters",
            ),
            deferred_def(
                "wld_render_report",
                "Render a report from a tool result, with schema and parameters",
            ),
            deferred_def("tv_chart_set_symbol", "Change the chart symbol"),
        ]
    }

    fn match_names(content: &str) -> Vec<String> {
        let parsed: serde_json::Value = serde_json::from_str(content)
            .unwrap_or_else(|_| panic!("a match set must be a JSON array; got: {content}"));
        parsed
            .as_array()
            .expect("match set must be an array")
            .iter()
            .map(|m| {
                m.get("name")
                    .and_then(|n| n.as_str())
                    .expect("every match carries a name")
                    .to_string()
            })
            .collect()
    }

    /// C-5, defect 1a. VERBATIM query from the captured session.
    ///
    /// The matcher required EVERY whitespace token to appear in the name or the
    /// description, so a longer, more descriptive query was strictly LESS
    /// likely to match than a terser one. "schema" and "parameters" are words
    /// ABOUT the tool, not words IN it — nothing in a real catalogue puts them
    /// in `wld_probe_secret`'s prose — so naming the tool exactly and then
    /// saying what you wanted from it made the tool unreachable.
    ///
    /// Measured cost: one captured session, 28 tool calls, every one of them
    /// ToolSearch, 19 returning no match, on claude-sonnet-5.
    ///
    /// MUTANT: restore `tokens.iter().all(...)` and this fails.
    ///
    /// Asserts on the PARSED match set, never on `content.contains(name)`: the
    /// miss message echoes the query back — `No deferred tools matching
    /// "wld_probe_secret tool schema parameters" found.` — so a `contains`
    /// check is true on both branches and can never fail. (Caught by the red
    /// run of this very test, and already documented on
    /// `a_late_registered_tool_is_invisible_until_the_catalog_is_refreshed`.)
    #[tokio::test]
    async fn a_descriptive_query_finds_the_tool_it_names() {
        let tool = ToolSearchTool::new(build_measured_defs());
        let result = tool
            .execute(json!({"query": "wld_probe_secret tool schema parameters"}))
            .await;
        assert!(!result.is_error);
        assert!(
            !result.content.starts_with("No deferred tools matching"),
            "a query that names the tool exactly must find it, however chatty \
             the rest of the query is; got: {}",
            result.content
        );
        let names = match_names(&result.content);
        assert!(
            names.iter().any(|n| n == "wld_probe_secret"),
            "the named tool must be in the match set; got: {names:?}"
        );
    }

    /// C-5, defect 1b. VERBATIM query from the captured session, trailing comma
    /// and all — models write tool names into prose lists.
    ///
    /// Tokens came straight out of `split_whitespace()`, so the punctuation
    /// stayed glued to the token and `aion_list_models,` could never be a
    /// substring of `aion_list_models`. The tool was unreachable by its own
    /// name.
    ///
    /// MUTANT: drop the punctuation trim from the tokeniser and this fails.
    ///
    /// Parsed match set, not `contains` — see the note on the test above.
    #[tokio::test]
    async fn a_trailing_comma_does_not_hide_the_tool() {
        let tool = ToolSearchTool::new(build_measured_defs());
        let result = tool.execute(json!({"query": "aion_list_models,"})).await;
        assert!(!result.is_error);
        assert!(
            !result.content.starts_with("No deferred tools matching"),
            "punctuation must be trimmed off a token, not searched for; got: {}",
            result.content
        );
        assert_eq!(match_names(&result.content), vec!["aion_list_models"]);
    }

    /// No-regression floor for the single-token case that always worked, AND
    /// the precision floor for the relaxed matcher: "probe" must return the one
    /// tool it names, not the catalogue.
    ///
    /// MUTANT: drop the score floor (admit every deferred tool) and the second
    /// assertion fails — "rank instead of require-all" degrading into "match
    /// everything" is the obvious wrong way to fix defect 1.
    #[tokio::test]
    async fn a_single_token_query_still_matches_and_stays_narrow() {
        let tool = ToolSearchTool::new(build_measured_defs());
        let result = tool.execute(json!({"query": "probe"})).await;
        assert!(!result.is_error);
        let names = match_names(&result.content);
        assert!(
            names.iter().any(|n| n == "wld_probe_secret"),
            "single-token search must keep working; got: {names:?}"
        );
        assert!(
            !names.iter().any(|n| n == "aion_list_models"),
            "a specific query must not drag in unrelated tools; got: {names:?}"
        );
    }

    /// C-5, the floor the fix has to earn. Relaxing "every token must match"
    /// to "rank by how well the tokens match" is only safe if the tool the
    /// caller NAMED comes back FIRST — otherwise the fix trades "no match" for
    /// "buried in noise", which reads the same to a model.
    ///
    /// On this query `aion_describe_tool` matches four query words to
    /// `wld_probe_secret`'s one, so a plain matched-token count ranks the decoy
    /// first. Specificity, not count, has to drive the order.
    ///
    /// MUTANT: rank by matched-token COUNT (name hits, then desc hits) instead
    /// of by matched-token LENGTH and this fails while
    /// `a_descriptive_query_finds_the_tool_it_names` above stays green.
    #[tokio::test]
    async fn a_chatty_query_ranks_the_named_tool_first() {
        let tool = ToolSearchTool::new(build_measured_defs());
        let result = tool
            .execute(json!({"query": "wld_probe_secret tool schema parameters"}))
            .await;
        assert!(!result.is_error);
        let names = match_names(&result.content);
        assert_eq!(
            names.first().map(|s| s.as_str()),
            Some("wld_probe_secret"),
            "the tool the query NAMED must rank first, ahead of tools that only \
             matched the chatty filler words; got: {names:?}"
        );
    }

    /// The same ranking claim at CATALOGUE scale, where it actually has to
    /// hold: the five-tool fixture above cannot show whether the intended tool
    /// survives two thousand competitors that all match the filler word
    /// "tool". It also pins the cap — the body carries every match's full
    /// input schema, so an uncapped relaxed match would dump the catalogue
    /// into one tool result.
    ///
    /// MUTANT: drop `scored.truncate(MAX_MATCHES)` and the length assertion
    /// fails (2000 matches); rank by matched-token count and the first-place
    /// assertion fails.
    #[tokio::test]
    async fn ranking_and_the_cap_hold_against_a_full_catalogue() {
        let mut defs: Vec<ToolDef> = (0..2000)
            .map(|i| {
                deferred_def(
                    &format!("synthetic_tool_{i}"),
                    &format!("An entirely synthetic tool number {i}"),
                )
            })
            .collect();
        defs.push(deferred_def("wld_probe_secret", "Probe a stored secret"));
        let tool = ToolSearchTool::new(defs);

        let result = tool
            .execute(json!({"query": "synthetic_tool_42 tool schema parameters"}))
            .await;
        assert!(!result.is_error);
        let names = match_names(&result.content);
        assert_eq!(
            names.first().map(|s| s.as_str()),
            Some("synthetic_tool_42"),
            "the named tool must outrank 2000 tools that share the filler word \
             \"tool\"; got: {names:?}"
        );
        assert_eq!(
            names.len(),
            MAX_MATCHES,
            "a relaxed match against a full catalogue must be capped"
        );
        assert!(
            !names.iter().any(|n| n == "wld_probe_secret"),
            "a tool matching none of the tokens must not be padded in; got: {names:?}"
        );
    }

    /// NEGATIVE CONTROL. Without this, "rank instead of require-all" could
    /// quietly become "match everything" and every positive test above would
    /// still pass.
    ///
    /// MUTANT: drop the score floor and this fails.
    #[tokio::test]
    async fn a_query_matching_nothing_still_reports_no_match() {
        let tool = ToolSearchTool::new(build_measured_defs());

        for query in [
            "zzzznotpresent",
            "zzzznotpresent qqqqmissing wwwwabsent",
            "quantum brioche kayak",
        ] {
            let result = tool.execute(json!({ "query": query })).await;
            assert!(!result.is_error);
            assert!(
                result.content.starts_with("No deferred tools matching"),
                "query {query:?} matches nothing and must say so; got: {}",
                result.content
            );
        }
    }

    /// C-5, defect 2 (the one the comment in `execute_with_ctx` documents): the
    /// snapshot is taken at construction and still marks a hydrated tool
    /// deferred, so searching for the same tool twice returned a BYTE-IDENTICAL
    /// body. The model reads that as "the schema still has not loaded" and
    /// searches again — measured against a real MCP server as ten identical
    /// searches with no call ever attempted, the run killed by the engine's
    /// repeated-tool-call guard.
    ///
    /// The snapshot cannot be made hydration-aware from inside this tool (the
    /// hydrated set lives on the engine). What it CAN do is remember what it
    /// already handed out, so the second answer is not the same bytes and says
    /// out loud that the tool has been callable since the first search.
    ///
    /// MUTANT: return the same `status` string on every search and this fails.
    #[tokio::test]
    async fn a_repeat_search_does_not_return_the_identical_body() {
        let tool = ToolSearchTool::new(build_measured_defs());

        let first = tool.execute(json!({"query": "wld_probe_secret"})).await;
        let second = tool.execute(json!({"query": "wld_probe_secret"})).await;
        assert!(!first.is_error && !second.is_error);
        assert_ne!(
            first.content, second.content,
            "a repeat search must not return the identical body — identical \
             bytes are what the model reads as 'not loaded yet' and loops on"
        );

        // Same tool, same array shape: the engine's hydration recorder still
        // has to be able to parse the second body.
        assert_eq!(match_names(&second.content), vec!["wld_probe_secret"]);

        let status = serde_json::from_str::<serde_json::Value>(&second.content)
            .expect("second body must still be a JSON array")[0]["status"]
            .as_str()
            .expect("a match must carry a status")
            .to_string();
        assert!(
            status.to_lowercase().contains("already"),
            "the repeat answer must say the tool was ALREADY loaded, not repeat \
             the first-load wording; got: {status}"
        );
    }

    #[tokio::test]
    async fn search_by_exact_name() {
        let tool = ToolSearchTool::new(build_tool_defs());
        let result = tool.execute(json!({"query": "SpawnTool"})).await;
        assert!(!result.is_error);
        assert!(result.content.contains("SpawnTool"));
        assert!(result.content.contains("Spawn sub-agents"));
        assert!(result.content.contains("parameters"));
    }

    /// A match must tell the caller the tool is now CALLABLE, not merely
    /// describe it.
    ///
    /// Without this the result is indistinguishable from "still deferred", and
    /// a model that has just hydrated a tool searches for it again instead of
    /// calling it. Measured against a real MCP server: ten byte-identical
    /// searches, no call ever attempted, the run terminated by the engine's own
    /// repeated-tool-call guard — with EVERY MCP tool unreachable that way, on
    /// a two-tool server as much as a hundred-tool one.
    ///
    /// MUTANT: delete the `"status"` field from the pushed match object and
    /// this fails. It asserts the callability signal specifically, not merely
    /// that some JSON came back — `search_by_exact_name` above already covers
    /// name/description/parameters and stayed green throughout the outage.
    #[tokio::test]
    async fn a_match_states_that_the_tool_is_now_callable() {
        let tool = ToolSearchTool::new(build_tool_defs());
        let result = tool.execute(json!({"query": "SpawnTool"})).await;
        assert!(!result.is_error);

        let parsed: serde_json::Value =
            serde_json::from_str(&result.content).expect("a match set must be a JSON array");
        let first = parsed
            .get(0)
            .expect("SpawnTool must match its own exact name");
        let status = first
            .get("status")
            .and_then(|s| s.as_str())
            .expect("a match must carry a `status` saying the tool is now callable");
        assert!(
            status.contains("callable"),
            "status must state callability, got: {status}"
        );

        // The engine's hydration recorder parses this same body and reads
        // `.name` off each element, so the array shape must survive.
        assert_eq!(
            first.get("name").and_then(|n| n.as_str()),
            Some("SpawnTool"),
            "the array shape `record_hydrated_tools` parses must be preserved"
        );
    }

    /// TEST B from the Wayland Desktop handoff, 2026-08-04. Their defect,
    /// their call, reproduced here.
    ///
    /// A multi-word query must match a tool whose description contains those
    /// words. It could not: the matcher asked whether the WHOLE query appeared
    /// verbatim as one substring, so "sub agents parallel" had to be present
    /// letter-for-letter in a single description. Real callers write queries
    /// like "tvcontrol TradingView chart"; none of them could ever match.
    ///
    /// MUTANT: restore `name_l.contains(&query_lower) ||
    /// desc_l.contains(&query_lower)` and this fails while every other test in
    /// this module stays green — which is exactly what happened in production.
    #[tokio::test]
    async fn a_multi_word_query_matches_words_scattered_through_a_description() {
        let tool = ToolSearchTool::new(build_tool_defs());

        // SpawnTool's description is "Spawn sub-agents". Both words are present
        // but REVERSED relative to the description, so the phrase "agents spawn"
        // appears nowhere verbatim — which is precisely what the old
        // whole-query substring compare demanded.
        let result = tool.execute(json!({"query": "agents spawn"})).await;
        assert!(!result.is_error);
        assert!(
            result.content.contains("SpawnTool"),
            "a multi-word query whose words all appear must match; got: {}",
            result.content
        );

        // This assertion used to demand the OPPOSITE — that a query carrying
        // one unmatched token return nothing — on the reasoning that OR over a
        // chatty query returns the whole catalogue and buries the tool the
        // caller meant. The concern was right; the remedy was the defect
        // (C-5): it made every descriptive query strictly worse than a terse
        // one, and cost a captured session 19 no-match searches out of 28.
        //
        // Burying is now prevented by RANKING plus a cap, not by exclusion, so
        // an unmatched token no longer vetoes a match. The original concern is
        // held by `a_chatty_query_ranks_the_named_tool_first` (the intended
        // tool must come back FIRST) and by
        // `a_query_matching_nothing_still_reports_no_match` (zero matched
        // tokens is still no match).
        let result = tool
            .execute(json!({"query": "agents spawn zzzznotpresent"}))
            .await;
        assert!(!result.is_error);
        assert_eq!(
            match_names(&result.content),
            vec!["SpawnTool"],
            "one token that matches nothing must not veto the tokens that do"
        );
    }

    #[tokio::test]
    async fn search_case_insensitive() {
        let tool = ToolSearchTool::new(build_tool_defs());
        let result = tool.execute(json!({"query": "spawntool"})).await;
        assert!(!result.is_error);
        assert!(result.content.contains("SpawnTool"));
    }

    #[tokio::test]
    async fn search_by_description_keyword() {
        let tool = ToolSearchTool::new(build_tool_defs());
        let result = tool.execute(json!({"query": "plan"})).await;
        assert!(!result.is_error);
        assert!(result.content.contains("EnterPlanMode"));
    }

    #[tokio::test]
    async fn search_excludes_non_deferred() {
        let tool = ToolSearchTool::new(build_tool_defs());
        let result = tool.execute(json!({"query": "Read"})).await;
        // "Read" is not deferred, should not appear in results
        assert!(
            !result.content.contains("\"name\": \"Read\"")
                || result.content.contains("No deferred tools")
        );
    }

    #[tokio::test]
    async fn search_no_match() {
        let tool = ToolSearchTool::new(build_tool_defs());
        let result = tool.execute(json!({"query": "nonexistent"})).await;
        assert!(!result.is_error);
        assert!(result.content.contains("No deferred tools"));
    }

    #[tokio::test]
    async fn search_empty_query_returns_error() {
        let tool = ToolSearchTool::new(build_tool_defs());
        let result = tool.execute(json!({"query": ""})).await;
        assert!(result.is_error);
    }
}
