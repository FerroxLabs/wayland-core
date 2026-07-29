# 24-GRADE-NOTES — working notes for the Phase 24 verdict

Lane `grade-24`. Worktree `/Users/seandonahoe/dev/waylandcore-frontier-worktrees/lane-grade-24`,
branch `lane/grade-24`, base `861d1b1a`.

Append-and-recommit after every measurement (LANE-BRIEF §6b-i). This file is the resume point.

---

## T+0 — established facts

**Confirmed: Phase 24 has NO verdict file.** Search over `.planning/phases` for `*VERDICT*`
returns verdicts for 21, 22, 27, 28, 29, 30 — and nothing for 23A, 23B, 24, 25, 26. That is the
five-of-eleven gap named in the brief; 24 is one of them.

Instrument note: that is an ABSENCE claim, so per §3b-i it needs a live-instrument proof. The
same `find` in the same invocation returned 10 positive hits (`27-PHASE-VERDICT.md` etc.), so the
instrument was alive. Query recorded verbatim below in the measurement log.

**The five Success Criteria, verbatim from `.planning/ROADMAP.md` (Phase 24 section, line 112ff):**

> **Goal**: Operators can install, run, automate, connect, inspect, recover, and support one
> persistent Core runtime on every OS family.
>
> 1. Native service lifecycle, profile isolation, active-turn visibility, drain, restart, upgrade,
>    and rollback work without lost or duplicate delivery.
> 2. Scheduled, event-driven, webhook, polling, and commitment work has bounded history, retry,
>    continuation, and delivery.
> 3. Reference channels prove setup/auth, access, routing, media, native actions, idempotency,
>    reconnect/reload, and health.
> 4. Typed authenticated clients recover event gaps and produce useful redacted health/log/support
>    evidence.
> 5. Setup-to-recovery journeys pass on macOS, Linux, and Windows.

**Grade vocabulary:** MET / MET-WITH-STATED-EXCEPTIONS / PARTIAL / NOT MET.

---

## Method I am committing to before I look at the evidence

Stating this first so the grade cannot be reverse-engineered from what I find.

1. **Re-derive, never inherit.** `RC-READINESS.md` and `MILESTONE-RC.md` are declared partly stale
   and one holds a superseded section. I read them for pointers to primary evidence only; every
   number in the verdict comes from the SUMMARY/evidence file that produced it, or from a
   measurement I take.
2. **C3 is graded clause-by-clause.** The criterion names eight clauses (setup/auth, access,
   routing, media, native actions, idempotency, reconnect/reload, health). A criterion made of
   eight conjuncts cannot be graded as one blob. Each clause gets its own status, its own adapter
   coverage, and its own platform coverage. The criterion grade is then the floor of the clauses,
   not their average.
3. **Two instruments are known bad and both bear on this phase:**
   - nextest "flakiness" here was **fd/inotify exhaustion**, not real failure — 40 runs, zero real
     failures. So a red attributed to flakiness is not automatically a defect, AND a green taken
     under contention is not automatically a pass.
   - `no-tests = "fail"` is **silently ignored** by the installed nextest. A green suite may have
     executed nothing. Any criterion resting on "the suite is green" is downgraded explicitly and
     the `N passed` count must be read back.
   - Corollary already burned into §3b-i: a **known-negative assertion is self-passing on a dead
     instrument**. C3's strongest new evidence includes "zero advertised-but-dead" and
     "DIVERGENT=0" and "lost=0 duplicated=0" — every one of those is a negative. I must check each
     had a live-instrument / positive-control proof, or downgrade it.
4. **Merged vs pending is a first-class distinction.** Two lanes' evidence (`native-actions`,
   `e2e-product-smoke`) is on unmerged branches. Unmerged work is real evidence of capability but
   it is NOT in the release candidate. I will grade twice where it matters: as-merged, and
   as-if-pending-lands.
5. **Absences get their query recorded** (§3b-i.4).

## Grading stance

An inflated grade costs a customer who trusts a false claim; a deflated one costs weeks rebuilding
what works. Neither is safe. So the tiebreak is not "be conservative" — it is **be specific**: name
the clause, the adapter, and the platform, so a reader can see exactly how much is true rather than
reading a single word that is wrong in both directions.

---

## Measurement log

### M0 — verdict-file absence (T+0)

```
/usr/bin/find .planning/phases -name "*PHASE-VERDICT*" -o -name "*VERDICT*"
```
10 hits, none under `24-*`. Instrument alive (positive hits present in same run).

### M1 — evidence inventory for Phase 24 (T+0)

`.planning/phases/24-gateway-automation-channels-typed-api/` holds 18 evidence directories and
~40 markdown artifacts including 4 numbered PLAN/SUMMARY pairs (24-01..24-04), plus lane
artifacts for B (gateway-surface), C (arrival), C1, C2, C3 (+DISCORD/FINISH/H2/H4/TG-EMAIL), C5
(+FINISH), CHANNEL-LEASE, CHANNEL-STARVATION, EMAIL-MSTEAMS, H5, H6, MATRIX-SIGNAL,
MEDIA-ACTIONS, MEDIA-BOUNDS, MEDIA-LIVE, MSTEAMS-ATTACH, RECONNECT, and a `24-PHASE-REPORT.md`.

Still to establish (in order):
- [ ] read `24-PHASE-REPORT.md` — what it already claims and whether it is a verdict in disguise
- [ ] C1 from primary evidence (24-01, CHANNEL-LEASE, H5/H6)
- [ ] C2 from primary evidence (24-02, CHANNEL-STARVATION)
- [ ] C3 clause matrix across adapters × platforms
- [ ] C4 from primary evidence (24-03, 24-04, OPENAPI-CONSUMER)
- [ ] C5 re-derive the prior MET-on-three-platforms receipt
- [ ] the two pending branches read from their branch tips
- [ ] fence exposure vs `861d1b1a`
- [ ] release-blocking answer + costed gap list
