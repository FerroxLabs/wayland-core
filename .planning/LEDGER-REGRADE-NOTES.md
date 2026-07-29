# LEDGER-REGRADE — working notes

**Lane**: `lane/ledger-regrade`
**Base**: `gh/plan/f20-unified-audit-repair` @ `71acfd19258e0fc7484d80a0a95be3f29d0ee2b5`
**SHA asserted** against `/usr/bin/git ls-remote gh plan/f20-unified-audit-repair` — match.
**Started**: 2026-07-30

## Mandate

Re-measure every `#### <criterion>` row in `.planning/CRITERIA-GAP-LEDGER.md` against HEAD,
correct headline grades where evidence moved, flag stale correction blocks, and name every
row whose falsifier cannot pass in any achievable world. Produce `.planning/CRITERIA-STATUS.md`.

**Product is an accurate record. No product source file is changed by this lane.**

## Row inventory (19 `####` headers, 18 distinct criteria)

| # | line | Row | Headline as written |
|---|---|---|---|
| 1 | 54 | 21-C3 | NOT MET |
| 2 | 127 | 22-C1 | FAILED (one surface of three) |
| 3 | 170 | 22-C3 | FAILED (measured, not built) |
| 4 | 235 | 22-C3 (2nd header, correction) | PARTIAL, not FAILED |
| 5 | 274 | 22-C4 | PARTIAL |
| 6 | 297 | 22-C5 | PARTIAL |
| 7 | 324 | 23A-C1 | NOT MET |
| 8 | 388 | 24-C1 | NOT MET (re-graded) |
| 9 | 490 | 24-C2 | PARTIAL |
| 10 | 547 | 24-C3 | PARTIAL (Linux), NOT MET (macOS, Windows) |
| 11 | 563 | 24-C4 | MET on Linux / HTTP+SSE only |
| 12 | 582 | 24-C5 | NOT MET (no evidence, any platform) |
| 13 | 616 | 25-C2 | NOT MET |
| 14 | 647 | 25-C4 | NOT MET |
| 15 | 678 | 27-C1 | PARTIAL |
| 16 | 701 | 27-C2 | NOT MET |
| 17 | 740 | 27-C3 | NOT MET |
| 18 | 764 | 27-C4 | NOT MET |
| 19 | 813 | 27-C5 | NOT MET |

`22-C3` occupies two `####` headers (rows 3 and 4) — the 2026-07-29 correction was given its own
header rather than being nested, which is why the file has 19 headers for 18 criteria.

`26-SC2` is named in the orchestrator's movement list but **has no row in this ledger** — §5
states Phases 26/28/29/30 were deliberately out of scope. To be verified, not assumed.

## Method

Every measurement gets a control **in both directions** (LANE-BRIEF §3b-iii):
- known-positive: an instrument that reports a hit, proving it is alive;
- known-negative / can-it-pass: the state that would make the check flip, and whether that state
  is achievable at all.

All load-bearing commands via `/usr/bin/grep`, `/usr/bin/git` (LANE-BRIEF §3b — `rtk` rewrites
`grep`, `git log`, `ls`, `cargo`, `git status --porcelain`).

## Progress log

- [x] Worktree created, SHA asserted.
- [x] LANE-BRIEF read in full (§3b-iii and "your brief's MEASUREMENTS are probably stale" noted).
- [x] Ledger read in full; row inventory above.
- [ ] Per-row measurement.
- [ ] `.planning/CRITERIA-STATUS.md`.
