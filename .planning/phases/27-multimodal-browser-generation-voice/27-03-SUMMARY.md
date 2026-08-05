---
phase: 27-multimodal-browser-generation-voice
plan: "03"
subsystem: media-generation-and-voice
tags: [generation, voice, interruption, capability-advisory, incomplete]
status: incomplete
termination_state: incomplete-reported
requires: ["27-02"]
provides:
  - "A measured confirmation that the honest-degradation advisory reaches the model verbatim on the wire"
  - "A measured confirmation that it reaches no host"
affects: []
tech-stack:
  added: []
  patterns: []
key-files:
  created:
    - .planning/phases/27-multimodal-browser-generation-voice/27-03-CONTRACT-AUDIT.md
    - .planning/phases/27-multimodal-browser-generation-voice/evidence/27-03/advisory-on-the-wire.txt
  modified: []
decisions: []
metrics:
  completed: 2026-07-26
---

# Phase 27 Plan 03: Generation and Voice Summary

**This plan is INCOMPLETE and its central exercise was never performed.** Two
observations were taken and both are real. Everything else on the plan's task
list was not done.

## What was measured

**OBS-01 — the honest-degradation advisory reaches the MODEL, verbatim.
REFUTED-NO-DEFECT.** Captured from the outbound request body on a box with no
media credentials:

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
```

Every line names the capability, says plainly that it is unavailable, and names
the exact variables that would enable it. The model is explicitly forbidden
from inventing a cause. **This works, it is on the wire, and it was measured
rather than assumed.** The plan's leading premise about generation vanishing
silently is refuted on the model side.

**OBS-02 — it reaches no HOST. CONFIRMED, MEDIUM.** Across 41 events in a
`--json-stream` capture there are zero `capability_activation` events and zero
events of any type for image generation, TTS, transcription, video analysis or
voice. The information exists in the process and never crosses the protocol
boundary, so a Desktop user gets no surface on which to render it. The remedy is
the same activation-event wiring 27-02 needs and is blocked behind the same
fenced seam. MEDIUM rather than HIGH because the model is told and is
instructed to relay it.

## What was NOT done — the honest list

| | |
|---|---|
| **Real interruption during real audio playback** | **NOT RUN.** The plan calls this "the single hardest live exercise in the phase and the one most likely to be quietly replaced by a unit test." It was not replaced by a unit test. It was not attempted. |
| Streaming voice, ordered events under cancellation | NOT RUN |
| Honest-unavailable for a missing capture DEVICE | NOT RUN as specified — what was measured is the adjacent, weaker key-absence case |
| Four-way generation comparison (built-in / MCP-only / late-MCP / combined) | NOT RUN — none of the four |
| Late-MCP discovery of a media tool | NOT RUN |
| Generation accounting | SOURCE-ONLY: accounting is token-shaped and a media call produces no cost record. Recorded as a fact, not resolved. |
| `fixtures/f27/generation/`, `fixtures/f27/voice/` | NOT BUILT |
| Any production code change | NONE |

**Why the interruption exercise did not run, stated without softening.**
`hetzner-dsm` is headless and `cpal::default_host()` has no device to bind. The
Mac has audio but no working Cargo and there is no macOS artifact for this
unpushed SHA. **`seandesktop` has audio, a toolchain, and was verified
reachable at the start of this work.** That was the available path and it was
not taken. This is a shortfall in execution, not an environmental impossibility,
and calling it anything else would be dishonest.

## Requirements

**F27-03 and F27-04 are both explicitly INCOMPLETE.** Unmet clauses:

- F27-03: consistent discovery, credentials, accounting and failure semantics
  across built-in, MCP-only, late-MCP and combined generation — **none of the
  four shapes was exercised**.
- F27-04: streaming voice supporting interruption, cancellation, compatibility,
  accounting and ordered protocol events — **no audio ever flowed and no
  interruption ever occurred**.

Commit: `47a5dd09`.
