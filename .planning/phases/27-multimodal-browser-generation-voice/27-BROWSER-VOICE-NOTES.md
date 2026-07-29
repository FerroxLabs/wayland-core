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

## Log

- **T+0** worktree verified, brief + verdict + MEDIA-* ledger row read.
- **T+1** M-1 recorded: narrowing landed and is wired; lane pivots from build
  to measure. NOTES committed (this file).
