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

## Plan

1. Vision trait impls (anthropic / openai / gemini) — covers video_analyze too.
2. TTS.
3. Per shape: wire a real record, or cut it from v1. No guessed prices.
4. Both directions on every guard (§3b-iii): non-zero cost recorded for a
   billed call, AND the guard reddens when the cost is dropped.
