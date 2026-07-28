# 27-CREDENTIALLED — running NOTES (append-only, committed continuously)

Lane `lane/27-credentialled`, branched from `plan/f20-unified-audit-repair` @ `3cfc336f`.
Worktree `/Users/seandonahoe/dev/waylandcore-frontier-worktrees/lane-27-credentialled`.

**No credential value appears in this file or any artefact this lane writes.**
The key is read from `~/.wayland-secrets/flux.env` at use time and never echoed.

---

## T+0 — established before touching anything (read, not assumed)

### The provider surface the key unlocks

- `crates/wcore-providers/src/flux_router.rs` — 212 lines, OpenAI-compatible provider,
  resolved via `FLUX_API_KEY` → `[providers.flux-router]`.
- `crates/wcore-agent/src/tool_backends/image_gen.rs:849-1000` — the built-in image
  generator derives `https://api.fluxrouter.ai/v1/images/generations` from a
  `flux-router`-shaped provider config (`dalle_backend_from_config`). Unit-tested at
  `image_gen.rs:928`, `:953`, `:985`. So **shape A's generator has a live route the moment
  the key is present** — that is C3's credential-blocked cell.

### The transcription resolver — measured, matches the inventory

`crates/wcore-agent/src/tool_backends/mod.rs` `build_transcription_backend()`:

```
1. GROQ_API_KEY  -> OpenAiCompatWhisperBackend(https://api.groq.com/openai/v1/audio/transcriptions, whisper-large-v3-turbo, "groq")
2. OPENAI_API_KEY-> OpenAiCompatWhisperBackend(https://api.openai.com/v1/audio/transcriptions, whisper-1, "openai")
otherwise -> None, tool hidden
```

**Confirms INV-26-27's measurement**: two env keys, nothing else, no config route.
**But the shape is more favourable than the inventory implies**: the backend is
`OpenAiCompatWhisperBackend::new(key, url, model, label)` — fully parameterised. There is
no OpenAI-specific wire code to write; the resolver's *key list* is the only thing that
excludes flux-router. That distinction decides whether C4-transcription is "needs a new
backend" (it does not) or "needs a resolver arm" (it does). Recorded before I attempt it.

### What C3's 7 NOT MEASURED cells actually are

From `evidence/27-gaps/c3-generation/README.md` + `four-shapes-linux-HEAD-status.txt`
(`WLRC=0 PASS=10 FAIL=0 NOT_MEASURED=7 WLDONE`):

- accounting × 4 (shapes A/B/C/D)
- failures × 3 (shapes B/C/D — the control proves the path, each shape did not)

The prior lane's stated blocker for B/C/D was **not** a media key: *"invoking one needs a
model turn and no inference server runs on the measurement host."* flux-router is an
inference server (77 models, OpenAI-compatible chat completions). So B/C/D accounting +
failures are unblocked by the same key, via `flux-fast`/`flux-standard`, not just shape A.

### Traps I am carrying in from the brief (do not re-learn these)

- `flux-fast` is a **reasoning** model: a 16-token budget returns HTTP 200 with empty
  content and all 16 tokens billed as `reasoning_tokens`. Budget completions generously or
  a starved call reads as a defect.
- **Piper is not a route.** `piper_download` registered nowhere, `build_piper_tts_backend()`
  returns `None` unconditionally, `synthesize` is a hard stub, feature off by default.
  Two planning documents recommend it; both are wrong and are on my correction list.
- Byte-count every capture; `echo "EXIT=${PIPESTATUS[0]}"` after any pipeline.
- Run test targets by file, never by filter (a filter matching no test exits 0 at 0 tests).

## Plan of attack (priority order, per brief)

1. C3 accounting on the NOT MEASURED shapes, using `flux-image` (+ `flux-fast` for the
   model turn in B/C/D).
2. C4 transcription via `flux-voice*` — **first** establish whether the product can reach
   it at all. If it cannot, that is the finding; do not hack around it in-lane.
3. C4 TTS / barge-in, only if (2) lands and the shape permits.
4. Residual closable-without-credential items if time remains.

Anything unmeasurable stays **NOT MEASURED**. Not FAIL, not PASS.

---

## T+ log (appended after every measurement)

- **T+0** — worktree created at `3cfc336f`, NOTES committed. Nothing measured yet beyond
  the source reads above.

- **T+25 — provider route surface, measured against the live API.**
  `POST /v1/<path>` with `{}` body, to establish route existence before spending anything:

  | path | HTTP | body |
  |---|---|---|
  | `audio/transcriptions` | **400** | `invalid or missing multipart audio upload` (param `file`) |
  | `audio/speech` | 500 | `Internal server error` |
  | `audio/translations` | **404** | `Not Found` |
  | `images/generations` | **400** | `prompt required` |
  | `responses` | 400 | `model is required` |
  | `embeddings` | 500 | missing `input` |

  404 vs 400 is the discriminator: `audio/translations` does not exist; the other five do.
  `/v1/models` returns **77** ids (HTTP 200, 9,958 bytes), including `flux-image`,
  `flux-voice`, `flux-voice-fast`, `flux-voice-accurate`. Model metadata carries **no**
  capability field — the route probe above is the only way to know what serves what.

- **T+35 — C4 transcription: the provider round-trip WORKS, verbatim.**
  Positive input built locally and free: macOS `say -v Samantha` → `afconvert -f WAVE -d
  LEI16@16000 -c 1` → `speech.wav`, 115,554 bytes, **1 ch / 16000 Hz / 16-bit / 55,735
  frames / 3.48 s** — the same format `f27_voice_capture` produces.

  ```
  POST /v1/audio/transcriptions  model=flux-voice-fast   HTTP=200  75 bytes
  {"text":" The quick brown fox jumps over the lazy dog near the riverbank."}
  ```
  `flux-voice` returned byte-identical output. Verbatim match to what `say` was given.

  **Negative control fires** — same duration, same format, all-zero samples
  (`silence.wav`, 111,514 bytes):
  ```
  HTTP=200  22 bytes   {"text":" Thank you."}
  ```
  Different text, so the positive is not a driver that always reports the expected string.
  (`" Thank you."` is the known Whisper silence hallucination.)

  **`response_format=verbose_json` — the exact format the product sends** (see
  `openai_compat_whisper.rs:61`) — is served correctly: `task`, `language: English`,
  `duration: 3.483437568`, `segments[]` with `start`/`end`/`text`/`tokens`. 458 bytes.
  So the wire is compatible with the shipped backend's own request shape, not merely with
  "OpenAI-ish".

- **T+35 — C4 accounting: TWO separate defects, both structural.**
  1. **The provider reports cost only in HTTP response headers**, never in the JSON body:
     `x-flux-cost-usd: 0.016670`, `x-flux-billed-seconds: 10`, `x-flux-routed-model`.
  2. **`OpenAiCompatWhisperBackend` cannot see it, and could not record it if it did.**
     `openai_compat_whisper.rs:85-86` takes `resp.status()` then immediately `resp.text()`
     — **response headers are never read**. And `TranscriptionOutcome::Ok { transcript,
     language, segments }` has **no cost or usage field at all**, so the type system has
     nowhere to put one. The transcription path has **no accounting surface**, independent
     of provider.

  Contrast with chat: `/v1/chat/completions` puts `cost_usd` **in `usage`** —
  `{"completion_tokens":29,"prompt_tokens":10,"cost_usd":0.000126,...}` — as well as in
  `x-flux-cost-usd`. So C3's model-turn accounting has a body-visible record; C4's
  transcription accounting does not.

  **Measured prices** (bound the run budget): trivial chat call `$0.000126`; one 3.48 s
  transcription `$0.016670` (billed at a **10-second floor** — 3.48 s of audio bills as 10).

- **T+35 — `flux-fast` reasoning-budget trap reproduced and avoided.** At `max_tokens=2000`,
  `content='PONG'` with `completion_tokens=29` of which `reasoning_tokens=26`. Only 3
  tokens were visible output. A 16-token budget would have spent everything on reasoning and
  returned empty — exactly as the brief warned. Also worth recording: `flux-fast` is a
  **router alias**, `x-flux-original-model: flux-fast` → `x-flux-routed-model:
  deepseek-v4-flash`. The model that answers is not the model you name.

- **T+40** — hetzner worktree `hz/27-cred` at `/root/wayland-27cred` created at lane HEAD
  `8ff6a3eb`; `cargo build --release -p wcore-cli` running (`/root/wayland-27cred-build.log`).
  Host has `node v22.21.1` (the MCP fixture needs it) and 714 G free on `/`.
