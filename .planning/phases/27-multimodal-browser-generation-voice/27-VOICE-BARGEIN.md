---
lane: voice-bargein
criterion: 27-C4
grade-27-C4: NOT MET (3 of 5, up from 1 of 5)
barge-in: >-
  IMPLEMENTED and proven against the REAL CpalAudioPlayer, not the mock.
  play() now spawns the OS player and keeps a handle; stop() kills it and
  waits until it is reaped. Known-negative executed: restoring the empty
  stop() body reds the new test (`0 passed; 1 failed`, 6.01s — the 5 s
  program ran to completion); the fix greens it. Wired at
  VoiceMode::start_capture, the single chokepoint every capture path goes
  through, so a user starting capture cuts the audio.
compatibility: >-
  check_requirements() now has a production call site that GATES. bootstrap
  computes the report once and hands it to VoiceModeTool::with_requirements,
  which refuses start / toggle_record when the seams cannot complete a
  capture -> transcribe cycle, naming the missing piece. Its destructive
  start()/cancel() recorder probe is replaced by AudioRecorder::is_wired() —
  same claim strength (resolvability, not liveness), no microphone opened at
  startup, and no longer able to discard a live recording.
protocol-events: >-
  Settled with evidence and DEFERRED, 3/3 panel unanimous. The protocol crate
  states its own gap: "legacy ordinary turn and tool events still have no
  producer event ID or monotonic sequence" (contract/generate.rs:41-45).
  ToolRequest/ToolRunning/ToolResult carry msg_id + call_id — correlation, not
  order. New voice variants would inherit that absence exactly, so they cannot
  make the `ordered` clause pass; they would only add typing, at the cost of an
  unregenerable contract drift and an eleventh advertised-but-dead surface.
  Filed as .planning/SEAM-REQUESTS/VOICE-STREAMING-STATE.md. NO CONTRACT DRIFT
  WAS INTRODUCED.
ci: >-
  The 11 dead voice tests now run. `grep -rn voice .github justfile` returned
  ZERO before this lane; a step on the containerized Linux job now runs them
  and ASSERTS THE EXECUTED COUNT against a floor, because a cargo filter that
  matches nothing exits 0 having run zero tests.
new-finding: >-
  The playback seam is dead at BOTH ends, not just at stop(). VoiceMode::play()
  has zero production call sites too, and CpalAudioPlayer is the only local
  audio-output implementation in the tree (TTS writes files; SpotifyPlaybackTool
  is a remote Web API). So barge-in is now correct and unreachable: in the
  shipped binary the agent never speaks, so there is nothing to interrupt. I am
  NOT claiming a user can today cut the agent off mid-sentence.
fence-exposure: >-
  ZERO. `git diff <BASE> -- crates/wcore-cli/src/lib.rs crates/wcore-cli/src/main.rs`
  is 0 lines, against a 328-line liveness control on a path I did change.
  6 files changed vs BASE, 0 untracked. No contract regeneration, no PR, no
  merge, no tag, no runner change.
status: complete
---

# Phase 27 — lane `voice-bargein` — closing the C4 gap

Base `75d8a8f0` (`lane/voice-mac` HEAD). Branch `lane/voice-bargein`.

C4: *"Streaming voice supports interruption, cancellation, compatibility,
accounting, and ordered protocol events."*

The prior lane graded C4 **NOT MET, 1 of 5** and did the grading work
honestly. I verified its findings before building on them — all three
re-measured claims held — and closed two clauses, settled a third with
evidence, and put the dead tests into CI.

---

## Grade per clause

| clause | prior | now | how |
|---|---|---|---|
| **interruption** | NOT MET | **MET (seam), blocked end-to-end** | real `stop()` kills the player; wired to `start_capture`; known-negative shown red. **But nothing calls `play()` in production** — see §1.4 |
| **cancellation** | MET | MET | unchanged; prior lane's live capture proof stands |
| **compatibility** | NOT MET | **MET** | `check_requirements()` gates `start`/`toggle_record` from a production call site |
| **accounting** | NOT MET | **NOT MET** | untouched. The prior lane established it is an architecture-level gap for every media tool, not a voice defect. I did not re-open it and take no credit |
| **ordered protocol events** | NOT MET | **NOT MET, now diagnosed** | the protocol has no monotonic sequence on ANY event, by its own contract doc. Voice variants cannot fix that |

**C4 overall: NOT MET — 3 of 5.** It is not 5 of 5 and I will not dress it as
one: accounting is untouched, and ordering is a protocol-wide deficiency a
voice lane cannot close.

---

## 1. Interruption — the headline defect, fixed

### 1.1 What was there

`CpalAudioPlayer::stop()` was `{}` with a comment saying the omission was
deliberate, and `play()` blocked on `Command::status()` — so there was no
moment at which an interrupt could be delivered, and nothing to deliver it to.
Both `stop_count` assertions in the tree ran against `CapturingAudioPlayer`.
I re-ran the census unproxied with a liveness control:

```
/usr/bin/grep -rn "AudioPlayer"     --include='*.rs' crates | wc -l  -> 37   (control)
/usr/bin/grep -rn "CpalAudioPlayer" --include='*.rs' crates          -> 9 lines
```

9 lines: doc, struct, impl, Default, impl AudioPlayer, one production wiring
line, one comment, two tests. **No `.stop()` call site.** Prior lane confirmed.

### 1.2 What it is now

* `play()` spawns the OS player via `tokio::process` and publishes a handle
  *before* awaiting, so a `stop()` racing it finds a handle rather than an
  empty slot.
* `stop()` fires the interrupt, then **waits until the child has been reaped**
  (bounded by `PLAYBACK_STOP_TIMEOUT = 2 s`), so a caller returning from
  `stop()` can rely on the audio having actually ceased rather than on a signal
  having been sent. A wedged player warns instead of hanging the interrupt path.
* `kill_on_drop(true)`, so cancelling the `play()` future itself also cuts the
  audio — otherwise the child outlives every handle we hold.
* Superseding the handle (a second `play()`) drops the old sender, which
  resolves the old task's interrupt arm: playback never outlives its own slot.

### 1.3 The known-negative, executed

Assertion order matters here, so I state which one fired. On hetzner, at the
same commit, with the pre-lane empty `stop()` body restored in place:

```
running 1 test
test tool_backends::voice_mode::tests::stop_cuts_in_flight_playback_and_kills_the_player_process ... FAILED
panicked at crates/wcore-agent/src/tool_backends/voice_mode.rs:1095:9:
an interrupted playback must not report success — the audio did not finish
test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 2187 filtered out; finished in 6.01s
```

**6.01 s** — the 5-second program ran to completion, exactly as a no-op stop
predicts. With the fix restored, the same binary is green and the whole
14-test filter finishes in 7.31 s. The negative was applied and reverted by
file copy, never by a git operation (LANE-BRIEF §0), and the tree was verified
byte-clean afterwards (`git status --porcelain` empty).

The test carries three assertions, and a control:

1. **control / known-positive** — left alone, the same command runs to
   completion, reports success, and writes its marker file. Without this arm,
   "marker absent" below would pass just as happily on a command that never ran
   at all (§3b-i: a broken instrument confirms a negative for free).
2. interrupted playback must not report success;
3. a 5 s player must be cut in under 3 s;
4. **dead, not detached** — wait past the program's natural end and confirm it
   never reached its post-sleep side effect. The control proves that side
   effect is reachable.

The command under test is a stand-in, not `afplay`/`aplay`, because those are
not installable on every host this suite runs on. **The code path is the
production one — only the program differs** — and it is built through
`wcore_config::shell::shell_command_builder`, never `Command::new("sh")`
(AGENTS.md §Forbidden). The Darwin arm below covers the real macOS player.

### 1.4 What I refuse to claim: the user still cannot interrupt the agent

**`VoiceMode::play()` has zero production call sites, exactly as `stop()` did.**
I searched the concept, not one keyword, with a known-positive control:

```
control: "transcribe" across crates                  -> 138  (instrument alive)
play_audio 1 | playback 86 | speak 78 | AudioPlayer 48 | audio_out 0 | PlaySync 2
every non-test line that could START playback:
  wcore-agent/.../voice_mode.rs:379   cpal Stream::play()  (a different trait)
  wcore-tools/.../voice_mode.rs:745   VoiceMode::play's own body
  wcore-tools/.../voice_mode.rs:749   stop_playback's own definition
```

I chased the two plausible second paths and ruled both out by reading them:
`SpotifyPlaybackTool` drives a remote device over the Web API (no
`Command::new`, no `afplay`), and `wcore-fixture-harness`'s "playback" is
fixture replay. **`CpalAudioPlayer` is the only local audio output in the
tree, and the TTS tool writes files without playing them.**

So barge-in is now correct and, today, unreachable: **in the shipped binary the
agent never speaks, so there is nothing to interrupt.** The defect named in my
dispatch is fixed and proven; the user-visible capability needs a separate
piece — a caller that hands TTS output to `VoiceMode::play` — which lives
outside C4's interruption clause and outside this lane's files. Naming that is
more useful than a grade of MET that a user could not reproduce.

### 1.5 The interrupt is wired, not left as a seam

`VoiceMode::start_capture()` now stops playback before starting the recorder.
The user starting capture *is* the barge-in signal, and `start_capture` is the
single chokepoint every capture path goes through — the tool's `start` and
`toggle_record` actions, and the TUI Ctrl+Space binding, which dispatches
`{"action":"toggle_record"}` through that same tool. Two tests assert the wiring
at the seam and at the *actions a host actually dispatches*, because a seam
nothing routes through is the pattern this lane exists to fix.

---

## 2. Compatibility — `check_requirements()` now gates something

7 occurrences before: 1 definition, 2 doc mentions, 4 test lines. **Zero
production callers**, while `build_voice_mode_backend`'s own doc promised the
user would get *"the clearer 'STT provider: MISSING' message ... rather than a
silent hide"*. They got the silent hide.

I considered deleting it, per my brief's second option, and rejected that: the
readiness report is the only thing that can answer "why is voice not working",
and the resolver's doc depends on it. So it is now consumed:

* `bootstrap.rs` computes the report **once**, at wiring time, and passes it to
  `VoiceModeTool::with_requirements`;
* the tool refuses `start` / `toggle_record` when the seams cannot complete a
  capture → transcribe cycle, returning the report's own details, so the user
  sees `STT provider: MISSING (no TranscriptionBackend wired)` at the moment it
  matters instead of recording audio that can never be transcribed;
* `stop` / `cancel` / `status` stay ungated so a session that somehow started
  can always be wound down.

**Two things I would not accept, and what I did instead.** The report is
computed once at wiring time, not per keystroke, because probing on the capture
path would have called the STT backend on every push-to-talk. And the recorder
probe is no longer a `start()`/`cancel()` dry-run: that opened the microphone
(a TCC prompt at every launch on macOS) and, worse, **would have discarded a
user's in-flight recording** the first time anything consulted the report
mid-session. It is now the non-destructive `AudioRecorder::is_wired()` —
which is exactly as strong a claim as before (the prior lane graded the old
probe "resolvability, not liveness"; a successful `start()` never looks at a
sample either), at none of the cost. A regression test asserts the readiness
check does not touch the recorder at all while recording.

Three tests, including a **control**: the same seams, the same actions, built
*without* a report, are allowed through — proving the refusal comes from
`check_requirements`' verdict and not from the action names.

---

## 3. Ordered protocol events — settled, and deliberately not "fixed"

The prior lane explicitly declined to claim the generic ladder is ordered. I
settled it, and the evidence is the protocol crate's own words —
`crates/wcore-protocol/src/contract/generate.rs:41-45`:

> `ordinary_turn_tool_replay_reducer`: legacy ordinary turn and tool events
> **still have no producer event ID or monotonic sequence.**

`ToolRequest` / `ToolRunning` / `ToolResult` / `ToolCancelled` carry `msg_id`
and `call_id`. That is **correlation identity, not order**: a host can bind a
result to its request; it cannot verify sequence or detect a gap. So the honest
answer to "does the generic ladder cover it" is **no — and neither would voice
events**, because new variants inherit that absence exactly.

I put it to the panel. **3/3 DEFER**, and all three reached the operative
reason independently: unsequenced, unconsumed variants would be an eleventh
"advertised but dead" surface *and* would drift a Desktop fixture corpus I am
forbidden to regenerate.

| leg | vote |
|---|---|
| codex gpt-5.6-sol | `VOTE=DEFER` |
| gemini-3.1-pro-preview | `VOTE=DEFER` |
| kimi K3 | `VOTE=DEFER` |
| internal adversarial | argued ADD — half right, did not move the vote |

kimi named the strongest objection to its own vote — *"deferring feels like
dodging the acceptance criterion"* — and answered it: the criterion names
*ordering*, and the variant proposal provably cannot deliver ordering.

**Internal adversarial pass, arguing FOR adding them now:** the TUI bridge
already emits a `ProtocolEvent::Info` on every voice toggle, so a typed event
would have a production consumer from day one and would not be dead on
arrival; and a Desktop host genuinely cannot build a mic indicator by
string-matching `"Recording started…"`. **That is right about the gap and wrong
about the clause.** It argues for a typed *state* surface — which is real, and
is why I filed `.planning/SEAM-REQUESTS/VOICE-STREAMING-STATE.md` — not for the
`ordered` clause, which stays failed either way.

**I did NOT run `wcore-contract generate` and I introduced no contract drift.**
My brief said drift would be acceptable if I added events; I did not add them,
so there is none.

---

## 4. CI — the 11 dead tests now run, with the count asserted

```
/usr/bin/grep -rn "voice" .github justfile   -> 0 matches, before this lane
```

Zero. On any platform, in any configuration. `voice` is off by default and
`tool_backends::voice_mode` is `#[cfg(feature = "voice")]`, so the whole
module — including the empty `stop()` — was invisible to every green run.

Added as a **step on the existing containerized Linux job**, not a new job: that
image already installs `libasound2-dev`, so the cost is one incremental rebuild
of `wcore-agent` rather than a fresh dependency graph on a fresh runner.

**The gate can fail, and I built it assuming it would try not to.**
`cargo test <filter>` exits 0 having run ZERO tests when the filter matches
nothing — LANE-BRIEF §3.2 flavour (c), the one that is easiest to write by
accident because the command *looks* targeted. So the step reads the executed
count back out and compares it to a floor:

```bash
n="$(printf '%s\n' "$out" | sed -n 's/^test result: ok\. \([0-9]*\) passed;.*/\1/p' | tail -1)"
if [ "$n" -lt "$min" ]; then echo "::error::$crate ran $n voice tests, expected >= $min"; exit 1; fi
```

with `set -o pipefail` so no pipe steals cargo's exit status. Floors are the
counts measured at this commit: **wcore-agent 14, wcore-tools 32**. A filter
typo, a renamed module or a deleted suite reds the step instead of passing
silently.

---

## Gate results (all read back, never inferred from exit status)

All figures below came from `/usr/bin/env cargo` on `hetzner-dsm` at
`83034afe`, unproxied, because `rtk` rewrites cargo output and strips the
`0 ignored` / `0 filtered out` fields the anti-vacuity rule depends on.

```
cargo test -p wcore-agent --features voice --lib voice_mode
  test result: ok. 14 passed; 0 failed; 0 ignored; 0 measured; 2174 filtered out; 7.31s
cargo test -p wcore-tools --lib voice_mode
  test result: ok. 32 passed; 0 failed; 0 ignored; 0 measured; 968 filtered out
cargo test -p wcore-tools --lib                      (collateral check)
  test result: ok. 997 passed; 0 failed; 3 ignored; 0 measured; 0 filtered out
cargo clippy -p wcore-agent -p wcore-tools --features wcore-agent/voice --all-targets -- -D warnings
  rc=0
cargo check -p wcore-cli --features voice --all-targets
  clean  (proves the feature-on binary, including the bootstrap gate, builds)
cargo fmt --all -- --check                           (Mac, permitted)
  rc=0
```

`0 ignored` on every line: no test was silenced to reach green, and nothing
here is a suite that exits 0 having run nothing.

## Fence and hygiene

```
BASE=$(git merge-base HEAD plan/f20-unified-audit-repair)  -> 75d8a8f0…
git diff "$BASE" -- crates/wcore-cli/src/lib.rs crates/wcore-cli/src/main.rs | wc -l   -> 0
liveness control, a path I DID change (wcore-tools/src/voice_mode.rs)                  -> 328
files changed vs BASE: 6   untracked: 0
```

The zero is a measurement, not a dead command. No merge, no PR, no tag, no
issue closed, no `wcore-contract generate`, no runner change, no
`Co-Authored-By`.

## Disclosures

**Darwin exception (LANE-BRIEF §0), used and disclosed.** Machine: Sean's Mac,
`sean-mac-arm64`. Command: `cargo test -p wcore-agent --features voice --lib
darwin_real_afplay` — one crate, one named test, never a workspace build,
never clippy, never release. It qualifies because `afplay` is the program the
production player resolves to on Darwin and **exists on no permitted host**:
hetzner is Linux and has no `afplay`, so the shipped macOS playback path is
provable nowhere else. Everything else in this lane ran on hetzner.

**No credential was used, needed, echoed, written or transmitted by any leg of
this lane.**

**`rtk`.** Every number in this report came from `/usr/bin/grep`,
`/usr/bin/git`, `/usr/bin/wc` or `/usr/bin/env cargo`.

## Open, and honestly open

1. **Accounting stays NOT MET.** Untouched by me. The prior lane's fairness
   check — no media tool does per-tool accounting — is the finding, and it
   belongs to the tool model, not to voice.
2. **Ordered protocol events stay NOT MET.** Needs a monotonic sequence on all
   protocol events. Protocol-owner work; seam request filed.
3. **Nothing calls `VoiceMode::play()`.** Until something does, barge-in is
   correct and unexercised in the shipped binary. §1.4.
4. **`voice` still must not be a default feature**, and I did not relitigate
   that — the ALSA measurement stands. This lane closes both of the two
   preconditions the prior lane named for re-asking the question later
   (implement barge-in; get the voice tests into CI), so the question is now
   re-askable. It is not re-asked here.
5. Linux and Windows runtime behaviour of the player: **not claimed.** The
   cross-platform test ran on Linux (hetzner) and the real-player arm on
   Darwin. `aplay` and the PowerShell `SoundPlayer` path are compile-checked
   only.
