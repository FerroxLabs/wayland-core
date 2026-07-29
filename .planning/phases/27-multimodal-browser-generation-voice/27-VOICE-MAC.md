---
lane: voice-mac
criterion: 27-C4
grade-27-C4: NOT MET
capture-device-proven-live: >-
  YES. A known 1 kHz tone was injected and detected in the capture at ratio
  116.66 against a same-device, same-duration control arm at 1.15 — separation
  101.7x, thresholds pre-registered in source before either arm ran. The control
  arm carried genuine ambient audio at rms 49 (ten times the RMS-5 that a prior
  withdrawn claim rested on) and still scored no 1 kHz content, so the
  discriminator is spectral, not amplitude. Detector self-test proves the
  discrimination executably: a quiet REAL tone (rms 42) scores 5e8 while LOUD
  dither (rms 400) scores 0.2 — RMS ranks the dead signal ABOVE the live one,
  Goertzel ranks the live one 2.5 billion times higher.
ships-by-default: >-
  NO, and it must not be defaulted yet. Measured, not assumed: `cargo tree
  --target x86_64-unknown-linux-gnu` with `--features voice` pulls cpal ->
  alsa -> alsa-sys (control: 0 without the feature), and alsa-sys's build.rs is
  `pkg_config::probe_library("alsa")` -> `rustc-link-lib=asound`, i.e. a hard
  dynamic NEEDED libasound.so.2, NOT dlopen. On a Linux host without ALSA,
  ld.so fails before main(), so the tool's device self-hide never executes.
  Defaulting voice would turn a missing audio library into total CLI failure
  for every headless Linux user who will never own a microphone. Two blocking
  preconditions before reconsidering: implement barge-in, and put the 11
  currently-dead voice tests into CI.
new-finding: >-
  C4's `interruption` clause is structurally unimplemented in production and was
  gradeable from source at ZERO hardware cost. `CpalAudioPlayer::stop()` is an
  empty body whose comment says the omission was deliberate, and `play()` blocks
  on `Command::status()`, so there is no moment at which an interrupt could even
  be delivered. Both `stop_count` assertions in the tree run against the mock;
  of 9 `CpalAudioPlayer` references, none calls `.stop()`. Separately: the prior
  lane's "the mic loop is compiled OUT" is half wrong — the 55 KB VoiceMode state
  machine is UNGATED and already ships in every binary; only the cpal recorder
  and one registration line are gated.
fence-exposure: >-
  ZERO. `git diff <BASE> -- crates/wcore-cli/src/lib.rs crates/wcore-cli/src/main.rs`
  is 0 lines, with a liveness control of 591 lines on a path I did change, so the
  zero is a measurement and not a dead command. 3 files changed vs BASE, 1 of them
  .rs (a new test file I own), 0 untracked. No contract regeneration, no PR, no
  merge, no tag, no workflow edit, no runner change.
status: complete
---

# Phase 27 — lane `voice-mac` — criterion C4

Base `fab334935235ada806304d7223094dd5d6d18dfb`. Branch `lane/voice-mac`.

C4: *"Streaming voice supports interruption, cancellation, compatibility,
accounting, and ordered protocol events."*

Dispatched to correct a write-off: the prior lane (`27-browser-voice`,
`a9f818d0`) deferred C4 as hardware-blocked, on the grounds that mic capture
needs a real capture device and hetzner cannot host one at any price. **That was
right about hetzner and wrong about the conclusion.** Sean's Mac has a
microphone. It always did.

**The correction is larger than the dispatch supposed.** Hardware was not the
binding constraint at all. **Three of C4's five clauses — interruption,
compatibility, ordered protocol events — were gradeable from source with no
microphone whatsoever, and the most important of them is a four-line finding.**
The mic mattered for exactly one clause, cancellation, which is now MET.

---

## Grade per clause

| clause | grade | how established | mic needed? |
|---|---|---|---|
| **interruption** | **NOT MET** | production `stop()` is an empty body; `play()` blocks on a subprocess | **no** |
| **cancellation** | **MET** | live, on a stream first proven to be flowing | yes |
| **compatibility** | **NOT MET** | `check_requirements()` has zero production call sites | no |
| **accounting** | **NOT MET** (not voice-specific) | 0 usage/cost/token in both voice files — and in every sibling media tool | no |
| **ordered protocol events** | **NOT MET** | `wcore-protocol` has zero voice/audio event identity | no |

**C4 overall: NOT MET — 1 of 5.** And the grade is bounded by a fact that
outranks all five: **`voice` is in no default feature set, so every clause above
describes a binary we do not ship.**

---

## 1. Interruption — NOT MET. The principal finding, and it needed no hardware.

`crates/wcore-agent/src/tool_backends/voice_mode.rs:628-632`, quoted byte-exact:

```rust
async fn stop(&self) {
    // The OS shell player is a one-shot subprocess. We let it finish
    // naturally — there is no cross-platform "stop a SoundPlayer"
    // signal that's worth the complexity vs the rare interrupt need.
}
```

The barge-in seam is an empty body, and the comment states the omission was
deliberate. Worse, `play()` calls `std::process::Command::…status()`, which
**blocks until `afplay` exits**. Playback is synchronous *and* the stop is a
no-op: there is no moment at which an interrupt could be delivered, and if one
were, it would do nothing. A user cannot stop the machine talking.

**And the test suite reports this surface as working.** `stop_count` is asserted
twice — `wcore-tools/src/voice_mode.rs:1198` and `:1345` — and **both run against
`CapturingAudioPlayer`, the mock**, which increments a counter. I enumerated all
**9** `CpalAudioPlayer` references in the tree unpiped, so no line is hidden and
no exit status is stolen: production wiring at `:729`, tests at `:947` (missing
file) and `:958` (`os_shell_command`). **Not one calls `.stop()`.**

The shape is: trait declares `stop` → mock implements and counts it → tests
assert the count → production implements it as `{}` → **zero tests touch
production.** That is the advertised-but-dead pattern with ten recorded
instances on this programme. This is a candidate eleventh, and it is unusual in
being self-documenting.

**I did not fix it, and that was a decision rather than a deferral.** LANE-BRIEF
§5 grades by impact: the capability is unreachable in every shipped build, so
real-world impact today is **zero → MEDIUM → BACKLOG**, and §5 warns explicitly
against inventing a stricter rule. *Against myself:* it is a contained ~30-line
fix in a file I own, and declining leaves a known gap. *Why that does not move
me:* the gap is worth more as a **checkable precondition blocking the shipping
recommendation** than as a quiet patch. Fixing it silently would convert a
blocking gate into an invisible one.

---

## 2. Cancellation — MET, live, with the precondition proved

"It stopped" is free on a stream that never started, so the test **hard-fails**
if the buffer is empty before the cancel.

```
PRE-CANCEL : is_recording=true current_rms=283   <- stream PROVEN FLOWING first
POST-CANCEL: is_recording=false rms=0 stop=Empty
```

Cancel clears the ring, drops the recording state, and a subsequent `stop`
yields `Empty` — the audio is genuinely discarded, not merely detached.

---

## 3. Compatibility — NOT MET

`VoiceMode::check_requirements()` builds a `VoiceRequirements` readiness report
(capture available, STT available, environment warnings). All **7** occurrences
of `check_requirements` in the tree: 1 definition, 2 doc mentions, 2 test call
sites, 2 test function names. **Zero production call sites.**

The consequence is documented in the product's own words. `voice_mode.rs:714`
justifies returning `Some(VoiceMode)` in keyless environments so the user gets
*"the clearer 'STT provider: MISSING' message from `VoiceMode::check_requirements`
**rather than a silent hide**"*. All 4 occurrences of `STT provider` are the doc
comment, the two `details.push` lines, and one test assertion. **Nothing in the
product calls the function, so the user receives exactly the silent hide the
comment says they were spared.**

Second, weaker but worth recording: `audio_capture_available` is set by
`recorder.start()` returning `Ok`. That is **resolvability, not liveness** —
`start()` never looks at a sample. This is the identical defect class the
sibling lane graded MEDIUM for `browser_suite` / `computer_use`, now found in a
third capability. My own arms show why it matters: a control arm at rms 49 with
no signal in it would pass `start()` just as happily as a live one.

---

## 4. Accounting — NOT MET, and I am explicitly declining to call it voice-specific

`usage`, `cost`, `token`, `accounting`, `billed` all return **0** in both voice
files. Instrument alive: `usage` occurs 4/3/1/5/1 times in five sibling
`wcore-tools` files.

But the fair comparison changes the finding. Sibling media tools:

| file | usage | cost | token |
|---|---|---|---|
| `image_gen.rs` | 0 | 0 | 1 |
| `tts.rs` | 0 | 1 | 0 |
| `video_analyze.rs` | 0 | 0 | 0 |

**No media tool does accounting.** Cost machinery exists (`cost_usd` /
`total_cost` in `protocol_sink.rs`, `output/mod.rs`, `bootstrap.rs`,
`council/run.rs`) but sits at the **LLM-turn layer**, not per-tool. STT is billed
per audio-second by Groq/OpenAI and is genuinely unaccounted — but so is image
generation and so is TTS. **This is an architecture-level gap for the whole
phase, not a voice defect.** Grading it against voice alone would have been
convenient and wrong.

---

## 5. Ordered protocol events — NOT MET

Concept-level search of all 20 files in `crates/wcore-protocol/src`, searching
the **concept and not one keyword** (§3b-i.3), controls first:

```
controls: "pub enum" 62 | "tooluse|tool_" 182     (instrument alive)
voice 2   audio 0   microphone|mic 0   speech 0   transcri 1
capture 6  record 38   stt|whisper 0   tts 0   listen 0
```

**The 2 `voice` hits are the substring inside `"Re: invoice"`** (`events.rs:1777`
and `:1787`, an email test). True count: **zero.** I nearly took that 2 at face
value — the keyword trap firing in the direction that would have manufactured a
capability. The single `transcri` hit is a doc comment stating the protocol
carries *"never transcript text"*, so the absence is deliberate.

`VoiceModeTool` exposes 5 discrete actions — `toggle_record`, `start`, `stop`,
`cancel`, `status` — each a separate tool call, so **ordering is observable only
through the generic ToolUse/ToolResult ladder**, which I did not re-prove and am
not claiming credit for. What does not exist is any **streaming** surface: a host
cannot be pushed recording state, and must poll `status` to learn RMS. For a
criterion about *streaming* voice, that is the clause failing on its own terms.

---

## The capture-liveness control, and its discriminating threshold

This is the part the dispatch was most emphatic about, because a prior lane
published *"audio flowed from a real microphone"* on **RMS 5** — which also
matches a muted device with dither — and had to withdraw it.

**So I did not measure amplitude.** I injected a known 1 kHz tone and asked
whether the capture *contains that frequency*, via a Goertzel filter. Broadband
noise has no spectral peak at any amplitude, so a louder number cannot fake it.

**Thresholds, pre-registered in source before either arm ran** (`TONE_PRESENT_RATIO
= 20`, `TONE_ABSENT_RATIO = 3`): `tone_ratio` is Goertzel power at 1 kHz over the
mean of four **non-harmonic** off-band probes (617/1481/2273/2887 Hz — non-harmonic
so speaker distortion at 2 kHz/3 kHz cannot inflate the floor). For any broadband
source — room noise, mic self-noise, dither, or the all-zero buffers a TCC-denied
capture yields — the expected ratio is ≈ 1. Twenty is **13 dB** above that: far
outside broadband variation, far below what an audible tone produces. The band
between 3 and 20 is an explicit **INDETERMINATE** zone, deliberately not split.

**Live arms** — real `CpalAudioRecorder`, real default input device (HyperX
QuadCast 2, 48 kHz → resampled to 16 kHz), host output volume **left at its own
setting of 6/100, verified 6 before and 6 after; I changed nothing**:

```
run 1  ARM B-control-no-tone  : samples=48051  rms= 49.0  tone_ratio=   1.15
run 1  ARM A-live-tone-playing: samples=48051  rms=433.3  tone_ratio= 116.66   -> 101.7x
run 2  ARM B-control-no-tone  : samples=48051  rms= 45.2  tone_ratio=   2.02
run 2  ARM A-live-tone-playing: samples=48051  rms=555.1  tone_ratio=2546.71   -> 1261.1x
```

**Reproduced across two independent runs**, both clearing the pre-registered
thresholds on both arms with three orders of magnitude to spare. The run-to-run
spread in the tone arm (116 → 2546) is acoustic — speaker wake state and room
conditions — and is precisely why the criterion is a *threshold with an
INDETERMINATE band*, not a number to be compared between runs.

**Why this discriminates rather than merely being a bigger number:** the control
arm is *not* silence. It carries genuine ambient room audio at **rms 49 — ten
times the RMS-5 the withdrawn claim rested on** — and still scores 1.15, i.e. no
1 kHz content. The two arms differ **only spectrally**. A dead or TCC-denied path
would score rms 0 and ratio 0 in *both*. And 48051 samples ≈ 3 s × 16 kHz confirms
the stream was genuinely flowing, not merely open.

**The detector's own self-test carries the refutation executably:**

```
SELF-TEST(3): quiet-tone rms= 42 ratio=499949020.7 | loud-dither rms=400 ratio=0.2
SELF-TEST(4): pure-tone   ratio=5.000e8            | digital-silence   ratio=0
```

Assertion (3) is the point: **RMS ranks the DEAD dither (400) 9.5× ABOVE the LIVE
tone (42). Goertzel ranks the tone 2.5 billion× above the dither.** For this
purpose amplitude is not a weak discriminator — it is *inverted*.

---

## Two defects in my own instruments, both repaired in-lane

§6b-ii: a written-up instrument defect is a defect you have agreed to keep.

**Defect #1 — my Goertzel scored the purest possible POSITIVE as 0.0.** The first
`tone_ratio` returned `0.0` whenever the off-band floor was ≤ EPSILON, reasoning
"zero floor = all-zero buffer". A mathematically pure tone leaks exactly zero into
other exact Goertzel bins, so a perfect tone and a dead capture path scored
**identically at 0.0**. My own known-positive assertion caught it. Without that
assertion this file would have reported "no tone" on every arm and published a
confident, entirely fabricated negative — §3b-i inside the instrument built to
avoid §3b-i. Repaired to discriminate on total signal power; **self-test assertion
(4) is its regression guard**, requiring pure tone and digital silence never to
score within `TONE_PRESENT_RATIO` of each other again.

**Defect #2 — the overflow gate was flaky, so I repaired it rather than
publishing it.** Run 1 FAILED (tail ratio 0.41 → "capture degraded past 60 s");
an immediate re-run PASSED (11227.13, retained exactly 960000 samples = the
documented cap, stop in 45 ms). **A gate that reports a product defect on one run
and a clean pass on the next is not measuring the product** — publishing run 1
would have been a fabricated HIGH. Repairs: a speaker wake-up pre-roll (the host's
default output is a Bluetooth speaker that sleeps and eats the first seconds), and
scoring the tone over the **whole retained buffer** as well as the tail, which
separates the two causes the single assertion conflated — *whole LOW* = the tone
never reached the mic (acoustic path, INDETERMINATE, says nothing about the ring);
*whole HIGH + tail LOW* = a real degradation.

**Defect #3 — the cause-separation I added to fix #2 was itself miscalibrated,
and it fired.** The repaired gate checked `whole_buffer_ratio` **first and
unconditionally**. But the tone deliberately occupies only 4 s of a 60 s retained
window, so coherent-gain dilution drives that ratio down by roughly `(4/60)²`.
Measured: **`whole=1.67` while `tail=137.03`** — the gate announced *"the tone
never reached the microphone"* in the very same run where the tail proved, at a
6.8× margin over threshold, that it plainly had. `whole_ratio` is only meaningful
as a **tie-breaker once the tail has already come back low**; checking it first
inverts the diagnosis. Repaired to the correct order: *tail HIGH → pass*; *tail
LOW + whole LOW → acoustic failure, INDETERMINATE*; *tail LOW + whole HIGH →
real degradation.*

That is **three instrument defects in one lane, all in the harness built to avoid
this exact failure class, and all found by assertions I had written to catch
myself.** I record that as the main methodological result: the self-tests were not
ceremony — without the known-positive in #1 this lane would have published a
fabricated negative, and without re-running #2 it would have published a
fabricated HIGH.

Also fixed: my stat-before-search helper used `/usr/bin/test`, **which does not
exist on macOS** (rc=127), so it refused a live 1800-line file. Caught by its own
known-positive. That one failed *safe* — it refused good files rather than passing
bad ones — and I record the polarity honestly rather than dressing it up.

---

## The shipping question — NO, and here is the cost

**`voice` must not be added to `wcore-cli`'s defaults yet.**

I put this to the cross-audit panel. Both legs that returned rested on one
factual question — is ALSA a hard load-time link? — and gemini named its own
strongest counter-argument as *"if the ALSA linkage is actually deferred via
runtime `dlopen`, the headless Linux risk is zero."* **So I measured it instead
of accepting either answer.**

Resolved the Linux graph from the Mac without building, with a control:

```
cargo tree -p wcore-cli --features voice --target x86_64-unknown-linux-gnu
   -> cpal v0.15.3 -> alsa v0.9.1 -> alsa-sys v0.3.1
same query WITHOUT the feature -> 0 matches   (control: the query discriminates)
```

Then read `alsa-sys 0.3.1`'s `build.rs` from the registry cache: it is
`pkg_config::probe_library("alsa")`, which emits `cargo:rustc-link-lib=asound`.
**That is a hard dynamic `NEEDED libasound.so.2`, not `dlopen`.**

**The escape hatch does not exist.** On a Linux host without ALSA, `ld.so` fails
**before `main()`** — so the tool's careful device self-hide never runs, because
no Rust code runs at all. The Cargo comment's rationale is correct, and correct
for a reason it does not state.

**Cost of defaulting it, stated plainly:**
- **Linux:** every headless user who will never touch voice gets a binary that
  refuses to start without `libasound.so.2`. This is the whole cost, and it is
  disqualifying on its own.
- **macOS / Windows:** near zero. cpal links CoreAudio/WASAPI, both always
  present. Measured: `Compiling cpal|hound|coreaudio` → 4, builds clean, no ALSA.
- **Code size:** lower than the prior lane implied. The 55 KB `VoiceMode` state
  machine is **already in every shipped binary** (`wcore-tools/src/lib.rs:240`,
  ungated; `cpal` appears once in that crate's Cargo.toml and it is a *comment*).
  Defaulting adds a **link dependency**, not 94 KB of new code.

**Risk if defaulted as-is** — and this is the part that is not about ALSA: we
would ship a voice assistant **the user cannot interrupt**. Presence creates
expectation; a barge-in that silently does nothing is worse than an absent
feature. Both panel legs reached this independently.

**Preconditions, in order:** (1) implement `CpalAudioPlayer::stop()` and unblock
`play()`; (2) add a `--features voice` CI job so the **11 voice tests that
currently run in no configuration** actually execute; (3) then reconsider, and
prefer per-target dependency scoping (`[target.'cfg(...)'.dependencies]`) over a
global default — which is a real Cargo capability, contra one panel leg's claim
that per-target defaults are inexpressible.

### Panel

| leg | vote |
|---|---|
| gemini-3.1-pro-preview | `SHIP=NO` |
| kimi K3 | `SHIP=CONDITIONAL` |
| codex gpt-5.6-sol | see below |
| internal adversarial | argued the ALSA premise, and lost to measurement |

Votes extracted **unanchored** (kimi bullet-prefixes and indents, which would
lose an anchored `^SHIP=` match) and taking the **last** match (codex repeats its
final block). Both returning legs agree on substance and differ only on ceremony
— kimi's own self-dissent concedes *"CONDITIONAL is just NO with extra ceremony."*
I take **NO**, because the ALSA link is a load-time failure and not a
configuration knob.

**Internal adversarial pass**, arguing against the consensus: *the tool self-hides
when no device is found (verified on a headless host), so a default-on voice
feature is harmless on servers.* — **Refuted, by my own measurement.** The
self-hide is Rust code; the ALSA link is resolved by `ld.so` before any Rust runs.
The safety mechanism is real and lives one layer too high to help. That refutation
is the single most useful thing the panel produced, and it came from checking a
premise both legs had asserted rather than measured.

---

## What I refused to claim

- **That the `Vec::remove(0)` ring-buffer defect is real.** `RingBuffer::push`
  does an O(n) memmove over 960,000 `i16` per sample past the 60 s cap, inside
  the cpal callback — ~30 GB/s by arithmetic, which *looks* fatal. It is not, on
  this hardware. Across three 70 s runs the ring retained **exactly 960000
  samples** (the documented 60 s cap) every time, `stop()` took 45–57 ms, and the
  tone injected *deep inside the overflow regime* came back intact in the
  retained tail at ratio **137.03**. **The arithmetic was a good hypothesis and
  the measurement refutes it here.** I record it as a concern for weaker hardware,
  explicitly NOT as a defect — my one apparent failure was my own harness
  (defects #2 and #3), not the product.
- **That the capture path is dead when a tone is not detected.** The repaired gate
  now reports INDETERMINATE and names the acoustic path, because a silent room and
  a broken mic are the same measurement without a control.
- **That accounting is a voice defect.** The fairness check says it is the tool
  model.
- **That the generic tool-event ladder is correctly ordered.** It very likely is;
  I did not re-prove it and take no credit for it.
- **That `voice` cannot ship.** It can, on macOS and Windows, at near-zero cost.
  What it cannot do is ship as a *global* default while ALSA is a hard link.
- **Anything about Linux or Windows runtime behaviour.** I ran on Darwin only.

## Disclosures

**Darwin exception (LANE-BRIEF §0), used and disclosed.** Machine: Sean's Mac,
`sean-mac-arm64`. Method: `cargo test -p wcore-agent --features voice` — one
crate, `--lib` and a single `--test` target, never a workspace build, never
clippy, never release. This qualifies: mic capture is platform behaviour **no
permitted host can demonstrate**, which is precisely the gap the exception was
granted for. hetzner is headless with no capture device; that part of the prior
lane's cost statement was correct. **I did not reconfigure, relabel or stop the
`sean-mac-arm64` runner**, and I did not change the host's audio settings —
output volume was 6/100 before and 6/100 after.

**Counts.** Every number reported here came from `/usr/bin/grep`, `/usr/bin/git`,
`/usr/bin/wc`, `/usr/bin/sed` or the **test binary invoked directly** rather than
through `cargo` — because `rtk` rewrites `cargo` output and strips `0 ignored` /
`0 filtered out`, the exact two fields the anti-vacuity rule depends on. Executed
counts were read back, never inferred from exit status:
`11 passed; 0 failed; 0 ignored; 0 measured; 2169 filtered out` for the existing
suite, and the new file's own run for mine.

**Credential.** `FLUX_API_KEY` was dot-sourced into a shell variable **on the Mac
only**, solely to run the mandated sweep; **no leg of this lane needed a
provider**, so it was never used for a call, never echoed, never written to disk,
never committed, and never reached hetzner. Sweep with a liveness control:

```
liveness control — files containing a known-present string : 2   (instrument alive)
changed files vs BASE containing the live key value        : 0
full commit-history diff BASE..HEAD containing the value    : 0
```

**Restart.** The orchestrator process exited mid-flight (account switch). All
commits survived; one modified file was verified complete and compiling before
being committed. Nothing was lost, because notes-first meant every measurement
was already on disk.
