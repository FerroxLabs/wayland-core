# SUMMARY — lane `media-gen-voice`

Branch `lane/media-gen-voice`. Base integration `b2ddf113`.
Full working notes, panel transcript pointers and instrument findings:
`MEDIA-GEN-VOICE-NOTES.md` in this directory.

## Honest verdict up front

**Neither `27-C3` nor `27-C4` is closed, and I am claiming neither.** Two real
things landed — the shipped binary stopped misdescribing voice, and the second
of eight billable media backends now records cost, including a money-correctness
bug fixed on the way. `MEDIA-*` does **not** move off SOURCE on this work.

## Brief premises — two held, one stale

| premise | verdict |
|---|---|
| `MediaCostLedger` wired for image only; video/TTS/voice have zero cost sites | **HELD** — 1 of 8 billable backends covered at base |
| `voice` absent from every `default` list | **HELD** — `wcore-cli/Cargo.toml` `default = ["remote-registry","workflow","monitor","review_artifact"]` |
| "the four generation shapes were never exercised" | **STALE** — the ledger's own 2026-07-30 re-grade records built-in *exercised*, MCP-only *exercised*, combined *measured*; only **late-MCP** is unexercised. The brief also treats `F-27C3-04` (a closed *finding*) as one of the four *shapes*; the shapes are discovery shapes, not media modalities |

## `27-C4` — the voice decision

**Decided: keep `voice` off by default, and stop the shipped binary
half-claiming it.** Cross-audit panel unanimous (`codex gpt-5.6-sol`,
`gemini-3.1-pro-preview`, `kimi K3` — all rc=0, votes extracted unanchored,
last match taken), plus an internal pass arguing against.

The half-claim was **not** in docs (0 hits across `docs/` + `README.md`;
controls confirm the matcher alive). It was in the TUI `/config` catalog: a
`voice_mode` row with `deferred: false`, description *"Local microphone capture
via cpal. No env var needed."*, badge *"· device not probed"* and hint *"(no env
var — auto-detected)"*. All three strings are true only of a build containing
the feature; together they read as *"the capability is here and your microphone
is broken"*, sending users after a hardware fault that cannot exist, with
nothing naming the build flag.

Rejected enabling it by default: that relinks `libasound.so.2` and reverses the
**published** `#14` CHANGELOG commitment that *"the default binary runs on
minimal Linux without libasound"*, breaking headless containers at
**dynamic-link load time** — the worst failure mode a CLI has, to satisfy a
grading checkbox. Also rejected platform-conditional cpal (forks the meaning of
"default build", needs `voice_mode.rs` to compile with cpal absent, and still
excludes the Linux users the first option would have hurt).

Landed: `ProviderStatus::NotCompiledIn { feature }` — *"· not in this build"*,
rendered muted rather than warning, with a remedy line naming
`--features voice` that replaces the misleading auto-detect hint.

**Still NOT MET.** The criterion grades the shipped artifact and `voice`
remains out of `default`, deliberately. The separate open blocker — **no local
speech-to-text path exists in the tree** — is untouched by me.

## `27-C3` — cost coverage

Measured per backend (`image_gen` as the known-positive proving the matcher
alive): **1 of 8 billable media backends recorded cost at base → 2 of 8 now.**

Wired **transcription**, chosen because it is the one shape where a real
provider-reported dollar figure exists: Phase 27 measured `x-flux-cost-usd`
present on live FluxRouter transcription and absent in every channel for an
image. It was read by nobody — `resp.headers()` was dropped on the floor and
`verbose_json`'s `duration` (the unit transcription is billed on) parsed for
nothing. Records now on every exit where the provider was *reached*; a request
that never left records nothing.

**Money-correctness bug fixed on the way.** `for_success` priced a call as
`usd_per_image * units.images`, so any operator with a matching rate-card entry
would have recorded **`$0.00` for a transcription that cost real money** — the
exact lie `media_cost.rs` exists to prevent, arriving through the pricing path.
Latent at base; my change made it reachable, so it is fixed here.

**Still unaccounted, not claimed:** TTS, `video_analyze`, and the three vision
backends. `video_analyze` is the largest exposure — one tool call is **9**
billable provider calls at the default frame count, all invisible. The
**late-MCP shape is untouched**.

## Gates — every number read from a file with an unproxied tool

| gate | result |
|---|---|
| `cargo test -p wcore-cli --lib voice_mode` | **2 passed; 0 failed; 0 ignored; 0 measured; 1897 filtered out**, rc=0 |
| `cargo test -p wcore-tools --lib media_cost` | **11 passed; 0 failed; 0 ignored; 0 measured; 1007 filtered out**, rc=0 |
| `cargo test -p wcore-agent --lib openai_compat_whisper` | **7 passed; 0 failed; 0 ignored; 0 measured; 2237 filtered out**, rc=0 |
| `cargo fmt --all -- --check` (Mac) | rc=0 |
| `cargo clippy -p wcore-tools -p wcore-agent -p wcore-cli --all-targets` | **rc=0**. Residual warnings are in `user_model_identity_wire` / `cache_ledger_engine_test`, both untouched by me and pre-existing. Stated precisely: my capture was `tail -40`, so "no warnings in my files" is asserted over the visible tail plus rc=0, not over the whole log |

All at asserted SHA `f4bc7fb38e037bebdecd623423653c00a7536b4f` on
`hetzner-dsm`, verified equal to the Mac HEAD after every fetch.

### Both directions, per LANE-BRIEF §3b-iii

| gate | can it pass? | can it fail? |
|---|---|---|
| voice row honesty | 2 passed at HEAD | revert resolver to `DeviceUnprobed` → **0 passed; 2 failed**, rc=101, diagnostic `left: "· device not probed" / right: "· not in this build"` |
| `$0.00` zero-guard | 11 passed at HEAD | disable the `images == 0` guard → **10 passed; 1 failed**, rc=101 |
| provider cost header read off the wire | 7 passed at HEAD | drop `x-flux-cost-usd` from `COST_HEADERS` → **5 passed; 2 failed**, rc=101 |

Source restored after every mutation (`git diff --stat` = 0 lines). Tests also
carry in-test known-positives and known-negatives so a dead matcher cannot
satisfy the negative-shaped assertions — detailed in NOTES.

**Unrun cells, counted and reported:** live FluxRouter transcription = **0 runs**
(see limitations); `--features voice` build = **0 runs**; TTS / `video_analyze` /
vision cost = **0 cells wired, 0 run**.

## Instrument findings

1. **My own poll harness was self-passing.** `OUT=$(grep -c WLDONE f || echo 0)`
   yields the two-line string `"0\n0"` on no match (grep prints `0` *and* exits
   1, so `|| echo 0` also fires), which `!= "0"` compares **true**. It reported
   "DONE" while the crate was still compiling and **hid a real `E0521` compile
   failure I would have reported as a pass.** Repaired to `grep -q` in the same
   lane per §6b-ii, with the required three-assertion self-test including "the
   old matcher gets the known-negative wrong".
2. **`cargo test ... -- --exact --list` printed `0 tests`** — §3.2 flavour (c), a
   filter matching no test name, exiting 0. The command looked targeted and
   proved nothing; only reading the `N passed` count caught it.
3. zsh ate an unquoted `--include=*.rs`, and `/usr/bin/ls` does not exist on
   macOS — both would read as clean zeros inside a counting pipeline.

## Limitations I did not work around

**Transcription cost is unit-proven, not live-proven.** The SSRF-safe client
(`SsrfSafeResolver`) dials only validated **public** IPs, so a loopback
`wiremock` test cannot exercise this backend at all — a test written that way
would fail at connect and prove nothing. A real-endpoint proof needs the burn
key on hetzner *plus* an audio fixture that transcribes to non-empty text; I
had the credential path available and did not have the fixture. **I did not use
the FluxRouter key at all, so there is no secret to sweep for.**

No audio was played or captured on the Mac. `crates/wcore-cli/src/main.rs` and
`lib.rs` were **not touched** — no shared-file fence contact. I stayed out of
the channel adapters, per the `lane/24c3-channels` boundary.
