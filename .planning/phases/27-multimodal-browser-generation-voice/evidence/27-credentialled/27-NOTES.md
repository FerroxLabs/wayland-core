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
