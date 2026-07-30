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

## Open / next

- Enumerate every billable media call site and grade cost coverage per shape.
- Decide the voice ship-or-document question (27-C4) and execute it.
