# 24-MEDIA-ACTIONS — running NOTES (append-only)

Lane: `lane/24-media-actions`. Base: `e77b44b0` (`plan/f20-unified-audit-repair` at fetch time).
Worktree: `/Users/seandonahoe/dev/waylandcore-frontier-worktrees/lane-24-media-actions`.

Committed at T+~12 min per LANE-BRIEF §6b-i, before any investigation, and re-committed after
every measurement.

---

## T+12 — criterion text located at source (not a paraphrase)

`.planning/ROADMAP.md:119`, Phase 24 Success Criterion 3:

> Reference channels prove setup/auth, access, routing, **media**, **native actions**,
> idempotency, reconnect/reload, and health.

Eight clauses. Per the lane assignment and `24-C3-FINISH.md:99`, **`media` and `native actions`
have never been measured on ANY adapter** across six consecutive lanes. Every other clause has at
least one adapter measurement.

`.planning/CRITERIA-GAP-LEDGER.md:311-325` grades `24-C3` **PARTIAL (Linux), NOT MET (macOS,
Windows)** and calls it partially release-blocking.

## T+12 — what I must establish BEFORE building anything

1. What does the criterion's `media` clause actually promise? `README.md:348` reportedly discloses
   image->description and voice->transcript are inert without a vision/transcription key. A key
   exists at `~/.wayland-secrets/flux.env` (LANE-BRIEF §0 sanctioned: stdin only, sweep after,
   disclose). So the "no key" excuse may no longer hold — CHECK, do not assume.
2. What is a `native action` on an INBOUND channel? Which adapters expose one, and what does
   exercising it look like? If few/none do, that is a finding about the clause.

## T+12 — traps I am holding (from LANE-BRIEF)

- §3.2 a green from universal denial: the access leg once passed on 3 adapters because everything
  was DENIED. Prove positives with counts.
- §3b-i a known-negative is self-passing on a dead instrument. If I report "no media events", I
  must first prove the instrument can SEE one (known-positive in the same invocation).
- §3b unproxied tools for any number that reaches the report: `/usr/bin/grep`, `/usr/bin/git`.
- §6b-ii repair my own instrument in-lane; self-test with 3 assertions incl. "old matcher misses".
- Byte-count every capture; `${PIPESTATUS[0]}` returns empty here.
- `wcore_types::process_liveness` exists — use it, do not hand-roll a liveness check.
- Do NOT edit `scripts/f24-inbound.mjs` (shared, in active use by other live lanes).
  Off-limits: `.github/workflows/ci.yml`, `crates/wcore-cli/src/{lib,main}.rs`,
  `.planning/BACKLOG.md`, `crates/wcore-channel-matrix/` (lane/24-h6 owns it).

## Status

NOTHING MEASURED YET. Investigation starts after this commit.
