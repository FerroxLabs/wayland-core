---
lane: 24-media-actions
criterion: "24-C3 (reference channels / the inbound matrix)"
clauses-targeted: "media, native actions — the two clauses at ZERO evidence across six prior lanes"
grade-24-C3: "STILL NOT MET, and this lane does not claim it. But the two clauses that had never been measured on ANY adapter are now measured on discord — a designated reference adapter — from the real binary on Linux, each with a one-variable negative control that reddens. `media` is proven in the DEGRADED direction only; the live vision leg is proven UNREACHABLE with the available credential, and the live transcription leg is reachable but NOT attempted. Six clauses remain at their prior grade, and macOS/Windows have nothing."
new-finding: "F24-C3-H6 (MEDIUM) — the declared per-adapter `MediaBounds` API is decorative: `media_bounds()` is read at exactly one site and that site is a test. Where a cap exists it is an unrelated hardcoded constant diverging from the declaration by 4-5x."
spend: "ZERO. No credential was used, read, printed, or transmitted. The live-vision leg was determined unreachable from source rather than attempted, and the transcription leg was not run."
status: complete
---

# 24-MEDIA-ACTIONS — the two clauses that had never been measured, measured

**Verdict up front: `24-C3` is NOT MET and I do not claim it.** Six lanes have declined it and
this is the seventh. What changed is narrow and real: `media` and `native actions` were at
**zero evidence on every adapter**; both are now measured on **discord**, one of the two
designated reference adapters, driven from the real `wayland-core` binary through `gateway run`
on Linux, with a negative control per clause that is proven to redden.

I also found that one of the two clauses is **partly unmeasurable as specified**, and say
exactly which part and why.

---

## 1. What the two clauses actually promise

Criterion source, read at origin rather than from a later paraphrase — `.planning/ROADMAP.md:119`,
Phase 24 Success Criterion 3:

> Reference channels prove setup/auth, access, routing, **media**, **native actions**,
> idempotency, reconnect/reload, and health.

### `native actions` — the term does not exist in the codebase; the concept does

`/usr/bin/grep -rn "native_action\|NativeAction" crates "--include=*.rs"` → **0**, with the
instrument proven alive in the same invocation by a known-positive (`ChannelManager`, **23
files**). `native action` appears in **zero** `.md` files outside `.planning/`. It is criterion
vocabulary with no implementation counterpart by that name.

Searching the **concept** instead (LANE-BRIEF §3b-i rule 3) finds it immediately. A native
action is the **ack state machine** — the platform-native affordance the runtime performs *on
the platform* in response to an inbound message. `wcore-agent/src/channel_inbound.rs:503-556`,
`run_turn`:

1. `ack.reactions()` → `react_on(ch, conv, msg_id, "👀")` **on receipt**
2. `ack.typing()` → `spawn_typing_keepalive(...)` under an `AbortOnDrop` guard for the turn
3. after dispatch → `react_on(..., "✅")` on success, `"❌"` on failure

Two facts about this surface explain why six inbound lanes never touched it:

- **`AckMode` defaults to `Off`** (`dispatch/access.rs:191`). Nothing fires unless a channel
  config asks for it. An inbound matrix that does not set `ack` exercises none of this.
- **Both `react_on` failures are swallowed** — `tracing::debug!` for the first, `let _ =` for
  the second. **Core's own logs therefore cannot prove a native action happened.** It has to be
  counted on the platform side. Fixture-side counting is the only valid instrument here, not a
  convenience.

### `media` — inbound enrichment, and it IS wired into the persistent gateway

`wcore-agent/src/channel_media.rs`. The path is live in production, not dead:

`channel_inbound_host.rs:220-240` builds `ChannelMediaEnricher` from `build_vision_backend()` +
`build_transcription_backend(config)` and hands it to `ChannelTurnDispatcher`, whose
`channel_dispatch.rs:138` is the **single production `.enrich(` call site** (the other 11 are
unit tests). `build_turn_prompt` (`channel_dispatch.rs:278-297`) folds the resulting
`Attachment::transcribed` text into the user prompt.

Critically, `enrich()` writes an honest degraded notice **even when inert**, and does so
**before attempting any fetch** (`channel_media.rs:164-172`). That is what makes the clause
measurable with no credential and no network.

---

## 2. Is media enrichment reachable at all? — the "no key" excuse HOLDS FOR VISION, NOT TRANSCRIPTION

The lane assignment asked me to check whether the `README.md:348` disclosure ("inert unless a
vision or transcription key is configured") still holds now that a provider key exists at
`~/.wayland-secrets/flux.env`. **It half holds, and the half matters.**

That file contains exactly one variable — **`FLUX_API_KEY`**. (Name only. The value was never
read, printed, transmitted, or used; see §7.)

| leg | resolver | consults `FLUX_API_KEY`? | reachable with the available credential |
|---|---|---|---|
| image → description | `build_vision_backend()` `tool_backends/mod.rs:321-338` | **NO** — only `ANTHROPIC_API_KEY`, `OPENAI_API_KEY`, `GEMINI_API_KEY` | **NO** |
| voice → transcript | `build_transcription_backend()` `tool_backends/mod.rs:370-403` | **YES — arm 4** | **YES** |

So the prior lane that proved the *transcription* resolver reaches `flux.env` was right, and
arm 4 is the mechanism. **Vision has no such arm.** `build_vision_backend` never consults
`FLUX_API_KEY`, so image→description is unreachable with the credential that exists.

**This is confirmed at runtime, not just from source.** The gateway log of the measured run
(`run3-A-gateway.log`) contains, verbatim:

```
WARN vision: no API key found (ANTHROPIC/OPENAI/GEMINI) — vision tool will be hidden
INFO transcription: using whisper-1 at http://127.0.0.1:36197/v1/audio/transcriptions (active OpenAI-wire provider)
```

**The second line is a finding in its own right and the most useful thing here for the next
lane.** Transcription resolved via **arm 3** — the *active OpenAI-wire provider* — straight to
my **local LLM fixture's base URL**. That means the transcription round-trip can be driven
**with no credential and no spend at all**, by pointing the active provider at a fixture that
serves `/v1/audio/transcriptions`. No Flux billing (`$0.016670` with a 10-second floor,
`tool_backends/mod.rs:365`) is required to close that leg. See §6.

---

## 3. What was measured

**Adapter: discord** — a designated reference adapter, driven end-to-end from
`wayland-core gateway run` against a local WS+REST fixture, on `hetzner-dsm` (Linux).
Driver: `scripts/f24-media-actions.mjs` (new, strictly additive — it *imports and subclasses*
`f24-discord-fixture.mjs`; `scripts/f24-inbound.mjs` was not touched).

Three legs, each differing from leg A by **exactly one variable**:

| leg | `ack` | attachment | purpose |
|---|---|---|---|
| A | `"both"` | image/png | positive for both clauses |
| B | `"off"` | image/png | native-actions negative control |
| C | `"both"` | none | media negative control |

### Results — two consecutive full runs, all four gates PASS

| gate | clause | kind | result | measured |
|---|---|---|---|---|
| **G1** | native actions | POSITIVE | **PASS** | `reactions=2 emojis=["👀","✅"] typing=1` |
| **G2** | native actions | NEGATIVE CONTROL | **PASS** | `turn_ran=true reactions=0 typing=0` |
| **G3** | media | POSITIVE | **PASS** | `turn_ran=true prompts=1 notice=true` |
| **G4** | media | NEGATIVE CONTROL | **PASS** | `turn_ran=true capture_alive=true notice=false` |

Run 3 `f9c7f5af…`, run 4 `2c4ab07e…` (`shasum -a256`, distinct `generated_at` and `out_dir`
confirmed with `/usr/bin/diff`). Both `all_pass=true`.

**G1 asserts emoji identity, not just a count.** A run that fired two 👀 and no completion
reaction has the same `reactions_total` as a correct one; only `["👀","✅"]` proves the actual
state machine ran. The observed sequence is exactly the one `run_turn` specifies.

**G2 is what defeats the universal-denial trap** the lane assignment warned about — the access
leg of this same criterion once passed on three adapters *because everything was denied*. G2
requires `turn_ran=true` **and** zero acks: the turn must still be admitted and dispatched. If
the binary had simply denied or never connected, G1's counts would be zero **and** G2 would
fail on `turn_ran`. G1 and G2 can only both pass if the difference is genuinely the `ack`
setting. This is a one-variable control, not an absence.

**G4 proves its own instrument alive** before trusting a negative (§3b-i): it requires
`capture_alive=true` — the control prompt must be captured *and* contain the leg's probe text —
before its `notice=false` is allowed to count. A dead capture cannot pass G4.

### The media evidence, verbatim

Leg A turn prompt (`run3-A-llm-journal.jsonl`, 561 bytes):

```
f24ma probe A-ack-both-image

[attachments received with this message:
  1. Image (image/png) — description: [Inbound image received but NOT analyzed: no vision
backend is configured, so the assistant cannot see this image. Do not guess its contents.
To enable image understanding, set ANTHROPIC_API_KEY, OPENAI_API_KEY, or GEMINI_API_KEY.]]
Current date: 2026-07-29
```

Leg C control (`run3-C-llm-journal.jsonl`, 245 bytes): `f24ma probe C-ack-both-noimage\nCurrent
date: 2026-07-29` — no attachment block, no notice.

**What this proves:** an inbound image on a reference adapter traverses gateway → adapter parse →
inbound subscriber → dispatcher → enricher → turn prompt, and the model is told *honestly* that
it cannot see the image. **What it does NOT prove:** that a real description or transcript is
ever produced. That is the degraded direction only, and I grade it as such.

---

## 4. NEW FINDING — F24-C3-H6 (MEDIUM): the declared media-bounds API is decorative

`wcore-channels/src/media.rs` is a 353-line module whose stated purpose is to enforce a declared
per-adapter bound and to guarantee "never drop silently". Measured call sites, whole workspace:

```
/usr/bin/grep -rn "media_bounds" crates "--include=*.rs"
  wcore-channel-email/src/lib.rs:535            fn media_bounds(...)    <- DECLARES
  wcore-channel-discord/src/lib.rs:405          fn media_bounds(...)    <- DECLARES
  wcore-channels/src/lib.rs:168                 fn media_bounds(...)    <- trait default
  wcore-channels/tests/framework_matrix.rs:156  fn media_bounds(...)    <- test impl
  wcore-channels/tests/framework_matrix.rs:373  let bounds = ch.media_bounds();   <- ONLY READER
```

**The only site that ever reads `media_bounds()` is a test.** `media::normalize` /
`normalize_all` likewise have no production caller. `manager.rs:774-785` (`fetch_media_on`) does
not consult bounds; it delegates straight to the adapter. Adapters build `Attachment` directly.

Three different numbers are in play, and the declared one is never the enforced one:

| adapter | **declares** | **actually enforced** | where | divergence |
|---|---|---|---|---|
| discord | 25 MiB / 10 | **100 MiB** | `discord/src/rest.rs:370` `MAX_MEDIA_BYTES` | **4× larger** |
| email | 10 MiB / 20 | **2 MiB** | `email/src/imap.rs:619` `MAX_INLINE_ATTACHMENT_BYTES` | **5× smaller** |
| other 8 | nothing declared | — | — | — |

`max_attachments` is enforced **nowhere at all**. The trait doc at `wcore-channels/src/lib.rs:165-166`
states the bound is *"Enforced by `media::normalize`"* — an enforcement that does not occur.

For discord specifically it is unenforceable *by construction*: `MessageAttachment`
(`gateway.rs:129-135`) deserializes only `url` and `content_type` and **never parses Discord's
`size` field**, so no per-attachment size bound can be applied there regardless of the dead
`normalize` path.

**Control proving this is a true negative, not a dead instrument:** the sibling declared bound
`max_message_len` has **9** non-definition production uses — so this search shape *does* find
consumers when they exist. `max_attachments` has **0**.

**Graded MEDIUM, deliberately not higher.** The OOM and SSRF defenses are real and correct
(CDN host allowlist, `read_body_capped`, and `rest.rs:615-622` tests refusal of
`169.254.169.254`). What is decorative is the *advertised* per-adapter bounds API and the
`MediaDisposition::Degraded` "never drop silently" record, which no production path emits. Per
LANE-BRIEF §5 this goes to BACKLOG, non-blocking — I am not inventing a stricter rule.

---

## 5. Instrument defects found — SEVEN, and three were in my own instrument

LANE-BRIEF §3b/§3b-i/§6b-ii warned that instruments carry the defect they hunt. Seven instances
this lane, each repaired in-lane rather than merely written up (§6b-ii):

| # | defect | effect | how caught |
|---|---|---|---|
| 1 | zsh ate unquoted `--include=*.rs` | `native_action` returned **0** — a free confirmation of the exact absence I was about to report | the known-positive control in the same invocation ALSO returned 0 |
| 2 | bare `wc -c` returns **0** for a 72-byte file | every byte-count would read 0 — indistinguishable from "capture failed" | `/usr/bin/wc` → 72, `/usr/bin/stat -f%z` → 72 |
| 3 | substring false **positive**: `ack_mode` matches inside **fallb`ack_mode`ls** | inflated a decorative-looking surface to **56** hits; true count **14** (38 contaminant) | `grep -w` |
| 4 | `head -30` truncated a call-site list | discord absent from it — one step from reporting "the reference adapter never builds an inbound attachment". It does (`gateway.rs:381`) | re-ran without `head` |
| 5 | **my driver**: `sleep()` used `spawnSync`, blocking Node's event loop | the fixture runs in-process, so it could never accept the dial — **all three legs NOT MEASURED**, looking like a product failure | manual `gateway run` showed the dial happening and getting ECONNREFUSED |
| 6 | **my driver**: read `report.reactions`, which `report()` does not expose | `emojis=[]` → G1 **false FAIL** while counts were already correct | asserting emoji identity, not just count |
| 7 | bare `diff` printed **"Files are identical"** for two differing files | a difference check that cannot detect a difference | `shasum` disagreed; `/usr/bin/diff` showed the deltas |

Defect 2 lands directly on this lane's "byte-count every capture" instruction. Defect 1 is the
sharpest: it is §3b-i happening live, and **only the known-positive control caught it**.

Worth stating plainly: defects 5 and 6 both produced **FAILs, not false passes**. My gates
failed loudly on my own broken instrument rather than passing on it, which is the safe
direction and is why they were found at all.

### The matcher self-test, and proof it can fail

`--selftest` runs three assertions, not two (§6b-ii): known-positive matches, known-negative
does not, **and the old broken matcher misses the real notice**. The third is the only one that
proves the repair does anything.

The "broken matcher" is the realistic mistake: transcribing `IMAGE_NO_VISION_NOTICE` straight
out of the `.rs` file keeps the source's `\`-continuation newline and indent inside the phrase,
so it can never match the runtime string. The captured prompt in §3 **confirms** the runtime
value has single spaces there — the hazard is real, not hypothetical.

**Mutation-proved the self-test reddens**, rather than trusting three PASSes:

| mutation | result |
|---|---|
| needle → `'zzz-never-appears-zzz'` | known-positive **FAIL**, third assertion **FAIL** |
| needle → `''` (matches everything) | known-negative **FAIL** |

And the gates themselves have demonstrably reddened on real runs, not only under mutation: run 1
failed all four (`identified=false`, correctly reported **NOT MEASURED** rather than passing on
zeros), and run 2 failed G1 alone on defect 6.

---

## 6. Exact remaining distance to `24-C3` MET

| # | what is left | cost |
|---|---|---|
| 1 | **`media`, live direction** — a real transcript or description produced end-to-end. **No credential needed**: §2 shows transcription arm 3 resolves to the *active OpenAI-wire provider*, i.e. a local fixture. Add `/v1/audio/transcriptions` to a fixture, drive an audio attachment on **telegram** (its `download_bytes` has no host allowlist, unlike discord's CDN lock, and `f24-tg-fixture.mjs` exists) | ~1 session, **zero spend** |
| 2 | **`media`, live vision** — genuinely blocked: `build_vision_backend` never consults `FLUX_API_KEY` and no ANTHROPIC/OPENAI/GEMINI key exists. Either a key, or a 4th arm mirroring transcription's | Sean-gated (credential) **or** ~0.5 session for the arm |
| 3 | **`native actions` breadth** — measured on discord only. telegram (react+typing), matrix (lane/24-h6 owns that crate), slack/whatsapp (react, no typing) | ~1 session for 2-3 adapters |
| 4 | **`reconnect/reload`** — still PARTIAL, and **F24-C3-H5 remains unfixed** (prior lane) | ~1 session |
| 5 | **macOS / Windows** — nothing for either, on any clause | ~2 sessions |
| 6 | **F24-C3-H6** (§4) — measured, **not fixed**. MEDIUM → BACKLOG | ~0.5 session |

`24-C3` remains a release blocker. It is closer than it was — the two clauses at zero are no
longer at zero, and item 1 turns out to need **no credential**, which was not known before this
lane.

---

## 7. Credential handling (LANE-BRIEF §0 disclosure)

**No credential was used. Spend: zero.**

`~/.wayland-secrets/flux.env` was inspected for **variable names only**, via
`grep -oE '(export[[:space:]]+)?[A-Za-z_][A-Za-z0-9_]*[[:space:]]*='`, which cannot emit a
value. The value was never read, printed, transmitted, written to disk, placed in `argv`, or
sent to hetzner. The live-vision leg was determined **unreachable from source** rather than
attempted, and the transcription leg was **not run**.

**Sweep, actually executed rather than asserted** — over every file this lane created (evidence
directory, driver, this report):

| check | result |
|---|---|
| extracted value length (length only, never the value) | **51** |
| sweep aborts if extraction is empty — so it cannot self-pass on a failed read | guard present |
| **known-positive control**: files containing the *name* `FLUX_API_KEY` | **3** (instrument alive) |
| **SWEEP**: files containing the live secret **value** | **0** |

The known-positive control matters: a sweep reporting "0 hits" is the single easiest result to
obtain from a broken instrument (§3b-i). The 3-file name hit proves the same `grep -rIl -F`
invocation over the same paths does find strings that are present.

---

## 8. What I did NOT do

- **Did not mark `24-C3` MET.** Six clauses untouched by this lane, two platforms with nothing.
- **Did not claim `media` MET.** Measured in the **degraded direction only**. No real
  description or transcript was ever produced. Saying otherwise would be the "green by
  universal denial" failure in a new costume — every attachment producing a "cannot see this"
  notice is *correct behaviour*, but it is not proof the enrichment works.
- **Did not attempt the live transcription leg**, though §2 shows it is reachable with zero
  spend. Out of budget, and I would rather hand over a precise, evidenced route than a rushed
  half-run. Route documented in §6 item 1.
- **Did not fix F24-C3-H6.** MEDIUM → BACKLOG per §5; fixing a bounds API across 10 adapters at
  the end of a lane is exactly the blind end-of-lane change a prior lane was right to refuse.
- **Did not touch** `scripts/f24-inbound.mjs`, `.github/workflows/ci.yml`,
  `crates/wcore-cli/src/{lib,main}.rs`, `.planning/BACKLOG.md`, or
  `crates/wcore-channel-matrix/` (lane/24-h6). **No Rust source was modified by this lane at
  all** — the measurement needed none, which is itself worth recording: both clauses were
  already reachable and simply had never been asked for.
- **Did not run the full workspace suite.** No Rust changed, and a full run under other lanes'
  load is not a measurement (§6).
- **Did not use the Darwin-behaviour exception.** Nothing here is macOS-specific; everything
  built and ran on hetzner.

## 9. Evidence

`.planning/phases/24-gateway-automation-channels-typed-api/24-MEDIA-ACTIONS-evidence/`

| file | bytes | what |
|---|---|---|
| `24-MEDIA-ACTIONS-NOTES.md` | — | append-only working record, first committed at T+13 before any investigation (§6b-i) |
| `run3-summary.json` | 4094 | run 3, all four gates, `all_pass=true`, sha `f9c7f5af…` |
| `run4-summary.json` | 4094 | run 4, reproducibility, `all_pass=true`, sha `2c4ab07e…` |
| `run3-A-llm-journal.jsonl` | 561 | leg A turn prompt carrying the degraded notice |
| `run3-C-llm-journal.jsonl` | 245 | leg C control prompt, no notice |
| `run3-A-gateway.log` | 8533 | runtime confirmation of the vision/transcription resolver split |

Byte counts via `/usr/bin/stat -f%z` — **not** `wc`, which this lane measured returning 0 for a
72-byte file (§5 defect 2).

Driver: `scripts/f24-media-actions.mjs`. Re-run with:

```bash
node scripts/f24-media-actions.mjs --selftest          # instrument, 3 assertions
node scripts/f24-media-actions.mjs --binary <wayland-core> --out <dir>
```
