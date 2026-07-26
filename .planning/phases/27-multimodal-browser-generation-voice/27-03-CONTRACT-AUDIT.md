# 27-03 — Generation and voice contract audit

**Determination: PARTIAL — and severely under-measured. Read §5 before using
anything here.**

This plan was measured only in part. What was measured was measured on real
hardware against the shipped binary and is recorded with its evidence. What was
not measured — which includes the plan's single hardest and most important
exercise, a real interruption during real audio playback — is named as NOT RUN
with a reason, not substituted for and not quietly dropped.

---

## 1. Provenance

Host `hetzner-dsm`, SHA `2ecdfdf54ff7fda920eec7d068337006e5da4ee4`, shipped
`target/release/wayland-core` built `--release --locked`. Instrument: the same
recording provider used by 27-01, which captures every outbound request body
verbatim — including the system prompt, which is where the answer to the
honest-degradation question turned out to live.

---

## 2. OBS-01 — the honest-degradation advisory REACHES THE MODEL — **REFUTED-NO-DEFECT**

The plan's stated truth was that an unconfigured generation backend makes the
tool vanish from the model's schema, and that `capability_advisory.rs` exists
because that silent drop made the model fabricate a cause.

**Measured: the advisory is real, it is complete, and it is on the wire.**

`crates/wcore-agent/src/bootstrap.rs:1983` appends
`render_capability_advisory(&registry)` to the system prompt. Captured verbatim
from the outbound request body on a box with no media credentials
(`evidence/27-03/advisory-on-the-wire.txt`):

```
# Unavailable capabilities
The capabilities below are NOT available in this session because their backend
is not configured. If the user asks for one, do NOT claim the ability does not
exist or invent another reason — tell them exactly what to configure:
- Image generation — unavailable: set OPENAI_API_KEY, FAL_API_KEY, GEMINI_API_KEY, or HF_API_KEY
- Image understanding (vision) — unavailable: set ANTHROPIC_API_KEY, OPENAI_API_KEY, or GEMINI_API_KEY
- Audio transcription — unavailable: set GROQ_API_KEY or OPENAI_API_KEY
- Text-to-speech — unavailable: set OPENAI_API_KEY or ELEVENLABS_API_KEY
- Video analysis — unavailable: set ANTHROPIC_API_KEY, OPENAI_API_KEY, or GEMINI_API_KEY (and install ffmpeg)
- Discord — unavailable: set DISCORD_BOT_TOKEN
```

Every line names the capability, states plainly that it is unavailable, and
names the exact environment variables that would enable it. The instruction to
the model explicitly forbids inventing a cause. **This is the honest-unavailable
behaviour the criterion asks for, and it already works.** Recorded as a
successful refutation; nothing was changed.

The operator-facing half is present too, on stderr:

```
WARN vision: no API key found (ANTHROPIC/OPENAI/GEMINI) — vision tool will be hidden
WARN transcription: no API key found (GROQ_API_KEY or OPENAI_API_KEY) — tool hidden
WARN tts: no TTS backend configured — set OPENAI_API_KEY or ELEVENLABS_API_KEY
     (or download Piper voices via piper_download). Tool hidden.
WARN video_analyze: ffmpeg not found on PATH — tool hidden (install ffmpeg to enable)
```

---

## 3. OBS-02 — but it does NOT reach the HOST — **CONFIRMED, MEDIUM**

The `--json-stream` capture from the same binary contains, across 41 events:

- **zero** `capability_activation` events for image generation, TTS,
  transcription, video analysis or voice;
- **zero** events of any type mentioning those capabilities;
- the only `info` events are provider retry notices.

Event types on the wire: `ready`, `execution_policy`, `workspace_policy`,
`capability_activation`, `stream_start`, `provider_attempt`,
`provider_retry`, `provider_failure`, `info`, `text_delta`, `session_cost`,
`stream_end`, `error`.

So the model is told honestly and the HOST is told nothing. A Desktop user
whose TTS is unconfigured gets no surface on which to render "unavailable — set
`OPENAI_API_KEY` or `ELEVENLABS_API_KEY`"; the information exists in the
process and never crosses the protocol boundary. The remedy is exactly the
remedy 27-02 needs — activation events for these capabilities — and is blocked
behind the same fenced seam.

**Severity MEDIUM**, not HIGH, because the model IS told and is instructed to
relay it, so the user can still learn the truth by asking. It is one indirection
away from honest rather than absent.

---

## 4. What the four-way generation comparison would have needed

The plan's central deliverable is a measured comparison of built-in, MCP-only,
late-MCP and combined generation across discovery, credentials, accounting and
failure semantics. **None of the four was measured.** Building the local MCP
server fixture that exposes a media tool with no network dependency
(`fixtures/f27/generation/`) is the prerequisite and it was not built.

One structural fact IS established from source and is recorded as SOURCE-ONLY
rather than as a finding: cost accounting in this engine is token-shaped — the
protocol's `session_cost` variant carries `per_turn` entries with
`input_tokens`/`output_tokens` — and the pricing crate has no notion of a media
call. A generation request is a billed external call that produces no cost
record. The `session_cost` event captured in
`evidence/27-01/live/linux-host-valid-png.jsonl` shows the token-shaped
structure directly. **That this is currently unaccounted is a measurable fact;
whether it matters is a decision, and neither was settled here.**

---

## 5. NOT RUN — named plainly

| Exercise | Status | Reason |
|---|---|---|
| **Real interruption during real audio playback** | **NOT RUN** | This is the plan's own "single hardest live exercise in the phase and the one most likely to be quietly replaced by a unit test". It was not replaced by a unit test — it was not run at all. `hetzner-dsm` is headless with no audio device, so `cpal::default_host()` has nothing to bind. The Mac has an audio device but no working Cargo, and no macOS artifact exists for this SHA. Windows (`seandesktop`) has audio and a toolchain and was reachable — that was the available path and it was not taken. |
| Streaming voice: ordered protocol events under cancellation | NOT RUN | Depends on the above. |
| Honest-unavailable on a machine with no capture device | **NOT RUN AS SPECIFIED** | The plan makes this a REQUIRED live assertion. What was measured is adjacent and weaker: the advisory names transcription and TTS as unavailable for want of a KEY. Whether the voice loop publishes an honest unavailable for want of a DEVICE — the actual assertion — was not exercised. |
| Four-way generation comparison | NOT RUN | Fixture prerequisite not built. |
| Late-MCP discovery of a media tool | NOT RUN | Same. |
| Generation accounting | SOURCE-ONLY | See §4. |
| Voice corpus (`fixtures/f27/voice/`) | NOT BUILT | — |
| Generation corpus (`fixtures/f27/generation/`) | NOT BUILT | — |

**No code was written for this plan.** No `27-03` production file was modified.

---

## 6. Disposition

Termination state: **incomplete, honestly reported.** The plan's own rules
permit terminating on a refutation, and §2 IS a genuine refutation of its
leading premise — the honest-degradation idiom for generation exists and works
on the model side, verbatim on the wire. But the plan's requirements F27-03 and
F27-04 turn on the three things in §5 that did not run, and **neither
requirement may be marked complete on this evidence.**

The most valuable single sentence in this document: **Criterion 4 of Phase 27
is not evidenced. Real streaming voice with a real interruption was never
exercised on any machine.**
