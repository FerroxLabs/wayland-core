# Phase 28 Certification Contract

**Status:** authored by plan `28-01`, 2026-07-27. Binding on plans `28-02`, `28-03` and `28-04`.
**Enforcement:** `.planning/scripts/f28-ledger.py`. The prose below is the specification; that
script is its enforcement. Where the two could disagree, `--check-contract` fails closed.

This document does four things and nothing else:

1. It quotes the authorities — Phase 28's four Success Criteria, the requirements, and the
   standing severity policy — **verbatim**, so nothing downstream reads a paraphrase.
2. It records the **already-decided** acceptance rule with its three binding amendments.
3. It defines the **severity rubric** including the A1 re-scoring procedure.
4. It defines the **four-class skip taxonomy**, the **sandbox activeness rule**, the **mandatory
   cells**, and the **carried-red ledger**.

**The acceptance rule was decided at planning time, 4-0, before any plan was written. It is not
this plan's to change and it is not any later plan's to soften.** The record is in
`28-01-decision-evidence/`. This document encodes it. Section 9 records one piece of
counter-evidence found while encoding it, as a finding, without acting on it.

---

## 1. The authorities, quoted verbatim

### 1.1 Phase 28 Success Criteria — `.planning/ROADMAP.md`, "Phase 28: Native Cross-Platform Certification"

> 1. Native macOS, Linux, and Windows pass the required hostile platform matrix with no skipped critical case.
> 2. The 1,000-session/concurrent-child soak has no secret leak, orphan process, unbounded resource use, or unacceptable quality/performance delta.
> 3. Signed receipts bind exact candidate, platform, posture, corpus, environment, artifacts, logs, and skip policy.
> 4. Zero findings remain at every severity before acceptance.

These four are the **only** completion authority for Phase 28. They are quoted, never narrowed,
and never restated in this document's own words.

### 1.2 Requirements — `.planning/REQUIREMENTS.md`

> - **F28-01**: Native macOS, Linux, and Windows E5 matrices cover sandbox probes, Unicode, long paths, UNC/reparse/symlink cases, process cleanup, suspend/resume, offline, disk-full/read-only, and hostile inputs.
> - **F28-02**: A 1,000-session and concurrent-child soak completes with secret canaries intact, no orphan processes, and bounded quality/performance deltas.
> - **F28-03**: Signed receipts bind the exact candidate, platform, posture, fixture corpus, environment, artifacts, logs, and skipped-case policy.
> - **F28-04**: No critical case is skipped and every finding at every severity is resolved before certification acceptance.

### 1.3 The standing severity policy — quoted verbatim from its canonical sources

**Source note, recorded because the plan directed otherwise.** Plan `28-01` instructed that the
standing severity policy be quoted verbatim *from `AGENTS.md`*. It is **not there**. This
repository's `AGENTS.md` contains no severity-policy text — `grep -n -i -E
'CRITICAL/HIGH|BACKLOG|non-blocking|disproved' AGENTS.md` returns nothing. The canonical sources
are `.planning/ROADMAP.md` (Constraints) and the commit that amended the rule. Both are quoted
below rather than a paraphrase being invented to fill the gap. See finding `F-28-01-002`.

From `.planning/ROADMAP.md`, Constraints:

> Findings at CRITICAL or HIGH must be fixed or disproved before native UAT and `phase.complete`; MEDIUM and below are logged to BACKLOG and do not block execution. Execution begins when no CRITICAL or HIGH finding is open, or after 2 review rounds, whichever comes first. A third round is not permitted; it escalates to Sean.

From commit `d0837aa7277c9fd97bba90a8c6da9e88e8a51b48`, `decision(20): bound the two
non-terminating finding rules`, 2026-07-25 12:20:08 +0700:

> (a) becomes: CRITICAL/HIGH must be fixed or disproved; MEDIUM and below
> are logged to BACKLOG and do not block execution.
> (b) becomes: execution begins when no CRITICAL or HIGH finding is open,
> or after 2 review rounds, whichever comes first. A third round is not
> permitted; it escalates to Sean.

---

## 2. The acceptance rule as adopted — `c4-disposition`, 4-0

**Panel split: 4 of 4 for `c4-disposition`.** Three external members plus one internal adversarial
pass which concurred *conditionally*; its conditions were adopted in full as amendments A1, A2 and
A3. Full record: `28-01-decision-evidence/decision-rationale.txt`. **The losing arguments are
recorded in `28-01-decision-evidence/decision-dissent.txt` and a later reader who wants to reopen
this decision should read the dissent first, not this section.**

The rule, quoted verbatim from `decision-rationale.txt`:

> "Resolved" is implemented as DISPOSITIONED, not as FIXED. The acceptance gate is:
>
>   ZERO findings at any severity lack an explicit, recorded, evidence-backed disposition.
>
> Four terminal dispositions exist, and every finding takes exactly one:
>
>   FIXED     — repaired, with the repair proved by an executable check.
>   DISPROVED — shown not to be a defect, with executable counter-evidence.
>   ACCEPTED  — real, not repaired, carried knowingly. Requires a named rationale, a named owner
>               and a BACKLOG id. Available at MEDIUM and LOW ONLY.
>   DEFERRED  — real, routed to a named later phase. Same three requirements. MEDIUM and LOW ONLY.
>
> CRITICAL and HIGH have exactly two available dispositions: FIXED or DISPROVED. Neither ACCEPTED
> nor DEFERRED is reachable at those severities, and the gate must fail closed rather than warn.

Every ACCEPTED and DEFERRED finding is enumerated **inside** the signed certification receipt with
its severity, rationale, owner and BACKLOG id.

### 2.1 The deciding fact, re-checked by this plan rather than inherited

The decision rests on one checkable claim: that Criterion 4 predates the severity amendment and is
therefore a survival of a superseded rule rather than an informed elevation above the current one.
Plan `28-01` re-ran the check rather than trusting the record:

```
$ git log -1 --format='%H %ad %s' --date=iso d0837aa7
d0837aa7277c9fd97bba90a8c6da9e88e8a51b48 2026-07-25 12:20:08 +0700 decision(20): bound the two non-terminating findin...

$ git log --format='%h %ad' --date=iso -S'Zero findings remain at every severity' -- .planning/ROADMAP.md
0192e3c0 2026-07-19 19:01:55 +0700
```

**Confirmed. Criterion 4's text landed 2026-07-19; the amendment landed 2026-07-25, six days
later.** The dissent states that this date claim "is the whole load-bearing structure" of the
decision. It holds. Section 9 records the one piece of counter-evidence that also holds.

### 2.2 The three binding amendments — quoted verbatim

> A1. SEVERITY IS RE-SCORED AGAINST PHASE 28'S OWN FOUR SUCCESS CRITERIA. No finding enters the
>     ledger at a severity inherited from an earlier phase. The inherited severity is recorded as
>     provenance only. Rationale: the dangerous findings on this candidate do not need anyone to
>     downgrade them — they ARRIVE below HIGH, scored against a different phase's criteria.
>     Inherited severity is a laundering channel that requires no launderer.

> A2. A FINDING THAT CONTRADICTS A PHASE 28 SUCCESS CRITERION IS CRITICAL OR HIGH BY CONSTRUCTION
>     AND CANNOT TAKE THE ACCEPT OR DEFER PATH. Not by judgement — by construction, because the
>     criterion it contradicts is the thing being certified.

> A3. THE RECEIPT MAY NOT CLAIM WHAT IT DOES NOT HAVE. Its defensible assertion is exactly: zero
>     undispositioned findings, zero skipped critical cases, zero unresolved CRITICAL/HIGH
>     findings. It may NOT assert "zero known defects" or "zero findings", and the receipt
>     verifier must REJECT a receipt that asserts either.

**A2 IS THE DECISION.** From the dissent, quoted because it is the instruction to a later reader:

> If a future plan, executor or reviewer keeps c4-disposition's four dispositions but drops or
> softens A2 — by scoring findings at their inherited severity, or by letting a
> criterion-contradicting finding take the ACCEPT path — then this decision has silently become
> c4-standing with paperwork, the dissent above becomes correct, and the certification is worth
> materially less than its own text claims. Check A2 before trusting any Phase 28 acceptance.

---

## 3. The severity rubric

### 3.1 The four bands

| Band | Meaning for Phase 28 | Available dispositions |
|---|---|---|
| CRITICAL | The candidate contradicts a Success Criterion in a way that would make the signed receipt false, **or** a security control reports itself active while being inactive. | FIXED, DISPROVED |
| HIGH | The candidate contradicts a Success Criterion, **or** a Criterion-subject property cannot be evidenced at all so the criterion cannot be honestly asserted. | FIXED, DISPROVED |
| MEDIUM | A real defect that contradicts none of the four criteria. | FIXED, DISPROVED, ACCEPTED, DEFERRED |
| LOW | A real but minor defect that contradicts none of the four criteria. | FIXED, DISPROVED, ACCEPTED, DEFERRED |

### 3.2 The re-scoring procedure (A1) — mandatory, no finding is exempt

Every finding entering the Phase 28 ledger, **including every finding inherited from Phases 20
through 27**, is scored by this procedure:

1. Record the severity it arrived with in `inherited_severity`. **This field is provenance only
   and is never used by a gate.** A finding carrying only an inherited severity is rejected with
   code `F28L-006`.
2. Identify the finding's **subject matter** — the property of the product it is about.
3. Compare that subject matter to the subject matter of each of the four Success Criteria in §1.1.
4. If the finding's subject matter **is the subject matter of a criterion**, record that criterion
   number in `contradicted_criterion` and score CRITICAL or HIGH. Per A2 the accept and defer
   paths are then closed **regardless of the severity recorded** — enforced by `F28L-007`, which
   fires on a non-empty `contradicted_criterion` even at MEDIUM or LOW, so a mis-scored severity
   cannot reopen the accept path.
5. Otherwise leave `contradicted_criterion` empty and score on ordinary merits.
6. Record the score in `p28_severity` and the reasoning in the ledger row.

**Direction of error.** A1 exists because dangerous findings on this candidate arrive
*pre-labelled below HIGH*, not because anyone downgrades them. Re-scoring is therefore expected to
move findings **up**. A re-score that moves a finding **down** from its inherited severity is
permitted but must carry its reasoning explicitly, and plan `28-04` cross-audits every such row.

### 3.3 The two calibration examples, worked

These two are the panel's own named examples. They are the calibration for everything else.

**Example 1 — `live_future_drop_reaps_descendant_job_tree`.**
Inherited: "known-red, non-gating" (sub-HIGH, scored against Phase 20/20A's criteria).
Subject matter: *a descendant process tree is not reaped; a process survives its owner.*
Criterion 2 subject matter: *the soak completes with "no orphan process."*
**Same subject.** `contradicted_criterion = 2`. **Score: HIGH. Accept path closed.**
Nobody downgraded this. The label was simply never re-scored against this phase's criteria — which
is precisely the laundering channel A1 closes.

**Example 2 — the AppContainer ACL lease SID/profile mismatch.**
Inherited: an environment quirk, sub-HIGH.
Subject matter: *a security control reports itself active while being inactive; the product
continues to execute unsandboxed and logs a message that reads like a platform limitation.*
Criterion 1 subject matter: *the hostile platform matrix, including sandbox probes.*
**Same subject.** `contradicted_criterion = 1`. **Score: CRITICAL. Accept path closed.**
A security control that is silently inactive is not a MEDIUM in any rubric worth the name.
Criterion 1 exists to force this into the open rather than inherit it.

### 3.4 The unproven-control question — recorded, and deliberately NOT applied

A corollary reading of A2 is available and this plan considered it: where a carried red means a
Criterion-subject property is **unverified** rather than known-broken, one could score the finding
on the property and close its accept path, because accepting an unverified Criterion-subject
property is exactly asserting the criterion on absent evidence.

Applied literally, that reading would move `KR-02` (Windows private DACL enforcement, unproven
because its tests cannot complete their reopen step) and `KR-03` (worker output-exhaustion buffer
retention, unproven because the test exceeds its budget) across the A2 line as well — four
crossings instead of two.

**This plan did NOT apply that corollary, and the reason is recorded rather than assumed.** The
decision record names exactly two crossings and plan `28-01` was directed to name two. Inventing a
stricter rule than the recorded decision is the failure that grew Phase 20 to 74 plans, and the
standing policy says so in terms. The corollary is recorded here as `F-28-01-001` so a later
reader can apply it deliberately rather than rediscover it.

**It costs less than it looks, because the structural rule in §5 reaches the same place by a
different road.** `KR-02` and `KR-03` describe properties that will be *matrix cells* in plan
`28-02`, and a cell that cannot produce positive evidence is a RED — not a green and not a skip.
The properties are therefore forced into the open as measured cells rather than laundered as
inherited rows. Where the inherited row and the measured cell disagree, **the measured cell
governs.**

---

## 4. The skip taxonomy — exactly four classes, and no fifth

> A CRITICAL CELL HAS NO LEGAL SKIP. There is no class, no evidence and no circumstance under
> which a critical cell may be skipped. A critical cell that cannot be run is a **RED**.

**No fifth class may be added mid-run. Adding one is a finding against the run, not a fix for
it.** A skip invented under time pressure to route around an inconvenient red is how a matrix
reports green while proving nothing. That is why this taxonomy is written before any case runs.

Every skip carries `skip_class` and that class's required evidence field. A skip with no class is
rejected at construction with `F28M-001`. A skip with a class not in this list is rejected with
`F28M-003`.

### 4.1 `platform-inapplicability`

**Means:** the case is *meaningless* on that OS family — not hard, not unsupported, meaningless.
**Required evidence:** `platform_fact` — the named platform fact that makes it meaningless, and
the observable that establishes it.
**Not acceptable:** "not implemented on this platform" (that is a defect, not an inapplicability);
"we don't test that here"; a maintainer's opinion.

### 4.2 `observation-blocked` — the class this contract constrains hardest

**Means:** the behaviour may or may not be correct, but the *observation channel* in the
certification environment is broken independently of the product, so no verdict is obtainable.

**Required evidence:** `control_ref` — a control **MEASURED IN THE CERTIFICATION ENVIRONMENT, AT
RUN TIME**, demonstrating the channel is broken **INDEPENDENTLY OF THE PRODUCT**, and constructed
so that **it fails when the channel is sound**. The reference binds a control id, the host it ran
on, and the run identifier: `control:<id>@<host>:<run-id>`.

**NOT ACCEPTABLE, BY NAME:**

- a citation of a handoff document;
- a citation of a prior phase, a prior run, or a prior SUMMARY;
- a citation of any file under `.planning/intel/` — **including
  `APPCONTAINER-SSH-LEASE-WEDGE.md` and `APPCONTAINER-SSH-LORE-READJUDICATION.md`, which report
  in the product's favour**; a laundering channel does not become sound by pointing it at good
  news;
- a citation of this contract;
- any inherited belief, in either direction, however well documented.

**Absent a passing control the cell is a RED, not a skip.** Enforced by `F28M-004`, which requires
`control_ref` to match the run-time control shape **and** to match none of the documentary-citation
patterns. Both directions are self-tested.

**Why this class is written with more force than the other three.** It sits directly on the
security dimension, and it is the one an executor under time pressure will reach for. The program
carried, for weeks, a rule stating that AppContainer "cannot be observed over SSH" and that one
must "never conclude a red from an SSH run". That rule rested on a single control which **varied
the logon while the lease directory was wedged** — an observation both competing hypotheses
predicted, so it never had the power to select between them.

**As of 2026-07-27 that lore is REFUTED on the measured host**
(`.planning/intel/APPCONTAINER-SSH-LORE-READJUDICATION.md`): `live_fs_acl` is **12/12 PASS** over
a session-0, non-interactive SSH logon on a clean lease directory, *including the exact test cited
as establishing the rule*. The real cause was a stale lease written by `wcore-sandbox`'s own test
suite into the **production** lease directory, under which the product ran **unsandboxed** while
logging "sandbox disabled".

**Two consequences, and the second is the one this section exists for.**

1. A Windows sandbox red observed over SSH is **evidence**, not an artifact. Reds discounted under
   the old rule need re-reading.
2. **The refutation does not license a skip either.** It was measured on **one host**, and the
   intel says so itself: *"I did not test other Windows hosts... the observation is one box."*
   Neither the lore nor its refutation may be cited as evidence for an `observation-blocked` skip
   in the certification environment. **Only a control measured there, at run time, counts.** A
   skip class whose evidence requirement can be satisfied by citing a document is not a skip
   class; it is a laundering channel, and pointing it at a favourable document rather than an
   unfavourable one changes nothing about its shape.

### 4.3 `architectural-impossibility`

**Means:** the behaviour **cannot exist** on that platform by construction.
**Required evidence:** `impossibility_check` — an **executable** check demonstrating the
impossibility and the product's fail-closed response to it.
**Not acceptable:** an argument that it would be hard, expensive, or out of scope. Difficulty is
not impossibility.
**Canonical instance:** bash cannot run under Windows AppContainer — msys requires
`\BaseNamedObjects`, AppContainer confines to `AppContainerNamedObjects` by construction
(`0xC0000022`). The check asserts the real fail-closed contract.

### 4.4 `unresolved-surface`

**Means:** the surface the cell would exercise was not landed by the phase that claimed it.
**Required evidence:** `phase` — which of 24, 25, 26, 27 — **and** `req_disposition`, that phase's
own recorded disposition of the requirement.
**Not acceptable:** "the surface wasn't found" with no phase attribution. An unattributed absence
is a resolver finding, not a skip.

---

## 5. The sandbox activeness rule — a first-class rule, not a note

> **A GREEN ON ANY SANDBOX-DIMENSION CELL REQUIRES POSITIVE EVIDENCE THAT THE SANDBOX WAS ACTIVE
> FOR THAT CELL. ABSENCE OF AN OBSERVED VIOLATION IS NOT EVIDENCE OF A SANDBOX. A CELL THAT CANNOT
> PRODUCE POSITIVE ACTIVENESS EVIDENCE IS A RED — NEVER A GREEN, AND NEVER A SKIP.**

This is the structural answer to the silent-disable defect and **it holds however the
observability question resolves.** Windows can run with the sandbox silently disabled on an
AppContainer ACL lease SID or profile mismatch. A cell that merely failed to observe a violation
would then report green over no sandbox at all — the sandbox was never there to violate.

The rule is enforced **in the generator's types, not by convention**
(`crates/wcore-eval-scenarios/src/e5_matrix.rs`):

- Activeness evidence is `ActivenessEvidence::Observed { probe, detail }` or
  `ActivenessEvidence::NotMeasured { reason }`.
- **There is no variant expressing "no violation observed."** It is not representable.
- `NotMeasured` carries a reason and **no count and no verdict**, per the standing rule that *a
  measurement that cannot be taken must never render as `0`*.
- A sandbox-dimension cell accepts a PASSED outcome only when constructed with `Observed` and
  non-empty `probe` and `detail`. `NotMeasured` on a sandbox cell yields a RED, rejected at
  construction with `F28M-006`.

The lease wedge and the silent-disable defect **may be two faces of one underlying defect.** The
matrix must be able to **distinguish** them rather than assume either: activeness is measured
per-cell, and the observability control occupies its own cell (§6).

---

## 6. The mandatory cells

Three cells are **mandatory**. The generator emits them **regardless of what surface resolution
produces**. All three are CRITICAL and therefore, by §4, unskippable.

| Cell id | OS | Dimension | Why mandatory |
|---|---|---|---|
| `w-sandbox-silent-disable` | windows | sandbox probes | The AppContainer ACL lease SID/profile mismatch under which the sandbox is silently inactive. Criterion 1 exists to force this into the open; a generator that could omit it would defeat the criterion. |
| `w-process-cleanup-descendant-tree` | windows | process cleanup | Descendant process-tree reaping. Criterion 2 names "no orphan process" as its own subject. |
| `w-sandbox-observability-control` | windows | sandbox probes | Whether the Windows sandbox is observable **in the certification environment** is a question this phase MEASURES. It occupies a cell so its answer is recorded as evidence rather than as an executor's recollection — in either direction. |

Absence of any of the three is rejected with `F28M-007`.

---

## 7. Carried-red ledger

Machine-readable form: `evidence/28-01/known-red.tsv`. That file is authoritative for later plans;
this table is its human rendering. Validated by `f28-ledger.py --check-rescoring`.

| id | Origin | Inherited | **P28 re-score** | Contradicts | Dispositions available |
|---|---|---|---|---|---|
| `KR-01` | `wcore-sandbox::live_integrity::live_future_drop_reaps_descendant_job_tree` — Windows, deterministic. | known-red/non-gating | **HIGH** | **2** | FIXED, DISPROVED |
| `KR-02` | `snapshot.rs` `windows_private_dacl_accepts_restrictive_deny_ace` / `..._rejects_null_empty_and_broad_allow` — WRITE_DAC reopen error 5; fails identically at parent. | known-red/non-gating | MEDIUM | — | FIXED, DISPROVED, ACCEPTED, DEFERRED |
| `KR-03` | `worker_runtime_limits::multi_worker_output_exhaustion_fails_without_retaining_buffers` — ~35s against a 20s budget. **Timeout deliberately NOT raised.** | known-red/non-gating | MEDIUM | — | FIXED, DISPROVED, ACCEPTED, DEFERRED |
| `KR-04` | bash cannot run under Windows AppContainer — msys needs `\BaseNamedObjects`, AppContainer confines to `AppContainerNamedObjects` (`0xC0000022`). Architectural. | known-red/architectural | LOW | — | FIXED, DISPROVED, ACCEPTED, DEFERRED |
| `KR-05` | AppContainer ACL lease SID/profile mismatch — the product runs with the sandbox **SILENTLY DISABLED** and logs a message that reads like a platform limitation. | environment quirk / non-gating | **CRITICAL** | **1** | FIXED, DISPROVED |
| `KR-06` | Whether the Windows sandbox is observable **in the certification environment**. | standing rule, now REFUTED on one host | **HIGH** | — | FIXED, DISPROVED |

### 7.1 The two A2 crossings, in terms

**`KR-01` and `KR-05` cross the A2 line. Their accept path is CLOSED.** Neither may take ACCEPTED
or DEFERRED at any point in Phase 28, by any plan, for any reason. `f28-ledger.py
--check-rescoring` proves this mechanically and plan `28-04` gates on the codes it emits.

This is the whole of A2's practical effect on the candidate as it stands today, and it is why A2
is not decoration: without it both of these walk in wearing an inherited sub-HIGH label, take the
paper path, and get signed into a receipt that asserts a clean orphan-process result and a working
sandbox while the ledger beneath it records the opposite.

### 7.2 `KR-06` — recorded so an unmeasured belief cannot be mistaken for a measured result

**Status: the LORE is REFUTED on one host; GENERALIZATION to the certification environment is
OPEN.** Both halves of that sentence are load-bearing.

- **Refuted:** `.planning/intel/APPCONTAINER-SSH-LORE-READJUDICATION.md`, 2026-07-27, `455dd836`,
  `SeanD@seandesktop`: `live_fs_acl` **12/12 PASS** over session-0 non-interactive SSH with
  `LEASE_BEFORE=0`, including `granted_path_is_readable_then_revoked` — the exact test the old rule
  cited as its control. Mechanism established in
  `.planning/intel/APPCONTAINER-SSH-LEASE-WEDGE.md`: 4/4 clean probes green, 2/2 wedged probes red,
  deterministic, session 0 throughout.
- **Open:** one host. The intel says so itself. Nothing about the certification environment is
  established by it.

Consequences, stated so no later plan has to infer them:

1. **No rule, gate or skip in Phase 28 may have "sandbox reds from SSH are artifacts" as a passing
   condition.** The rule that said so is false as written.
2. **The refutation is equally unusable as skip evidence.** §4.2 forbids citing either intel file.
   Only a control measured in the certification environment at run time counts.
3. **The lease wedge and the silent-disable defect may be two faces of one underlying defect.** The
   matrix must be able to distinguish them (§5, §6), not assume either.
4. Plan `28-02` settles the certification-environment question **by control, before any sandbox
   cell is graded.** `KR-06` is scored HIGH because it conditions Criterion 1's evidence; at HIGH
   the accept and defer paths are closed by severity, without invoking A2.

A repair for the wedge mechanism landed at `455dd836` (`lease_root()` under `cfg(test)` resolves to
a per-process temp directory; a test-origin lease is now named rather than reported as a generic
mismatch). **`KR-05` is not closed by that.** The resolver must establish whether the candidate
contains the repair; if it does not, `KR-05` stands at CRITICAL with its accept path closed.

---

## 8. What the receipt may claim (A3), stated here because plan 28-04 consumes it

Defensible, and exactly this:

- zero undispositioned findings;
- zero skipped critical cases;
- zero unresolved CRITICAL/HIGH findings.

**Forbidden:** "zero known defects", "zero findings", or any formulation implying either. The
receipt verifier must **reject** a receipt asserting them.

---

## 9. Findings raised by this plan against the record it encodes

Recorded rather than acted on, per the plan's own instruction: *"If you believe the decision is
wrong, record that in your SUMMARY as a finding and proceed under the decision as written."*
**The decision was not reopened and nothing in this contract deviates from it.**

**`F-28-01-003` — MEDIUM — the amendment commit explicitly disclaims Phase 28.**
The decision's load-bearing argument is that the severity amendment (`d0837aa7`, 2026-07-25) is the
"later instrument" and therefore governs Criterion 4 (`0192e3c0`, 2026-07-19). The date claim is
**confirmed** (§2.1). But `d0837aa7`'s own commit message ends:

> Phase 28's criteria are untouched (different phase).

That is the amendment's author stating, in the amending instrument itself, that it does **not**
reach Phase 28. This is the strongest available evidence for the losing `c4-literal` position and
it does not appear in `decision-rationale.txt`, `decision-dissent.txt`, or any captured panel
response. It does not by itself overturn the decision — the sentence is as consistent with "I am
not editing that phase's text" as with "that phase is exempt from this rule", and the dissent's own
reversal condition is narrower (*"if the human ever restates Criterion 4 in its literal form
KNOWING the current severity policy"*), which this is not. **It is recorded so that a reader
reopening this decision has the counter-evidence in hand.** Under A2 the practical gap between the
two readings is narrow: the findings `c4-literal` would protect are exactly the ones A2 already
removes from the accept path.

**`F-28-01-002` — LOW — the standing severity policy is not in `AGENTS.md`.**
Plan `28-01` directed that it be quoted verbatim from `AGENTS.md`; that file contains no such text.
§1.3 quotes `.planning/ROADMAP.md` and `d0837aa7` instead and says so. Routed to BACKLOG: either
add the policy to `AGENTS.md` §11, or correct the plans that cite it as living there.

**`F-28-01-001` — MEDIUM — the unproven-control corollary was considered and not applied.**
See §3.4. Recorded with its reasoning so a later reader can apply it deliberately.

---

## 10. Provenance

- Decided at planning time, 4-0, before any plan was written:
  `28-01-decision-evidence/decision-rationale.txt`.
- Losing arguments: `28-01-decision-evidence/decision-dissent.txt`.
- Why A2 exists: `28-01-decision-evidence/panel-internal.txt`.
- Captured panel responses: `panel-codex.txt`, `panel-gemini.txt`, `panel-kimi.txt`,
  `panel-question.txt`.

**The acceptance rule was decided at planning time and is not this plan's to change.** This
document encodes it. Plans `28-02`, `28-03` and `28-04` execute under it and may not soften A1, A2
or A3.
