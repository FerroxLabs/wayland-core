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
