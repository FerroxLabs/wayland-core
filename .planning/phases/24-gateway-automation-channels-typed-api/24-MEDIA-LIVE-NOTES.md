# 24-MEDIA-LIVE — working notes (append-only)

Lane `24-media-live`. Worktree `/Users/seandonahoe/dev/waylandcore-frontier-worktrees/lane-24-media-live`,
branch `lane/24-media-live`, merge-base `15cda12d6a189d7cad3daf0998eded4710f809af`.

Goal: drive `24-C3`'s `media` clause in the **POSITIVE** direction, live, on real hardware —
a real media attachment inbound on a reference adapter, enriched with **real produced content**
(a transcript), reaching the model, observably. Then grade `media` honestly.

Predecessor (`24-MEDIA-ACTIONS.md`) proved media in the **DEGRADED direction only**.

---

## T+0 — plan committed before investigation (LANE-BRIEF §6b-i)

1. Re-measure the vision-reachability claim from source at HEAD (do NOT inherit it).
2. Establish the transcription route: adapter choice, fetch path, mime routing.
3. Build known-ground-truth audio (macOS `say`, two distinct utterances).
4. Drive live on `hetzner-dsm` through `gateway run`, real binary.
5. One-variable negative control that is PROVEN to redden.
6. Anti-echo control: a second, different utterance must transcribe differently.
7. Secret sweep + disclosure.
8. Grade `media`; do NOT claim `24-C3`.

## T+0 — established from source at HEAD, before any run

### Vision leg — predecessor's unreachability finding RE-MEASURED at HEAD, still holds

`crates/wcore-agent/src/tool_backends/mod.rs`, `build_vision_backend()` (~line 321) consults
exactly three env keys — `ANTHROPIC_API_KEY`, `OPENAI_API_KEY`, `GEMINI_API_KEY` — and takes no
`&Config`, so it has **no config-resolved arm at all** (unlike transcription's arm 3). No
`FLUX_API_KEY` arm. To be re-checked: whether the credential could reach vision *indirectly*
(e.g. `OPENAI_API_KEY=<flux key>` with a redirectable base URL). That is the concept search,
not the keyword search — see §vision below once measured.

### Transcription leg — four arms, two of which the credential can satisfy

`build_transcription_backend(config)`:
1. `GROQ_API_KEY`
2. `OPENAI_API_KEY`
3. **active OpenAI-wire provider** from `Config` (native OpenAI or FluxRouter) — config-resolved
   `base_url`, model `flux-voice-fast` when `ProviderType::FluxRouter`
4. **`FLUX_API_KEY`** in env → FluxRouter default base URL, model `flux-voice-fast`

Arms 3 and 4 are documented as deliberately appended (not prioritised) so Groq users are not
silently moved onto a billed arm. Flux STT is billed `$0.016670` with a 10-second floor.

## Open questions to resolve next

- Which adapter can actually FETCH the media bytes? Predecessor says discord has a CDN host
  allowlist (would refuse a localhost fixture URL); telegram's `download_bytes` reportedly has
  none. Verify from source.
- What mime routing sends an attachment down the transcription arm vs the vision arm?
- Does the enricher fold the produced transcript into the turn prompt distinguishably from the
  degraded notice?

## Credential handling

`~/.wayland-secrets/flux.env` (mode 600, outside every repo). Loaded via `set -a; . file; set +a`
or stdin only. Never echoed, logged, committed, or written into any file. Sweep + hit count to be
reported before finish.
