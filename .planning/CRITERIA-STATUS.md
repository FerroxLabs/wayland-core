# CRITERIA STATUS — one line per criterion, measured 2026-07-30

**All grades measured at `71acfd19` by `lane/ledger-regrade`, with a control in both directions per
`LANE-BRIEF.md` §3b-iii** (can it fail, *and* can it pass). Full evidence and the superseded text
live in `CRITERIA-GAP-LEDGER.md`; the dated correction blocks there are authoritative over the
`####` headlines, which are deliberately left unedited by that file's convention.

**This file exists because the headlines misled.** Of 18 rows, **11 were stale** and 2 were graded
off instruments that could never pass.

> **The re-grade that produced this file was itself caught by the same defect.** It graded `27-C2(b)`
> "unchanged" off `bootstrap.rs:754`, a line that reads `true` forever, while the actual fix
> (`85b60a2f`, *"advertise browser/CUA capabilities on liveness, not linkage"*, 2026-07-28) was
> **already in its own ancestry**. Readiness is published 187 lines later via
> `PluginCapabilitySet::from_verified(..).narrowed_to_live()`, which runs real liveness probes.
> Found by `lane/27-c2b-readiness`, verified independently. **No audit is immune to the failure mode
> it is auditing for** — which is the strongest argument there is for the both-direction control.

| Criterion | Grade | One-sentence justification |
|---|---|---|
| `21-C3` | **NOT MET** | Tool *live* cells remain open and Windows is unmeasured; enforcement is equivalent by construction, so this is a proof gap, not an enforcement hole. |
| `22-C1` | **PARTIAL** ↑ | Typed Goal *control* landed on all three surfaces (5 commands + a typed refusal event); the producer-fixture clause is not closed. |
| `22-C3` | **PARTIAL** ↑ | Half A advanced — the last representable engine-verdict bypass is shut at the durable boundary for 5/5 owners and any sixth; un-goaled invocation stays opt-in. Half B closed, pre-existing. |
| `22-C4` | **PARTIAL** | One measurement moved; the gap did not. |
| `22-C5` | **PARTIAL** | The row is accurate as written. |
| `23A-C1` | **MET** (shipped surface) ↑↑ | **Moved further than any other row — both its earlier texts are now false. No longer release-blocking.** |
| `24-C1` | **PARTIAL** ↑ | The platform half is closed; the conjunction *"no delivery lost **and** none duplicated"* is not — no-loss fails on **7** of 10 adapters (this row said 9; corrected 2026-07-30 by `lane/24c1-declaration`, re-verified by `lane/24c3-channels`). Exactly-once is 3 of 10 — Slack, Matrix, Discord — and is now **declared** in `docs/delivery-semantics.md` with a per-cell citation and a drift test that fails the build if the doc and the code disagree. **Exactly-once is scoped to a delivery id, not a message** — this row previously read that `F24-GWP-H1` (a second scheduler firing mints a fresh id) therefore *defeats* it on all three, and **that is wrong**: a second scheduled occurrence is a NEW delivery id, each of which was delivered exactly once, so the three rows hold. What the scoping defeats is the reading *"exactly-once means the customer gets one message"*, which `docs/delivery-semantics.md` §4 and §5 now state directly. `F24-GWP-H1` is REFUTED (`lane/gwp-h1-duplicates`, 5 of 5 keyed jobs distinct, zero replays) and its gate is repaired (`lane/journey-gate-honesty`). |
| `24-C2` | **PARTIAL** | Grade unchanged, **but the sentence that made this the ledger's number-one release blocker is no longer true** — §3 item 1 must be re-ranked. |
| `24-C3` | **NOT MET** | Work landed, **the implementing lane declines to claim the criterion**, and a new HIGH is open and unfixed. |
| `24-C4` | **MET-WITH-STATED-EXCEPTIONS** ↑ | Was "MET on Linux / HTTP+SSE only"; the exceptions are now stated rather than embedded in the grade. |
| `24-C5` | **MET** ↑↑ | **The most stale row in the ledger — all three of its claimed absences are false at HEAD. No longer release-blocking.** Driver, receipt schema and three-platform receipts all exist. |
| `25-C2` | **MET** (as written) ↑ | Carries a recorded **dissenting reading**, deliberately carried forward rather than resolved. |
| `25-C4` | **PARTIAL** ↑ | The row's named unmet clause is **closed**; two open items it never knew about take its place. |
| `27-C1` | **PARTIAL** | Grade unchanged, **but the row's RED gate is now GREEN** — that sentence must not be read forward. |
| `27-C2` | **PARTIAL** ↑↑ | (a) and **(b) both CLOSED — see the 2026-07-30 late correction; (b) was already fixed on 2026-07-28 and the re-grade could not see it.** Only **(c)**, the three policy baselines, remains, and **two of its three legs are blocked on a display-capable host**. |
| `27-C3` | **PARTIAL** ↑ | `F-27C3-04` (image tool broken by default on FluxRouter) fixed and live-proved through `ProviderCompat`. |
| `27-C4` | **NOT MET** | Grade survives **for a different reason than the row states**: its "nothing was exercised" sentence is false (live capture at ratio 116.66 vs a 1.15 control; barge-in proven against the real player), but `voice` is absent from every `default` list, so the feature is not in the shipped artifact. |
| `27-C5` | **PARTIAL** ↑ | Three packaged smokes ran on real macOS/Linux/Windows — 8 PASS / 1 RED, byte-identical on all three. **MET for the shipped release, NOT MET for the candidate**; two aarch64 targets are NOT MEASURED (neither zero nor passing). |

## Not in this ledger

`26-SC2` has **no row here** — §5 declares Phases 26/28/29/30 out of scope, and `lane/ledger-regrade`
**refused to create one**, on the grounds that inventing a row would misrepresent the file's declared
scope. The work is real and recorded in `26-SC2-PEERS-SUMMARY.md`: peer coverage **2 of 4 → 4 of 4**.

## Two rows were graded off instruments that could never pass

- `22-C3` — its falsifier grepped a directory the adapter does not live in, so it reported FAILED
  **forever**. Exposed only by a known-positive in the same directory returning 21 hits.
- `27-C1` — the row's RED gate is now GREEN and the row still reads RED.

**A permanently-red gate proves as little as a permanently-green one.** See `LANE-BRIEF.md` §3b-iii.
