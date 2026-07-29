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

---

## T+45 — instrument built and MUTATION-PROVED

`scripts/f24-native-actions.mjs` (new, strictly additive — edits nothing; carries its OWN
telegram/matrix/slack/msteams fixtures so the shared `f24-tg-fixture.mjs`,
`f24-matrix-fixture.mjs`, `f24-msteams-fixture.mjs`, `f24-inbound.mjs` are untouched while
five lanes are live; subclasses `DiscordFixture` the way `f24-media-actions.mjs` did).

Why local fixtures rather than the shared ones: the shared tg fixture answers
`sendChatAction`/`setMessageReaction` through its catch-all, so a COUNT survives — but the
EMOJI does not. The shared matrix fixture records `sendReaction` without the `m.relates_to.key`.
This lane grades on emoji IDENTITY, so neither could serve it.

### Self-test: 4 assertions, all PASS — and both mutations redden

| mutation | effect | result |
|---|---|---|
| A3 reverted to count-only (`reactions.length >= 2`) | — | assertion 3 **FAIL**, others PASS |
| `not-supported` collapsed into `not-fired` | — | assertion 4 **FAIL**, others PASS |

Assertion 3 is the §6b-ii third assertion: two 👀 and NO terminal reaction. Count-only grading
calls that a complete ack cycle. Assertion 4 is the silent-default trap: the `send_typing`
trait default is a NO-OP `Ok(())`, so zero typing on a non-declaring adapter must read
`not-supported`, which is a different fact from `not-fired`.

### Build
`hetzner-dsm:/root/wayland-24na` @ 75babf32, `cargo build --release -p wcore-cli --bin wayland-core`
→ `Finished release profile in 5m 46s`, binary 96322688 bytes.

Ports chosen away from every live lane: webhook 21473 (18787 f24-inbound, 18211 discord,
19631-3 msteams-attach all taken). Fixture ports are ephemeral (bind :0).

## T+60 — LIVE RESULTS, three adapters (real binary, `gateway run`, hetzner-dsm)

| adapter | A1 receipt 👀 | A2 typing | A3 terminal ✅ | emojis counted at platform | negative control |
|---|---|---|---|---|---|
| telegram | fired | fired | fired | `["👀","✅"]` typing=1 | PASS reactions=0 typing=0, turn_ran=true |
| matrix | fired | fired | fired | `["👀","✅"]` typing=1 | PASS reactions=0 typing=0, turn_ran=true |
| slack | fired | **not-supported** | fired | `["👀","✅"]` typing=0 | PASS reactions=0 typing=0, turn_ran=true |

Runs `/root/f24na-run1` (telegram), `run2` (matrix), `run3` (slack). all_pass=true each.

Slack's `["👀","✅"]` is notable: slack does NOT receive unicode on the wire. `react`
(`slack/src/lib.rs:267`) maps through `api::slack_emoji_name` (`api.rs:242`) to the SHORTCODE
`eyes`/`white_check_mark`, and the fixture maps back. So this row additionally proves the
shortcode mapping is live end-to-end, not just that two reactions arrived.

Slack A2 `not-supported` is a MEASUREMENT, not an assumption: the fixture would have counted a
typing call at `/api/*typing*` had one been made, and counted zero. The declared surface says
slack keeps the trait's SILENT no-op default (`lib.rs:264-266` states this explicitly).

## T+95 — msteams + discord, keepalive lifecycle, and TWO instrument defects in my own gate

Full matrix `/root/f24na-full1`, 11/11 gates PASS:

| adapter | A1 receipt | A2 typing | A3 terminal | counted at platform |
|---|---|---|---|---|
| telegram | fired | fired | fired | `["👀","✅"]` typing=1 |
| matrix | fired | fired | fired | `["👀","✅"]` typing=1 |
| slack | fired | not-supported | fired | `["👀","✅"]` typing=0 |
| msteams | not-supported | fired | not-supported | `[]` typing=1 |
| discord | fired | fired | fired | `["👀","✅"]` typing=1 |

### Instrument defect 1 — `fx.replies is not iterable` killed the discord leg
`DiscordFixture` is the one fixture not derived from `AckLedger`. FAILED LOUDLY (threw) rather
than grading a zero — the safe direction. Repaired: describing fields (`replies`, `journal`)
guarded; grading fields (`reactions`, `typing`) deliberately left FATAL, because an
empty-array fallback would turn a missing instrument into a clean `not-fired` (§3b-i free
negative). Self-test assertions 5 and 6 cover both halves.

### Instrument defect 2 — MY KEEPALIVE GATE RAISED A FALSE LEAK ALARM
First run reported `typing_after=2` → a product leak. It was not. The window opened at
`turn_ran`+3s, but `turn_ran` is observed when the LLM fixture RECEIVES the request and the
keepalive leg then holds the response 12s — so the window opened ~9s BEFORE the turn ended and
counted two in-turn refreshes as leakage.

Real timeline, read back from the fixture journal (`/root/f24na-run7`), seconds after submit:
```
0.0 submit | 0.1 👀 | 0.1 typing | 5.1 typing | 10.1 typing | 12.7 ✅ + reply | (watch to ~17)
```
Repaired: the window is anchored to the first PLATFORM-SIDE post-guard-drop marker — the
terminal reaction, or the reply for a no-`react` adapter — because `run_turn:544-555` drops the
typing guard BEFORE sending either. Self-test assertions 7/8/9, where 9 is the §6b-ii third
assertion: the OLD marker reports 2 phantom signals on the same real timeline the repaired one
grades as 0.

**Post-repair verdict, twice: `during=3 after=0 loop_ran=true aborted=true`, watched 14s past
the guard drop — telegram (`run8`) and msteams (`run9`, exercising the reply-fallback marker).
`AbortOnDrop` genuinely aborts.**

### NEW FINDING candidate — asymmetric ack diagnostics
msteams + `ack = "both"`: reactions silently do nothing. Gateway log (8921 bytes, instrument
proven alive: 10 lines match the channel name) contains EXACTLY ONE diagnostic:
```
DEBUG ack 'seen' reaction failed (non-fatal) channel=f24na error=react is unsupported on platform msteams
```
That is the RECEIPT only. The TERMINAL reaction is `let _ =` (`channel_inbound.rs:552`) and
`react_on` (`manager.rs:750-763`) does not log either — so the terminal drop is silent at EVERY
log level. At the default `info` level the operator gets NOTHING for either.
