pub mod fold;
pub mod identifier_policy;
pub mod json;
pub mod level;
pub mod sanitize;
pub mod semantic;
pub mod toon;
pub mod transcript_rewrite;

pub use identifier_policy::IdentifierPolicy;
pub use level::CompactionLevel;
pub use semantic::{
    Chunk, ChunkRole, CompressionResult, CompressionRetention, SemanticCompressor, SemanticJudge,
};
pub use toon::toon_format_instructions;
pub use transcript_rewrite::{
    RewriteResult, RewriteRule, TranscriptEntry, rewrite_transcript_entries,
};

pub fn compact_output(text: &str, level: CompactionLevel) -> String {
    match level {
        CompactionLevel::Off => text.to_string(),
        CompactionLevel::Safe => sanitize::sanitize(text),
        CompactionLevel::Full => {
            let text = sanitize::sanitize(text);
            // Structured data must never meet the line fold. The fold is a
            // TEXT heuristic -- it collapses runs of similar-looking lines --
            // and pretty-printed JSON is full of lines that are legitimately
            // similar: enum members, repeated property blocks, arrays of
            // short strings. Collapsing any of those yields a body that no
            // longer parses.
            //
            // That is not a display problem. `AgentEngine::record_hydrated_
            // tools` parses THIS string to decide which deferred MCP tools to
            // force-admit into the outbound `tools[]`, so a folded catalogue
            // hydrates nothing, the tool never becomes callable, and every
            // repeat search returns a byte-identical body -- the engine's own
            // documented ten-identical-searches loop, reached from the far
            // end. Measured against this crate: a 5-tool catalogue collapsed
            // 27 lines to 5 with zero of 5 names surviving, and a 10-match
            // catalogue carrying enum members stayed unparseable even after
            // the similarity metric was corrected.
            //
            // Tuning the fold's similarity metric does NOT fix this, and was
            // tried: enum members are similar by any honest measure. The fold
            // simply must not run here. `compact_json` below then does the
            // structure-aware, lossless job on the same bytes.
            if looks_like_json(&text) {
                return json::compact_json(&text);
            }
            let text = fold::fold_repeated_lines(&text);
            json::compact_json(&text)
        }
    }
}

/// Whether `text` is a single JSON document, and therefore must be handed to
/// the structure-aware [`json::compact_json`] rather than the line fold.
///
/// `IgnoredAny` validates the syntax WITHOUT building a `serde_json::Value`
/// DOM, so this costs a scan rather than an allocation per tool result.
///
/// Deliberately conservative in two directions. It requires the document to
/// start with `{` or `[`, so prose that merely mentions JSON still folds. And
/// it answers false for NDJSON and for JSON that arrived already truncated,
/// both of which fail to parse as one document -- those still fold, which is
/// why [`fold::lines_are_similar`] normalises by the longer line rather than
/// the shorter one. This guard removes the damage on the whole-document path;
/// that metric bounds it on the paths this guard cannot recognise.
fn looks_like_json(text: &str) -> bool {
    let trimmed = text.trim_start();
    if !(trimmed.starts_with('{') || trimmed.starts_with('[')) {
        return false;
    }
    serde_json::from_str::<serde::de::IgnoredAny>(trimmed.trim_end()).is_ok()
}

pub fn compact_output_toon(text: &str) -> String {
    toon::try_toon_encode(text)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// wcore-agent runs EVERY tool result through `compact_output`. For
    /// `ToolSearch` that is not a nicety: its body is the hydration path, the
    /// only channel by which a deferred MCP tool's name and schema reach the
    /// model, and `AgentEngine::record_hydrated_tools` parses this exact
    /// COMPACTED string to decide what to force-admit into `tools[]`.
    ///
    /// Measured before the fix, against this shape: 27 lines collapsed to 5
    /// and 0 of 5 tool names survived, so a model driving a 101-tool server
    /// could not learn the name of a single one of its tools.
    ///
    /// This is a COMPOSITION failure (ToolSearch x per-result compaction), so
    /// it is pinned here at the `compact_output` seam rather than in `fold`.
    #[test]
    fn full_preserves_every_tool_name_in_a_search_result() {
        let names = [
            "chart_get_state",
            "chart_set_symbol",
            "watchlist_import",
            "indicator_add_from_search",
        ];
        // The real body shape: `to_string_pretty` over name/description/
        // parameters/status, with `parameters` a nested JSON Schema.
        let body = serde_json::to_string_pretty(
            &names
                .iter()
                .map(|n| {
                    serde_json::json!({
                        "name": n,
                        "description": format!("TradingView tool {n}"),
                        // Enum members and repeated property blocks are the
                        // shapes that fold LEGITIMATELY: short, uniformly
                        // indented, genuinely similar. A catalogue without
                        // them cannot detect the residual damage, and an
                        // earlier version of this test missed it for exactly
                        // that reason.
                        "parameters": {
                            "type": "object",
                            "properties": {
                                "symbol": {"type": "string", "description": "ticker"},
                                "timeframe": {
                                    "type": "string",
                                    "enum": ["1m", "5m", "15m", "1h", "4h", "1D", "1W"]
                                },
                                "style": {
                                    "type": "string",
                                    "enum": ["candles", "bars", "line", "area", "heikin"]
                                }
                            },
                            "required": ["symbol"]
                        },
                        "status": "loaded"
                    })
                })
                .collect::<Vec<_>>(),
        )
        .expect("serialize catalogue");

        let out = compact_output(&body, CompactionLevel::Full);

        assert!(
            !out.contains("similar lines") && !out.contains("identical lines"),
            "compaction folded a tool catalogue: {out}"
        );
        for n in names {
            assert!(out.contains(n), "compaction destroyed tool name {n}: {out}");
        }
        // The result must still be parseable, because the hydration recorder
        // parses the compacted string and bails silently if it is not.
        // Enum members are the residual the similarity metric could not
        // protect: they are similar by any honest measure, so only keeping
        // the fold away from JSON entirely saves them.
        for v in ["1m", "5m", "15m", "1h", "4h", "1D", "1W", "heikin"] {
            assert!(
                out.contains(v),
                "compaction destroyed enum member {v}: {out}"
            );
        }
        // The result must still PARSE, because `record_hydrated_tools` parses
        // this exact compacted string and bails silently when it cannot. A
        // body that keeps every name but no longer parses hydrates nothing.
        let parsed: serde_json::Value =
            serde_json::from_str(&out).expect("compacted catalogue must stay valid JSON");
        assert_eq!(
            parsed.as_array().map(|a| a.len()),
            Some(names.len()),
            "hydration would record nothing: {out}"
        );
    }

    /// NEGATIVE CONTROL. Without this the fix above could be "disable folding"
    /// and the test would still pass. Folding is why `Full` exists.
    #[test]
    fn full_still_folds_genuinely_repetitive_output() {
        let identical = ["warning: unused variable `x`"; 10].join("\n");
        let out = compact_output(&identical, CompactionLevel::Full);
        assert!(
            out.contains("identical lines"),
            "stopped folding identical lines: {out}"
        );
        assert!(out.lines().count() < 5, "fold saved nothing: {out}");

        let uniform: Vec<String> = (0..14)
            .map(|i| format!("Compiling crate-{i} v0.1.0"))
            .collect();
        let out2 = compact_output(&uniform.join("\n"), CompactionLevel::Full);
        assert!(
            out2.contains("similar lines"),
            "stopped folding uniform progress lines: {out2}"
        );
        assert!(out2.lines().count() < 5, "fold saved nothing: {out2}");
    }

    #[test]
    fn off_returns_unchanged() {
        let input = "hello\x1b[31m world\n\n\nfoo";
        assert_eq!(compact_output(input, CompactionLevel::Off), input);
    }

    #[test]
    fn safe_strips_ansi() {
        let input = "\x1b[32mOK\x1b[0m done";
        let result = compact_output(input, CompactionLevel::Safe);
        assert_eq!(result, "OK done");
    }

    #[test]
    fn safe_merges_blank_lines() {
        let input = "a\n\n\n\nb";
        let result = compact_output(input, CompactionLevel::Safe);
        assert_eq!(result, "a\n\nb");
    }

    #[test]
    fn safe_collapses_cr() {
        let input = "50%\r100%\nDone";
        let result = compact_output(input, CompactionLevel::Safe);
        assert_eq!(result, "100%\nDone");
    }

    #[test]
    fn full_folds_repeated_lines() {
        let lines: Vec<String> = (0..6)
            .map(|i| format!("Compiling dep-{i} v0.1.0"))
            .collect();
        let input = lines.join("\n");
        let result = compact_output(&input, CompactionLevel::Full);
        assert!(result.contains("[... 4 similar lines]"));
    }

    #[test]
    fn full_compacts_json() {
        let input = "{\n    \"id\": 1,\n    \"name\": \"Alice\"\n}";
        let result = compact_output(input, CompactionLevel::Full);
        assert!(result.len() < input.len());
    }

    #[test]
    fn safe_does_not_fold_lines() {
        let lines: Vec<String> = (0..6)
            .map(|i| format!("Compiling dep-{i} v0.1.0"))
            .collect();
        let input = lines.join("\n");
        let result = compact_output(&input, CompactionLevel::Safe);
        assert!(!result.contains("[..."), "Safe level should not fold lines");
    }
}
