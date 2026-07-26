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

## What a successor should do first

1. Land `.planning/SEAM-REQUESTS/27.md` SR-27-1..3. Criterion 2 is one small
   patch away from evidenced once the seam is open.
2. Fix `wcore-browser/src/tool.rs:499` — a two-word change to a string that
   currently sends every user in a circle.
3. Run the voice interruption on `seandesktop`. It is the only unmet criterion
   with no partial credit at all.
