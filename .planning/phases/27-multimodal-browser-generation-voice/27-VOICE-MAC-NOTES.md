# 27-VOICE-MAC — working notes (append-only, committed continuously)

Lane `voice-mac`. Branch `lane/voice-mac`. Base `fab334935235ada806304d7223094dd5d6d18dfb`.
Worktree `/Users/seandonahoe/dev/waylandcore-frontier-worktrees/lane-voice-mac` (verified via
`/usr/bin/git rev-parse --show-toplevel`).

Criterion owned: **27-C4** — *"Streaming voice supports interruption, cancellation,
compatibility, accounting, and ordered protocol events."* Graded **NOT MET** by lane
`27-browser-voice` (`a9f818d0`) and deferred as hardware-blocked.

---

## Pre-registered priors and ranking (written BEFORE measuring, so it cannot be retrofitted)

**Rank 1 — the shipping question.** Whether `voice` should be in `wcore-cli`'s
default features. I rank this ABOVE the five C4 properties because if the answer
is "no, and it never will be", then every property I prove is about a binary we
do not ship, and the correct C4 grade is bounded by that regardless of how the
interruption test goes. The dispatch names ten recorded instances of
advertised-but-dead capability on this programme; this is a candidate eleventh.

**Rank 2 — capture-device liveness.** Not a property of the product, a property
of my instrument. A prior lane on this programme published *"audio flowed from a
real microphone"* on RMS 5, which also matches a muted device with dither, and
had to withdraw it. I will not report ANY of the five properties before I can
discriminate live-mic from dead-mic with a stated threshold. Ranked above the
properties themselves because a property proved on a dead instrument is worse
than an unproved property.

**Rank 3 — the five named properties**, in this order: interruption,
cancellation, ordered protocol events, accounting, compatibility. Interruption
first because it is the one the prior lane called hardware-blocked, i.e. the
specific claim I was dispatched to correct.

**Prior I hold going in (recorded so it can be scored):** I expect the
streaming loop to exist and compile under `--features voice`, and I expect at
least one of the five properties to be genuinely unimplemented rather than
merely unexercised. The prior lane established the code is 94 KB across two
files (38.6 KB backend + 55.3 KB tool); that is too much code for all five to be
missing and too much for all five to be present without a protocol seam. If I
find all five clean, I should distrust my own test before believing it.

---

## Established at minute 0 (from the prior lane, re-verified here, not taken on trust)

| fact | source | my re-verification |
|---|---|---|
| `voice_mode` registration is `#[cfg(feature = "voice")]` | `bootstrap.rs:1361` | `/usr/bin/grep -rn 'feature = "voice"' crates/wcore-agent/src/` → 2 hits: `tool_backends/mod.rs:83`, `bootstrap.rs:1361` |
| `voice` not in `wcore-cli` default | `wcore-cli/Cargo.toml` | read directly: `default = ["remote-registry", "workflow", "monitor", "review_artifact"]`; `voice = ["wcore-agent/voice"]` declared separately |
| `voice` pulls cpal + hound | `wcore-agent/Cargo.toml:234` | `voice = ["dep:cpal", "dep:hound"]`, both `optional = true` |
| stated reason for OFF-by-default | Cargo comment, Issue #14 | *"A TUI must not hard-require ALSA at runtime"* — i.e. the reason is **Linux-specific** (`libasound.so.2`). Flagged: that reason does not obviously transfer to Darwin, which uses CoreAudio. **This is my first lead on the shipping question.** |
| `tts` + `transcribe_audio` are NOT feature-gated | `bootstrap.rs:1348`, `:1337` | to re-verify |

The prior lane's correction stands and I adopt it: **"voice is absent" is FALSE.**
TTS-out and STT-on-a-file ship. The streaming mic loop does not.

---

## The correction I was dispatched to make

Prior lane deferred C4 with cost *"hetzner-dsm is headless with no capture device
and cannot host it at any price"*. That is true of hetzner and **does not imply
C4 is unreachable**, because `sean-mac-arm64` is a registered self-hosted runner
with a microphone, and LANE-BRIEF §0's Darwin exception permits single-crate
single-test runs on the Mac for platform behaviour Darwin alone can demonstrate.
Mic capture is exactly that. I must disclose machine and method in the report.

**I note the asymmetry honestly:** the prior lane was not wrong to defer given
what it could see; it named the cost precisely enough that this lane could be
dispatched. That is the deferral working as intended, not a failure.

---

## Anti-self-passing commitments (§3.2, §3b-i) — pre-registered

1. **No absence claim without a known-positive in the same invocation.** Today a
   lane on this programme found a proof where `grep -c` on a MISSING FILE
   returned `0` and `0` was the success value. Before every absence I assert:
   `test -s <file>` first, then search, and show a non-zero count for something
   I know is there.
2. **Every count from `/usr/bin/grep`, `/usr/bin/git`, `/usr/bin/wc`.** `rtk`
   rewrites all three plus `cargo`, and strips `0 ignored` / `0 filtered out` —
   the exact fields needed to catch a suite that runs zero tests.
3. **Assert the executed test count** (`N passed`), never exit status. Three
   measured flavours of zero-test-green: all-`#[ignore]`, env-gated early
   return, filter matching no test name.
4. **Capture-liveness control before any audio claim** — see Rank 2. Threshold
   to be stated and justified, not chosen after seeing the numbers. I will
   pre-register the threshold before running the live arm.
5. **Interruption requires proof the stream was FLOWING first.** "It stopped" is
   free on a stream that never started.

---

## Fences (LANE-BRIEF §6)

`BASE=fab334935235ada806304d7223094dd5d6d18dfb`, captured once, quoted always.
Shared fence: `crates/wcore-cli/src/{lib,main}.rs` — additive contiguous only,
report line delta. Reserved: no merge, no PR, no tag, no release, no issue close,
no `wcore-contract generate`, no `.github/workflows/*`, and **do not reconfigure,
relabel or stop `sean-mac-arm64`** (cost two attempts to register).

---

## Log

- **T+0** — worktree verified, LANE-BRIEF read in full, prior lane report
  (`a9f818d0`, 27-BROWSER-VOICE.md) read in full. Priors and ranking pre-registered
  above. Nothing measured yet beyond the table above.

- **T+1 — M1. The prior lane's diagnosis is HALF WRONG, and the half that is wrong
  is the half that matters.** Prior lane: *"the streaming mic-capture loop is compiled
  OUT of the shipped artifact."* Measured:

  | component | file | size | gated? |
  |---|---|---|---|
  | VoiceMode state machine + `VoiceModeTool` | `wcore-tools/src/voice_mode.rs` | 55.3 KB | **NO — `pub mod voice_mode;` at `lib.rs:240`, ungated** |
  | cpal recorder / player (`CpalAudioRecorder`) | `wcore-agent/src/tool_backends/voice_mode.rs` | 38.6 KB | YES — `#[cfg(feature="voice")]` `mod.rs:83` |
  | registration | `bootstrap.rs:1361` | — | YES |

  `cpal` appears **1** time in `wcore-tools/Cargo.toml` and it is a **comment**;
  `grep -E '^\s*(cpal|hound)\s*='` → rc=1. Instrument alive (the count of 1 proves
  the file was read). So **the entire state machine that C4's five properties are
  about — start/stop/cancel/toggle, the RMS surface, the hallucination filter — is
  ALREADY COMPILED INTO THE DEFAULT SHIPPED BINARY.** What is compiled out is only
  the concrete mic device and the registration line.

  Consequence for the shipping question: turning `voice` on by default costs the
  **cpal+hound link only**, not 55 KB of new code. That materially lowers the price
  the prior lane implied.

- **T+2 — M2. `wcore-protocol` has ZERO voice/audio event identity.** Concept
  search (§3b-i.3 — concept, not keyword), whole crate, 20 files:

  ```
  liveness controls: "pub enum" 62   |  "tooluse|tool_" 182   (instrument alive)
  voice 2   audio 0   microphone|mic 0   speech 0   transcri 1
  capture 6  record 38  stt|whisper 0   tts 0   listen 0
  ```
  **The 2 `voice` hits are the substring in `"Re: invoice"`** (`events.rs:1777,1787`,
  an email test). True voice count = **0**. The single `transcri` hit is a doc
  comment stating the protocol carries *"never transcript text"* — i.e. the absence
  is deliberate, not an oversight.

  **This is the keyword trap firing in my favour and I nearly took it:** a naive
  `grep -c voice` returns **2**, which reads as "voice events exist". They do not.

- **T+3 — M3. Tool surface (this is what "ordered protocol events" can even mean).**
  `VoiceModeTool` exposes 5 discrete actions — `toggle_record`, `start`, `stop`,
  `cancel`, `status` (`voice_mode.rs:962`), each a separate LLM tool call. So
  ordering is observable **only** through the generic ToolUse/ToolResult ladder;
  there is no voice-specific event. `is_concurrency_safe() == false` — deliberately
  serialised, comment: *"The recorder owns a single mic device — overlapping starts
  would race on the audio handle."*

- **T+4 — M4, and it is the sharpest thing in the lane so far.** `stop` collapses
  three distinct states into one:
  ```rust
  Ok(RecordingOutcome::Empty) => ... "note": "recording was empty (too short / silent / cancelled)"
  ```
  **The product itself cannot distinguish silence from cancellation from a dead
  capture device.** That is precisely the discrimination failure that made a prior
  lane on this programme withdraw its RMS-5 claim — except here it is baked into
  the shipped API, not into a lane's harness. Candidate finding; severity to be
  argued, not asserted.

- **T+5 — build.** `cargo test -p wcore-agent --features voice --lib --no-run` on
  the Mac (Darwin exception, disclosed). `Compiling cpal|hound|coreaudio` → **4**
  matches, so cpal links on Darwin via CoreAudio with no ALSA involved. Note the
  Cargo comment justifying OFF-by-default says *"must not hard-require ALSA"* —
  **an argument that does not apply to Darwin at all.**

- **T+6 — LOW doc defect.** `voice_mode.rs:75` says *"60s at 16 kHz mono i16 =
  ~1.92M samples ≈ 3.8 MB"*. `SAMPLE_RATE = 16_000` (`wcore-tools/src/voice_mode.rs:79`)
  → 16_000 × 60 = **960,000 samples = 1.92 MB**. The comment states *samples* where
  it means *bytes*; out by 2×. Cosmetic, recorded not fixed (not my fence).

- **T+7 — candidate defect to measure, NOT yet claimed.** `RingBuffer::push` does
  `self.samples.remove(0)` when at capacity — an O(n) memmove over 960,000 i16 on
  **every sample** past 60 s, executed **inside the cpal input callback** (the
  closure passed to `build_input_stream` locks `state_for_data` and pushes). At 16 kHz
  that is ~1.9 MB moved 16,000×/s ≈ 30 GB/s. If real, the audio callback cannot keep
  up and capture degrades past 60 s. **I have not measured this yet and will not
  claim it until I have.** It is also only reachable on a >60 s recording.
