---
lane: 27-browser-voice
criteria-attempted: [27-C2, 27-C3, 27-C4]
grade-27-C2: PARTIAL
grade-27-C3: NOT MET
grade-27-C4: NOT MET
deferred:
  - id: C4-voice-interruption
    cost: "non-default `--features voice` build PLUS a host with a real capture device (seandesktop). hetzner-dsm is headless with no capture device and cannot host it at any price."
  - id: C3-four-generation-shapes
    cost: "an MCP media-tool fixture (none exists) for the MCP-only / late-MCP / combined shapes, plus a credentialled built-in run."
  - id: C2-policy-conjunct
    cost: "downloads-root confinement, the approval gate on a CUA op, and process-count before/during/after plus one reaper interval. No baseline added by this lane."
new-finding: >-
  browser_suite=true and computer_use=true now mean RESOLVABILITY, not liveness:
  measured true on /bin/true and on DISPLAY=:99 with no X server. Severity MEDIUM.
  Separately, the narrowing REASON reaches an operator on stderr and reaches no
  host at all — CapabilityId has 8 variants and none is Browser/ComputerUse/Web.
credential-disclosure: >-
  FLUX_API_KEY was loaded into a shell variable on the Mac to run the mandated
  sweep, and was never used for any provider call, never echoed, never written to
  disk, never committed. Sweep: 0 hits across all lane artifacts and all files
  changed vs BASE, with a liveness control of 35 files matching a known-present
  string. No credential ever reached hetzner-dsm.
fence-exposure: >-
  ZERO. `git diff --stat 861d1b1a -- crates/wcore-cli/src/lib.rs
  crates/wcore-cli/src/main.rs` is empty. No .rs file was modified at all
  (0 of 7 changed files). No contract regeneration, no seam change.
status: complete
---

# Phase 27 — lane `27-browser-voice`

Base `861d1b1a716240165209336b1fa38d36f9445716`. Branch `lane/27-browser-voice`.
Criteria owned: **C2**, **C3**, **C4**. Sibling lane `27-media-intake` owns C1
and the vision seam; no overlap was taken.

**This lane wrote no production code.** It was dispatched to check whether a
prior lane's readiness repair had landed and, if so, to *measure and grade
rather than rebuild*. The repair had landed. Everything below is measurement.

---

## What I ranked first, and why

Pre-registered in the NOTES **before** measuring, so it cannot be retrofitted:

**Rank 1 — C2 readiness truth.** A flag reading `true` where the capability
cannot run makes a host route work into a hole. Highest damage, and it was the
one item with a landed candidate fix never proved against a genuine negative.

**Rank 2 — C3 generation existence.** **Rank 3 — C4 voice**, on a prior that it
was in no shipped artifact. That prior turned out to be **right for the wrong
reason**, and I record it as a prior that had to be corrected mid-lane: voice is
not missing, it is 114 KB of real source that is *feature-gated out of the
default build*.

---

## C2 — GRADE: PARTIAL

C2 has two conjuncts. They land differently and the grade must say so.

### Conjunct 1a — "publish live readiness": the FLAG. **MET.**

**The prior lane's repair is real, and I proved it against a natural negative.**

`PluginCapabilitySet::narrowed_to_live()` (`protocol_sink.rs:186`) is called
unconditionally on the live bootstrap path (`bootstrap.rs:941`) — not behind a
feature, env var, or config flag — and covers both flags.

A/B on **one** 96 MB release binary on `hetzner-dsm`, which is dead in its
**natural** state (`which camofox-browser camoufox` → nothing; `DISPLAY` and
`WAYLAND_DISPLAY` both empty). Arm N plants **nothing**; that is a strictly
stronger negative than the existing e2e test's synthetic dead arm.

| flag | Arm N (natural) | Arm R (`/bin/true`, `DISPLAY=:99`) |
|---|---|---|
| `plugins` | **true** | **true** |
| `browser_suite` | **`<ABSENT>`** | **true** |
| `computer_use` | **`<ABSENT>`** | **true** |

`plugins: true` in **both** arms is the anchor: it is never narrowed, so arm N's
absence is liveness narrowing and not "the plugin was never discovered". Without
it the negative leg could pass for the wrong reason.

Arm N stderr, verbatim:
```
WARN not advertising browser_suite: the plugin is loaded but no backend can start
  reason=no browser backend can start: `camofox-browser` does not resolve on PATH
         and no sidecar answered http://localhost:9377/health
WARN not advertising computer_use: the plugin is loaded but no backend can start
  reason=neither DISPLAY nor WAYLAND_DISPLAY is set, so no display server is
         reachable and the X11 backend cannot connect
```

The exact state the phase verdict recorded as broken — both flags `true` on a
box with no browser binary and no display — **is fixed at this SHA**, on the
shipped binary. The MEDIA-* ledger row's `"browser_suite":true,"computer_use":true`
capture at `2ecdfdf5` no longer reproduces.

### Conjunct 1b — "publish live readiness": the REASON. **NOT MET.**

The WARNs are on **stderr**. The protocol stream carries nothing.

All 24 `capability_activation` events, enumerated from the same captures, name
exactly 8 capabilities — `pricing_refresher`, `mid_flight_monitor`,
`cooldown_tracker`, `learned_policy`, `smart_handoff`, `delegate_isolation`,
`procedure_skill_drafting`, `legacy_auto_skill_drafting` — and the ladder is
**byte-identical in arm N and arm R**. It does not react to backend liveness.

Corroborated in source with a block-found control (the `sed` range returned 10
lines, so the enum was located and the absence is a measurement):

```rust
pub enum CapabilityId {   // crates/wcore-protocol/src/events.rs
    PricingRefresher, MidFlightMonitor, CooldownTracker, LearnedPolicy,
    SmartHandoff, DelegateIsolation, ProcedureSkillDrafting,
    LegacyAutoSkillDrafting,
}                          // 8 variants. No Browser. No ComputerUse. No Web.
```

**SR-27-1 has not landed.** The verdict's sentence *"the activation ladder still
carries no identity for browser, computer use or web"* remains true, now proved
live rather than by source-reading.

**And the consequence is sharper than the seam request states.** The probe's own
doc comment claims the WARN is how the recorded panel dissent — that a silently
dropped capability becomes an un-debuggable missing feature — is *honoured*.
Measured: it is honoured for an operator reading stderr and **for nobody else**.
A Desktop host sees the capability simply stop being present, with no
`unavailable` + `reason_code`, even though the same stream already delivers
exactly that for eight other capabilities, including the closely analogous
`learned_policy → runtime_path_unwired`.

This is fenced work (`wcore-protocol`), so I did not make it.

### Conjunct 2 — "preserve sandbox, egress, approval, and cleanup policy". **NOT MEASURED BY THIS LANE.**

Stated plainly rather than folded into the grade. Downloads-root confinement,
the approval gate on a computer-use operation, and process count
before/during/after plus one reaper interval still have **no baseline**, exactly
as the verdict recorded. I added none. Only origin admission was ever measured,
by the earlier phase.

### The residual — severity MEDIUM

Arm R is the dispatch's third proof obligation answered **negatively**: a
capability reporting `true` was **not** shown to work.

- `browser_suite: true` was granted on `/bin/true` — an ELF that exits
  immediately. `curl http://127.0.0.1:9377/health` → `rc=7`, connection refused.
- `computer_use: true` was granted on `DISPLAY=:99` with no `/tmp/.X11-unix/X99`
  and **0** listeners on port 6099. **Liveness control for that zero:** `ss`
  reported **110** listening sockets on the box, so the `0` is a measurement and
  not a dead instrument.

So the repair moved the flag from **linkage → resolvability**, not to
**liveness**. `true` now means "a path resolved on PATH" / "a string is set in
the environment".

**Graded MEDIUM → BACKLOG, not HIGH.** The dominant real deployment — headless,
nothing installed — is now correct with no operator action, and that was the
shipping defect. The residual requires an operator to have nominated something
non-functional, and when they have, the operation fails with a real runtime
error rather than a silent circle.

**Dissent, recorded because it is the strongest counter and it changes what a
successor should do first:** the tool's own remedy string says *"export DISPLAY
for an available X server (e.g. an Xvfb instance)"*. An operator following that
advice into a container where Xvfb is dead or not yet up lands precisely in the
false-`true` state. That is the same shape as the `[browser]` vs
`[browser.policy]` defect which *was* graded HIGH. I hold MEDIUM because that
defect left the tool disabled with no diagnostic and no way out, whereas here
the operation errors actionably — but a successor who disagrees has a real case.

### Verified already-closed — do not re-fix

The verdict's open HIGH (`wcore-browser/src/tool.rs:499` naming `[browser]`
where the loader reads `[browser.policy]`) **is closed at this SHA**. `tool.rs`
now calls `config_hint::disabled_by_default_hint()`, and `config_hint.rs`
carries a guard asserting no snippet names a bare `[browser]`, round-tripped
through the real loader by `wcore-agent/tests/browser_config_hint_roundtrip.rs`.
Verified by inspection; I did **not** re-run that test (see deferrals).

---

## C3 — GRADE: NOT MET (existence answered: it EXISTS and it SHIPS)

The honest first question was "does it exist and is it reachable", not "does it
pass". Answer:

**Exists.** `crates/wcore-agent/src/tool_backends/image_gen.rs` is 58.3 KB — the
largest tool backend in the tree — plus
`crates/wcore-tools/src/image_generation_tool.rs`.

**Ships.** Registration at `bootstrap.rs:1304-1310` is **credential-gated only,
not feature-gated** — no `#[cfg]`. The tool is compiled into the default binary
and hides itself via `is_available()` when no backend key resolves. The
`allow_pollinations` fallback is passed `false` and the comment states the config
field that would expose it is future work, so the keyless path is currently
unreachable.

**Not exercised.** None of the four shapes (built-in, MCP-only, late-MCP,
combined) was run. Built-in is reachable with a credential; the other three need
an MCP media-tool fixture, and **no such fixture exists** — which is why the
prior phase could not reach them either. Deferred with that cost stated.

No absence claim is made about C3 anywhere in this report.

---

## C4 — GRADE: NOT MET, and the verdict's diagnosis needs correcting

**This is the lane's most consequential secondary result.**

The phase verdict called C4 *"an execution shortfall, not an environmental
impossibility"*, on the grounds that `seandesktop` has audio and a toolchain and
the path was simply not taken. **That is half right, and the missing half
changes what a successor must do.**

`bootstrap.rs:1361`:
```rust
#[cfg(feature = "voice")]
if let Some(vm) = crate::tool_backends::voice_mode::build_voice_mode_backend(&self.config) {
    registry.register(Box::new(wcore_tools::voice_mode::VoiceModeTool::new(vm)));
}
```
`wcore-agent/Cargo.toml`: `voice = ["dep:cpal", "dep:hound"]`, in no default.
`wcore-cli/Cargo.toml`: `default = ["remote-registry", "workflow", "monitor", "review_artifact"]`.

**`voice` is not in `default`.** The comment gives the reason outright: *"A TUI
must not hard-require ALSA at runtime, so the default binary is built without
it"* (Issue #14, cpal → `libasound.so.2`).

**So the streaming mic-capture loop is compiled OUT of the shipped artifact.**
No run of the shipped binary on any host — including `seandesktop` — could have
exercised `voice_mode`. Exercising C4 requires **building a non-default artifact
first**. The verdict's route was not merely untaken; it was incomplete.

**Boundary — do not overstate this.** Two adjacent voice surfaces are **not**
feature-gated and **do** ship, credential-gated only: `tts` (`bootstrap.rs:1348`)
and `transcribe_audio` (`bootstrap.rs:1337`). A flat "voice is absent" claim
would be **false**. The precise claim: *TTS-out and STT-on-a-file ship; the
streaming mic loop that C4's interruption and cancellation clauses are about
does not.*

**Deferred, on measured cost:** `cargo build -p wcore-cli --features voice`
**plus** a host with a real capture device. hetzner-dsm is headless with no
capture device and cannot host the second half at any price. That is
`seandesktop`, and it is a full build plus an audio-driven interruption run.

---

## Cross-audit panel

Two questions: C2 `PARTIAL` vs `NOT MET`, and residual `HIGH` vs `MEDIUM`.

| leg | GRADE | SEVERITY |
|---|---|---|
| gemini-3.1-pro-preview | `PARTIAL` | `MEDIUM` |
| kimi K3 | `PARTIAL` | `MEDIUM` |
| **codex gpt-5.6-sol** | **VOTE DROPPED** | **VOTE DROPPED** |
| internal adversarial | argued `NOT MET` and `HIGH` — see below | |

**The codex leg is recorded as dropped, not omitted.** It exceeded a 7-minute
wall clock and was killed with rc=143 having produced no parseable
`GRADE=`/`SEVERITY=` line. Per §4 a silently-dropped vote is the same defect
class as a self-passing gate, so it is named rather than quietly excluded. The
panel is therefore **2 of 3 legs**, unanimous where it voted.

**Internal adversarial pass** (arguing against the consensus, as required):

*Against PARTIAL:* C2 is a conjunction, and I measured one half of one conjunct
while the policy conjunct remains ~75% unmeasured. A criterion cannot be PARTIAL
on the strength of the part you happened to measure. — **Partly accepted.** It
does not move the grade, because PARTIAL means precisely "some clauses met, some
not", and grading NOT MET would discard a real, live-proved repair and repeat the
phase's error pointing the other way. But it forced conjunct 2 to be broken out
above and labelled **NOT MEASURED BY THIS LANE** rather than folded silently
into "partial".

*Against MEDIUM:* the product's own remedy text steers operators into the
residual. — **Recorded as live dissent** in the C2 section rather than dismissed.

---

## Rules-of-evidence work, including one defect in my own instrument

- **Free zero caught by a control (twice).** Unquoted `--include=*.rs` was eaten
  by zsh and returned `no matches found` for both target searches; discarded and
  re-run quoted.
- **Instrument defect found AND repaired in-lane** (§6b-ii). I searched
  `crates/wcore-cli/src/cli.rs` for a tool-enumeration surface and got `0` for
  `json-stream` — **a string occurring 22 times in that crate**. Cause: there is
  no `cli.rs`. Every search against it returns a free zero, and zero is the
  *success value* for "this surface does not exist". Repaired rule: **stat the
  target before searching it.** Self-test, three assertions: (1) known-positive
  passes — 22 hits in `main.rs`; (2) known-negative fails — a non-statable path
  is refused, not scored 0; (3) **the old instrument would have missed it** — it
  returned 0 for a string present 22 times, so the repair is not a no-op.
- **A refutation, recorded as a result.** I predicted the probe resolved a
  different program than the supervisor spawns (`camofox-browser` vs the
  verdict's `spawn camoufox`). It does not: `supervisor.rs:71-72` and
  `liveness.rs:88` resolve identically, and `camofox` is the real upstream
  package name (`@askjo/camofox-browser`). Work correctly not done.
- **A prior of mine corrected mid-lane.** I pre-registered that voice was in no
  shipped artifact "because it doesn't exist". It exists, substantially; it is
  feature-gated out. Same conclusion, different and materially better reason.
- All reported numbers came from `/usr/bin/grep`, `/usr/bin/git`, `/usr/bin/sed`
  or `python3` — never the `rtk`-proxied forms.

---

## Credential handling

`FLUX_API_KEY` was loaded via `set -a; . ~/.wayland-secrets/flux.env; set +a`
into a shell variable **on the Mac only**, solely to run the mandated sweep. It
was **never used for a provider call** (C3 was not exercised), never echoed,
never written to disk, never committed, and **never reached hetzner-dsm**.

`.planning/scripts/f24-secret-sweep.sh` **does not exist at this base** — control:
30 scripts are present in that directory, so the directory is alive and the
absence is real. Sweep run manually with a liveness control:

```
liveness control — files containing 'browser_suite' : 35   (instrument alive)
lane artifacts containing the live key value        :  0
files changed vs BASE containing the value          :  0
```

---

## Fence exposure and hygiene

```
BASE=861d1b1a716240165209336b1fa38d36f9445716
git diff --stat "$BASE" -- crates/wcore-cli/src/lib.rs crates/wcore-cli/src/main.rs
  → (empty)
```

**ZERO fence exposure.** 7 files changed vs BASE, **0** of them `.rs`. No
contract regeneration, no seam change, no PR, no merge, no tag.

hetzner worktree `/root/wayland-27bv` and its `target/` removed after
measurement, returning **2.2 GB** (`git worktree remove` confirmed; the path no
longer stats). Disk checked before building — `698G` free of 1.8T, well clear of
the ~150G floor — and the build was targeted (`-p wcore-cli --bin wayland-core`),
never a full-workspace build, per the §2 rule written after a previous lane
filled the shared disk.

---

## What a successor should do first

1. **Land SR-27-1** (`Browser`, `ComputerUse`, `Web` on `CapabilityId`) and emit
   the ladder entries. The reasons already exist, fully formed, with remedies —
   they are being written to stderr and thrown away. This is the largest
   honesty gain per line of code in C2 and it is purely fenced work.
2. **Build `--features voice` on `seandesktop`** before attempting C4 at all.
   Any C4 attempt against a default binary will measure nothing.
3. **Baseline C2's policy conjunct** — downloads-root confinement, the CUA
   approval gate, and process count plus one reaper interval. Untouched by this
   lane and by the phase before it.
4. Backlog the resolvability residual (MEDIUM), and read the recorded dissent
   before deciding it is not HIGH.
