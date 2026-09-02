use std::collections::HashSet;
use std::sync::Arc;

use async_trait::async_trait;
use parking_lot::RwLock;
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

/// Upper bound on how many tokens are SUBSTRING-scanned against the catalogue.
///
/// A query is meant to be intentional prose. The longest genuine one in any
/// captured session is four words ("wld_probe_secret tool schema parameters");
/// naming a tool and saying what you want from it does not reach two dozen.
/// 64 leaves an order of magnitude of headroom over that while bounding the
/// worst case at 64 × catalogue substring compares, instead of letting a
/// pasted-in JSON document scan the catalogue a thousand times over.
///
/// It does NOT bound the exact-name pass below, which is a hash lookup per
/// tool and stays complete: a tool NAMED past the cap is still found.
const MAX_QUERY_TOKENS: usize = 64;

/// The metalanguage of a serialized tool catalogue: JSON literals, JSON-Schema
/// keywords, and the four keys this very tool emits (`name`, `description`,
/// `parameters`, `status`).
///
/// These are the words a JSON blob is MADE of, and real MCP prose is full of
/// them too ("returns a JSON object whose properties describe the required
/// parameters"), so scoring them makes every tool in a catalogue look
/// equally relevant to a document that mentions no tool at all. Measured on a
/// live GPT-5.6 Sol run: a blob query scored 48 points of pure scaffolding per
/// decoy against 32 for the tool it actually named, so the named tool was cut
/// off by [`MAX_MATCHES`] entirely — 25 searches, no match, run killed.
///
/// Dropped only when the query has something else to go on, mirroring the
/// one-character rule below: a caller whose whole query is "schema" still gets
/// a substring search for "schema".
const STRUCTURAL_NOISE: &[&str] = &[
    "additionalproperties",
    "allof",
    "anyof",
    "array",
    "boolean",
    "const",
    "default",
    "description",
    "enum",
    "false",
    "format",
    "integer",
    "items",
    "json",
    "name",
    "null",
    "number",
    "object",
    "oneof",
    "parameters",
    "properties",
    "required",
    "schema",
    "status",
    "string",
    "title",
    "tool",
    "tools",
    "true",
    "type",
];

fn is_structural_noise(token: &str) -> bool {
    STRUCTURAL_NOISE.contains(&token)
}

/// How much of the query the miss message may quote back.
///
/// The miss message exists so the caller can see WHICH query missed, and 200
/// characters is more than any intentional query needs. Echoing an unbounded
/// query is what turns a pasted-in JSON catalogue into a tool RESULT of the
/// same size: the blob then occupies the context twice, and the newest thing
/// the model sees is its own blob rather than anything it can act on. In the
/// live GPT-5.6 Sol run that motivated this, that pair repeated 25 times.
const MAX_ECHOED_QUERY_CHARS: usize = 200;

/// The query as the miss message may quote it — bounded by
/// [`MAX_ECHOED_QUERY_CHARS`], on a char boundary, with an explicit marker so a
/// truncated echo can never be mistaken for the query itself.
fn echoable(query: &str) -> String {
    if query.chars().count() <= MAX_ECHOED_QUERY_CHARS {
        return query.to_string();
    }
    let head: String = query.chars().take(MAX_ECHOED_QUERY_CHARS).collect();
    format!(
        "{head}… (query truncated; it was {} characters. A query is a tool name \
         or a few keywords — do not paste a tool catalogue or a previous \
         ToolSearch result into it.)",
        query.chars().count()
    )
}

/// How many characters of deferred-tool NAMES a miss message may list.
///
/// A miss cannot be terminal without saying what IS in the session — but a
/// catalogue is exactly what must never be poured back into the context
/// (see [`MAX_ECHOED_QUERY_CHARS`]). 400 characters is roughly a dozen
/// MCP-length names: enough to show the SHAPE of what is here, bounded so a
/// 300-tool server cannot turn a miss into a blob of ToolSearch's own making.
const MAX_MISS_CATALOG_CHARS: usize = 400;

/// The miss response.
///
/// The whole response used to be the first line. That is a dead end carrying
/// zero information: it cannot distinguish "you phrased the query badly" from
/// "no such capability exists in this session", so the rational next move is
/// to re-phrase — and that is exactly what happened. Measured live by the
/// Wayland Desktop lane (GPT-5.6 Sol, new binary, 2026-08-08): ten consecutive
/// ToolSearch calls, every one `status=Success` with no matcher miss to blame,
/// against ONE connected MCP server carrying two tools, hunting a web-search
/// tool and a `research-advisor` skill that were never in the session. Query 7
/// was `research-advisor`; the next three were re-phrasings of the same absent
/// capability. The loop is the message design, not the model.
///
/// So the miss states the three things a caller needs in order to STOP: that
/// nothing matched, what the deferred set actually holds, and that the query
/// was already compared against all of it — the last is what makes "search
/// again" provably useless rather than merely discouraged. Truncating the name
/// list does not weaken that claim, because the match pass above scans every
/// deferred tool regardless of how many are listed here; the omitted count is
/// stated out loud so a truncated inventory cannot read as a complete one.
fn miss_message(echoed_query: &str, deferred: &[&str]) -> String {
    let head = format!("No deferred tools matching \"{echoed_query}\" found.");
    let total = deferred.len();
    if total == 0 {
        return format!(
            "{head} This session has NO deferred tools at all, so ToolSearch \
             cannot return one however it is phrased. This says NOTHING about \
             skills, or about MCP connector tools already attached to this \
             session: ToolSearch indexes neither. Invoke a skill by name with \
             the Skill tool, and test a connector by CALLING one of its own \
             tools. Proceed without the deferred tool."
        );
    }

    let mut listed = String::new();
    let mut included = 0usize;
    for name in deferred {
        let sep = if included == 0 { "" } else { ", " };
        if listed.len() + sep.len() + name.len() > MAX_MISS_CATALOG_CHARS {
            break;
        }
        listed.push_str(sep);
        listed.push_str(name);
        included += 1;
    }
    let inventory = match total - included {
        0 => listed,
        // Even the first name blew the budget — say the count rather than
        // emit an empty inventory that reads as "there is nothing here".
        _ if included == 0 => format!("{total} tools, none short enough to list here"),
        omitted => format!("{listed} (+{omitted} more of {total} not listed)"),
    };

    format!(
        "{head} All {total} deferred tools in this session were compared \
         against that query and none matched. The deferred set is: \
         {inventory}. ToolSearch indexes DEFERRED TOOLS ONLY. It does not \
         index skills, and it does not index MCP connector tools that are \
         already attached to this session, so a miss here is NOT evidence \
         that a skill or a connector is absent - report either as missing \
         only after invoking the skill by name with the Skill tool, or \
         CALLING one of the connector's own tools, and seeing that fail. If \
         what you wanted really is a deferred tool, this session does not \
         have it and re-running ToolSearch with different wording cannot make \
         it appear."
    )
}

/// Upper bound on returned matches. Each match carries the tool's FULL input
/// schema, so an unbounded relaxed match could pour an entire MCP catalogue
/// into one tool result. Matches are RANKED before the cut, so this is a
/// relevance cut, not an arbitrary one.
const MAX_MATCHES: usize = 10;

/// Told on every match, because the success body is the artifact that gets
/// pasted back. Measured (Wayland Desktop / GPT-5.6 Sol, 2026-08-08): a
/// 12712-char ToolSearch result went straight back in as the next `query`
/// and was answered with a 12748-char echo of itself. [`echoable`] bounds
/// the damage on the MISS path, but only after the paste has happened —
/// this says it a turn earlier, on the body being pasted.
///
/// It rides `status` rather than a prose line wrapped around the JSON
/// because `AgentEngine::record_hydrated_tools` parses the WHOLE content as
/// a `serde_json::Value::Array` and records nothing otherwise — a preamble
/// would leave every "successfully" searched tool uncallable.
const DO_NOT_PASTE_BACK: &str = "Do not pass this result back to ToolSearch \
     as a query — a query is a tool name or a few keywords.";

/// Status line on a tool the engine has not admitted yet.
const STATUS_FIRST_LOAD: &str = "LOADED — this tool is now callable by name. \
     Call it directly on your next step; searching for it again returns this \
     same result and makes no progress.";

/// Status line on a tool the engine has already admitted. See the
/// repeat-search note in [`ToolSearchTool::execute_with_ctx`].
const STATUS_ALREADY_LOADED: &str = "ALREADY LOADED — an earlier ToolSearch \
     in this session already returned this tool, and it has been callable by \
     name ever since. Searching again cannot change that. Call it directly, \
     now.";

/// The session's hydrated-tool set, shared by handle.
///
/// The authority is the engine (`AgentEngine::hydrated_tool_names`) — it is
/// what decides whether a deferred tool is force-admitted into the outbound
/// `tools[]`. `wcore-tools` sits BELOW `wcore-agent` and must never depend on
/// it, so the engine publishes into this handle rather than the search tool
/// reaching upward. The handle is owned by [`crate::registry::ToolRegistry`],
/// which outlives every `ToolSearch` instance it builds.
pub type HydratedTools = Arc<RwLock<HashSet<String>>>;

/// Built-in tool that searches for deferred tools and loads their full schema.
/// Core tool (never deferred itself) — always available to the LLM.
pub struct ToolSearchTool {
    /// Snapshot of all tool definitions (taken at construction time).
    tool_defs: Vec<ToolDef>,
    /// Handle onto the session's hydrated set. Read-only from here: this
    /// tool REPORTS hydration, it does not decide it. See the repeat-search
    /// note in [`Self::execute_with_ctx`] for the measured loop that
    /// motivates reporting it at all.
    hydrated: HydratedTools,
}

impl ToolSearchTool {
    /// Standalone instance with a private, permanently empty hydrated set.
    /// Registry-built instances use [`Self::with_hydration`] so the set
    /// survives a catalog rebuild.
    pub fn new(tool_defs: Vec<ToolDef>) -> Self {
        Self::with_hydration(tool_defs, HydratedTools::default())
    }

    /// Instance that reports hydration from a SHARED set.
    ///
    /// `refresh_tool_search_catalog` rebuilds this tool on bootstrap, on every
    /// config-MCP registration, on `/mcp add`, and from the TUI engine bridge.
    /// Passing the registry's handle through is what stops a rebuild from
    /// forgetting what the engine has already admitted.
    pub fn with_hydration(tool_defs: Vec<ToolDef>, hydrated: HydratedTools) -> Self {
        Self {
            tool_defs,
            hydrated,
        }
    }
}

#[async_trait]
impl Tool for ToolSearchTool {
    fn name(&self) -> &str {
        "ToolSearch"
    }

    /// This body must never be byte-sliced, so it is given room the default
    /// 50 000 cannot guarantee.
    ///
    /// `truncate_result` runs BEFORE compaction and cuts on a character
    /// count, not a record boundary. A ToolSearch result cut mid-object is no
    /// longer parseable JSON, and `AgentEngine::record_hydrated_tools` parses
    /// this exact body to decide which deferred tools to force-admit into
    /// `tools[]` -- so a sliced catalogue hydrates NOTHING, and the tool the
    /// model just searched for never becomes callable. Measured: ten matches
    /// carrying twenty-property schemas serialise to 66 712 chars, comfortably
    /// past the default, and the slice lands mid-object.
    ///
    /// The body is already bounded by construction -- [`MAX_MATCHES`] caps it
    /// at ten ranked matches -- so raising the ceiling cannot unbound it. This
    /// buys headroom for those ten to carry full schemas intact rather than
    /// arrive as unparseable wreckage. Dropping whole matches would also work,
    /// but silently returning fewer tools than matched is the failure mode
    /// this tool exists to avoid.
    fn max_result_size(&self) -> usize {
        100_000
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

        // An EXACT token match on a tool's full name is unambiguous intent and
        // cannot happen by accident, so it outranks any amount of scoring below
        // — see the tier in the sort. Built from the full deduped token set,
        // BEFORE the noise filter and the cap: this is one hash lookup per
        // tool, not a scan, so there is no reason to bound it, and bounding it
        // is what would make a name buried at token 300 of a pasted-in document
        // unreachable.
        let exact_names: HashSet<&str> = tokens.iter().copied().collect();

        // Drop the JSON/JSON-Schema scaffolding — see [`STRUCTURAL_NOISE`].
        if tokens.iter().any(|t| !is_structural_noise(t)) {
            tokens.retain(|t| !is_structural_noise(t));
        }
        // Bound the substring work — see [`MAX_QUERY_TOKENS`].
        tokens.truncate(MAX_QUERY_TOKENS);

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
        //
        // Scoring is a TIER below exact-name, though, because summed scores do
        // not survive a pathological query: a pasted-in JSON document gives
        // every tool in a verbose catalogue dozens of incidental points, and no
        // weighting of a single 16-character name beats that in aggregate.
        // (score is only comparable within a tier.)
        let mut scored: Vec<(bool, usize, usize)> = Vec::new(); // (exact name, score, def index)

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
            let exact = exact_names.contains(name_l.as_str());
            // The floor. Zero matched tokens is still NO MATCH — without it,
            // "rank instead of require-all" degrades into "match everything",
            // and every positive case above would still look fixed.
            if exact || score > 0 {
                scored.push((exact, score, idx));
            }
        }

        // Exact-name tier first, then score. Stable sort: ties keep registry
        // order.
        scored
            .sort_by_key(|&(exact, score, _)| (std::cmp::Reverse(exact), std::cmp::Reverse(score)));
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
        // The snapshot itself still cannot express hydration, and the
        // authority for it is the engine (`AgentEngine::hydrated_tool_names`),
        // which is what force-admits a hydrated tool into the outbound
        // `tools[]`. The engine publishes that set into [`Self::hydrated`], so
        // the status line reports the ENGINE'S state rather than a count of
        // how many times this instance has been asked. Two consequences the
        // tests pin: a tool the engine never admitted looks identical on every
        // search (nothing changed, so nothing should read as if it had), and a
        // catalog rebuild cannot resurrect the pre-hydration answer, because
        // the rebuilt instance shares the same set.
        let hydrated = self.hydrated.read();
        let matches: Vec<Value> = scored
            .iter()
            .map(|&(_, _, idx)| {
                let def = &self.tool_defs[idx];
                let already = hydrated.contains(&def.name);
                let loaded = if already {
                    STATUS_ALREADY_LOADED
                } else {
                    STATUS_FIRST_LOAD
                };
                json!({
                    "name": def.name,
                    "description": def.description,
                    "parameters": def.input_schema,
                    "status": format!("{loaded} {DO_NOT_PASTE_BACK}"),
                })
            })
            .collect();
        drop(hydrated);

        if matches.is_empty() {
            let deferred: Vec<&str> = self
                .tool_defs
                .iter()
                .filter(|d| d.deferred)
                .map(|d| d.name.as_str())
                .collect();
            return ToolResult {
                content: miss_message(&echoable(query), &deferred),
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

    /// A catalogue shaped like a real MCP server's: long `snake_case` names and
    /// VERBOSE descriptions written in exactly the vocabulary a serialized tool
    /// catalogue is made of — "object", "properties", "required",
    /// "parameters", "type", "string", "name". Read any of the MCP servers this
    /// repo actually talks to and this is what their prose looks like.
    ///
    /// That overlap is the whole point: it is what lets a JSON blob in the
    /// query out-score the one tool the blob actually names.
    fn build_verbose_mcp_defs() -> Vec<ToolDef> {
        let mut defs: Vec<ToolDef> = (0..300)
            .map(|i| {
                deferred_def(
                    &format!("mcp__acme_suite__operation_{i}"),
                    &format!(
                        "Operation {i}: returns a JSON object whose properties \
                         describe the required parameters, the type of each \
                         string field and the name of the resource."
                    ),
                )
            })
            .collect();
        defs.push(deferred_def("wld_probe_secret", "Probe a stored secret"));
        defs
    }

    /// The pathological query from a live Wayland Desktop screenshot (GPT-5.6
    /// Sol, 2026-08-08): the model put a giant JSON blob in `query` — the UI
    /// rendered it as `"[ { [... 197 similar lines] } ]"` — and did it 25 times
    /// in a row until the engine's repeated-tool-call guard killed the run
    /// ("stopped early before finishing").
    ///
    /// The SHAPE is what matters: a serialized tool catalogue. Every word in it
    /// is either JSON / JSON-Schema scaffolding (`name`, `description`,
    /// `parameters`, `type`, `object`, `properties`, `required`, `string`,
    /// `integer`, `status`) or vocabulary invented for this fixture and absent
    /// from [`build_verbose_mcp_defs`]. So the ONLY thing in the blob that can
    /// legitimately match a real tool is `names_a_real_tool`, when supplied —
    /// which is what makes the positive and negative cases below a controlled
    /// pair differing in exactly one token.
    fn pathological_catalogue_blob(names_a_real_tool: Option<&str>) -> String {
        let alien = [
            "zephyr",
            "ledger",
            "reconcile",
            "quiescing",
            "glyph",
            "tessellations",
        ];
        let mut entries: Vec<Value> = (0..30)
            .map(|i| {
                json!({
                    "name": format!("mcp__zephyr_ledger__{}_{i}", alien[i % alien.len()]),
                    "description": "Reconcile zephyr ledger shards upstream, \
                                    quiescing glyph tessellations.",
                    "parameters": {
                        "type": "object",
                        "properties": {
                            "shard_id": {"type": "string"},
                            "ledger_ref": {"type": "integer"},
                        },
                        "required": ["shard_id"],
                    },
                    "status": "LOADED",
                })
            })
            .collect();
        if let Some(real) = names_a_real_tool {
            // Mid-blob, not at the front: a model pasting a result back does
            // not helpfully lead with the one name that matters.
            entries.insert(
                entries.len() / 2,
                json!({
                    "name": real,
                    "description": "Reconcile zephyr ledger shards upstream.",
                    "parameters": {"type": "object", "properties": {}},
                    "status": "LOADED",
                }),
            );
        }
        serde_json::to_string_pretty(&Value::Array(entries)).unwrap()
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

    /// C-5b. The pathological query, from a LIVE Wayland Desktop screenshot
    /// (GPT-5.6 Sol, 2026-08-08): 25 consecutive ToolSearch calls whose `query`
    /// was a giant JSON blob — the UI rendered it `"[ { [... 197 similar lines]
    /// } ]"` — and the run ended "stopped early before finishing", killed by
    /// the engine's repeated-tool-call guard.
    ///
    /// Ranking alone does not rescue this. The blob is made of the same words
    /// an MCP catalogue's prose is made of, so on a 300-tool catalogue each
    /// decoy collects `object` + `properties` + `required` + `parameters` +
    /// `type` + `string` + `name` = 48 points of scaffolding, while the tool
    /// the blob NAMES scores 32 for its own name. 300 decoys outrank it and
    /// MAX_MATCHES cuts it off entirely: a guaranteed miss, 25 times over.
    ///
    /// MUTANT (measured, not asserted from the armchair): revert BOTH the
    /// noise filter and the exact-name tier — i.e. exactly the first-pass C-5
    /// code — and this fails with
    /// `left: Some("mcp__acme_suite__operation_0")`, the named tool nowhere in
    /// the ten returned matches. Reverting the noise filter ALONE leaves this
    /// test green, because the exact-name tier still carries it; that half is
    /// pinned by the negative control below, which is the test that actually
    /// fails when only the filter goes.
    #[tokio::test]
    async fn a_pathological_json_blob_still_finds_the_tool_it_names() {
        let tool = ToolSearchTool::new(build_verbose_mcp_defs());
        let query = pathological_catalogue_blob(Some("wld_probe_secret"));
        assert!(
            query.split_whitespace().count() > 300,
            "the fixture must stay pathological — hundreds of tokens, not a \
             tidy phrase; got {} tokens",
            query.split_whitespace().count()
        );

        let result = tool.execute(json!({ "query": query })).await;
        assert!(!result.is_error);
        assert!(
            !result.content.starts_with("No deferred tools matching"),
            "a blob that names a real tool must not be a guaranteed miss"
        );
        let names = match_names(&result.content);
        assert_eq!(
            names.first().map(|s| s.as_str()),
            Some("wld_probe_secret"),
            "the tool the blob NAMES must rank first, ahead of 300 tools that \
             only matched JSON scaffolding; got: {names:?}"
        );
    }

    /// NEGATIVE CONTROL for the test above, and the reason the fix is a noise
    /// filter rather than "be more tolerant". Same blob, same 300-tool
    /// catalogue, one difference: no real tool name anywhere in it.
    ///
    /// Every non-structural word in the blob is invented for the fixture and
    /// absent from the catalogue, so the only thing left that could match is
    /// the JSON scaffolding. If scaffolding alone matches, ToolSearch answers a
    /// blob with ten arbitrary tools and their full schemas — which is worse
    /// than a miss, because it looks like an answer.
    ///
    /// MUTANT: drop the structural-noise filter and this fails with ten
    /// `mcp__acme_suite__operation_*` matches.
    #[tokio::test]
    async fn a_pathological_json_blob_naming_nothing_still_reports_no_match() {
        let tool = ToolSearchTool::new(build_verbose_mcp_defs());
        let result = tool
            .execute(json!({ "query": pathological_catalogue_blob(None) }))
            .await;
        assert!(!result.is_error);
        assert!(
            result.content.starts_with("No deferred tools matching"),
            "JSON scaffolding is not a search term; got: {}",
            match_names(&result.content).join(", ")
        );
    }

    /// The other half of "a pathological query is handled sanely": a MISS on a
    /// blob must not answer with the blob.
    ///
    /// The miss message quotes the query back, so before this bound a 200-line
    /// pasted-in catalogue produced a tool RESULT of the same size — the blob
    /// now sits in the context twice, and the newest thing the model sees is
    /// its own blob rather than anything it can act on. That is the shape of
    /// the live loop: 25 blob searches, 25 blob echoes, run killed.
    ///
    /// MUTANT: echo `query` unbounded and this fails.
    #[tokio::test]
    async fn a_miss_on_a_blob_does_not_echo_the_blob_back() {
        let tool = ToolSearchTool::new(build_verbose_mcp_defs());
        let query = pathological_catalogue_blob(None);

        let result = tool.execute(json!({ "query": query.clone() })).await;
        assert!(!result.is_error);
        assert!(
            result.content.starts_with("No deferred tools matching"),
            "precondition: this query must miss; got: {}",
            result.content
        );
        assert!(
            result.content.len() < query.len() / 4,
            "a miss must not answer a {}-char blob with a {}-char echo of it",
            query.len(),
            result.content.len()
        );
        assert!(
            result.content.contains("truncated"),
            "a truncated echo must say so, or it reads as the query itself; \
             got: {}",
            result.content
        );

        // An ordinary query is still quoted in full — the bound is for blobs,
        // and a miss the caller cannot read is a worse miss. Asserted as
        // "quoted verbatim, no truncation marker" rather than as the whole
        // response: the response now also carries the deferred inventory and
        // the terminality claim (see
        // `a_miss_says_what_is_available_and_that_re_searching_cannot_help`),
        // which this bound has no opinion about.
        let ordinary = tool.execute(json!({"query": "zzzznotpresent"})).await;
        assert!(
            ordinary
                .content
                .starts_with("No deferred tools matching \"zzzznotpresent\" found."),
            "the bound must not touch a query anyone would actually type; got: {}",
            ordinary.content
        );
        assert!(
            !ordinary.content.contains("truncated"),
            "an untruncated query must not be marked truncated; got: {}",
            ordinary.content
        );
    }

    /// C-6. MEASURED live by the Wayland Desktop lane (GPT-5.6 Sol, new
    /// binary, 2026-08-08): ten consecutive ToolSearch calls, EVERY one of them
    /// `status=Success` with no matcher miss to blame. One MCP server was
    /// connected (`wayland-team-guide`, two tools) and the model was hunting a
    /// web-search tool and a `research-advisor` skill that did not exist in the
    /// session at all. Query 7 was literally `research-advisor`; queries 8-10
    /// were "web search operation schema", "internet search" and "search the
    /// web for sources" — three re-phrasings of the same absent capability.
    ///
    /// The miss response was, verbatim and in its entirety, `No deferred tools
    /// matching "research-advisor" found.` That is a dead end carrying zero
    /// information: it cannot distinguish "you phrased the query badly" from
    /// "no such capability exists here", so re-phrasing is the RATIONAL next
    /// move. The loop is the message design, not the model.
    ///
    /// A miss is terminal only if it says what IS here and that the query was
    /// already compared against all of it.
    ///
    /// A ToolSearch miss must NOT be usable as evidence that a SKILL or an
    /// attached MCP CONNECTOR is missing. ToolSearch indexes deferred tools
    /// only; skills and already-connected MCP tools are not in that index.
    ///
    /// Measured on a live Wayland Desktop session (2026-09-02): the engine
    /// logged `Connected to 'com.ferroxlabs-tvcontrol': 112 tools` and copied
    /// `skills/tide-morning-brief` into the workspace, yet
    /// `ToolSearch("tide-morning-brief")` answered "All 158 deferred tools in
    /// this session were compared ... none matched" and the model then told
    /// the user the skill AND the connector were "not available in this
    /// session". Both were present. The old wording closed with "tell the
    /// user the capability is unavailable", which is what licensed that.
    /// See also `wcore-mcp/src/tool_proxy.rs` on the 101-tool instance.
    ///
    /// MUTANT: restore the unscoped "this session does not have it ... tell
    /// the user the capability is unavailable" wording and this fails.
    #[tokio::test]
    async fn a_miss_is_not_evidence_that_a_skill_or_connector_is_absent() {
        let tool = ToolSearchTool::new(build_measured_defs());
        let result = tool.execute(json!({"query": "tide-morning-brief"})).await;
        assert!(!result.is_error);
        assert!(
            result.content.starts_with("No deferred tools matching"),
            "precondition: this query must miss; got: {}",
            result.content
        );

        let lower = result.content.to_lowercase();
        // It must name the limit of its own index...
        assert!(
            lower.contains("does not index skills"),
            "a miss must say ToolSearch does not index skills; got: {}",
            result.content
        );
        assert!(
            lower.contains("mcp connector tools"),
            "a miss must say ToolSearch does not index attached MCP connector \
             tools; got: {}",
            result.content
        );
        // ...and it must name the instrument that CAN answer the question,
        // so a miss cannot be reported as an absence without a real check.
        assert!(
            lower.contains("skill tool"),
            "a miss must point at the Skill tool for skills; got: {}",
            result.content
        );
        // The sentence that licensed the false report must be gone.
        assert!(
            !lower.contains("tell the user the capability is unavailable"),
            "a miss must not instruct the caller to declare a capability \
             unavailable; got: {}",
            result.content
        );
    }

    /// MUTANT: return the bare one-line miss and this fails.
    #[tokio::test]
    async fn a_miss_says_what_is_available_and_that_re_searching_cannot_help() {
        let tool = ToolSearchTool::new(build_measured_defs());
        let result = tool.execute(json!({"query": "research-advisor"})).await;
        assert!(!result.is_error);
        assert!(
            result.content.starts_with("No deferred tools matching"),
            "precondition: this query must miss; got: {}",
            result.content
        );

        // What IS available. Without it the caller cannot tell an absent
        // capability from a badly phrased query, and rephrasing is rational.
        for name in [
            "wld_probe_secret",
            "aion_list_models",
            "tv_chart_set_symbol",
        ] {
            assert!(
                result.content.contains(name),
                "a miss must state what IS available — {name} is missing from: {}",
                result.content
            );
        }

        let lower = result.content.to_lowercase();
        // That the whole deferred set was already checked. This is what makes
        // "search again" provably useless rather than merely discouraged.
        assert!(
            lower.contains("none matched"),
            "a miss must say the whole deferred set was already compared \
             against the query; got: {}",
            result.content
        );
        // And the terminality claim itself, said plainly.
        assert!(
            lower.contains("cannot make it appear"),
            "a miss must say plainly that re-searching cannot surface an \
             absent capability; got: {}",
            result.content
        );
    }

    /// The bound on the fix above. Telling the caller what IS available must
    /// not become the second way ToolSearch answers with a catalogue — the
    /// echo bound already refuses the first
    /// ([`a_miss_on_a_blob_does_not_echo_the_blob_back`]). A short, ordinary
    /// query against a 301-tool MCP catalogue is the case the echo bound
    /// cannot cover, because there is no blob to be four times larger than.
    ///
    /// MUTANT: list every deferred name instead of bounding at
    /// [`MAX_MISS_CATALOG_CHARS`] and this fails (301 names, ~9KB).
    #[tokio::test]
    async fn a_miss_against_a_large_catalogue_stays_bounded() {
        let tool = ToolSearchTool::new(build_verbose_mcp_defs());
        let result = tool.execute(json!({"query": "zzzznotpresent"})).await;
        assert!(!result.is_error);
        assert!(
            result.content.starts_with("No deferred tools matching"),
            "precondition: this query must miss; got: {}",
            result.content
        );
        assert!(
            result.content.len() < 1200,
            "a miss must stay bounded against a large catalogue; got {} chars: {}",
            result.content.len(),
            result.content
        );
        // Bounded by TRUNCATION, not by silence: the count it did not list has
        // to be stated, or the inventory reads as complete when it is not.
        assert!(
            result.content.contains("more of 301"),
            "the omitted count must be stated out loud; got: {}",
            result.content
        );
    }

    /// NEGATIVE CONTROL for the two tests above. The inventory and the
    /// "searching again cannot help" advice belong to a MISS. Leaking either
    /// into a HIT tells the model to stop searching in the one case where the
    /// search worked, and breaks the JSON array `record_hydrated_tools` parses.
    ///
    /// MUTANT: append the miss guidance unconditionally and this fails.
    #[tokio::test]
    async fn a_hit_carries_none_of_the_miss_guidance() {
        let tool = ToolSearchTool::new(build_measured_defs());
        let result = tool.execute(json!({"query": "wld_probe_secret"})).await;
        assert!(!result.is_error);
        assert_eq!(match_names(&result.content), vec!["wld_probe_secret"]);

        let lower = result.content.to_lowercase();
        for leak in [
            "no deferred tools matching",
            "none matched",
            "cannot make it appear",
        ] {
            assert!(
                !lower.contains(leak),
                "miss guidance {leak:?} must not appear on a hit; got: {}",
                result.content
            );
        }
        assert!(
            !result.content.contains("tv_chart_set_symbol"),
            "a hit must not list the rest of the catalogue; got: {}",
            result.content
        );
    }

    /// The cap on substring-scanned tokens ([`MAX_QUERY_TOKENS`]) is a work
    /// bound, and a work bound that can hide the tool the caller NAMED is just
    /// the old guaranteed-miss wearing a different hat. A pasted-in document
    /// does not put the name it mentions in the first 64 words.
    ///
    /// MUTANT: build `exact_names` from the CAPPED token list instead of the
    /// full one and this fails.
    #[tokio::test]
    async fn the_token_cap_cannot_hide_an_exactly_named_tool() {
        let tool = ToolSearchTool::new(build_verbose_mcp_defs());
        let mut query: Vec<String> = (0..MAX_QUERY_TOKENS * 3)
            .map(|i| format!("zzfiller{i}"))
            .collect();
        query.push("wld_probe_secret".to_string());

        let result = tool.execute(json!({ "query": query.join(" ") })).await;
        assert!(!result.is_error);
        assert_eq!(
            match_names(&result.content),
            vec!["wld_probe_secret"],
            "a name past the cap must still be found — the cap bounds substring \
             scanning, not exact-name lookup"
        );
    }

    /// C-5, defect 2 (the one the comment in `execute_with_ctx` documents): the
    /// snapshot is taken at construction and still marks a hydrated tool
    /// deferred, so searching for the same tool twice returned a BYTE-IDENTICAL
    /// body. The model reads that as "the schema still has not loaded" and
    /// searches again — measured against a real MCP server as ten identical
    /// searches with no call ever attempted, the run killed by the engine's
    /// repeated-tool-call guard.
    ///
    /// The hydrated set lives on the engine, so this tool cannot decide the
    /// answer — it reports the set the engine published into its shared
    /// handle. Once the engine has admitted the tool, the second answer is not
    /// the same bytes and says out loud that the tool has been callable since
    /// the first search.
    ///
    /// MUTANT: return the same `status` string on every search and this fails.
    /// MUTANT: ignore the shared set and count this instance's own returns
    /// instead, and `an_unhydrated_repeat_search_is_byte_identical` fails.
    #[tokio::test]
    async fn a_repeat_search_does_not_return_the_identical_body() {
        let hydrated = HydratedTools::default();
        let tool = ToolSearchTool::with_hydration(build_measured_defs(), hydrated.clone());

        let first = tool.execute(json!({"query": "wld_probe_secret"})).await;
        // What the engine's `record_hydrated_tools` publishes after parsing
        // the body above: the tool is now force-admitted into `tools[]`.
        hydrated.write().insert("wld_probe_secret".to_string());
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

    /// NEGATIVE CONTROL for the test above. The changed body has to MEAN
    /// "the engine admitted this tool", not "you asked twice" — otherwise the
    /// signal the model is being taught to read is noise.
    ///
    /// MUTANT: go back to an instance-local `already_returned` set and this
    /// fails while `a_repeat_search_does_not_return_the_identical_body` stays
    /// green — which is exactly the first-pass fix this replaces.
    #[tokio::test]
    async fn an_unhydrated_repeat_search_is_byte_identical() {
        let tool = ToolSearchTool::new(build_measured_defs());

        let first = tool.execute(json!({"query": "wld_probe_secret"})).await;
        let second = tool.execute(json!({"query": "wld_probe_secret"})).await;
        assert!(!first.is_error && !second.is_error);
        assert_eq!(
            first.content, second.content,
            "with nothing hydrated, the answer has not changed and must not \
             pretend it has"
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

    /// The SUCCESS body is the artifact the model pasted back.
    ///
    /// Measured, Wayland Desktop / GPT-5.6 Sol, 2026-08-08: a 12712-char
    /// ToolSearch result went straight back in as the next `query`, and was
    /// answered with a 12748-char echo of itself. The miss path already warns
    /// about this ([`echoable`]) — but only AFTER the paste has happened,
    /// which is one whole turn too late. The success body has to say it
    /// first, and the only channel in that body that reaches the model
    /// without changing the array shape is `status`.
    ///
    /// MUTANT: drop the no-paste-back clause from the STATUS constants and
    /// this fails.
    #[tokio::test]
    async fn a_match_tells_the_caller_not_to_paste_the_result_back() {
        let hydrated = HydratedTools::default();
        let tool = ToolSearchTool::with_hydration(build_measured_defs(), hydrated.clone());

        // BOTH status lines: the paste-back in the live run followed a
        // successful search, and a repeat search is exactly the state the
        // model is in when it reaches for the previous result.
        let first = tool.execute(json!({"query": "wld_probe_secret"})).await;
        hydrated.write().insert("wld_probe_secret".to_string());
        let second = tool.execute(json!({"query": "wld_probe_secret"})).await;

        for (label, body) in [("first load", &first), ("already loaded", &second)] {
            assert!(!body.is_error);
            let parsed: serde_json::Value = serde_json::from_str(&body.content)
                .unwrap_or_else(|_| panic!("{label}: body must be a JSON array"));
            for m in parsed.as_array().expect("array") {
                let status = m["status"].as_str().expect("a match carries a status");
                let lower = status.to_lowercase();
                assert!(
                    lower.contains("do not") && lower.contains("query"),
                    "{label}: the status must tell the caller not to pass this \
                     result back as a query; got: {status}"
                );
            }
        }
    }

    /// NEGATIVE CONTROL for the test above, and the reason the warning went
    /// into `status` rather than into a prose line wrapped around the JSON.
    ///
    /// `AgentEngine::record_hydrated_tools` parses the body with
    /// `serde_json::from_str::<Value>` and returns early unless the WHOLE
    /// content is a `Value::Array`, reading `.name` off each element. One
    /// prose line before the array and it records nothing — the tool never
    /// gets force-admitted into `tools[]`, so a "successful" search leaves it
    /// uncallable, which is a worse loop than the one being closed.
    ///
    /// MUTANT: prepend any preamble to the returned content — e.g.
    /// `format!("Note: …\n{json}")` — and this fails.
    #[tokio::test]
    async fn the_success_body_stays_a_bare_json_array_of_named_matches() {
        let tool = ToolSearchTool::new(build_measured_defs());
        let result = tool.execute(json!({"query": "wld_probe_secret"})).await;
        assert!(!result.is_error);

        let parsed =
            serde_json::from_str::<serde_json::Value>(&result.content).unwrap_or_else(|e| {
                panic!("the whole body must parse as JSON, or the engine records nothing: {e}")
            });
        let arr = parsed
            .as_array()
            .expect("the top level must be an ARRAY — `record_hydrated_tools` bails otherwise");
        assert!(!arr.is_empty());
        for m in arr {
            assert!(
                m.get("name").and_then(|n| n.as_str()).is_some(),
                "every element must carry a string `name`; got: {m}"
            );
            assert!(
                m.get("parameters").is_some(),
                "every element must still carry its schema; got: {m}"
            );
        }
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
