# Tool-token empirical baseline — 2026-05-15

ScriptedProvider baseline. Numbers below reflect tool-result
serialization cost; live-API verification still needed (see
runbook §2 below).

## Methodology

- Provider: `ScriptedProvider` (deterministic, no network)
- Each tool invoked once through `execute_tool_calls_with_budget`
against a clean `ToolRegistry`
- `Read` result captured verbatim (no truncation applied here)
- Heuristic column: `chars / 4` rounded up (ceil division)
- Scripted input_tokens column: `ScriptedProvider` `Usage` payload,
seeded with `(chars * 0.27).round_down()` to make the gap visible
- `delta` = scripted_input_tokens − heuristic_tokens. Negative
delta means the heuristic over-estimates billable tokens for
this tool result, positive means under-estimates.

**The scripted column is a synthetic baseline, not a live
provider number.** Live-API verification (§2) replaces it with
real Anthropic / OpenAI / Bedrock / Vertex tokenization.

## Results

| Tool | Scenario | Result chars | Heuristic tokens (chars/4) | Scripted provider input_tokens | Delta |
|------|----------|--------------|----------------------------|--------------------------------|-------|
| Read | 100-line file | 3291 | 823 | 888 | 65 |
| Bash | echo hello | 36 | 9 | 9 | 0 |
| Grep | 1000-line haystack, 20 hits | 2693 | 674 | 727 | 53 |
| Glob | *.txt in workdir | 51 | 13 | 13 | 0 |
| Write | 23-byte new file | 127 | 32 | 34 | 2 |
| Edit | single replacement | 141 | 36 | 38 | 2 |

## Runbook for live-API verification

Required env vars (any subset — the bench skips providers whose
creds are missing):

- `ANTHROPIC_API_KEY`
- `OPENAI_API_KEY`
- `GEMINI_API_KEY` (Vertex AI / Google Generative Language)
- `AWS_ACCESS_KEY_ID` + `AWS_SECRET_ACCESS_KEY` + `AWS_REGION` (Bedrock)

Run from the engine repo root:

```bash
vx cargo run --release -p wcore-agent \
--bin tool_token_bench \
--features test-utils,live-api \
-- --live-api
```

The live-API path is currently scaffolded only — the runner
returns exit 2 with a pointer to this doc. Wiring up the
per-provider round-trip is captured as a follow-up:

1. For each tool row above, build the same `ToolUse` block.
2. Hand the `ContentBlock::ToolResult` to the provider as a
single-turn `LlmRequest`.
3. Capture `Usage` from the provider's `LlmEvent::Done`.
4. Re-render this markdown with a per-provider column set:
`(anthropic_input_tokens, openai_input_tokens, ...)`.
5. Output: `docs/tool-token-live-<date>.md`.

Until step 5 lands, app-side budget UIs should keep using the
`chars / 4` heuristic with the caveat documented in
`wcore-protocol::events::Usage`: this is a structural
baseline, not a billable-token oracle.
