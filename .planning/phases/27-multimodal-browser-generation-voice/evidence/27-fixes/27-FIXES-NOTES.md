# 27-FIXES — running notes (committed early per LANE-BRIEF §6b-i)

Lane `lane/27-fixes`, branched from `plan/f20-unified-audit-repair` @ `54203b25`.
Worktree `/Users/seandonahoe/dev/waylandcore-frontier-worktrees/lane-27-fixes` (verified
`git rev-parse --show-toplevel`, NOT the forbidden `/Users/seandonahoe/dev/waylandcore`).

**No credential value appears in this file or in any artefact this lane writes.**

## Mandate (three defects, all measured by `lane/27-credentialled` in the prior hour)

1. **Transcription resolver cannot reach a working provider.** `build_transcription_backend`
   (`crates/wcore-agent/src/tool_backends/mod.rs:344`) accepts `GROQ_API_KEY` or
   `OPENAI_API_KEY` and nothing else. Flux serves `/v1/audio/transcriptions` verbatim in the
   exact `verbose_json` shape `openai_compat_whisper.rs:61` already sends. Add the arm
   (~8 lines), then **re-measure C4's transcription clause through the product**, not curl.
2. **HIGH — `wayland-core image` with no `--model` returns 401 on a working key.**
   `BL-F27-FLUX-IMAGE-DEFAULT-ARM-401`. Must first establish WHICH defect it is:
   (a) the default model id is genuinely unentitled → upstream 401 truthful, our defect is
   that we surface it as "your key is bad"; or (b) something else. The two fixes differ.
3. **No cost record for any media call.** C3 accounting FAILS in all four shapes. Note:
   `session_cost` is **honest** — it reports `priced: false` and "cost is unpriced, not $0".
   The prior lane explicitly recorded that as NOT a defect. Job is to make media calls
   priced, not to fix a false zero. If materially more than a small change, report shape+cost
   rather than half-build.

## Starting position (inherited, from 27-CREDENTIALLED-SUMMARY.md)

- Flux routes that serve: `/v1/chat/completions`, `/v1/images/generations`,
  `/v1/audio/transcriptions`. Do NOT serve: `/v1/audio/speech` (500), `/v1/audio/translations` (404).
- Transcription cost arrives **only in HTTP response headers** (`x-flux-cost-usd`,
  `x-flux-billed-seconds`), never in the JSON body. `openai_compat_whisper.rs:85-86` takes
  `resp.status()` then `resp.text()` — **headers are never read**. `TranscriptionOutcome::Ok`
  has no cost field.
- Image generation: the API reports **no cost at all** — no `usage`, no `x-flux-*` header.
  So image pricing may be structurally unavailable from this provider.
- Unit prices: text ≈ $0.000126; transcription $0.016670 at a **10-second floor**; image ≈ $0.08.
- Prior lane spent ~US$0.82. My budget: keep well under $1, report actual.
- Headless measurement hosts need `WAYLAND_VAULT_PASSPHRASE` (or `[session] enabled = false`)
  or every turn dies with "Session persistence authority unavailable" while discovery still
  looks healthy.

## Known determination lever for defect 2 (cheap)

The prior lane established that requesting a disallowed model on `/v1/audio/speech` returns a
**named entitlement error listing the key's allowed models**. If that same listing behaviour
holds on `/v1/images/generations`, I can determine entitlement of `flux-image-together-flux`
for ~$0.0001 without paying for an image. That is the first thing to try.

## Traps I must not repeat (from the brief)

- Pipe steals exit status; `echo "EXIT=${PIPESTATUS[0]}"` after a pipeline returns empty here.
- Byte-count every capture.
- Run test targets **by file, never by filter** (a filter matching no test exits 0 having run 0).
- Assert the `N passed` count; never trust exit status.
- `wcore-agent --lib` fails 13-19 in parallel, passes 2145/0 serially — **run serially**.
- Two messages on one stdin silently drops the second turn — drive turns one at a time.
- Any defect I find in my OWN harness gets repaired in THIS lane, with a three-assertion
  self-test whose third assertion proves the old form would have missed it (§6b-ii).

## Log

- [t0] Worktree created, toplevel verified, NOTES committed. Nothing measured yet.
