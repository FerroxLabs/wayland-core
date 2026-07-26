---
phase: 27-multimodal-browser-generation-voice
plan: "01"
subsystem: attachment-and-document-intake
tags: [intake, toctou, magic-bytes, provider-compat, vision-gate]
status: complete
termination_state: 3
termination_state_name: "Partially refuted"
requires: []
provides:
  - "wcore_tools::media_intake — one bounded, open-once, magic-byte-validated intake"
  - "crates/wcore-fixture-harness/fixtures/f27/intake/ — 18-entry pinned corpus with digests"
  - "supports_vision honoured by the Anthropic and Gemini message builders"
affects:
  - crates/wcore-tools/src/pdf_tool.rs
  - crates/wcore-providers/src/anthropic_shared.rs
  - crates/wcore-providers/src/gemini.rs
tech-stack:
  added: []
  patterns: [open-once-then-read-by-descriptor, magic-byte-admission, typed-refusal-reasons]
key-files:
  created:
    - crates/wcore-tools/src/media_intake.rs
    - crates/wcore-fixture-harness/fixtures/f27/intake/
    - .planning/phases/27-multimodal-browser-generation-voice/27-01-INTAKE-AUDIT.md
  modified:
    - crates/wcore-tools/src/lib.rs
    - crates/wcore-tools/src/pdf_tool.rs
    - crates/wcore-providers/src/anthropic_shared.rs
    - crates/wcore-providers/src/gemini.rs
decisions:
  - "supports_document_input was NOT added: ContentBlock has no document variant, so the field would gate nothing"
  - "The composer path was NOT rewritten through media_intake: it was measured already correct"
metrics:
  completed: 2026-07-26
---

# Phase 27 Plan 01: Attachment and Document Intake Summary

Measured four intake paths through the shipped binary on real hardware, refuted
three of the plan's four premises, and closed the two divergences that were
real — a PDF path that let a third party re-resolve the caller's filename, and
two provider message builders that ignored the vision capability gate entirely.

## Termination state

**State 3 — partially refuted.** Some divergences were real and some were not.
Only the CRITICAL/HIGH ones the audit REACHED through the binary were closed.

## What was measured, and where

- Host `hetzner-dsm`, phase-dedicated worktree `/root/wayland-p27`, SHA
  `2ecdfdf54ff7fda920eec7d068337006e5da4ee4`.
- `target/release/wayland-core`, `cargo build --release --locked -p wcore-cli`.
- Instrument: `.planning/scripts/f27-mock-provider.py`, a recording provider
  that captures every outbound request body verbatim. Without it the
  degradation question is unanswerable except by reading source.

Full detail: `27-01-INTAKE-AUDIT.md`. Ledgers: `evidence/27-01/OBS-LEDGER.tsv`
(12 rows) and `evidence/27-01/LIVE-LEDGER.tsv` (9 rows).

## The two divergences that were real, and are closed

**D1 — the vision gate did not exist on two builders. HIGH.** The same PNG,
driven through the shipped binary twice with `supports_vision = false` and
then `= true` as the only variable, produced a **byte-identical outbound
request** carrying an inline `image` part in both cases. OpenAI, Bedrock and
Cohere substitute `[image omitted: model not vision-capable]`; Anthropic and
Gemini did not. Both now gate on the same field with the same wording, named
once as `VISION_OMITTED_PLACEHOLDER`.

**D2/D3/D4 — the PDF path re-resolved the name and had no ingest bound. HIGH.**
Settled by syscall trace, not by inference. `strace -f -y` around the shipped
binary counted **three by-name resolutions** of the caller's path followed by
an `openat` issued by `pdf_extract`, which had received only the path and never
saw a validated handle. The composer path, traced identically, showed two
by-name resolutions, one `openat`, and every later fact taken by descriptor.
The PDF tool now goes through `media_intake::admit_path` and calls
`extract_text_from_mem`, so the extractor never sees a path. It also gains
`MAX_PDF_INGEST_BYTES`, enforced from the descriptor's metadata before a
payload byte is read — previously the only size discipline was
`MAX_PDF_TEXT_BYTES`, which caps RETURNED text and therefore fires after a full
parse.

## The three premises that were refuted

- **The composer path is already correct.** Open-once, bounded at both ends,
  cross-checks extension against detected bytes, and reports every refusal to
  the USER as a wire-level `error` event. The `VISION_MIN_BYTES` boundary is
  exact: 15 bytes rejected, 16 bytes admitted. Seven corpus entries, seven
  correct outcomes, clean exit every time. **Nothing was changed.**
- **The PDF mismatch case was already safe.** A `.pdf` whose bytes are not a
  PDF was already refused and the user already saw it — by the parser, after
  ingestion, blaming extraction. The mechanism was wrong; the outcome was not.
- **Documents are not silently dropped at the provider boundary.**
  `ContentBlock` has five variants and none of them is a document. A PDF or
  office document crosses as `ToolResult { content: String }`, which every
  provider carries — confirmed end to end by the sentinel arriving in a captured
  outbound body.

## The plan instruction I did not follow, and why

The plan directed adding **`supports_document_input`** to `compat.rs`, and one
of its gates asserts that identifier appears there at least twice. **It was not
added, and that gate is RED.**

A provider-capability field gates a wire construct. There is no document wire
construct to gate. The field would have been a gate that can never fire —
precisely the self-passing-gate shape this phase's own rules forbid. Building it
to satisfy a gate would have been building a decoration.

Two other gates are RED for a related reason: the plan requires `media_intake`
to be referenced from `attachments.rs` and `channel_media.rs`. Both paths were
MEASURED already correct, and rewriting a measured-correct path to satisfy a
structural check would spend risk on a defect that does not exist. **The
chokepoint is real but partial: the PDF path routes through it; the composer
and channel paths use the same mechanism, duplicated rather than shared.**

## Test evidence

`cargo nextest run -p wcore-tools -p wcore-providers --no-fail-fast` on
`hetzner-dsm`:

| | run | passed | failed | skipped |
|---|---|---|---|---|
| before | 2130 | 2128 | **2** | 3 |
| after | **2132** | **2132** | **0** | 3 |

The two intermediate failures were the wording pins described below; both pass
on strengthened assertions. Fifteen tests were added (thirteen in
`media_intake`, two in `pdf_tool`).

`cargo clippy --workspace --all-targets --all-features -- -D warnings`: **clean,
exit 0.** `cargo fmt --all -- --check`: clean.

Full-workspace `nextest --profile ci` was **NOT** run.

## Deliberate wording changes

`corrupt_non_pdf_file_returns_error` previously asserted the message contained
"Failed to extract text". A non-PDF is now refused from its header before the
parser sees a byte, so the message names the real cause. The assertion GAINED a
clause (it now also asserts the extractor was never reached) rather than losing
one. `missing_file_returns_error` is **unchanged** — a dedicated
`IntakeError::NotFound` variant was added specifically so the "not found"
wording a host may match on survives unification.

No test was deleted, `#[ignore]`d, `#[allow]`ed, re-gated, or had a timeout
raised. No accepted-format set was widened. No cap was loosened.

## Seams

None touched. `crates/wcore-protocol/`, the desktop manifest,
`wcore-config/src/config.rs`, `.github/workflows/`, `Cargo.toml` and
`Cargo.lock` are all clean. `compat.rs` was not modified either, for the reason
above.

## Not run

- **PTY terminal drive** — `evidence/27-01/live/NOT-RUN-pty.txt`. The TUI half
  of Criterion 1 is unproven.
- **Native macOS leg** — `evidence/27-01/live/NOT-RUN-macos.txt`. No macOS
  artifact exists for this SHA because the branch was never pushed. No Linux
  result is presented as a macOS result anywhere.
- **Re-traced syscalls against the changed binary.** The repair removes the
  path-taking call, which is verifiable from the diff, but the trace-level
  re-proof was not taken.

## Requirements

**F27-01 is NOT marked complete.** Two of its clauses are evidenced (one
bounded validated intake for the document path; explicit degradation on
unsupported providers, proved live on the image class). Two are not: the
"one intake path" clause is only partially true, and the host-message half was
proved on the wire but never in the terminal.

Commits: `9f885cbf` (source), `47a5dd09` (audit + evidence).
