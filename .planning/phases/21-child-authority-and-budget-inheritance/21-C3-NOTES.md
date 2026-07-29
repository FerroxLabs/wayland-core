# 21-C3 — Hostile-corpora equivalence: working notes

Lane `21-c3-hostile`. Branch `lane/21-c3-hostile`, base `5457710e` on
`plan/f20-unified-audit-repair`.

This file is appended to after **every** measurement, per LANE-BRIEF §6b-i. It is the
resume point if this lane dies.

---

## 0. The specification (from `21-04-PHASE-VERDICT.md` §1, Criterion 3)

> CRITERION :: 3 :: NOT-MET :: Standalone and host-protocol hostile corpora prove
> equivalent enforcement.

The verdict names **four** clauses that are unmet. These, and only these, are my
specification:

| # | Unmet clause | Verdict source |
|---|---|---|
| C3-a | **Three of eleven dimensions have no host-protocol expression at all** — `tool` and `fan_out` cannot be requested on the host child-spawn API; `egress` cannot be attempted because the child registry carries no network-capable tool (`Unknown tool: WebFetch`) | VERDICT §1 C3 bullet 1 |
| C3-b | **Fan-out is undetermined live**, on both platforms and both surfaces | VERDICT §1 C3 bullet 2 |
| C3-c | **The Windows standalone live surface has no actor at all** — every PTY-backed transport unavailable, piped fallback has no approval channel | VERDICT §1 C3 bullet 3 |
| C3-d | **The `tool` dimension's REFUSED is jointly attributable** to tool authority and to workspace containment; C1 records the tool guard ABSENT, so that REFUSED must not be read as evidence of tool enforcement | VERDICT §1 C3 bullet 4 |

The lane brief states the gap is **proof narrowness, not a broken enforcement
property**. So the job is to widen the proof over these four clauses, not to hunt a
defect — though a defect found while widening is a bonus and must be reported loudly.

## 0b. The trap I am specifically exposed to

A hostile corpus produces "nothing bad happened" for a genuine pass and for a probe that
never ran. Rules I bind myself to before reporting anything:

1. Assert the corpus artifact **exists and is non-empty** before asserting what is not
   in it.
2. Every known-negative must be **shown to actually fail** — feed a corpus that should be
   rejected and watch it go red.
3. A uniform answer across every input with **no known-positive** is the self-passing
   shape, not a result.
4. Read back `N passed; M failed; K ignored; F filtered out` — never trust exit status
   (LANE-BRIEF §3.2). `rtk` strips `ignored`/`filtered out`, so cargo goes through
   `/usr/bin/env cargo` or an absolute path.

---

## 1. Log

### t0 — orientation (no measurement yet)

Read: `LANE-BRIEF.md`, `21-04-PHASE-VERDICT.md`, `21-05-CRITERION3-REPAIR.md`.

Corpus surface inventory (from `wc -l`, unproxied):

```
1312 crates/wcore-cli/tests/portability_hostile_corpus.rs
 897 crates/wcore-cli/tests/child_authority_corpus.rs
 370 crates/wcore-cli/tests/child_authority_corpus/cases.rs
1593 crates/wcore-cli/tests/child_authority_corpus/live.rs
2205 crates/wcore-cli/tests/child_authority_corpus/surfaces.rs
```

Working hypothesis of the tractability order (to be revised by measurement):

- **C3-d is a differential-control problem, not a product problem.** The probe target
  sits *outside* the child's isolated-mutation checkout, so an absent Bash effect is
  attributable to containment OR to authority. Running a **second probe inside** the
  checkout separates them: if Bash lands inside and not outside, the refusal was
  containment and the tool guard is confirmed absent live (which would corroborate
  F21-02-01 on the live surface for the first time). This also supplies the
  **known-positive** the corpus is missing on that dimension.
- **C3-b is a missing live control.** The in-process fan-out probe already runs an
  at-cap control first (`21-05` §1.3) precisely because zero children is the correct
  enforcement outcome and is indistinguishable from a broken fixture without one. The
  live probe has no such control. Porting it is mechanical.
- **C3-a splits.** `tool`/`fan_out` inexpressibility is a typed fact about
  `SubAgentConfig` + `spawn_host_child` (`ForkOverrides::default()` hardcoded) and is
  honest. `egress` inexpressibility is a *registry* fact (`Unknown tool: WebFetch`) and
  may be reachable if the host child spawn path admits a toolset. TO BE MEASURED.
- **C3-c is a Windows PTY claim to re-measure.** "`portable_pty`'s ConPTY backend does
  not surface the spawned binary's stdout to the master end" is a strong negative
  claim and per LANE-BRIEF §3b-i a negative is the easiest thing to pass without work.
  Re-measure it directly on `SeanD@seandesktop` under `D:\` before either accepting or
  rejecting it.

Nothing measured yet. Next: read the four corpus files and locate the tool probe, the
fan-out probe and the host-protocol spawn path.
