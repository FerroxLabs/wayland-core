---
lane: lane/record-truth
date: 2026-07-29
base: ef1d97beb61f1b084bdfba745e8f49830924d757 (plan/f20-unified-audit-repair)
scope: "Phases 21, 22, 23A/23B, 27 and the cross-cutting record. No product code changed."
build: "NONE. Every claim is a source or git measurement on the Mac, or an attributed quotation of a number another lane measured on hetzner-dsm."
---

# Record reconciliation — making the record match the tree

Four tasks. All four completed. **No product source file was modified**; the only
executable artifact added is a checker under `.planning/scripts/`.

**Standing caveat, stated once and applying to everything below:** this lane ran
no build and executed no Rust test. Where a number appears (8 turns vs 3, `10
passed`, 16 routes), it is another lane's measurement, attributed to the document
and host that produced it. I verified that the code, commits and tests those
numbers refer to exist at HEAD and are wired; I did not re-run them.

---

## Task 1 — Phase 21 was graded before its own work landed

**Verdict: the brief's claim holds, with one correction to it.**

`21-REVERIFICATION.md` is stamped `verified_at_sha: ac94b1d5`, committed
**2026-07-27 07:58:42 +0700**. Four commits landed the same day, after it:

| SHA | committed | subject |
|---|---|---|
| `10947402` | 08:47:45 | sub-allocate a narrowed execution envelope to delegated children |
| `373599ea` | 08:54:18 | invert the no-channel canary to assert the channel exists |
| `d12d7d48` | 09:17:17 | unblind the budget no-channel canary |
| `d29413c1` | 21:07:51 | grade the budget legs on enforcement, not on channel absence |

Verified at HEAD: `ChildBudgetRequest` (`wcore-types/src/spawner.rs:555`),
`ForkOverrides.budget` (`:597`), `sub_budget_narrowed`
(`wcore-budget/src/execution.rs:586`), production callers at
`wcore-agent/src/spawner.rs:1350` and `:1377` — both above that file's first
`#[cfg(test)]` at line 1448, so genuinely production. The request is
child-fillable from a shipped surface: `wcore-tools/src/delegate.rs:105`
`parse_budget`.

**The correction to the brief.** §4 gave three measurements; they do **not** all
fall. One survives:

1. **SURVIVES** — `begin_active_turn(turn_id, None)` is still the sole production
   caller (`engine.rs:6203`). **But it measures the per-turn engine path, not the
   child-spawn path**, so it no longer supports the conclusion it was used for.
2. **FALSE** — `execution.rs:591` is `self.sub_budget(Some(narrowed))`, production
   (first `#[cfg(test)]` at line 964).
3. **FALSE** — `spawner.rs:1350`/`1377` forward a caller-supplied request into
   `sub_budget_narrowed`, which forwards `Some(..)` on.

So the premise is 1/3 intact and 2/3 false, and the intact third is about a
different code path. Saying "the premise evaporated" flat would itself have been
a confidently-wrong record.

**Live evidence** (`21-02-VACUITY-SUMMARY.md` §3.1, hetzner-dsm): control child 8
turns vs narrowed child 3, under a 900-token sub-allocation of a 100 000-token
root; the child charges 400/turn, was permitted two and refused the third. The
mutation control matters more than the number — reverting the seam to
`sub_budget(None)` serves the narrowed child **8** turns and the differential
collapses. The gate can fail.

**Action: `F21-02` re-graded NOT MET → MET WITH STATED EXCEPTIONS**, by **dated
addendum** appended to `21-REVERIFICATION.md`, body and frontmatter untouched.
Exceptions carried: only the token dimension is live-driven; the widening
direction was already unamplifiable so testing it is largely theatre; Linux only;
`Spawn`/`spawn_host_child` still cannot carry a budget request.

**Unchanged: SC3 and F21-04 stay NOT MET, F21-03 stays FENCED, and the phase goal
remains NOT ACHIEVED.** One requirement moved on measurement. The phase did not.

I left the frontmatter deliberately disagreeing with the addendum and said so in
it — silently editing a prior verifier's machine-readable header is the hazard
that motivated the addendum form in the first place.

---

## Task 2 — `F23A-01-H2` is fixed; the census under it is not

**Fix verified.** `81508b74` (08:15:18) added the RED reproducer
`crates/wcore-agent/src/orchestration/d1_refusal_terminal_tests.rs`; `32a5fc90`
(08:27:20) fixed it, 7 files, +240/−36, including `orchestration/mod.rs` (+135) —
the exact `PreparedToolLease::start` → `lease.fail(...)` span the seam request
named as the suspected leak. RED-before-GREEN, in the right order.

The five tests **are wired** (`orchestration/mod.rs:78`), so they are not an
orphaned file, and one of the five is a control
(`approval_denial_control_leaves_turn_committable`), which is what makes the other
four falsifiable.

**The census is a different measurement and it has not been taken.**
`crates/wcore-eval-scenarios/tests/f23a_boundary_drive.rs` last changed at
`481682b0`, **2026-07-26 22:49:40** — the day before the fix. Every census number
on record was produced against a build whose dominant failure mode has since been
removed. `WAYLAND_F23A_SELFTEST` exists in source (`:21`, `:41`) but no artifact
shows it was ever made to fire.

**I did not re-run it, and I did not mark it done.** The brief asked me to say
precisely what it needs; that is written into
`.planning/phases/23A-governed-skills/23A-STATUS-CORRECTION.md` §2, and the
non-obvious half is the instruction to **read back the executed count rather than
the exit status**, and to run the selftest control as a two-run differential that
must disagree. A control that agrees with the uncontrolled run is inert, and the
census would prove nothing whatever its numbers said.

Blocker: this lane has no hetzner worktree and the brief forbids cargo on the Mac.
Tracked as `F23A-01-CENSUS-UNMEASURED` (MEDIUM).

**Records corrected** without rewriting five prior lanes' conclusions: a dated
`23A-STATUS-CORRECTION.md`, plus a pointer appended to each of
`23A-01-LIVE-EVIDENCE.md`, `23A-01-SUMMARY.md`, `23A-02-SUMMARY.md`,
`23A-03-SUMMARY.md`, `23A-04-SUMMARY.md`.

---

## Task 3 — findings routed to files nobody reads

### 3.1 What was actually dropped — 20 findings, 2 of them HIGH

Method: every id named in a `.planning/SEAM-REQUESTS/*.md` entry addressed to
`BACKLOG.md`, and every summary asserting a filing, checked with
`grep -c <id> .planning/BACKLOG.md`. Zero ⇒ dropped.

| id | sev | source |
|---|---|---|
| `F21-02-F2` (`max_iterations` unclamped) | MEDIUM | 21-02-VACUITY-SUMMARY §5 F-2 |
| `F22-M1`, `F22-M2` | MEDIUM ×2 | SEAM-REQUESTS/22.md |
| `F23A-01-M1`, `M2`, `M3` | MEDIUM ×3 | SEAM-REQUESTS/23A.md SR-23A-1 |
| **`F23A-01-H2`** | **HIGH** | SEAM-REQUESTS/23A.md SR-23A-2 — *fixed, filed as a closed record* |
| `23B-M1` … `23B-M4` | MEDIUM ×4 | SEAM-REQUESTS/23B.md SR-23B-04 |
| **`23B-H1`** | **HIGH** | SEAM-REQUESTS/23B.md SR-23B-05 — **still open** |
| `F27-M1` (MCP tool naming), `F27-M2` (file sizes) | MEDIUM ×2 | 27-GAPS-SUMMARY, SEAM-REQUESTS/27.md |
| `F29-03-08` | MEDIUM | 29-03-SUMMARY §250 |
| `F24-01-M1`, `F24-01-M2`, `F24-J-M1`, `F24-C-M1` | MEDIUM ×4 | SEAM-REQUESTS/24.md, 24-C5 summaries |
| `F24-01-L1`, `F26-02-D` | LOW ×2 | SEAM-REQUESTS/24.md, 26-02-SUMMARY |
| `F26-02-B`, `F26-02-E` | MEDIUM ×2 | 26-02-SUMMARY |

**`23B-H1` is the one that should worry a reader.** A cleanly-exited run — no
crash, no kill — can write a session journal the product cannot read back;
`--list-sessions` still lists it, every operator verb that reads it fails, so
there is no repair path. 8/8 and 9/10 reproductions under load, 0/3 quiet.
Confirmed pre-existing against a pristine binary. Its finder graded it HIGH and
filed it nowhere. Under the standing severity policy a HIGH must be fixed or
disproved with executable evidence, so it should not simply age in BACKLOG.

**All severities carried exactly as their finders gave them.** I re-graded
nothing. Where I have a view (`23B-H1` — I agree with HIGH) I say so in the row
and leave the number alone.

**Two id-less findings were assigned ids** (`F21-02-F2`, `F27-M1`, `F27-M2`)
because being unnamed is part of why they were lost — you cannot grep for a
finding with no id, which is also why the Phase 27 MCP-naming MEDIUM is invisible
to the checker below and had to be found by reading.

### 3.2 Not dropped — recorded so nobody re-files them

`F23A-01-H1`, `F21-02-01`, `F21-02-02`, `F21-02-03`, `F24-J-H2`, `F24-J-H3`,
`F26-02-A`, `F26-04-A` were flagged by the checker and are absent from BACKLOG
**correctly** — they were *fixed*, not filed. This is the checker's known
false-positive class, reported rather than suppressed.

### 3.3 Lanes that filed correctly — the contrast matters

Phase 28 (63 rows) and Phase 29 (43) filed properly, as did the Phase 30-04 lane:
`c2b57d53` writes **+61 lines to BACKLOG.md** in the same commit as its verdict,
carrying all six of its MEDIUM/LOW items (`BL-F30-REFCOUNT-GATE`,
`BL-F30-FORCED-MET-SED`, `BL-F30-VERDICT-VERIFY-ARG`, `BL-F30-VACUOUS-MAIN-GATE`,
`BL-F30-AUDIT-CEILING-PREMISE`, `BL-F30-ROADMAP-STALE-STATUS`). **Verified, not
trusted** — that lane's claim is true. `.planning/SEAM-REQUESTS/30.md` (also
`c2b57d53`) contains four fenced-file requests and **no** BACKLOG rows, so it
drops nothing.

This is a per-lane discipline failure, not a broken process. Phases 22, 23A, 23B
and 27 had **zero** BACKLOG entries between them before this lane.

---

## The anti-drop mechanism, and its red-before-green proof

`.planning/scripts/check-finding-disposition.py`. Two shapes:

- **SHAPE 1 — a claim without an effect.** A summary or seam request says a
  finding reached BACKLOG.md; the id is absent. Reads the record, not the claim.
- **SHAPE 2 — a mutation gate that mutates nothing.** A falsification gate forces
  a value and asserts refusal, but its `sed` matches nothing, so it verifies an
  unchanged document and proves nothing. **This one refuses to infer at all**: it
  takes the sed and its real target file and measures whether the pattern occurs.

### Proof it goes red on the historical cases

| case | before | after |
|---|---|---|
| SHAPE 1, whole repo | **27 findings RED** (`evidence/record-truth/RED-before-filing.txt`) | **0, CLEAN** (`GREEN-after-filing.txt`) |
| SHAPE 2, real `BL-F30-FORCED-MET-SED` artifacts | **RED at `30-04-PLAN.md:271`** (`shape2-RED-real-case.txt`) | n/a — not this lane's to fix |
| SHAPE 2, whole repo | **found a NEW, previously unreported instance** (`shape2-NEW-finding-30-03.txt`) | filed as `F30-03-NOOP-SED` |

The SHAPE 2 proof uses the **real** `30-04-PLAN.md` and the **real**
`evidence/30-04/phase-verdict.json` recovered from `32cc7ac8`, not a mock. That
document contains **4 `"grade"` keys and 0 `"verdict"` keys**, which is why the
gate's sed was a no-op.

### It found a defect nobody had reported

**`F30-03-NOOP-SED` (MEDIUM, new).** `30-03-PLAN.md:251` breaks an evidence
reference to prove the publish path refuses it:
`sed 's#evidence/30-02/results.json#evidence/30-02/does-not-exist.json#'` against
`evidence/30-03/claims-register.json`. **That string occurs 0 times in that file**
— the register references `legs.tsv`, `peer-clones.txt`, `protocol.json`,
`protocol.sha256`, `verifier-known-good-bad.txt`, and no `results.json`. The
"broken" register is byte-identical to the good one. **Second instance of the
shape in Phase 30 alone**, found by the instrument built for the first. Owned by
Phase 30's lane; filed so it is not lost.

### Self-test: 8 assertions, three of them "the old matcher missed it"

Per LANE-BRIEF §6b-ii, each shape carries the third assertion that actually
proves the repair does something. `--self-test` is 8/8.

**Two defects in this instrument were found and repaired in-lane, not written up:**

1. **It was line-scoped**, so it missed the entire `SEAM-REQUESTS/23A.md` case —
   the claim is a `**File:** .planning/BACKLOG.md` header and the ids sit in a
   fenced block below. That is the same line-oriented under-detection §6b-ii
   records, reproduced inside the instrument built to hunt it. Repaired to
   section scope; **assertion A4** pins it by running the old matcher alongside
   and requiring it to find nothing.
2. **`lstrip("./")` stripped the dot from `.planning/...`**, so no dotted
   repo-relative target ever resolved and SHAPE 2 reported CLEAN **by failing to
   look** — a self-passing gate in the self-passing-gate detector. **Assertion
   B4** pins it the same way.

A precision fix (only a file passed as a direct argument to `sed` is its target)
removed 5 false positives from pipeline seds. A checker that cries wolf gets
switched off.

### What the mechanism does NOT cover — stated, not hidden

- **Findings with no id are invisible to SHAPE 1.** The Phase 27 MCP-naming
  MEDIUM had no id and had to be found by reading. Any lane that writes "MEDIUM →
  BACKLOG" without an id defeats this check entirely. **The cheap fix is a
  convention, not code: give every finding an id at the moment you record it.**
- SHAPE 1 only scans summaries, verdicts, gaps documents and seam requests.
  `PLAN.md` is excluded deliberately — it restates the severity policy as
  boilerplate next to ids it is not claiming to have filed.
- SHAPE 2 only understands `sed` substitutions with a file argument. A mutation
  via `python`, `jq`, or a heredoc is not covered.
- Neither shape is wired into CI. **It is a script someone must run.** Making it a
  phase-closure gate is the obvious next step and I did not take it, because
  wiring a gate into other lanes' closure paths while five lanes are mid-flight is
  not my call to make unilaterally.

---

## Task 4 — gap-ledger rows

### Corrected: `23A-C1`'s flag is hidden

The row says `--skills-promote` is declared "**not hidden**" and calls it
"RELEASE-BLOCKING at the advertisement level". At HEAD `main.rs:473` reads
`#[arg(long, value_name = "PROCEDURE_ID", hide = true)]`, above a comment citing
this ledger row by name, guarded by
`crates/wcore-cli/tests/skills_promote_not_advertised.rs`. **The advertisement
complaint is closed; the criterion stays NOT MET** — `run_skills_promote` is still
a `bail!`, and *revoked* and *rolled back* are still unimplemented.

### Corrected: `22-C3` is PARTIAL — and its falsifier is broken

Three things, which need separating:

1. **The adapter was built.** `26be00cd` adds `goal/strategy.rs` (+667 lines);
   merged at `f68f3ddd`, self-graded **PARTIAL** by its own lane (`aa60fc4b`). At
   `f68f3ddd`, `strategy.rs` carries **45** `GoalTerminalState` references and
   adapts all five owners. *"No lane has attempted it"* is false.
2. **It is not in the integration branch.** `git branch --contains f68f3ddd`
   lists `inv/*` and `lane/*` only; `plan/f20-unified-audit-repair` is at
   `ef1d97be`, where it is genuinely absent. Both halves are true and I have
   stated both.
3. **The row's falsifier would never have noticed.** Its evidence is *"grep
   `GoalTerminalState` under `crates/wcore-agent/src/orchestration/` returns zero
   hits"*. The adapter lives under `goal/`. **Measured: that grep returns zero
   even at `f68f3ddd` where the adapter exists** — it reports FAILED forever,
   including after closure. A self-*failing* gate, the mirror of §3.2's class.
   Repaired in place: grep across `crates/wcore-agent/src/` and require a consumer
   adapting each of the five owner types; verified against `f68f3ddd` at 45 vs 0.

Both corrections keep the original text intact above them.

### Found stale but NOT substantiated — left alone, as instructed

- **`22-C4`, `22-C5`, `24-C1` and the Phase 24 rows generally.** Phase 24 has
  moved a lot and several rows read stale, but Phase 24 lanes were **active while
  I ran** and the brief fences me out of `.planning/phases/24-*`. Anything I
  measured would have been racing a live lane. Not touched.
- **`21-C3`, `25-C4`.** Not examined; outside the four tasks and I had no
  measurement.
- **`23A-C1`'s cost lines** ("3–4 lane-sessions"). Plausible, unverifiable by me.
- **Phase 30 rows.** Fenced out by the brief. `F30-03-NOOP-SED` is filed to
  BACKLOG rather than written into any Phase 30 document.

---

## What I did not do

- **No build, no cargo, no test execution**, per the brief. Every code claim is a
  source/git measurement; every number is attributed to the lane and host that
  produced it.
- **Did not re-run the 16-route quarantine census** (Task 2's substantive half).
  Needs a hetzner worktree this lane does not have. What it needs is written out
  precisely instead, and the census is **not** marked done.
- **Did not re-run `f21_02_child_budget_live.rs`.** The 8-vs-3 figure is
  `21-02-VACUITY-SUMMARY.md`'s, attributed.
- **Did not edit any prior verification's body or frontmatter.** All corrections
  are dated addenda or separate files.
- **Did not touch `.planning/phases/24-*` or `.planning/phases/30-*`**, nor any
  product source file.
- **Did not wire the checker into CI or any phase-closure gate.**
- **No merge, no PR**, per instructions.

_lane/record-truth · base `ef1d97be` · 2026-07-29_
