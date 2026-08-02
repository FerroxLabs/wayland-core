# Phase 27 — verdict against the ROADMAP Success Criteria

**Phase goal, verbatim from `.planning/ROADMAP.md`:** "Attachments, documents,
browser/CUA/web, generation, and voice work consistently across providers and
hosts with honest readiness and bounded authority."

**THE GOAL IS NOT ACHIEVED.** One criterion is partially met. Four are not met.
The honest summary of this phase is that it produced good measurement and one
real repair, and left the majority of what it promised unproved.

---

## Criterion 1 — "Standalone and host messages use one bounded, validated attachment/document intake path and degrade explicitly on unsupported providers."

**GRADE: PARTIAL.**

**Met.** The document (PDF) path now goes through one bounded, open-once,
magic-byte-validated intake and gains an ingest cap enforced from the
descriptor's own metadata before any payload read. Explicit degradation on
unsupported providers is met for the image class and was proved live: the
Anthropic and Gemini builders were measured emitting a byte-identical outbound
request whether `supports_vision` said false or true, and both now substitute
`[image omitted: model not vision-capable]`. Both standalone and host surfaces
were exercised against the shipped binary and every refusal was proved to reach
the user.

**Not met.** "**One** intake path" is not true. The composer path and the
channel enricher were measured already correct and were deliberately not
rewritten through the new chokepoint, so the mechanism is shared for documents
and duplicated for images and channel media. The plan's gate requiring
`media_intake` in `attachments.rs` and `channel_media.rs` is **RED** and is
reported RED.

Separately, the plan's `supports_document_input` gate is **RED** because that
field was not added — measured as un-gateable, since `ContentBlock` has no
document variant and documents cross as tool-result text. That refutation is
evidenced and I stand behind it, but it is a departure from the plan and it is
recorded as one.

The TUI half was never exercised — no PTY drive was performed — and the macOS
leg has no artifact for this unpushed SHA.

## Criterion 2 — "Browser, CUA, and web surfaces publish live readiness and preserve sandbox, egress, approval, and cleanup policy."

**GRADE: NOT MET.**

**Nothing is published.** Readiness at HEAD is exactly as dishonest as it was
before this phase ran. `browser_suite` and `computer_use` are still `true` on a
box with no browser binary and no display, and the activation ladder still
carries no identity for browser, computer use or web.

What the phase did produce is the measurement that makes the fix unambiguous —
five single-variable captures showing the claim invariant under every absence,
plus the operation on that same machine failing with `spawn camoufox: No such
file or directory` — and a 4-0 adjudicated decision with its dissent preserved.
Implementation stopped at a fenced protocol seam and is filed as
`.planning/SEAM-REQUESTS/27.md`.

**Policy preservation is one-quarter measured.** Origin admission holds and
fails closed with a stated reason. Downloads-root confinement, the approval
gate on a computer-use operation, and the process count before/during/after a
session plus one reaper interval have **no baseline at all**.

**A new HIGH is open and unfixed:** the browser tool's own remediation text
names `[browser] allowed_origins` when the key actually read is
`[browser.policy] allowed_origins`. Following it verbatim leaves the tool
disabled. An unavailable whose stated fix is wrong fails this criterion's own
honesty bar.

## Criterion 3 — "Built-in, MCP-only, late-MCP, and combined media generation expose consistent discovery, credentials, accounting, and failures."

**GRADE: NOT MET.**

**None of the four generation shapes was exercised.** No MCP media-tool fixture
was built, so MCP-only, late-MCP and combined were never reachable.

One real result was obtained and it is a good one: the honest-degradation
advisory reaches the model verbatim on the wire, naming each unavailable
capability and the exact variables that would enable it, with an explicit
instruction not to invent a cause. Measured, not assumed. The matching gap is
that it reaches no host — zero events on the protocol stream — so a Desktop
user has nothing to render.

Accounting is recorded as SOURCE-ONLY: cost is token-shaped and a media call
produces no cost record. That it is unaccounted is a fact; whether it matters
was not decided.

## Criterion 4 — "Streaming voice supports interruption, cancellation, compatibility, accounting, and ordered protocol events."

**GRADE: NOT MET. NOTHING WAS EXERCISED.**

No audio flowed on any machine. No interruption occurred. No cancellation was
driven. No voice event ordering was observed. The phase brief singles this out
as the criterion that "must be exercised with real audio flowing and a real
interruption", and the plan singles it out as the exercise "most likely to be
quietly replaced by a unit test."

It was not replaced by a unit test. It was not attempted at all.

`hetzner-dsm` is headless and has no capture device, and the Mac has no working
Cargo — but **`seandesktop` has audio, a toolchain, and answered a reachability
probe at the start of this work.** The path existed and was not taken. This is
an execution shortfall, not an environmental impossibility.

## Criterion 5 — "Deterministic corpora and packaged smokes pass on native macOS, Linux, and Windows."

**GRADE: NOT MET.**

**Zero packaged smokes ran on zero platforms.** Every Linux measurement in this
phase came from a `cargo build --release` binary inside a build tree. That is
not a packaged artifact and is not counted as one anywhere in this phase's
evidence.

One deterministic corpus exists and is genuinely deterministic — 18 intake
entries with pinned bytes, byte lengths and SHA-256 digests, regenerable
identically on any platform. No suite consumes it, and the browser, generation
and voice corpora were never built.

---

## Requirements disposition

| Requirement | Disposition | Unmet clauses |
|---|---|---|
| F27-01 | **INCOMPLETE** | "one" intake path (partial — documents only); host/terminal half unproved (no PTY drive); macOS leg NOT RUN |
| F27-02 | **INCOMPLETE** | live readiness is not published at all; three of four policy guarantees have no baseline; Windows and macOS handshakes NOT RUN |
| F27-03 | **INCOMPLETE** | none of the four generation shapes exercised; accounting unresolved; host-visible failure surface absent |
| F27-04 | **INCOMPLETE** | no audio ever flowed; no interruption; no cancellation; no event-ordering observation |
| F27-05 | **INCOMPLETE** | no packaged smoke on any platform; no corpus-driven suite; no candidate sealed |

**No requirement is marked complete in `REQUIREMENTS.md`.**

---

## What is genuinely worth keeping from this phase

1. **The vision-gate repair.** A configuration knob that silently did nothing
   on two of the engine's message builders now works. Measured live by
   capturing the outbound request body twice with one variable changed, which is
   the only way that question is answerable.
2. **The PDF re-resolution repair.** Settled by syscall trace, not by reading
   source, and the trace also vindicated the composer path — which is why that
   path was left alone.
3. **The readiness measurement.** Five single-variable captures making it
   impossible to argue that the capability flags are anything but linkage. The
   fix is now a small, well-specified patch behind a filed seam request rather
   than an open question.
4. **The `[browser]` vs `[browser.policy]` defect**, found only because the
   product's own instructions were followed literally rather than read.
5. **Three refutations recorded as results:** the composer path, the
   document-degradation premise, and the generation advisory. Each one is work
   correctly NOT done.

## One thing I got wrong, recorded so it is not repeated

To establish whether 39 full-suite failures were mine or pre-existing, I built a
second full `target/` tree on `hetzner-dsm`. That filled the last of a shared
1.8 TB disk on which five other phases were building concurrently, and the
control run died with `rustc-LLVM ERROR: IO failure on output stream: No space
left on device`.

The 39 failures turned out to be caused by the same exhaustion — they are
`wcore-agent` delegated-mutation and `wcore-swarm` worktree tests that admit on
free disk and correctly failed closed with
`DispatchAdmission("... only 0 bytes are available")` while `df` read
`0 100% /`. None is in a crate this phase touched, and re-running the two
changed crates after freeing space gave 2132/2132.

I removed the extra tree and pruned `target/debug`, returning **129 GB**. But
between roughly the time the box hit 100% and the time I freed it, **any other
phase's capacity-admitting test on that box would have failed for the same
reason and would have looked like a regression in its own work.** If a
concurrent phase reports unexplained `DispatchAdmission` or worktree-landing
failures in that window, this is the cause.

The rule this earns: **on a shared build box, check `df` before creating a
second target tree, and prefer a narrower control** — running the specific
failing tests at the base commit in an existing tree, rather than building a
whole second workspace to answer one question.

## What a successor should do first

1. Land `.planning/SEAM-REQUESTS/27.md` SR-27-1..3. Criterion 2 is one small
   patch away from evidenced once the seam is open.
2. Fix `wcore-browser/src/tool.rs:499` — a two-word change to a string that
   currently sends every user in a circle.
3. Run the voice interruption on `seandesktop`. It is the only unmet criterion
   with no partial credit at all.

---

# SUPERSEDING BLOCK — 2026-08-01, lane `verdict-truth-text`, base `02575b6f`

**This block supersedes the grades of Criteria 2 and 4 above, and the "successor should do first"
list.** Criteria 1, 3 and 5 are **not** re-derived here and stand as written; see
`CRITERIA-STATUS.md` for their current text, which this lane did not re-measure.

**This is the phase where the corrections run in BOTH directions and the net is not comfortable.**
Two criteria were publishing worse-than-true grades. One of those upgrades makes the product
**more** exposed, not less, and this block says so first rather than burying it.

**Text only.** Zero files under `crates/`, `.github/`, `docs/` or `scripts/` were changed by the
lane that wrote this. No cargo was run. Full sweep and method:
`.planning/VERDICT-TRUTH-2026-08-01.md`.

## Criterion 4 — **NOT MET → PARTIAL, and it is now RELEASE-RELEVANT**

The grade above reads **"NOT MET. NOTHING WAS EXERCISED."** Its load-bearing structural claim,
carried forward into the gap ledger, was that `voice` is absent from every `default` list — so the
feature is not in the shipped artifact and the criterion is cheap. **At `02575b6f` that is false:**

```
crates/wcore-cli/Cargo.toml:31     default = ["remote-registry", "workflow", "monitor", "review_artifact", "voice"]
crates/wcore-cli/Cargo.toml:62     voice = ["wcore-agent/voice"]
crates/wcore-agent/Cargo.toml:234  voice = ["dep:cpal", "dep:hound"]
```

A default `cargo build -p wcore-cli` — which is what the release builds — links voice.
`CpalAudioPlayer` is production code (`voice_mode.rs:584`, `impl AudioPlayer for CpalAudioPlayer`
at `:691`), and the device-absent path is a real runtime string at `:823`
(*"voice_mode: cpal could not bind a default input device — tool hidden"*), which is what makes
the presence/absence control possible at all.

**Read this correctly: NOT MET → PARTIAL is NOT good news here.** A NOT MET on an unshipped
feature costs nothing. A **shipped** voice surface whose `voice_mode → transcribe_audio` handoff
is unproven on all three platforms is exactly the silent-failure class `CRITERIA-GAP-LEDGER.md`
pre-registered as blocking:

> *"If the `voice` feature is ever enabled in a release build, this criterion becomes blocking
> immediately, because a shipped voice surface with zero interruption evidence is exactly the
> silent-failure class that blocks 24-C2."*

It has been enabled. So it is. **The row moved up a grade and up a risk tier at the same time.**

**Still not MET, and this block declines to claim it:** the capture→transcribe handoff is unproven
end to end; **no product surface enumerates the tool registry headlessly**, so *"the tool is
REACHABLE"* cannot be observed from the CLI — only that the code is linked, which is the identical
blindness that let 22-C1 sit at zero call sites; and **#938** (FluxRouter STT returns 402
`premium_locked` through the product while a direct curl with the same key returns 200) is OPEN.

### A live artefact of the transition, found by this lane and left for an owner

```
.github/workflows/ci.yml:851   # `voice` is off by default (it hard-links libasound.so.2 on Linux —
.github/workflows/ci.yml:852   # see crates/wcore-agent/Cargo.toml:234), and `tool_backends::voice_mode`
```

That comment was true of `wcore-agent` in isolation and is **false of `wcore-cli`, which is what
ships**. The CI step it annotates is fine — `ci.yml:869-894` runs the voice suite with an explicit
executed-test floor (`ran $n voice tests, expected >= $min — a suite that exits 0 having run
nothing is not a suite`). **The step is honest; the comment lies.** Fixing it is a `.github/` edit
and outside a text lane's fence, so it is reported, not made.

## Criterion 2 — **NOT MET → MET-WITH-STATED-EXCEPTIONS**

The grade above reads *"Nothing is published. Readiness at HEAD is exactly as dishonest as it was
before this phase ran."* and names a new HIGH: the browser tool's own remediation text pointing at
`[browser] allowed_origins` when the loader reads `[browser.policy] allowed_origins`, so following
it verbatim leaves the tool disabled.

**Both halves are closed at `02575b6f`.** The remediation-string defect is not only fixed but
machine-guarded — `crates/wcore-cli/tests/remedy_advertisements.rs` carries it as row 1 of a census
(`:14`, *"loader reads `browser.policy.*`; the key parsed cleanly and was **silently discarded**"*)
with an explicit detectability assertion at `:760`, and `crates/wayland-browser/src/plugin.rs:49`
now documents the correct `[browser.policy] allowed_origins` form. Readiness is linkage-derived
rather than hard-coded `true`.

`CRITERIA-STATUS.md` additionally records the three policy baselines closed on **two** platforms,
and — the more valuable line — that the macOS half of baseline 3 was previously
`#![cfg(target_os = "linux")]`, compiling to an empty harness that printed
`test result: ok. 0 passed` and exited **0**. **This phase's Criterion 2 therefore contained one
gate that could not pass and one that could not fail, at the same time.** Those are the same bug
wearing different colours, and this file is the place that fact belongs.

**Exceptions, stated not embedded:** Windows is NOT MEASURED for all three baselines; baseline 2's
real-desktop half has no macOS twin (writing one posts real HID events to the machine Sean is
using — a deliberate non-attempt, recorded as a gap, not a pass); baseline 3c is `1 ignored` on
macOS; and the *"must land inside the downloads root"* half is **vacuous in the shipped product**
because no backend implements `Download`.

## The "successor should do first" list is superseded

1. *"Land SR-27-1..3"* — the seam is landed; readiness is published.
2. *"Fix `wcore-browser/src/tool.rs:499`"* — done, and regression-guarded by
   `remedy_advertisements.rs`.
3. *"Run the voice interruption on `seandesktop`"* — **still the right instruction, and now more
   urgent than when it was written**, because the surface ships. It is no longer *"the only unmet
   criterion with no partial credit"*; it is a shipped capability with an unproven handoff.

## What this block does not claim

Criteria 1, 3 and 5 were not re-derived by this lane. In particular, `27-C1` carries an open
macOS-only HIGH (`F-M1-01` / #937, `media_intake::open_once` refusing every path the platform's
own temp APIs hand out) **plus two negative arms that are vacuous on macOS**, and `27-C3`'s
late-MCP shape is still not exercised. Absence from this block is **not** a clean bill of health.

_Corrected 2026-08-01 · base `02575b6f` · lane `verdict-truth-text` · source measurement only,
two-directional controls, no cargo, no `crates/` edit._
