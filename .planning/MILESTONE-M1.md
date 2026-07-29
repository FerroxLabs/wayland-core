# MILESTONE M1 — Close the gaps, state the position

**Proposed 2026-07-29.** Supersedes `MILESTONE-RC.md` for planning. Scope: every gap measured by
the 2026-07-29 grading wave — the first time all eleven phases were graded — plus peer parity.

**Size: ~55–65 lane-sessions. Target: 3 working days, not 3 weeks.** The rest of this document is
about why that is achievable and what has to change to make it so.

---

## 1. Why the last two weeks were slow, measured

Today produced 20 completed lanes. The work was never the constraint. Four things were:

| bottleneck | cost | fixed? |
|---|---|---|
| **CI dead** — 4 clippy lines, then queue starvation, then 1 more clippy line | 4 days of blind building | yes, three times over |
| **Zombie runs** — a merged lane's run held macOS runners for **10.5 h**, re-acquiring them as they freed | 4 h 20 m of blocked Darwin | yes — cancel-at-merge is now procedure |
| **Merge serialisation** — every lane merged one at a time through the orchestrator | ~5 min × 20 lanes of context | **no — fix in §3** |
| **Five phases never graded** | days spent on `24-C3` while the real blocker sat inside a criterion marked MET | yes, graded today |

**The lesson that generalises:** the lanes measured honestly and graded themselves generously.
Nobody read the disclosures back against the criterion text. **Aggregation, not measurement, was
the missing job.**

---

## 2. What changed today that makes speed possible

1. **CI produces verdicts again**, and Darwin work can route to `sean-mac-arm64` — a real
   Apple-silicon runner with **detected** labels, not asserted ones.
2. **Every phase is graded.** We know the gaps by name, cost and dependency.
3. **The flake was never real** — 40 runs, zero real test failures. A green can be trusted.
4. **`0 of 25` prior grades were instrument artifacts.** Nothing gets re-litigated.
5. **Voice is not hardware-blocked.** The Mac has a microphone and is a registered runner. That
   was a wrong host, mis-recorded as a wrong-hardware constraint.

---

## 3. How we go fast — the four mechanics

**(a) Fan out to 12–16 lanes, not 6–8.** Measured today: across 20 lanes, **fence conflicts were
approximately zero** — most touch no shared file, and `wcore-cli/src/{lib,main}.rs` diffs came back
empty from lane after lane. The parallelism ceiling was never contention; it was orchestrator
attention.

**(b) Merge trains, not merge queues.** Batch 5–8 completed lanes into one verify-and-merge pass
instead of one at a time. Same verification per lane, a fraction of the overhead.

**(c) Cancel each lane's CI runs when merging it.** A merged lane leaves a live run that keeps
winning runner capacity. This cost us 4 h 20 m today and is now part of the merge step.

**(d) Land `ci-selfhosted-mac` first, in W0.** Until the arm64 build routes to the Mac, every wave
competes for GitHub's hosted macOS pool, which is where all three CI stalls originated.

---

## 4. Waves — sequenced by dependency, not by phase

### W0 — Merge debt (≈1 h, blocks everything)
Land the 18 finished branches. `ci-selfhosted-mac` **first**, then the rest as one train.
Cancel each lane's runs while merging. One green CI run over the merged tree.

### W1 — Media assault (parallel, ≈8 lanes)
`MEDIA-*` is at **`SOURCE`**, our weakest family, and the most visible thing a user touches.
One bounded intake path (27-C1, still RED) · media generation + MCP fixture (27-C3) ·
voice on the Mac (27-C4) and the ship-by-default decision · capability **liveness** (a `true` is
still granted on `/bin/true` and a dead `DISPLAY`) · `SR-27-1` host-visible narrowing reason ·
migration import + `F26-GRADE-H1` + `peer_skill_roots`.

### W2 — Criteria closure (parallel, ≈8 lanes)
22-C3 five engines → one Goal transition · 22-C1 third surface · 22-02 Task 3 (two capability rows
still "runtime path unwired") · 23B-C3 memory outbound proof · 23B-C4 cache/compaction (**never
started**) · 23B Windows session driver · 21-C3 hostile-corpora equivalence · 24-C3 eight clauses
on macOS/Windows · 24-C2 webhook + poll · 25 cloud-cancel / ssh-cleanup / egress-deny ·
26-C3 migration rollback.

### W3 — Position (parallel, ≈6 lanes)
Amend the frozen protocol for absolute paths → correctness and recovery become **publishable** ·
replace the broken cost observable (invariant across harnesses today) · build the security and
cognitive-tax trials, which **do not exist** · provision OpenClaw · add **grok** to the ledger, our
only same-language peer and never examined · re-pin both peers, 16 days stale.

### W4 — Ship
Supply-chain closure · `BL-F28-VACUOUS-GREENS` (44 binaries reachable by bare `cargo test`) ·
second contract regeneration **as the last action before the tag** · RC cut.

---

## 5. Standing rules for every lane in M1

Earned today, each from a real failure:

1. **A known-negative assertion is self-passing on a dead instrument.** Five instances found today,
   including a redaction proof where `grep -c` on a **missing file** returned `0` — and `0` was the
   success value. It had already fired on a run where the export failed.
2. **Assert the artifact exists and is non-empty before asserting what is not in it.**
3. **Read the selection back from the product's own output.** `/root/.wayland/.env` on hetzner
   injects `ANTHROPIC_API_KEY` regardless of shell `unset` (`LANE-BRIEF` §3b-ii).
4. **Instruments can produce false positives, not just false negatives.** Two lanes today caught
   their own harnesses filing defects against correct product behaviour — one a false security
   escape.
5. **Grade against the criterion text, not against your own scope.** This is the aggregation
   failure that cost the programme two weeks.

---

## 6. What stays Sean's

Merge to main · tag / publish · core#254 reply · close #142 · macOS CI dispatch — **the only
credential-gated item left in the entire programme.**

Everything else in M1 is buildable now, with hardware and credentials already in hand.
