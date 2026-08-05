---
lane: 24-native-actions
criterion: "24-C3 (reference channels / the inbound matrix)"
clause-graded: "native actions — the ack state machine, graded per adapter and per affordance"
adapters-measured: "6 of 10 — telegram, matrix, slack, whatsapp, msteams, discord. This is EVERY adapter that declares either affordance. The 4 that declare neither (email, signal, imessage, sms) are unmeasured and unclaimed."
grade-24-C3: "STILL NOT MET, and this lane does not claim it. Nine lanes have now declined it. What changed: `native actions` was measured on ONE adapter and is now measured on SIX, from the real binary through `gateway run` on Linux, counted on the platform side, with a one-variable negative control per adapter proven to redden. Seven other clauses stand at their prior grade and macOS/Windows have nothing on any of them."
new-finding: "F24-C3-H8 (MEDIUM) — asymmetric ack diagnostics. An operator who configures `ack` on an adapter with no reaction support gets silence: at the default `info` level NOTHING is logged, and even at `debug` only the RECEIPT failure appears. The TERMINAL reaction is `let _ =` (channel_inbound.rs:552) and `react_on` (manager.rs:750-763) logs nothing, so that drop is invisible at EVERY log level."
fence-exposure: "ZERO. `git diff --stat 75babf32 -- crates/wcore-cli/src/lib.rs crates/wcore-cli/src/main.rs` is empty. No Rust source, no .github/**, no shared script modified. Two files added, both new."
status: complete
---

# 24-NATIVE-ACTIONS — one adapter to six, per affordance, counted on the platform

**Verdict up front: `24-C3` is NOT MET and I do not claim it.** Eight lanes have declined it and
every one was right; this is the ninth. What changed is narrow and real: the `native actions`
clause had evidence on **one** adapter. It now has evidence on **six** — which is *every adapter
in the codebase that declares either affordance* — each graded per affordance, each with a
one-variable negative control.

I also found **two defects in my own instrument**, one of which produced a **false leak alarm
against the product**, and I report both rather than only the repaired result.

---

## 1. What the clause promises, and why six inbound lanes never touched it

`.planning/ROADMAP.md:119`, Phase 24 Success Criterion 3:

> Reference channels prove setup/auth, access, routing, media, **native actions**, idempotency,
> reconnect/reload, and health.

The term `native action` does not exist in the Rust source. The **concept** is the ack state
machine in `wcore-agent/src/channel_inbound.rs` `run_turn` (:512-555), gated by `AckMode`:

| id | affordance | source |
|---|---|---|
| **A1** | 👀 receipt reaction, **before** dispatch | `react_on(.., "👀")` :519-521 |
| **A2** | typing keepalive under an `AbortOnDrop` guard | `spawn_typing_keepalive` :534-540, loop :628-636 |
| **A3** | ✅ on `Ok` / ❌ on `Err`, **after** dispatch | `react_on(.., emoji)` :545-555 |

Two facts, inherited from the predecessor lane and re-confirmed here from source, decide the
method:

1. **`AckMode` defaults to `Off`** (`dispatch/access.rs:191`). Nothing fires unless a channel
   config asks. Every leg below sets `ack` explicitly — which is exactly why six consecutive
   inbound lanes exercised none of this.
2. **Every failure on this path is swallowed.** `tracing::debug!` at :523, `let _ =` at :552,
   `let _ =` at :632. **Core's own logs therefore cannot prove a native action happened.**
   Fixture-side counting is the instrument, not a convenience — a log-side count measures
   intent, not effect.

---

## 2. The declared surface, and why the two trait defaults are asymmetric

Trait defaults, `wcore-channels/src/lib.rs`:

- `react` :294 → `Err(ChannelError::Unsupported)` — a **loud** default
- `send_typing` :277 → **no-op `Ok(())`** — a **silent** default

That asymmetry is the sharpest trap on this clause. An adapter with no `send_typing` override
returns `Ok(())` and emits nothing on the wire, **so any error-side or log-side instrument would
report "typing succeeded" on a platform that has no typing API at all.** Only a platform-side
count separates those, which is why A2 grades `not-supported` (a distinct verdict from
`not-fired`) for slack and whatsapp, and why the instrument's self-test asserts that distinction
explicitly.

Census, whole workspace, unproxied and with quoted globs:

```
/usr/bin/grep -rn "    async fn react("  crates "--include=*.rs"     -> 6 (5 adapters + trait)
/usr/bin/grep -rn "async fn send_typing" crates "--include=*.rs"     -> 12 (4 adapters + trait + tests)
```

**Instrument proven alive on a known-positive in the same shape:** `max_message_len` — a sibling
declared surface that *does* have consumers — returns **9** files. So this search finds
declarations when they exist; the zeros below are measurements, not free negatives.

| adapter | `react` | `send_typing` |
|---|---|---|
| discord | YES `lib.rs:444` | YES `lib.rs:435` |
| telegram | YES `lib.rs:373` | YES `lib.rs:360` |
| matrix | YES `lib.rs:324` | YES `lib.rs:305` |
| slack | YES `lib.rs:267` | **NO** — silent no-op default |
| whatsapp | YES `lib.rs:327` | **NO** — silent no-op default |
| msteams | **NO** — `Unsupported` | YES `lib.rs:381` |
| email, signal, imessage, sms | NO | NO |

msteams is the exact inverse of slack/whatsapp. Both are measured deliberately: a matrix
containing only adapters that support **both** could not distinguish `not supported` from
`fired nothing`.

---

## 3. THE MATRIX — measured, on the platform side, from the real binary

**Adapters: 6.** Driven end-to-end from `wayland-core gateway run` on `hetzner-dsm` (Linux),
release binary built at `75babf32`. Driver: `scripts/f24-native-actions.mjs` (new, strictly
additive — see §6).

**13/13 gates PASS**, over two full runs (`/root/f24na-final1`, `/root/f24na-final2`) whose gate
verdicts and matrix are byte-identical while `generated_at` and `out_dir` differ. Summary
hashes `aa50e741…` / `e984b423…` (`/usr/bin/shasum -a256`).

| adapter | **A1** receipt 👀 | **A2** typing keepalive | **A3** terminal ✅ | counted at the platform |
|---|---|---|---|---|
| **telegram** | **fired** | **fired** | **fired** | `["👀","✅"]`, typing=1 |
| **matrix** | **fired** | **fired** | **fired** | `["👀","✅"]`, typing=1 |
| **slack** | **fired** | **not supported** | **fired** | `["👀","✅"]`, typing=0 |
| **whatsapp** | **fired** | **not supported** | **fired** | `["👀","✅"]`, typing=0 |
| **msteams** | **not supported** | **fired** | **not supported** | `[]`, typing=1 |
| **discord** | **fired** | **fired** | **fired** | `["👀","✅"]`, typing=1 |

**5/5 adapters declaring `react` fire both reactions. 4/4 declaring `send_typing` fire typing.
ZERO advertised-but-dead instances on this surface** — worth stating plainly, because that class
has nine recorded instances elsewhere in this program and was the thing I was sent to look for.

### The "not supported" answers, with their reasons

- **slack** — `lib.rs:264-266` states it: Slack has no bot-usable typing API, so `send_typing`
  deliberately keeps the trait no-op. **This is a measurement, not an assumption:** the fixture
  serves and counts a typing endpoint, and counted zero.
- **whatsapp** — `lib.rs:324-326`: WhatsApp's typing indicator is tied to a per-message read
  receipt, which needs the message id, **and the keepalive does not carry one**. A platform
  constraint, not an omission.
- **msteams** — no `react` override at all; the Bot Framework activity surface this adapter
  speaks has no reaction verb. Falls through to `Err(Unsupported)`.

### Slack's row proves more than it looks like

Slack does **not** receive unicode on the wire. `react` maps through `api::slack_emoji_name`
(`api.rs:242`) to the shortcodes `eyes` / `white_check_mark`, and the fixture maps back. So
`["👀","✅"]` on that row additionally proves **the shortcode mapping is live end to end** — not
merely that two reactions arrived.

### The negative control, and what it defeats

Per adapter, leg N changes **exactly one variable** — `ack = "both"` → `ack = "off"` — and
requires `turn_ran = true` **and** zero reactions **and** zero typing.

The `turn_ran` conjunct is the point. This criterion's *access* leg once passed on three
adapters **because everything was denied**. If the binary had failed to attach or refused the
message, leg P's counts would be zero (P fails) *and* leg N would fail on `turn_ran`. P and N
can only both pass if the difference is genuinely the `ack` setting. All six pairs did.

### A2 graded as a LIFECYCLE, not a count

Every prior measurement of this clause asserted `typing >= 1`. That assertion passes identically
whether the `AbortOnDrop` guard works or leaks, because the keepalive fires once immediately.
So A2 is additionally graded over a **~12s turn** (LLM fixture holds the response) with a **14s
post-turn watch**. Measured, telegram and msteams, repeatedly:

```
during = 3    after = 0    loop_ran = true    aborted = true    watched 14.0s past the guard drop
```

Real timeline, read back from the fixture journal (seconds after the inbound submit):

```
0.0 submit | 0.1 👀 | 0.1 typing | 5.1 typing | 10.1 typing | 12.7 ✅ + reply | (watch to ~17)
```

Three refreshes on a 5s cadence proves the **loop** iterates, not just its first send. Zero in
the 14 seconds after the guard drop proves **`AbortOnDrop` genuinely aborts.** The msteams
variant exercises the reply-fallback marker (no `react`), i.e. a different code path in the
verdict function.

---

## 4. NEW FINDING — F24-C3-H8 (MEDIUM): asymmetric ack diagnostics

Configure `ack = "both"` on an adapter with no reaction support and the reactions silently do
nothing. The gateway log from the msteams positive leg — **8921 bytes, instrument proven alive
(10 lines match the channel name)** — contains exactly one diagnostic:

```
DEBUG ack 'seen' reaction failed (non-fatal) channel=f24na error=react is unsupported on platform msteams
```

That is the **receipt** only. The **terminal** reaction is `let _ =` (`channel_inbound.rs:552`),
and `react_on` (`manager.rs:750-763`) does not log either — it returns the error to a caller that
discards it. **So the terminal drop is invisible at every log level, including `trace`.** At the
default `info` level the operator gets nothing at all for either.

The behaviour is by design (best-effort acks must never be fatal) and I am not arguing with that.
What is wrong is that **the config key is accepted, reachable, and produces no observable and no
diagnostic** — the operator cannot tell "my ack config is working" from "this platform will never
do that" without watching the platform.

**Graded MEDIUM, deliberately not higher.** No correctness or security consequence; the acks are
documented as best-effort. Per LANE-BRIEF §5 this goes to BACKLOG, non-blocking. **Not fixed** —
the fix touches `wcore-agent`, a crate several lanes depend on, and a blind end-of-lane change to
a shared crate is exactly what a prior lane was right to refuse.

*(Adjacent, sub-finding, not worth its own id: `manager.rs:748-749` documents "platforms without
reactions → `Rejected` via the trait default". The trait default returns `Unsupported`, not
`Rejected` — the trait's own doc at `lib.rs:288-293` records that this was changed in Phase 24
precisely because folding them together let callers retry forever. Stale doc comment.)*

---

## 5. Instrument defects — TWO, both mine, and one accused the product falsely

LANE-BRIEF §3b/§6b-ii warns that instruments carry the defect class they hunt. Two instances,
both repaired in-lane rather than written up and left (§6b-ii), each with a self-test assertion
proving the repair does something.

### Defect 1 — `fx.replies is not iterable` killed the discord leg

`DiscordFixture` is the one fixture here not derived from my `AckLedger` base: it carries
`reactions` and `typing` but no `replies`. The unguarded read threw and took the whole leg out.

**It failed loudly rather than grading a zero, which is the safe direction.** Repair: the
fields that only *describe* (`replies`, `journal`) are guarded; the fields that *grade*
(`reactions`, `typing`) are deliberately left **fatal**, because an `[]` fallback there would
turn a missing instrument into a clean `not-fired` — the free-negative trap of §3b-i.

### Defect 2 — MY KEEPALIVE GATE RAISED A FALSE LEAK ALARM AGAINST THE PRODUCT

First keepalive run reported `typing_after = 2` — a post-turn keepalive leak, i.e. **the exact
defect I was sent to look for, apparently confirmed.** It was not real.

The window opened at `turn_ran` + 3s. But `turn_ran` is observed when the LLM fixture *receives*
the request, and the keepalive leg then holds the response open for 12s — **so the window opened
about nine seconds before the turn ended and counted two in-turn refreshes as leakage.**

This is the sharpest thing this lane produced, and it cuts the opposite way from §3b-i's usual
warning: a broken instrument gives you a **free negative**, but it can also hand you a **free
positive finding**, which is far more attractive to report and just as wrong.

Repair: the window is anchored to the first **platform-side post-guard-drop marker** — the
terminal reaction, or the reply for a no-`react` adapter — because `run_turn:544-555` drops the
typing guard **before** sending either. The pre-repair artifact is retained
(`run7-PRE-REPAIR-false-leak-result.json`) rather than deleted.

### The self-test: 9 assertions, and mutation-proved to redden

Three of the nine are §6b-ii third-assertions — each proves the *old* instrument would have got
it wrong, which is the only kind of assertion that proves a repair does anything:

| # | assertion | proves |
|---|---|---|
| 3 | two 👀 and **no** terminal reaction grades `not-fired`, where the old count-only matcher grades `fired` | emoji identity, not `length >= 2` |
| 5 | a fixture with no `replies` reads successfully, where the old unguarded read **threw** | defect 1 |
| 9 | the old turn-observation marker reports **2 phantom** post-turn signals on the same real timeline the repaired marker grades as **0** | defect 2 |

Assertion 7 is the real measured timeline; assertion 8 is a synthetic genuine leak that the gate
**must** redden on, or it cannot detect what it exists for. Assertion 6 guards the free negative.

**Mutation-proved rather than trusted** — two independent mutations of the shipped instrument:

| mutation | result |
|---|---|
| A3 reverted to count-only (`reactions.length >= 2`) | assertion 3 **FAIL**, all others PASS |
| `not-supported` collapsed into `not-fired` | assertion 4 **FAIL**, all others PASS |

Each reddens its intended assertion and nothing else.

---

## 6. What I built, and what I deliberately did not touch

`scripts/f24-native-actions.mjs` — new, **strictly additive, edits nothing.** It carries its own
telegram / matrix / slack / whatsapp / msteams fixtures rather than editing
`f24-tg-fixture.mjs`, `f24-matrix-fixture.mjs`, `f24-msteams-fixture.mjs` or `f24-inbound.mjs`,
all of which are in use by concurrently-running lanes. It subclasses `DiscordFixture`, the
pattern `f24-media-actions.mjs` established.

That was not only a merge-safety choice. **The shared fixtures cannot serve this measurement:**
the tg fixture answers `sendChatAction` / `setMessageReaction` through its catch-all, so a
*count* survives but the **emoji does not**; the matrix fixture records `sendReaction` without
`m.relates_to.key`. This lane grades on emoji identity, so neither could have served it.

Ports: webhook **21473**, chosen away from every live lane (18787 `f24-inbound`, 18211 discord,
19631-3 `msteams-attach`). All fixture ports bind `:0`. **No global `pkill` was ever run** — every
child is reaped by handle.

Re-run:

```bash
node scripts/f24-native-actions.mjs --selftest                 # 9 assertions
node scripts/f24-native-actions.mjs --binary <wayland-core> --out <dir> \
     --adapters telegram,matrix,slack,whatsapp,msteams,discord --keepalive
```

---

## 7. What I did NOT do — and what remains for `24-C3`

- **Did not mark `24-C3` MET.** One clause of eight moved. Seven stand where they were.
- **Did not measure the 4 adapters that declare neither affordance** (email, signal, imessage,
  sms). From source their answer is `not-supported` on all three affordances by trait default —
  but that is a *source reading, not a measurement*, and per §3b-i I will not report an absence
  I did not instrument. **Unmeasured and unclaimed.**
- **Did not test `ack = "reactions"` or `ack = "typing"` alone.** Only `"both"` and `"off"`.
  `AckMode::reactions()`/`typing()` (`access.rs:83-91`) are trivially separable, but the partial
  modes are not exercised end to end by this lane.
- **macOS and Windows: NOTHING, on this criterion or any other.** No permitted host runs
  macOS, and nothing here was run on Windows. This gap is unchanged by this lane.
- **Did not use the Darwin-behaviour exception.** Nothing here is macOS-specific.
- **Did not fix F24-C3-H8.** MEDIUM → BACKLOG per §5.
- **Did not modify any Rust source.** The measurement needed none — which is itself the
  result: this surface was already correct on every adapter that claims it, and had simply
  never been asked for.
- **Did not run the full workspace suite.** No Rust changed, and a full run under five other
  lanes' load is not a measurement (§6).
- **Did not** touch `crates/wcore-cli/src/{lib,main}.rs`, `.github/workflows/*`, any shared
  `f24-*` fixture, or run `wcore-contract generate`. No PR, no merge, no tag, no issue closed.

### Remaining distance to `24-C3` MET

| # | what is left | cost |
|---|---|---|
| 1 | `media`, live direction — see 24-MEDIA-LIVE / F24-C3-H7 | tracked there |
| 2 | `native actions` on the 4 no-affordance adapters — confirm the trait defaults hold live | ~0.5 session |
| 3 | partial `ack` modes (`reactions` / `typing` alone) | ~0.25 session |
| 4 | `reconnect/reload` — PARTIAL; F24-C3-H5 still unfixed | ~1 session |
| 5 | **macOS / Windows — nothing on any clause** | ~2 sessions |
| 6 | F24-C3-H8 (§4) and F24-C3-H6 — measured, not fixed, MEDIUM → BACKLOG | ~0.5 session |

`24-C3` remains a release blocker. `native actions` is the clause that moved: from one adapter
to the whole declared surface, with the `AbortOnDrop` lifecycle proven rather than assumed.

---

## 8. Evidence

`.planning/phases/24-gateway-automation-channels-typed-api/24-NATIVE-ACTIONS-evidence/`

| file | bytes | what |
|---|---|---|
| `24-NATIVE-ACTIONS-NOTES.md` (parent dir) | — | append-only working record, first committed at T+13 **before any run** (§6b-i), re-committed after every measurement |
| `final1-summary.json` | 33705 | canonical 6-adapter run, 13/13 gates, sha `aa50e741…` |
| `final2-summary.json` | 33705 | reproducibility run, identical verdicts, sha `e984b423…` |
| `final1-telegram-keepalive-result.json` | 2407 | keepalive lifecycle: `during=3 after=0 aborted=true` |
| `run9-msteams-keepalive-result.json` | 2724 | keepalive via the **reply-fallback** marker |
| `final1-whatsapp-result.json` | 1666 | the sixth adapter, closing the declared surface |
| `full1-msteams-gateway.log` | 8921 | F24-C3-H8: the single DEBUG line, and the silence around it |
| `run7-PRE-REPAIR-false-leak-result.json` | 2037 | **the false leak alarm, retained** — `typing_after=2` against a product that does not leak |

Byte counts via `/usr/bin/stat -f%z`, **not** `wc` — a prior lane measured the proxied `wc -c`
returning 0 for a 72-byte file. Hashes via `/usr/bin/shasum -a256`. Every count in this report
comes from an unproxied absolute-path tool.

## 9. Fence exposure vs `75babf32`

```
BASE=75babf329235484684ecee3a65973b0c197840c1
/usr/bin/git diff --stat "$BASE" -- crates/wcore-cli/src/lib.rs crates/wcore-cli/src/main.rs
   -> EMPTY
```

**Zero.** Files changed vs BASE: **2, both added** — `scripts/f24-native-actions.mjs` and the
NOTES file (evidence committed subsequently). Rust source touched: **0**. `.github/**`: **0**.

`git diff --name-only` is blind to untracked files (§3.2), so `git status --porcelain
--untracked-files=all` was run separately: the only untracked paths were this lane's own
evidence artifacts, now committed. Nothing for the orchestrator to serialize — no protocol
seam, no contract request, no shared-file edit.
