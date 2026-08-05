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

## T+40 — route SETTLED from source. The design isolates the variable.

**Adapter: telegram.** Reasons, each read at source:
- `TelegramConfig::api_base_url` exists (added by F24-C3-H4) — `config.rs:48`. Both the bot-method
  base AND the file-download base derive from it (`api.rs:658 file_download_url`), so ONE fixture
  serves `getUpdates`, `getFile` and the media bytes.
- `api::download_bytes` (`api.rs:898`) has **no host allowlist**. Discord's does
  (`rest.rs:337 MEDIA_HOSTS = cdn.discordapp.com, media.discordapp.net`, enforced at `rest.rs:349`),
  so discord physically cannot fetch from a localhost fixture. That is why the predecessor could only
  reach the degraded direction on discord.
- `msg.voice` → `MediaKind::Audio` (`longpoll.rs:149-159`), and `resolve_attachments`
  (`longpoll.rs:217`) sets `Attachment.path = file_id`, resolved lazily by
  `TelegramChannel::fetch_media` (`lib.rs:379-396`).

**Which transcription arm fires — this is the crux.** `openai_wire_media_base`
(`tool_backends/shared.rs:56-77`) returns `Some` **only** for `ProviderType::OpenAI` and
`ProviderType::FluxRouter`; `_ => return None`. So:
- If the chat provider is declared `openai` (as the predecessor's discord harness did), **arm 3
  captures transcription and sends it to the LOCAL LLM fixture** — which is exactly the
  `transcription: using whisper-1 at http://127.0.0.1:36197/...` line in the predecessor's log.
- If the chat provider is a **Tier-2 OpenAI-compatible** type (`together`, `config.rs:2415`), arm 3
  returns `None`, chat still speaks OpenAI wire to the local fixture, and **arm 4 (`FLUX_API_KEY`)
  resolves transcription to the real FluxRouter** at `https://api.fluxrouter.ai/v1` +
  `audio/transcriptions`, model `flux-voice-fast`.

That is the design: **chat → local fixture (turn prompt captured), transcription → real Flux.**
One variable, and the credential is the only thing separating leg A from leg B.

Arms 1 and 2 (`GROQ_API_KEY`, `OPENAI_API_KEY`) must be absent from the gateway env — they are,
and their absence is what makes the negative control total.

**Audio, known ground truth** (macOS `say -v Samantha` → `afconvert -f WAVE -d LEI16@16000 -c 1`):
- `a1.wav` 133028 B — "The quantum ferret audited nineteen crimson bicycles on Thursday morning."
- `a2.wav` 136820 B — "Seventeen velvet lighthouses inspected the marmalade orchestra last winter."
Header verified `RIFF....WAVE` → `detect_audio_mime` returns `audio/wav`
(`transcription_tools.rs:109`), which is in `SUPPORTED_AUDIO_MIMES`. Both are far above
`TRANSCRIPTION_MIN_BYTES` (16) and far below `TRANSCRIPTION_MAX_BYTES` (25 MiB).

**Legs** (each differs from A by exactly one variable):

| leg | audio | `FLUX_API_KEY` | purpose |
|---|---|---|---|
| A | a1 | present | POSITIVE — real transcript reaches the model |
| B | a1 | **absent** | NEGATIVE CONTROL — must redden to the degraded notice |
| C | a2 | present | ANTI-ECHO — different audio must give different transcript |

Leg C is the control the predecessor's shape could not have: a backend returning a canned string
would pass a naive positive gate. C requires the derived text to TRACK THE AUDIO.

## T+70 — instrument self-test MUTATION-PROVED, and Flux STT probed live

`node scripts/f24-media-live.mjs --selftest` → 6/6 PASS, on Mac and on hetzner.
Three assertions per §6b-ii, and each **mutation-proved to redden**:

| mutation | assertion that FAILED | rc |
|---|---|---|
| needle → `'zzz-never-appears-zzz'` | known-positive | 1 |
| needle → `''` | known-negative | 1 |
| broken-matcher → `'no transcription backend'` (a form that DOES match) | **THIRD** | 1 |

The third is the one that matters: it proves the "old broken matcher misses it" assertion actually
discriminates, rather than passing on any input.

**Flux STT probed directly from hetzner before spending any gateway run.** Credential delivered on
**curl's stdin** via `curl --config -` — never in `argv`, never written to hetzner's disk.

| audio | HTTP | time | transcript returned |
|---|---|---|---|
| a1.wav | **200** | 2.12s | `" The Quantum Ferret audited 19 Crimson Bicycles on Thursday morning."` |
| a2.wav | **200** | 1.45s | `" 17 Velvet Lighthouses inspected the Marmalade Orchestra last winter."` |

So the credential is live, `flux-voice-fast` serves `/v1/audio/transcriptions` in the
`verbose_json` shape the backend parses, and hetzner has egress to it.

**Note for the scorer:** the engine renders numbers as DIGITS — "nineteen"→"19", "seventeen"→"17".
So a1 recovers 7/8 of its content words and a2 recovers 6/7; both clear `minHits=5` with margin.
**Cross-hits are zero in both directions on the REAL transcripts**, not merely on my synthetic
sentences — which is what makes the anti-echo gate meaningful.

## Credential handling

`~/.wayland-secrets/flux.env` (mode 600, outside every repo). Loaded via `set -a; . file; set +a`
or stdin only. Never echoed, logged, committed, or written into any file. Sweep + hit count to be
reported before finish.
