# SUMMARY — lane `media-cost-complete`

Branch `lane/media-cost-complete`. Merge-base `5cd37f79` (captured once,
unproxied). Working notes and full instrument findings:
`MEDIA-COST-COMPLETE-NOTES.md` beside this file.

## Verdict up front

**Cost coverage goes from 2 of 8 billable media backends to 8 of 8.** Every
remaining shape was **wired (a)**; **nothing was cut**. `video_analyze`'s nine
provider calls are now nine visible ledger rows.

But two things must be said plainly, and they qualify that number:

1. **A dollar figure is only obtainable on the FluxRouter-routed arms.**
   Anthropic vision, Gemini vision and native OpenAI/ElevenLabs speech return
   no cost in any channel. Those calls now record real, varying billable units
   and an explicit `Unpriced { ProviderReportsNoCost }` — visible, labelled
   spend, not silent spend — but not a price. Under the brief's strictest
   reading ("no provider cost and no rate card ⇒ (b)") those three arms are
   candidates for cutting. **My recommendation is that they ship** — reasoning
   and the dissent are below. This is a judgement call and it is Sean's to
   overturn.
2. **I found and fixed a bigger problem than the one I was sent for: the whole
   ledger was inert.** Details below.

## HIGH — the ledger had no production caller at all

Measured before writing any code, with `/usr/bin/grep` over `crates`:

| symbol | production sites | test sites |
|---|---|---|
| `with_cost_ledger` | **0** | 3 |
| `MediaCostLedger::shared()` / `::new()` | **0** | 5 |
| `MediaCostLedger::summary()` | **0** | 7 |
| `with_rate_card` | 1 (`bootstrap.rs:1334`, image_gen only) | 2 |

Known-positive proving the matcher alive in the same invocation:
`with_rate_card` returns its production hit.

So the shipped binary could not compute a `MediaCostSummary` for a real session
at all, and an operator who had filled in `[tools.media_pricing]` had it applied
to image generation and **silently ignored for every other billable shape** —
the same family as the `$0.00` bug, arriving through the wiring rather than the
pricing.

**Wiring three more `account()` calls into an unbound ledger would have added
three log lines and called it coverage.** `bootstrap.rs` now builds one
session-scoped ledger and hands it, with the operator rate card, to image_gen,
transcription, all three vision arms, both TTS arms, `video_analyze`, and the
channel media enricher.

## Per shape: wired or cut

| # | shape | decision | dollar figure available? |
|---|---|---|---|
| 1 | `image_gen` | (covered at base) | no — provider reports none for images |
| 2 | transcription | (covered by predecessor) | **yes** — `x-flux-cost-usd` |
| 3 | TTS / OpenAI arm | **(a) wired** | only when routed via Flux's OpenAI-wire base |
| 4 | TTS / ElevenLabs arm | **(a) wired** | no |
| 5 | `anthropic_vision` | **(a) wired** | no |
| 6 | `openai_vision` (also serves FluxRouter) | **(a) wired** | **yes** — header *and* `usage.cost_usd` |
| 7 | `gemini_vision` | **(a) wired** | no |
| 8 | `video_analyze` | **(a) covered transitively** | inherits 5-7 |

`piper` (local synthesis), `voice_mode` (delegates) and `google_meet` are not
billable media backends and are excluded — `google_meet` was **checked, not
assumed**: its 20 HTTP calls go only to Google Meet/OAuth hosts and it
references no `TranscriptionBackend` or `VisionBackend`.

**`video_analyze` makes no provider HTTP call of its own.** Its only outbound
request fetches the user's video. All nine billable calls go through
`VisionBackend::analyze()`, so instrumenting the three trait impls is the only
layer where the HTTP response — and therefore any provider-reported cost —
exists. Instrumenting `video_analyze` itself could only have synthesised a
figure, which is forbidden.

### Why I did not cut the three unpriceable arms

- They are not silent. Each records real units that vary with the work (tokens
  for vision, characters for speech) plus an explicit reason saying no price is
  obtainable. That is the opposite of the failure mode the brief is about.
- **The rule that would cut them also cuts `image_gen`** — FluxRouter returns
  no figure for images either, which is measured in `media_cost.rs`'s own
  header. Image generation is the one shape that was covered at base and it
  ships today in exactly this state. A rule that retroactively cuts the
  reference implementation is the wrong rule.
- The honest fix is a **per-token / per-character rate card**, which
  `MediaRateCard` does not have — it is per-artifact only. **I did not build
  it.** It is the top follow-up and it is small.

**Dissent, recorded:** a reviewer could reasonably hold that "records units but
can never produce a price" fails the brief's bar, and cut Anthropic/Gemini
vision and ElevenLabs from v1. That position is coherent; I rejected it on the
`image_gen` precedent above.

## The money bug I was told to look for — it was there, twice over

The predecessor fixed `for_success` pricing transcription as
`usd_per_image * images`. That guard keyed on `images == 0`, which was correct
while audio was the only zero-artifact shape. **It is not any more.** Vision is
token-billed and speech is character-billed and both also have `images == 0`,
so:

- `is_duration_billed()` was `images == 0`, so a vision call and a speech call
  would each have been filed as a **duration-billed call contributing 0.0
  seconds** — a seconds-denominated count inflated with calls that have no
  seconds.
- `summary_line()` would have rendered a vision call as **"0 image(s)"**, which
  reads as "nothing was produced", i.e. as free — the same misreading `$0.00`
  produces.

`MediaUnits` now **declares** its `BillingBasis` rather than having it inferred,
and the summary buckets on it. Pixels, seconds, tokens and characters are four
different billable units; summing any two means nothing and filing one under
another means something false.

## Gates — every number read from a file with an unproxied tool

All at asserted SHA `9ddac01d` on `hetzner-dsm`, verified equal to Mac HEAD
after every fetch.

| gate | result |
|---|---|
| `cargo test -p wcore-tools --lib media_cost` | **16 passed; 0 failed; 0 ignored; 0 measured; 1007 filtered out**, rc=0 (11 at base + 5 new) |
| `cargo test -p wcore-agent --lib tool_backends::` | **258 passed; 0 failed; 3 ignored; 0 measured; 1994 filtered out**, rc=0 — reproduced twice |
| `cargo fmt --all -- --check` (Mac) | rc=0 |
| `cargo clippy -p wcore-tools -p wcore-agent --all-targets` | rc=0, **0 warnings in files I touched** |

The clippy claim is asserted over the **whole log**, not a tail: every `-->`
location in it is one of `tests/cache_ledger_engine_test.rs` or
`tests/user_model_identity_wire.rs`, both pre-existing and untouched. (The
predecessor could only assert this over a tail and said so; this closes that.)

### Both directions, per §3b-iii

Every guard was mutated and re-run. Each case asserts the mutation **actually
changed the file** (md5 before/after) before trusting the result, then restores
with `git checkout -- <path>`. Hetzner `git status --porcelain` empty after.

| mutation | can it fail? |
|---|---|
| M1 drop anthropic cost-header read | **0 passed; 1 failed**, rc=101, `left: None / right: Some(0.00421)` |
| M2 drop openai-vision cost-**body** read | **0 passed; 1 failed**, rc=101, `left: None / right: Some(0.00421)` |
| M3 drop TTS cost-header read | **0 passed; 1 failed**, rc=101, `left: None / right: Some(0.000765)` |
| M5 disable the rate-card basis guard | **13 passed; 3 failed**, rc=101, `left: Some(0.0) / right: None` — the `$0.00` bug reappearing, caught by all three basis guards (duration, token, character) |

Can they pass? Yes — all green at HEAD, and each test contains its own
known-negative in the same body (a billed call with the header records
non-zero; the *same* call without it records `unpriced`, never `$0.00`). Either
half alone is self-passing: the negative passes on a backend that records
nothing, the positive on one that cannot tell free from unknown.

## What I did NOT do, and one instrument that lied

**The bootstrap ledger binding is guarded by NO test.** M4 was meant to prove
it and produced no evidence:

- **First attempt was VACUOUS** — `cargo fmt` had wrapped the call across three
  lines so the `sed` matched nothing. The md5 guard reported
  `MUTATION_APPLIED=NO` instead of "nothing reddened". Without it I would have
  concluded the binding was unguarded on the strength of a mutation that never
  ran (§3b-i).
- **Retry applied (6 lines) and 21 tests failed — none of them a media test.**
  They were the LANE-BRIEF §6 contention cluster. Proven, not assumed: three
  identical unmutated full runs at the same SHA failed **17, 20 and 21** tests
  with *different* sets, `session::` alone single-threaded passed **31 passed;
  0 failed**, and `tool_backends::` alone passed **258 passed; 0 failed**.

So: the binding is verified by reading the code and by compilation only. **A
test does not exist that fails if someone deletes it.** Named as a gap rather
than papered over.

Also not done, and not claimed:

- **No user-visible surface for the session total.** `MediaCostSummary` still
  has zero production consumers — the ledger now accumulates and nothing
  displays it. Per-call visibility exists (a structured `wcore::media_cost`
  tracing event with units and cost on every billable call, plus the record
  JSON in `image_gen`'s tool result), but there is no "you spent $X this
  session" anywhere. **This is the single largest remaining gap.**
- **Per-token / per-character rate card** — not built (see above).
- **Zero live provider calls. I did not use the FluxRouter burn key at all, so
  there is no secret to sweep for and nothing was spent — $0.00 of real money.**
  Every proof is against a wiremock server over the real HTTP path.
- `video_analyze`'s nine-call fan-out is proven at ledger level
  (`video_analyze_fan_out_is_nine_visible_token_billed_calls`), **not** by
  running ffmpeg against a real video with nine mocked vision calls.

## Refuted premise — the predecessor's stated limitation

`MEDIA-GEN-VOICE-SUMMARY.md` records that transcription is unit-proven rather
than HTTP-proven because `SsrfSafeResolver` "dials only validated public IPs, so
a loopback `wiremock` test cannot exercise this backend at all".

**False, and measured so before I relied on it.** At base commit `c564674c`,
`cargo test -p wcore-agent --lib tts::` gave **21 passed; 0 failed; 0 ignored;
0 measured; 2226 filtered out**, rc=0, including
`openai_tts_writes_bytes_to_output_path ... ok` — which serves bytes from a
wiremock `server.uri()` through the production `build_ssrf_safe_tool_client()`
and asserts `bytes_written` against the payload length. Cause: **reqwest does
not invoke a custom DNS resolver for an IP literal**, so `SsrfSafeResolver` is
never consulted for `127.0.0.1`.

Consequence: every guard in this lane runs over the real HTTP path rather than
at record-assembly level, and the same is now available to retro-prove
transcription.

The brief's other premises held, except the denominator: the predecessor's own
table lists **7** billable rows while its prose says "of 8". Both are defensible
— TTS has two provider arms with separate billing — so I count 8 and say which
taxonomy I used rather than inheriting a figure.

## Shared-file fence

`crates/wcore-cli/src/lib.rs` and `crates/wcore-cli/src/main.rs`: **not
touched.** Verified with `git diff --name-only $BASE -- <both paths>` against
the merge-base SHA captured once at start (`5cd37f79`), not the branch name.
Ten files changed, all in `wcore-agent` / `wcore-tools` plus this evidence
directory. `Cargo.lock` untouched — no dependency changes.
