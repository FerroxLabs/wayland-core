# F16 Flux Fork Wedge Root Fix

Status: the Flux fork wedge slice of F16 is implemented and sealed by the
commit containing this document. F16 remains open for its other independently
releasable reliability slices.

Base: `70cd5d9b6b5b10233a484ca0b38d3630b521eece`

Board crosswalk: FerroxLabs/wayland#862. The issue remains open for release
coordination; this work does not push, merge, or close it.

## Reproduction and root cause

Two recorded Flux/Anvil forge runs reached the child turn cap after the
OpenAI-compatible stream emitted a valid tool name and arguments without the
required tool-call id. Core executed the tool, but the next outbound request's
existing empty-id safety guard removed both the assistant call and matching
tool result. Flux therefore received no evidence that the tool had completed
and repeated blind turns until the eleven-turn boundary.

The evidence signature was the repeated warning that two of three tool
messages with an empty or missing `tool_call_id` were stripped immediately
before each failed driver turn. A later session-seat fallback succeeded, which
ruled out the issue's original semantic-cache suspicion as the primary cause.

## Fix

The OpenAI-compatible chat stream now assigns every missing tool-call id a
non-empty internal id scoped to that response and call index. The synthesized
id enters the same `LlmEvent::ToolUse` path as a provider-supplied id, so the
engine persists one stable assistant-call/result pair and replays both on the
next request. Provider-supplied ids remain unchanged.

The existing outbound guard is deliberately retained. It still rejects truly
malformed historical or externally supplied empty-id pairs; the stream parser
now prevents newly received valid calls from becoming that malformed state.

## Verification

Authoritative Cargo execution ran on the Linux Hetzner proof harness from the
staged tree; Cargo was not run on macOS except `cargo fmt`.

- `cargo test -p wcore-providers missing_tool_call_id_is_synthesized_and_round_trips -- --nocapture`: 1 passed, 0 failed.
- `cargo nextest run -p wcore-providers --no-fail-fast`: 939 passed, 0 failed, 0 skipped.
- The regression parses a missing-id SSE tool call, reconstructs the persisted
  assistant tool call and tool result, serializes the next OpenAI request, and
  proves both messages survive with the same non-empty id.

## Remaining F16 boundaries

This slice closes the demonstrated #862 protocol-progress wedge. It does not
claim all of F16 complete. Provider worker cancellation, the unresolved native
browser sidecar contract, and remaining packaged native-platform regressions
retain separate proof obligations under F16.
