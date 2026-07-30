# NOTES — lane `media-gen-voice`

Base: `b2ddf113681647221dc9e5bbfc7de79b1da90b54`, branch `lane/media-gen-voice`.
Appended after every measurement per LANE-BRIEF §6b-i.

## Brief-premise verification (LANE-BRIEF: "your brief's measurements are probably stale")

### P1 — "video/TTS/voice have ZERO cost sites; MediaCostLedger is wired for image"

**HOLDS.** Measured at base, unproxied, quoted globs:

```
/usr/bin/grep -rn "MediaCostLedger" --include="*.rs" crates/
```

Production hits, all in one tool:
- `crates/wcore-tools/src/image_generation_tool.rs:68,295,336`
- definition `crates/wcore-tools/src/media_cost.rs:421,425`
- test-only `crates/wcore-agent/tests/f27_media_generation.rs:55,395,405`

Zero hits in `tts.rs`, `piper.rs`, `video_analyze.rs`, `voice_mode.rs`,
`openai_compat_whisper.rs`, or any vision backend.
Instrument liveness control in the same capture: `grep -c "pub "
crates/wcore-tools/src/media_cost.rs` → **49** (non-zero ⇒ grep alive).

### P2 — "`voice` absent from every `default` feature list"

**HOLDS.** `crates/wcore-cli/Cargo.toml`:
`default = ["remote-registry", "workflow", "monitor", "review_artifact"]`;
`voice = ["wcore-agent/voice"]` declared separately, in no default list.

### P3 — "the four generation shapes were never exercised"

**STALE — the brief contradicts the ledger it told me to read.** `27-C3`'s
2026-07-30 re-grade table (`CRITERIA-GAP-LEDGER.md:1272-1277`) records
built-in **exercised**, MCP-only **exercised**, combined **measured**, and only
**late-MCP NOT EXERCISED**. The brief additionally treats `F-27C3-04` (a closed
*finding*) as one of the four *shapes*; it is not — the shapes are discovery
shapes (built-in / MCP-only / late-MCP / combined), not media modalities.

So the residual 27-C3 work is narrower than briefed: **late-MCP** plus the
**accounting clause**, which is where P1's zero-cost-site finding lands.

## Instrument faults hit in the first 10 minutes (both predicted by the brief)

1. zsh ate `--include=*.rs` unquoted → `no matches found`, which would have read
   as a clean zero. Quote every glob.
2. `/usr/bin/ls` does not exist on macOS (it is `/bin/ls`) — an absolute path
   that is absolute and *wrong* still fails; the failure was loud here, but the
   same typo inside a counting pipeline would produce a silent zero.

## 27-C4 — the voice decision, taken

### The half-claim, located

Not in docs (measured: **0** hits for voice-mode wording across `docs/` and
`README.md`; control — 53 doc files contain "tool", 58 md hits for "image" via
the identical command shape, so the matcher was alive). It is in the **shipped
TUI**: `crates/wcore-cli/src/tui/surfaces/config.rs` renders a
`Tools & Providers` row

- `name: "voice_mode"`, `deferred: false`
- `description: "Local microphone capture via cpal. No env var needed."`
- badge `"· device not probed"`, hint `"(no env var — auto-detected)"`

All three strings are true only of a build that contains the feature. Together
they read as *"the capability is here and your microphone is not working"* —
sending the user after a hardware fault that cannot exist, with nothing on
screen naming the flag that would give them voice. `is_ok()` was already
correctly `false`, so this is a **wording/state** defect, not a readiness lie of
the `browser_suite:true` kind — but it is the same family.

### Panel (LANE-BRIEF §4) — unanimous B

`codex gpt-5.6-sol` **B**, `gemini-3.1-pro-preview` **B**, `kimi K3` **B**;
rc=0 for all three, votes extracted unanchored with the last match taken.
Options put to them: (A) add `voice` to `default`, (B) keep it off and fix the
row to name the build flag, (C) platform-conditional cpal so voice defaults on
for macOS/Windows only.

Decisive reasoning, which I adopt: **closing an acceptance criterion is not a
reason to reverse a published compatibility commitment.** `CHANGELOG.md:607`
(issue #14) states *"ALSA is no longer a hard dependency — cpal is gated behind
an off-by-default `voice` feature, so the default binary runs on minimal Linux
without libasound."* Users have built deployment images on that. (A) relinks
`libasound.so.2`, so every minimal container and headless server gets a binary
that fails at **dynamic-link load time** — it does not start at all. That is the
worst failure mode a CLI has, introduced to satisfy a grading checkbox. Kimi's
framing of the actual defect is the sharpest: the badge reads as **"broken"**
when the truth is **"not built"**.

(C) rejected: it makes "default build" mean different things per OS (docs, CI
matrix, bug reports all fork), requires the 1213-line `voice_mode.rs` and its
`cfg` sites to compile with cpal absent, and still excludes the Linux users (A)
would have hurt — complexity that serves neither the criterion nor the
constraint. It is also architectural, and unprovable on the only host I can
build on.

### Internal adversarial pass (against B)

The real charge is that **B closes nothing** — 27-C4 stays NOT MET and this is
relabeling. I accept that and do not claim the criterion. But B is not
cosmetic: the ledger's stated reason MEDIA-* is held at SOURCE is that
*"readiness is still unpublished and still dishonest at HEAD"*. This row is
that pathology in miniature, and removing a false claim from the shipped binary
is real product value, where shipping a binary that cannot start on minimal
Linux is negative product value.

### Landed (`4f7f31a1`)

`ProviderStatus::NotCompiledIn { feature }` — label `"· not in this build"`,
rendered **muted rather than warning** (nothing on the user's machine to fix),
carrying a `remedy()` line naming `--features voice`, which also replaces the
misleading `"(no env var — auto-detected)"` hint. Catalog description now states
the build requirement unconditionally — true in *both* builds, so the static
table stays pure data with no `cfg`. `ProviderStatus` has **0** references
outside `config.rs` (control: 52 refs total), so the change is contained.

### Both-direction control (LANE-BRIEF §3b-iii)

| direction | how | result |
|---|---|---|
| **can it pass?** | real HEAD `4f7f31a1`, `cargo test -p wcore-cli --lib voice_mode` on hetzner | `2 passed; 0 failed; 0 ignored; 0 measured; 1897 filtered out`, rc=0 |
| **can it fail?** | mutation: `resolve_voice_mode_status` reverted to `DeviceUnprobed` (1 site) | `0 passed; 2 failed`, rc=**101**, diagnostic `left: "· device not probed" / right: "· not in this build"` |

Source restored after the mutation — `git diff --stat` = 0 lines.
The tests also carry an in-test known-negative (a compiled-in provider must not
report `NotCompiledIn` nor carry a remedy) and a known-positive liveness control
(`google_meet.description` contains "OAuth" but not `--features voice`), so a
dead `contains` cannot satisfy them.

**Instrument trap hit live and worth recording:** `cargo test -p wcore-cli --lib
voice_mode -- --exact --list` reported **`0 tests`** — LANE-BRIEF §3.2 flavour
(c), a filter matching no test name, exiting 0. The command looked targeted and
proved nothing. Only reading the `N passed` count off the real run caught it.

### Honest verdict on 27-C4

**Still NOT MET, and I am not claiming it.** The criterion is graded on the
shipped artifact and `voice` remains out of every `default` list — deliberately.
What changed is that the shipped artifact no longer misrepresents the gap. The
remaining blocker named by `27-GAPS-SUMMARY.md:140-143` — **no local
speech-to-text path exists in the tree** — is untouched by me and is the thing
that would make a default-on voice mode useful rather than merely present.

## Open / next

- Enumerate every billable media call site and grade cost coverage per shape.
