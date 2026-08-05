---
phase: 27-multimodal-browser-generation-voice
plan: "04"
subsystem: corpus-pinning-and-packaged-smokes
tags: [packaged-smoke, three-platform, not-run]
status: incomplete
termination_state: not-started
requires: ["27-01", "27-02", "27-03"]
provides:
  - "One pinned, digest-carrying corpus (the intake half only)"
affects: []
tech-stack:
  added: []
  patterns: []
key-files:
  created: []
  modified: []
decisions: []
metrics:
  completed: 2026-07-26
---

# Phase 27 Plan 04: Corpora and Packaged Smokes Summary

**This plan did not run.** One of its three tasks is partially satisfied as a
by-product of 27-01. The other two were not started.

## What exists

**A pinned corpus, for the intake class only.**
`crates/wcore-fixture-harness/fixtures/f27/intake/` holds 18 entries plus
`MANIFEST.tsv` carrying each entry's byte length and SHA-256. It is generated
by `.planning/scripts/f27-build-intake-corpus.py` from pinned bytes — nothing
downloaded, nothing random — so a regeneration on any platform produces
identical bytes. That satisfies the determinism property Task 1 asks of a
corpus.

It does not satisfy Task 1. The browser, generation and voice corpora were
never built, and no corpus-driven suite consumes any of them.

## What does not exist

| Task | Status |
|---|---|
| Task 1 — one corpus-driven suite plus two packaged smokes running identically on all three platforms | **NOT RUN.** No suite. No smoke. No platform. |
| Task 2 — seal a candidate, measure the extracted-binary claim on the Linux box, put a captured three-platform dry run to the four-way panel, authorize one dispatch | **NOT RUN.** No candidate was sealed, no dry run was captured, and no panel was convened for this plan. |
| Task 3 — dispatch once, launch the packaged artifact natively on macOS, Linux and Windows, certify with every gap named | **NOT RUN.** No dispatch. No packaged artifact was launched on any platform. |

## Why, stated plainly

The plan's whole shape is: seal what 27-01, 27-02 and 27-03 produced, prove it
identically on three platforms from the SHIPPED PACKAGE, and certify. Two of
its three inputs did not land — 27-02 stopped at a fenced seam and 27-03 never
exercised voice — so there was no coherent candidate to seal.

That is an explanation, not a defence. Criterion 5 asks for packaged smokes on
native macOS, Linux and Windows. **Zero of the three ran.** The Linux
measurements throughout this phase were taken from a `cargo build --release`
binary in a build tree, which is not a packaged artifact and must not be
counted as one.

Both other platforms were reachable and unused: `ssh SeanD@seandesktop`
answered `WIN_OK` at the start of this work, and this Mac was available
throughout.

## Requirements

**F27-05 is explicitly INCOMPLETE.** Every clause is unmet: no packaged smoke
ran on any platform; no deterministic corpus drives any suite; no candidate was
sealed; no dispatch was authorized or fired.

## What a successor needs

1. Land `.planning/SEAM-REQUESTS/27.md` (SR-27-1..3) so 27-02's decision can be
   implemented and there is something worth sealing.
2. Run 27-03's voice interruption on `seandesktop`, which has audio, a
   toolchain, and was reachable.
3. Then, and only then, seal one candidate and run Task 2 and Task 3 as
   written.
