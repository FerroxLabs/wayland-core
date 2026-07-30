# RELEASE-RANK NOTES — live investigation log

Lane `release-rank`. Branch `lane/release-rank`, forked from integration head
`8955ee6e43d2a6bd6ede0a522eb19cd2eddaaad7`.

**Mandate:** re-rank `CRITERIA-GAP-LEDGER.md` §3 (MUST CLOSE / CAN SHIP OPEN) measured at HEAD.
The existing §3 was written by `lane/criteria-gap` at `873cc389` and is stale.

**Method constraint accepted up front:** grade off the **code and tests at HEAD**, never off a
`SUMMARY.md`. Where a summary and the source disagree, the source wins and that is stated.
Every gate run in **both directions** (LANE-BRIEF §3b-iii): show the instrument can fail *and*
can pass. All load-bearing greps via `/usr/bin/grep`; all git via `/usr/bin/git`.

**This lane modifies no `.rs` file.** Measurement + documentation only.

---

## Status: IN PROGRESS

## Hypotheses handed to me (orchestrator's own words: "probably stale")

| # | §3 item | Claim | Verdict |
|---|---|---|---|
| 1 | 1 / `24-C2` | 3 of 8 trigger kinds can never fire; "worst failure mode in the ledger" | TBD |
| 2 | 2 / `27-C2(a)` | `[browser]` → `[browser.policy]` remediation string | TBD |
| 3 | 3 / `24-C5`+`24-C1` | `24-C5` MET; `24-C1` PARTIAL, no-loss failing 9/10 adapters | TBD |
| 4 | 4 / `23A-C1` | MET on shipped surface, no longer blocking | TBD |
| 5 | 5 / `27-C2(b)` | closed by liveness probe (NOT `bootstrap.rs:754`, which is `true` forever) | TBD |
| 6 | 6 / `24-C3` | still genuinely NOT MET | TBD |
| — | CAN SHIP OPEN | anything moved the *wrong* way; specifically `27-C3` cost record | TBD |

## Measurements taken so far

(none yet — this file committed at minute ~10 per LANE-BRIEF §6b-i so that a mid-run death
resumes from here rather than from zero)
