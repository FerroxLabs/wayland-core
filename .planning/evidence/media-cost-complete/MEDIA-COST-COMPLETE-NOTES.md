# NOTES — lane `media-cost-complete`

Branch `lane/media-cost-complete`. Base integration `5cd37f79`.
Predecessor: `lane/media-gen-voice` (`.planning/evidence/media-gen-voice/`).

Append-and-commit after every measurement (LANE-BRIEF §6b-i).

## Instrument state (checked at start)

- `rtk` rewrite CONFIRMED live in this worktree: `git status --short` returned
  the single word `ok` on a clean tree. Every number below therefore comes from
  an absolute-path tool redirected to a file and read with the Read tool.
- Known-positive / known-negative control for the grep instrument, run in the
  same invocation: `image_gen` → **16 files**; `zzz_no_such_symbol_zzz` → **0**.
  Matcher alive.

## Premise check against the brief

| brief premise | verdict |
|---|---|
| cost coverage is 2 of 8 billable backends | **denominator disputed** — the predecessor's own table lists **7** billable rows (image_gen, whisper, tts, video_analyze, anthropic_vision, openai_vision, gemini_vision), with `piper` and `voice_mode` marked not-billable. "8" appears to count the 9-row table minus one. Re-measuring myself before reporting any N-of-M. |
| `video_analyze` = 9 billable provider calls | **HELD, and it matters more than stated** — see below |
| TTS + three vision backends uncovered | to verify |
| `for_success` $0.00 bug shape may exist elsewhere | to verify |

## Architectural finding — `video_analyze` is not its own billable backend

`crates/wcore-agent/src/tool_backends/video_analyze.rs` makes **zero provider
HTTP calls**. Its only outbound reqwest use is `download_remote_video()` (line
~201), which fetches the *user's video*, not a provider. Every billable call is
delegated to `VisionBackend::analyze()`:

- per-frame loop, line ~410: `self.vision.analyze("image/jpeg", &bytes, &prompt)`
- synthesis pass, line ~443: `self.vision.analyze(...)`

with `DEFAULT_FRAME_COUNT` frames + 1 synthesis = the 9.

**Consequence for the fix:** instrumenting the three `VisionBackend`
implementations covers `video_analyze` as a side effect, and does so at the
layer where the HTTP response — and therefore any provider-reported cost
header — actually exists. Instrumenting `video_analyze` itself could only ever
synthesise a figure, which is forbidden. So the plan is: wire the trait impls,
and let video_analyze produce 9 genuine records rather than 1 invented one.

## FINDING (HIGH) — the ledger is INERT in the shipped product

This reframes the deliverable, so it is recorded before any code is written.

Measured with `/usr/bin/grep` over `crates --include="*.rs"`, every hit listed:

| symbol | production call sites | test call sites |
|---|---|---|
| `with_cost_ledger` | **0** | 3 |
| `MediaCostLedger::shared()` / `::new()` | **0** | 5 |
| `MediaCostLedger::summary()` | **0** | 7 |
| `with_rate_card` | **1** (`bootstrap.rs:1334`, image_gen only) | 2 |

Known-positive proving the matcher alive: `with_rate_card` returns the
`bootstrap.rs:1334` production hit in the same invocation.

So in the shipped binary today:

- **`image_gen`** — rate card bound; ledger **unbound**. Its `account()` returns
  the record JSON into the tool result, so the record does reach the model and
  the host **per call**. Session totalling does not exist.
- **transcription** — **no** rate card (bootstrap never binds one), **no**
  ledger, and `account()`'s returned record is discarded at all three call
  sites. Production effect is **a `tracing::info!` line and nothing else**.
- Nothing anywhere reads `MediaCostSummary`. The ledger is **write-only, and
  nothing writes to it.**

**Consequence:** adding `account()` calls to three more backends would add three
more log lines, not cost coverage. A record that reaches no ledger and no
surface is the same category of claim as a `$0.00` — it looks like accounting
and totals nothing. Binding a real session ledger is therefore part of the fix,
not a nice-to-have, and the *operator rate card never reaching transcription*
is a second live gap of the same family as the `$0.00` bug.

## Wire-contract safety

`price_source` / `media_cost` appear in **0** contract fixtures (control: the
same search finds `crates/wcore-protocol/contracts/desktop/v1/...` JSON, so the
matcher was alive). Extending `MediaUnits` therefore needs no contract
regeneration — which is forbidden to this lane anyway.

## Plan

1. Extend `MediaUnits` with an explicit billing basis + token/character units.
   Vision is token-billed and TTS is character-billed; neither is an artifact,
   so `images` must stay 0 for both or a per-image rate card would price them
   (the `$0.00` bug's mirror image — a *wrong* figure rather than a zero one).
2. Vision trait impls (anthropic / openai / gemini) — covers video_analyze too.
3. TTS.
4. Bind one shared session ledger + the operator rate card in `bootstrap.rs`
   across every billable media backend, closing the inert-ledger finding.
5. Both directions on every guard (§3b-iii): non-zero cost recorded for a
   billed call, AND the guard reddens when the cost is dropped.

---

## REFUTED — "a loopback wiremock cannot exercise this backend at all"

The predecessor recorded this as the reason transcription is unit-proven rather
than HTTP-proven: `SsrfSafeResolver` dials only validated public IPs, so a
`127.0.0.1` mock supposedly cannot be reached.

**Measured false.** `cargo test -p wcore-agent --lib tts::` at base commit
`c564674c` on `hetzner-dsm`: **21 passed; 0 failed; 0 ignored; 0 measured;
2226 filtered out**, rc=0 — including
`tool_backends::tts::tests::openai_tts_writes_bytes_to_output_path ... ok`,
which serves audio bytes from a wiremock `server.uri()` through
`build_ssrf_safe_tool_client()` and asserts `bytes_written` against the payload
length. That assertion cannot pass unless the loopback connection succeeded.

Cause: **reqwest does not invoke a custom DNS resolver for an IP literal.**
`SsrfSafeResolver` gates name resolution; `127.0.0.1` needs none, so it is never
consulted. The redirect policy still applies to hops.

Consequence: every backend in this family can be proven over the real HTTP
path, and all of this lane's cost guards are, rather than at record-assembly
level only.

## Denominator, re-measured (the brief's "of 8" was inherited, not verified)

Per-backend scan of `tool_backends/*.rs` plus `image_generation_tool.rs`,
counting `MediaCostRecord` references against outbound `.post(`/`.get(`.
Controls in the same capture: `image_gen`'s accounting lives at the **tool**
layer (`image_generation_tool` cost_refs=4) not the backend layer
(`tool_backends/image_gen` cost_refs=0), and `http_github` cost_refs=0.

Billable media backends — i.e. those that make a provider call for media
generation or analysis:

| # | backend | billable call | status |
|---|---|---|---|
| 1 | `image_gen` (via `ImageGenerationTool`) | images | covered at base |
| 2 | `openai_compat_whisper` | transcription | covered by predecessor |
| 3 | `tts` / OpenAI arm | speech | **wired here** |
| 4 | `tts` / ElevenLabs arm | speech | **wired here** |
| 5 | `anthropic_vision` | vision | **wired here** |
| 6 | `openai_vision` (also serves FluxRouter) | vision | **wired here** |
| 7 | `gemini_vision` | vision | **wired here** |
| 8 | `video_analyze` | **none of its own** — fans out 9 calls to 5-7 | covered transitively |

Not billable, correctly excluded: `piper` (local synthesis, 0 provider calls),
`voice_mode` (delegates to transcription, 0 own calls), and `google_meet` —
which I checked rather than assumed: its 20 HTTP calls go only to
`meet.google.com` / `meet.googleapis.com` / `www.googleapis.com` /
`oauth2.googleapis.com` / `accounts.google.com`, and it references no
`TranscriptionBackend` or `VisionBackend`. It is a meeting-API client, not a
media-generation backend.

**The predecessor's own table listed 7 rows marked billable while its prose
said "1 of 8".** Both numbers are defensible depending on whether TTS's two
provider arms count separately; they are separate providers with separate
billing, so I count 8 and say so explicitly rather than inheriting a figure.

## Mutation battery — every guard proven able to FAIL

Run on `hetzner-dsm` at asserted SHA `9ddac01d`. Each case asserts the mutation
actually changed the file (md5 before/after) before trusting its result, then
restores with `git checkout -- <path>`.

| mutation | applied? | result |
|---|---|---|
| M1 drop anthropic cost-header read | YES | **0 passed; 1 failed**, rc=101, `left: None / right: Some(0.00421)` |
| M2 drop openai-vision cost-**body** read | YES | **0 passed; 1 failed**, rc=101, `left: None / right: Some(0.00421)` |
| M3 drop TTS cost-header read | YES | **0 passed; 1 failed**, rc=101, `left: None / right: Some(0.000765)` |
| M4 unbind bootstrap ledger | **NO — VACUOUS** | see below |

**M4 was vacuous and the harness said so rather than passing.** `cargo fmt` had
wrapped the bootstrap call across three lines, so the single-line `sed` matched
nothing; without the md5 guard this would have reported "mutation applied, no
test failed" and I would have concluded the binding was unguarded on the
strength of a mutation that never happened. This is §3b-i exactly — an absence
confirmed for free by a dead instrument — and it is why the md5 assertion was
written in before the first run. Re-run as M4-retry.

`git status --porcelain` on the hetzner worktree after all restores: empty.
