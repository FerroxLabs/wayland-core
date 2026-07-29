# 24-NATIVE-ACTIONS — working NOTES (append-only)

Lane `24-native-actions`. Branch `lane/24-native-actions`. Base `75babf32`.
Committed first at T+~12min per LANE-BRIEF §6b-i, BEFORE any build or run.

---

## T+12 — the declared surface, measured from source

`native action` = the **ack state machine** in `wcore-agent/src/channel_inbound.rs` `run_turn`.
Three affordances: (1) 👀 receipt reaction, (2) typing keepalive under `AbortOnDrop`,
(3) ✅/❌ terminal reaction.

### Declared per-adapter support — trait-override census

Instrument: `/usr/bin/grep -rn "    async fn react(" crates "--include=*.rs"` and
`/usr/bin/grep -rn "async fn send_typing" crates "--include=*.rs"`.
Globs QUOTED — predecessor lane's defect #1 was zsh eating `--include=*.rs`, which
returns 0 and hands you a free confirmation of any absence you were about to report.
Instrument proven alive: both searches return NON-ZERO (6 and 12 hits respectively),
and the trait-default definition site appears in each, which is a known-positive.

Trait defaults (`wcore-channels/src/lib.rs`):
- `react` :277→294 default = `Err(ChannelError::Unsupported{op:"react"})`
- `send_typing` :277 default = **no-op `Ok(())`** ← NOTE: a silent success, not an error.
  **This is an advertised-but-dead hazard by construction**: an adapter with no typing
  override returns `Ok(())` and the ack machine will believe typing "worked".

| adapter | `react` override | `send_typing` override |
|---|---|---|
| discord | YES `lib.rs:444` | YES `lib.rs:435` |
| telegram | YES `lib.rs:373` | YES `lib.rs:360` |
| matrix | YES `lib.rs:324` | YES `lib.rs:305` |
| slack | YES `lib.rs:267` | **NO** → default no-op `Ok(())` |
| whatsapp | YES `lib.rs:327` | **NO** → default no-op `Ok(())` |
| msteams | **NO** → default `Unsupported` | YES `lib.rs:381` |
| email / signal / imessage / twilio-sms | NO | NO |

msteams is the inverse of slack/whatsapp: typing but no reactions. That makes the
pair (msteams, slack) the sharpest test of "not supported" being reported honestly
rather than being indistinguishable from "fired nothing".

### The two facts that decide method (both inherited, both re-confirmed here)

1. `AckMode` defaults to `Off` — `wcore-channels/src/dispatch/access.rs:191`. Config
   must ask for it explicitly or NOTHING fires.
2. Both `react_on` failure paths are swallowed (`tracing::debug!`, `let _ =`), so
   **Core's logs cannot prove a native action happened**. Fixture-side counting is
   the only valid instrument. Counting from Core's logs measures intent, not effect.

## T+12 — plan

Drive real binary via `gateway run` on hetzner. Per adapter × per affordance, count on
the FIXTURE side. Negative control per claim = one variable (`ack: both` → `ack: off`).
Watch for: (a) advertised-but-dead override, (b) typing keepalive outliving its turn /
leaking a task — test `AbortOnDrop` actually aborts.

Ports: pick high/odd, five other lanes live. No global pkill.

## STILL TO ESTABLISH
- [ ] Do the slack/whatsapp typing defaults get *called* by the ack machine (i.e. does
      `ack.typing()` fire a no-op that looks like success)?
- [ ] Which fixtures can count reactions/typing on their platform surface.
- [ ] Live runs per adapter.
- [ ] `AbortOnDrop` keepalive termination proof.
