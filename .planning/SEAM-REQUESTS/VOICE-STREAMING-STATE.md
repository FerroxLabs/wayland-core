# SEAM REQUEST — typed voice streaming state on the wire

Filed by lane `voice-bargein` (Phase 27, criterion C4), 2026-07-29.
**Not actioned in-lane.** Requires a `wcore-contract generate` run, which is
release coordination and is forbidden to lanes (LANE-BRIEF §0).

## What Core has today

`VoiceModeTool` exposes five discrete actions — `toggle_record`, `start`,
`stop`, `cancel`, `status`. Each is an ordinary tool call, so a host observes
them through the generic ladder: `ToolRequest` → `ToolRunning` → `ToolResult`.

Recording state and RMS level reach a host by exactly two routes:

1. polling the `status` action (`{"is_recording": bool, "current_rms": i32}`);
2. free-text `ProtocolEvent::Info` strings emitted by the TUI bridge —
   `crates/wcore-cli/src/tui/engine_bridge.rs` sends the literal
   `"Recording started…"` / `"Recording stopped, transcribing…"`.

Measured (`/usr/bin/grep` over all 20 files of `crates/wcore-protocol/src`,
with `pub enum` = 62 and `tooluse|tool_` = 182 as liveness controls): **zero**
voice / audio / microphone / speech / stt / tts event identity.

## What is asked for

A typed, push-based voice state surface so a host (Desktop especially) can
render a microphone indicator and a level meter without string-matching an
`Info` line. Roughly: capture started / stopped / cancelled, plus a throttled
level event.

## What this request deliberately does NOT claim to fix

**It does not make C4's `ordered protocol events` clause pass.** The protocol
crate says so itself, in `crates/wcore-protocol/src/contract/generate.rs:41-45`:

> `ordinary_turn_tool_replay_reducer`: legacy ordinary turn and tool events
> still have no producer event ID or monotonic sequence.

`ToolRequest` / `ToolRunning` / `ToolResult` / `ToolCancelled` carry `msg_id`
and `call_id` — correlation identity, not order. A host can bind a result to
its request; it cannot verify sequence or detect a gap. **New voice variants
would inherit that absence exactly**, so adding them buys typing, not ordering.

Ordering is therefore a **protocol-wide** deficiency (a monotonic sequence on
all events), not a voice feature. It is already tracked as a deferral in the
contract corpus notes above. Filed here so the two gaps go to the owner who
can actually close each one.

## Panel

Question put to the cross-audit panel (LANE-BRIEF §4): add voice event
variants now, or record the two gaps and defer?

| leg | vote |
|---|---|
| codex gpt-5.6-sol | `VOTE=DEFER` |
| gemini-3.1-pro-preview | `VOTE=DEFER` |
| kimi K3 | `VOTE=DEFER` |
| internal adversarial | argued ADD — see below |

3/3 unanimous, and all three reached the same operative reason independently:
unsequenced, unconsumed variants would be an eleventh instance of the
"advertised but dead" pattern *and* would drift a fixture corpus this lane
cannot regenerate. kimi named the strongest objection to its own vote —
"deferring feels like dodging the acceptance criterion" — and answered it: the
criterion names *ordering*, and the variant proposal provably cannot deliver
ordering.

**Internal adversarial pass, arguing FOR adding them now:** the TUI bridge
already emits an `Info` event on every voice toggle, so a typed event would
have a production consumer from day one and would NOT be dead on arrival; and
a Desktop host genuinely cannot build a mic indicator on a free-text string.
— **Half right, and it does not change the vote.** The consumer argument is
sound and is exactly why this request exists. But it argues for a *typed state
surface*, which is what is filed here; it does not argue that the surface makes
the `ordered` clause pass, because the new variants would carry no sequence
either. Landing it in-lane would also mean drifting the Desktop contract corpus
with no coordinated Desktop change and no ability to regenerate — trading a
documented gap for an undocumented one.
