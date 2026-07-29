# 27-BROWSER-VOICE — running notes

Lane `27-browser-voice`. Branch `lane/27-browser-voice`. Base `861d1b1a`.
Criteria owned: **C2** (browser/CUA/web readiness + policy), **C3** (media
generation shapes), **C4** (streaming voice).
Sibling lane `27-media-intake` owns C1 and the vision seam — not touched here.

Append-and-commit after every measurement (LANE-BRIEF §6b-i).

---

## T+0 — worktree verified

```
/usr/bin/git rev-parse --show-toplevel
  → /Users/seandonahoe/dev/waylandcore-frontier-worktrees/lane-27-browser-voice
/usr/bin/git rev-parse --abbrev-ref HEAD → lane/27-browser-voice
/usr/bin/git rev-parse HEAD              → 861d1b1a716240165209336b1fa38d36f9445716
/usr/bin/git merge-base HEAD plan/f20-unified-audit-repair
                                         → 861d1b1a716240165209336b1fa38d36f9445716
```

`BASE=861d1b1a716240165209336b1fa38d36f9445716` — captured once, quoted
everywhere. Fence diffs go against this SHA, never against the branch name
(LANE-BRIEF §6).

---

## M-1 — the prior lane's readiness narrowing DID land, and it covers BOTH flags

The dispatch brief told me to check this before building anything. It landed.

**Instrument liveness control first** (§3b-i — a known-negative is self-passing
on a dead grep). Known-positive in the same tool/flags:

```
/usr/bin/grep -rn "from_verified" --include="*.rs" crates/ | wc -l  → 22
```

Non-zero, so `/usr/bin/grep -rn --include="*.rs" crates/` is alive. Globs are
quoted — zsh ate `--include=*.rs` unquoted on the first attempt and returned
`no matches found` for BOTH searches, which is exactly the free-zero this rule
exists to catch. First attempt discarded.

Same live instrument, target search:

```
/usr/bin/grep -rn "narrowed_to_live" --include="*.rs" crates/
  crates/wcore-agent/tests/capability_liveness_narrowing.rs:10   (doc)
  crates/wcore-agent/tests/capability_liveness_narrowing.rs:15   (doc)
  crates/wcore-agent/tests/capability_liveness_narrowing.rs:75   (call, test)
  crates/wcore-agent/tests/capability_liveness_narrowing.rs:104  (call, test)
  crates/wcore-agent/src/output/protocol_sink.rs:186             (definition)
  crates/wcore-agent/src/bootstrap.rs:941                        (CALL — production)
```

**Findings, source-read (not yet live-proved):**

1. `PluginCapabilitySet::narrowed_to_live()` exists at
   `crates/wcore-agent/src/output/protocol_sink.rs:186`, marked `27-C2(b)`.
   It runs `wcore_browser::liveness::probe(CamoufoxBackend::default_url())`
   and `wcore_cua::liveness::probe()`, and on `probe.unavailable()` sets the
   flag to `false` with a WARN carrying `reason` + `remedy`.
2. It is **monotone-clearing by construction** — both narrowings are inside
   `if out.<flag> { … }` and only ever assign `false`. So the Wave-SC plugin
   identity guarantee from `from_verified` is preserved: a `false` can never
   become `true` here.
3. **It is wired on the live path**, `crates/wcore-agent/src/bootstrap.rs:939-942`:
   ```rust
   let plugin_capabilities =
       PluginCapabilitySet::from_verified(&verified_plugins)
           .narrowed_to_live()
           .await;
   ```
   Unconditional — not behind a cargo feature, an env var, or a config flag.
   That rules out the "constructed but unwired" (`RuntimePathUnwired`) shape
   that Phase 27's own seam request anticipated.
4. It covers **both** flags this lane was pointed at: `browser_suite` and
   `computer_use`.
5. The doc comment claims no wire change is implied (same field, same type,
   same value domain, `schema_digest` blind to it) — so **no `CONTRACT_MINOR`
   bump and no `wcore-contract generate`**, which keeps this inside my fences.
   SR-27-2 in `.planning/SEAM-REQUESTS/27.md` asked for a minor bump on the
   *other* design (`chain-plus-derived-flags`); the design that actually landed
   is the narrowing one, which does not need it. **To verify, not assume.**

**Consequence for this lane:** C2's headline defect is no longer "build the
fix". It is **"is the landed fix real on a box where the capability genuinely
cannot run, and does the shipped binary's `ready` frame show it?"** That is a
measure-and-grade job, exactly as the dispatch predicted. I am not rebuilding it.

**Not yet established (open):**
- (a) Do `wcore_browser::liveness::probe` / `wcore_cua::liveness::probe`
  actually return `unavailable()` on hetzner-dsm (no camoufox binary, no
  display)? A probe that returns `Indeterminate` everywhere is a no-op wearing
  a fix's clothes — and the doc comment explicitly biases toward
  `Indeterminate` ("anything undecidable without launching a backend keeps the
  capability"). **This is the single highest-risk assumption in the lane.**
- (b) Does the flag reaching the wire in the real `ready` event change? The
  ledger records `"browser_suite":true,"computer_use":true` captured at
  `2ecdfdf5` on that machine. An A/B on ONE binary at HEAD is the shape that
  answers it.
- (c) Is `browser_suite` even reachable? It is gated on the `wayland-browser`
  plugin being loaded. If the shipped binary loads no such plugin, the flag is
  `false` for the *wrong reason* and the A/B proves nothing about the probe.
  Must separate "false because no plugin" from "false because probe cleared it".
- (d) The open HIGH from the phase verdict: `wcore-browser/src/tool.rs:499`
  names `[browser] allowed_origins` where the key actually read is
  `[browser.policy] allowed_origins`. Not yet confirmed at this SHA.

---

## Ranking decision (pre-registered, before measuring)

Recording this now so it cannot be retrofitted to whatever I happen to finish.

**Rank 1 — C2 readiness truth.** A flag that reads `true` on a box where the
capability cannot run makes a host *route work into a hole*. It is the
advertised-but-dead class in its most damaging form, and it is the one item
here with a landed candidate fix that has never been proved against a genuine
negative. Highest damage, lowest remaining cost.

**Rank 2 — C3 generation existence.** The phase verdict says none of the four
shapes was ever exercised. The first honest question is not "does it pass" but
"does it exist and is it reachable". A costed existence answer is a real
deliverable.

**Rank 3 — C4 voice.** The verdict records no audio ever flowed. My prior
(to be checked, not asserted) is that voice is compiled into no shipped
artifact, which makes it the cheapest to defer and the most expensive to prove
— it needs `seandesktop` (audio + toolchain), and hetzner-dsm is headless with
no capture device.

**I will not finish all three.** Deferral will be stated with its cost.

---

## M-2 — the probes are real, and one suspicion of mine is REFUTED

Read both probe implementations in full.

`wcore-browser/src/liveness.rs` (257 lines) and `wcore-cua/src/liveness.rs`
(162 lines). Both are non-executing by design, both are monotone-clearing, and
both narrow only on `Unavailable` — `Indeterminate` deliberately keeps the
capability.

- **Browser:** `Ready` if `which(camofox_program())` resolves, else `Ready` if a
  sidecar answers `<base>/health` in 500 ms, else `Unavailable`. Feature
  `browserbase` (credentialed) and feature `chromium` both short-circuit to
  `Indeterminate` and never narrow.
- **CUA:** on Linux, `Unavailable` iff neither `DISPLAY` nor `WAYLAND_DISPLAY`
  is set. macOS/Windows → `Indeterminate` (no honest non-executing probe for a
  window-server session).

**REFUTED — a defect I predicted and did not find.** I suspected the probe
resolved a *different* program name than the supervisor spawns: the probe
resolves `camofox-browser` (no `u`) while the recorded live failure in the
phase verdict is `spawn camoufox: No such file or directory`. If so the probe
would be checking the wrong binary. It is not:

```
crates/wcore-browser/src/supervisor.rs:71-72   WAYLAND_CAMOUFOX_BIN | "camofox-browser"
crates/wcore-browser/src/liveness.rs:88        WAYLAND_CAMOUFOX_BIN | "camofox-browser"
```

Byte-identical resolution in both. `camofox` is the real upstream package name
(`@askjo/camofox-browser`, `backends/camoufox.rs:3`); the crate/module is
spelled `camoufox`, the program is not. Recorded as work correctly NOT done.

## M-3 — HIGH candidate: the `true` arm has never been shown to WORK

`crates/wcore-cli/tests/plugin_discovery_e2e.rs` is the prior lane's A/B and it
is genuinely good — same binary, same plugins, one variable, and its negative
leg anchors on `plugins: true` so it cannot pass for the wrong reason.

But look at how the **positive** leg plants its fact (lines 88-99):

```rust
cmd.env("WAYLAND_CAMOUFOX_BIN",
        std::env::current_exe().expect("resolve test binary path"));
cmd.env("DISPLAY", ":0");
```

The "browser" is **the test binary itself**, and the "display" is a string
`:0` that nothing connects to. The test is honest about it — its own assertion
message says *"browser_suite is pure linkage here"*. And that is correct for
what that test is for: it proves the narrowing fires.

**But it means the `true` arm is still unproven.** The dispatch's third proof
obligation is explicit: *a capability that reports `true` must be shown to
actually work, not merely to link.* After this repair, `browser_suite: true`
means **"a path resolved"** — satisfiable by `/bin/echo`, by a text file with
the +x bit, or by the test binary. `computer_use: true` on Linux means
**"a string is set in the environment"** — `DISPLAY=:99` with no X server
satisfies it.

So the repair moved the flag from *linkage* to *resolvability*, not to
*liveness*. That is a real and worthwhile narrowing — it removes the exact
headless case that started this — but it does not discharge C2's honesty bar,
and grading it as if it did would repeat the phase's original error one level up.

**To measure, not assert:** whether `DISPLAY=:99` with no X server yields
`computer_use: true` on the shipped binary. That is a one-variable positive
control for the false-`true` residual, and hetzner can run it.

## M-4 — C3 and C4 existence: my pre-registered prior on voice was WRONG

Instrument control first (`media_intake`, the sibling lane's file, known to
exist): `/usr/bin/grep -rl … | wc -l` → **2**. Non-zero, instrument alive.

**C4 voice — EXISTS, and substantially.** I predicted it was compiled into no
shipped artifact. That prediction was wrong and I am recording it as wrong:

```
crates/wcore-agent/src/tool_backends/voice_mode.rs   37.8K
crates/wcore-agent/src/tool_backends/tts.rs          41.0K
crates/wcore-agent/src/tool_backends/piper.rs        35.0K
crates/wcore-agent/src/tool_backends/openai_compat_whisper.rs  5.5K
crates/wcore-tools/src/{voice_mode,tts_tool,transcription_tools}.rs
crates/wcore-agent/examples/f27_voice_capture.rs   ← built for THIS criterion
```

**C3 generation — EXISTS.**

```
crates/wcore-agent/src/tool_backends/image_gen.rs    58.3K  (largest backend)
crates/wcore-tools/src/image_generation_tool.rs
```

So for both, "does it exist" is answered **yes** and the open question is
narrower: **is it reachable from the shipped binary**, which the same binary
run that serves C2 can answer cheaply. No ledger claim of absence is made.

## M-5 — hetzner is a genuine negative control (not a synthetic one)

```
ssh hetzner-dsm → Ubuntu-2404-noble-amd64-base
df -h /root → 1.8T total, 698G avail (59% used) — safe to build
which camofox-browser camoufox → (nothing, rc=1)
DISPLAY=[] WAYLAND_DISPLAY=[]
```

This is better than the e2e test's synthetic Dead arm: the box is dead in its
**natural** state, with no env manipulation at all. Worktree `/root/wayland-27bv`
created detached at `861d1b1a`. Release build of `-p wcore-cli --bin wayland-core`
started (targeted, per §2 — not a full workspace build).

## Log

- **T+0** worktree verified, brief + verdict + MEDIA-* ledger row read.
- **T+1** M-1 recorded: narrowing landed and is wired; lane pivots from build
  to measure. NOTES committed (this file).
- **T+2** M-2 probes read; camofox/camoufox naming suspicion refuted.
- **T+3** M-3 recorded: HIGH candidate — `true` proves resolvability, not work.
- **T+4** M-4 recorded: voice and generation both EXIST; my voice prior was wrong.
- **T+5** M-5 hetzner negative control confirmed natural; release build running.
