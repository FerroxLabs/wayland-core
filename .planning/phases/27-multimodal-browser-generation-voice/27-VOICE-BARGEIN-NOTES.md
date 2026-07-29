# NOTES — lane `voice-bargein` (C4 gap closure)

Base `75d8a8f0` (`lane/voice-mac` HEAD). Branch `lane/voice-bargein`.
Notes-first per LANE-BRIEF §6b-i. Appended after every measurement.

## Measurement log

### M1 — `CpalAudioPlayer` reference census (re-verified, unproxied)

```
/usr/bin/grep -rn "AudioPlayer" --include='*.rs' crates | wc -l   -> 37   (liveness control)
/usr/bin/grep -rn "CpalAudioPlayer" --include='*.rs' crates       -> 9 lines
```

9 lines: doc(17), struct(558), impl(560), Default(585), impl AudioPlayer(592),
production wiring(729), a comment(880), tests(947, 958). **Confirms the prior lane:
no `.stop()` call site on the production player.** `stop()` at :628-632 is `{}`.

### M2 — NEW, and it widens the finding: `VoiceMode::play()` also has ZERO production call sites

```
/usr/bin/grep -rn "\.play(" --include='*.rs' crates
```
8 hits. One is `cpal::Stream::play()` (agent voice_mode.rs:379, unrelated trait).
The other 7: `voice_mode.rs:893,950` (agent tests), `wcore-tools:1187,1194,1195,1256`
(tools tests), and `wcore-tools:720` — which is `VoiceMode::play`'s own body.

```
/usr/bin/grep -rn "stop_playback" --include='*.rs' crates   -> 1 line: its own definition
```

So the playback seam is dead at BOTH ends: nothing plays, and nothing stops.
`CpalAudioPlayer` is the only audio-output impl in the tree
(`grep afplay|aplay|SoundPlayer|ffplay` → only voice_mode.rs + a mac live test).
TTS **writes files, it does not play them** (`tool_backends/tts.rs` has no
`Command::new`, no player).

**Consequence for my brief:** fixing `stop()` is necessary but NOT sufficient for
"the user speaking over the agent cuts the audio", because in this binary the agent
never speaks. I must not claim barge-in end-to-end on the strength of a working
`stop()`. Plan: fix the seam so it is genuinely interruptible AND prove the
interrupt reaches a real OS process; state the missing playback caller plainly.

### M3 — `check_requirements()` call sites (re-verified)

7 occurrences (agent doc:715, def:750, doc:918, tests:1314,1316,1330,1332).
**Zero production callers.** Confirms prior lane.

Candidate real home found: `engine_bridge.rs:2910` emits
`"Voice capture unavailable — run /doctor for details."` — the product already
promises the readiness report to the user, via a surface
(`tui/surfaces/diagnostics.rs`) that has no voice check. That is a promise/impl gap
and the natural gate.
Caveat measured from source: `check_requirements` performs a `recorder.start()` +
`cancel()` dry-run, so it MUST NOT be called at bootstrap (would grab the mic /
raise the macOS TCC prompt on every CLI start) and MUST NOT be wired into the
tool's `status` action (documented side-effect-free; a `cancel()` there would
discard a live user recording).

### M4 — `voice` feature gating (context)

`bootstrap.rs:1361` `#[cfg(feature = "voice")]` guards ONLY the registration line.
The 55 KB `VoiceMode` state machine in wcore-tools is ungated and ships today.

## Open / to establish

- [ ] make `play()` cancellable + `stop()` real; known-negative (restore `{}` body → red)
- [ ] decide `check_requirements` — gate it or delete it
- [ ] settle whether protocol needs voice event identity
- [ ] get the 11 voice tests into CI
