# 30-01 — Independent review of CTRL-01 (`.planning/intel/COMPETITIVE-LEDGER.md`)

**Reviewed at base `eab69cdbc244cfe90b0a623a9fb15c80da249d24`** (lane `lane/30-01`).
Every figure below is as of that commit and says which command produced it.
`COMPETITIVE-LEDGER.md` and `REQUIREMENTS.md` were **not edited** — gate-checked
unmodified. F30-01 asks Phase 30 to *review* the ledger; a reviewer who rewrites the
row he is grading has graded his own work.

Machine-checkable index: `evidence/30-01/LEDGER-REVIEW.tsv` — 60 determinations,
every one naming a captured file under `evidence/30-01/` that exists and holds real
output. Re-runnable: every script in `evidence/30-01/` takes the repo root as an
argument and can be pointed at a different tree.

---

## 0. The headline, stated first because it inverts the usual finding

**CTRL-01's schema is sound. Its contents are stale, and the staleness runs in the
direction nobody checks for: the ledger materially UNDERSTATES the program.**

All ten coverage families pass all seven required clauses. Both pinned peer
baselines re-verify exactly. 39 of 42 evidence IDs resolve to concrete objects.
That is a clean result and it is reported as found, not inflated.

But **thirteen claims the tracking documents make are falsified by the tree**, and
every one of them understates what has landed. The ledger was refreshed at
`77e20891` (2026-07-28 08:28 +0700) against base `42e1f2b2`; between that moment and
my base, plans 26-02, 26-04, 28-03, 28-04, 29-03, 29-04 and the Phase 25 cloud and
second-host lanes all landed. The ledger is *dated*, not wrong — everything it says
was true when written. The defect is that **a reader cannot tell**, and Phase 30's
whole purpose is to hand Sean a positioning decision built on current facts.

---

## 1. Evidence-ID resolution — 42 IDs, mechanically resolved

Method: `evidence/30-01/resolve-evidence-ids.py`. For each declared ID the Artifact
cell is parsed and every object it names is resolved by RUNNING something — `stat`
for a path, `git cat-file -t` for a commit, `grep -c -F` of the named artifact for a
build digest or a workflow-run id. Per-ID captures are in `evidence/30-01/ids/`.

| Outcome | Count |
|---|---|
| CONFIRMED | 39 |
| PARTIAL (artifact real, citation imprecise) | 1 |
| UNRESOLVED | 2 |

**The plan was calibrated at eight evidence IDs; there are 42.** The 2026-07-28
refresh added 34, exactly as it claims. The plan's LEDGER-STRUCTURE gate
(`==8` evidence rows, `==20` family rows) is therefore RED against a tree measuring
**42 and 26**. The plan anticipated this in the gate's own comment — *"if CTRL-01
grows a family or an evidence ID before this runs, the count moves and the review is
re-scoped rather than silently stale"* — so the review is re-scoped to 42/26 and the
gate is reported red with its cause. **The ledger was not edited to make it pass.**

### The two UNRESOLVED IDs

**`PEER-PROBE-2026-07-26` — HIGH.** The Artifact cell names **no openable artifact
at all**. It reads: *"Structural probes (`git ls-tree`, `git grep`) executed against
both peer trees at the `BASE-2026-07-13` commits on 2026-07-26."* That describes a
**method**, not an object. No captured output exists anywhere in the repository.
Six coverage families cite it, and in AUTH-\*, GATEWAY-\*, REACH-\*, PORT-\*,
MEDIA-\* and SUPPLY-\* it carries **half the Delta column** — every "Probe:" clause
rests on it. Contrast `CTRL01-PANEL-2026-07-28`, which names a directory of four
captured files and resolves cleanly. A reader of this repository cannot check a
single probe finding.

*Why this matters for Phase 30 specifically:* 30-03 publishes claims. Any published
claim resting on a peer comparison inherits this. The probes may well be correct —
this finding is that **nobody can tell**, not that they are wrong.

**`F05-TRUTH-{n}` — LOW.** A template ID, not an instance. It names *"Row `{n}` of
the F05 startup truth table, `…-f05-capability-activation-receipt.md` §2"*, where
the leading `…` elides the filename. The concrete citations used in family rows
(`F05-TRUTH-1`, `-2`, `-6`, `-8`) do resolve, because the full path is given one row
earlier under `F05-RECEIPT@0825c92d`. Mechanically unresolvable; resolvable by a
reader who cross-references. Recorded, not escalated.

### The one PARTIAL

**`F28-MATRIX-651` — LOW.** Names `results.json` with no directory. The file is real,
at `phases/28-native-cross-platform-certification/evidence/28-02/results.json`
(364.3 KB), one level deeper than the bare filename implies. Citation imprecision,
not missing evidence.

### An instrument correction worth recording

My first resolver run reported **4 additional UNRESOLVED IDs**. All four were my
own defect, not the ledger's: `git cat-file` was being run on truncated **sha256
build-artifact digests** (`e8431ba2…`) and on **decimal workflow-run ids**
(`30184651330`), which `[0-9a-f]{8,40}` happily matches. A fifth near-miss came from
the ledger's bare-filename sibling shorthand (`<dir>/A.md + B.md`). Had I shipped
the first run I would have filed a HIGH against CTRL-01 for a category error of my
own. **The corrections are in the committed script; only reading the output caught
them.**

---

## 2. Pinned peer baselines — both re-verified, read-only

Method: `evidence/30-01/verify-peer-baselines.sh`. Capture: `peer-baselines.txt`.
The ledger's own sentence about these pins was not trusted; each leg was re-run.
Only `cat-file`, `merge-base`, `show`, `status` and `rev-parse` were used — **no
write verb of any kind in either tree**, and both were left with clean working trees.

| Peer | Pin | `cat-file -t` | ancestor of HEAD | version read at the pinned commit | Ledger records | |
|---|---|---|---|---|---|---|
| Hermes Agent | `dbe734be…` | `commit` | rc=0 ✅ | `git show dbe734be:pyproject.toml` line 10 → `version = "0.17.0"` | 0.17.0 | **AGREES** |
| OpenClaw | `11a0ad10…` | `commit` | rc=0 ✅ | `git show 11a0ad10:package.json` line 3 → `"version": "2026.6.2",` | 2026.6.2 | **AGREES** |

Both HEADs match the ledger's declared forward candidates (`d59b79fa`, `3659c85e`).
`PEER_BASELINE_RESULT=PASS`. **No finding.** This is the ledger's strongest section.

---

## 3. Coverage families — all ten pass all seven clauses

Method: `evidence/30-01/check-families.py`. Capture: `family-clause-check.txt`.
Clauses checked: maturity ∈ the declared eight-state enum; security authority owner
present; evidence IDs present **and drawn from the declared index**; peer baseline is
the pinned token (not `UNPINNED`); delta present; limitation present; last-refresh
phase present.

**Result: 10 of 10 families, zero clause defects, zero undeclared evidence IDs,
zero `UNPINNED`.** CTRL-01's self-declared schema exit condition is **MET**.

This is deliberately reported as a clean result. The plan's NO-MALFORMED-ROW gate
exists precisely so a review that honestly finds few defects is not penalised —
inventing findings to look productive is the failure mode it guards.

---

## 4. Claim-vs-tree — thirteen falsified claims, two controls held

Method: `evidence/30-01/check-staleness.sh`. Capture: `staleness-check.txt`.
Each block states a claim a tracking document makes and re-derives the underlying
fact from the tree. **Where a document and the tree disagree, the tree wins.**

**Crucially, file existence is NOT execution.** A SUMMARY can exist purely to record
a non-start — `23A-02-SUMMARY.md` is `status: not_started`, tagged
`[not-executed, blocked]`, with `provides: []` and `created: []`. Every falsified
claim below is therefore re-checked on the summary's **own declared status**. My
first pass used bare `test -f` and misclassified 23A-02; that is corrected in the
committed script.

| ID | Claim (source) | Tree says | Sev |
|---|---|---|---|
| STALE-01 | PORT-\*: "26-02 … unstarted" | `26-02-SUMMARY.md` `status: complete`, F26-02 claimed, quarantine proven live | **HIGH** |
| STALE-02 | PORT-\*: "26-04 … unstarted" | `26-04-SUMMARY.md` `status: complete`, F26-05 claimed, 19 hostile corpora | **HIGH** |
| STALE-03 | NATIVE-\*: "no soak (28-03 not landed)" | `28-03-SOAK-RESULTS.md` present, `28-03-SUMMARY.md` complete | MED |
| STALE-04 | NATIVE-\*: "no signed platform-binding receipt" | `28-04-CERTIFICATION-RECEIPT.json`, 95.5 KB, signed | MED |
| STALE-05 | NATIVE-\*: "no finding adjudication" | `28-04-FINDING-LEDGER.md`, 73.4 KB | MED |
| STALE-06 | NATIVE-\*: "the phase is not closed" | `28-04-PHASE-VERDICT.md`: C1–C3 MET WITH STATED EXCEPTIONS, **C4 NOT MET** | MED |
| STALE-07 | SUPPLY-\*: 29-03 is the next proof | `29-03-SUMMARY.md` complete | MED |
| STALE-08 | SUPPLY-\*: 29-04 is the next proof | `29-04-SUMMARY.md` complete, termination state 2 | MED |
| STALE-09 | "ROADMAP.md still reads 'Not started' for phases 21–29" | `grep -c 'Not started'` → **0**; reconciled at `e007b907` | LOW |
| STALE-10 | "REQUIREMENTS.md Phase 24 says only 24-01 executed" | register carries its own "is out of date" correction | LOW |
| STALE-11 | REACH-\*: "three of four criteria are NOT MET" | `25-PHASE-STATUS.md` rows 7–10: **4 of 4 carry a bolded MET** (C3 MET on Linux / PARTIAL Windows) | **HIGH** |
| STALE-12 | REACH-\*: cloud leg blocked "for want of a credential only Sean can mint" | `25-CLOUD-SUMMARY.md`; "Criterion 1 is now MET" at `5e620ef0` once Sean minted the Fly credential | **HIGH** |
| STALE-13 | REACH-\*: "no second physical host … no SSH trust relationship" | `25-HOSTS-SUMMARY.md` (21.6 KB); C2 and C4 closed by `lane/25-hosts` | **HIGH** |
| **CTRL-A** | 24-C3 (inbound channel matrix) has NOT landed | ABSENT — **claim holds** | — |
| **CTRL-B** | 30-02 has NOT landed | ABSENT — **claim holds** | — |

**The two controls holding is what makes the other thirteen meaningful.** A checker
that falsifies everything put to it is worthless; this one correctly confirms the
two things that genuinely have not landed, including `24-C3`, which my brief names
as still in flight.

### Severity reasoning, stated because it is a judgement

These are graded on **substance**, not on tidiness. STALE-01/02 and STALE-11/12/13
are HIGH because they are not cosmetic: PORT-\*'s limitation asserts *"the entire
import half is unbuilt"* and *"nothing has yet imported anything … the migration
security boundary has never been crossed"*, and REACH-\*'s asserts three unmet
criteria and two Sean-reserved blockers. **All five statements are false against the
tree**, and both families are candidates for promotion that this review is
structurally barred from performing.

I am the reviewer. **Re-grading is the owning phase's action, not mine** — that is
the whole point of the read-only fence. So these HIGHs are discharged the only way a
reviewer can discharge them: named, evidenced, severity-stated, and handed to the
row owner. They do **not** block 30-02.

---

## 5. What this review is NOT able to settle

Stated rather than smoothed:

- **Whether the ten families are the right partition of the shipped surface.** §6 of
  the surface inventory shows six shipped top-level commands that no family claims.
  Changing the partition is the owning phases' decision.
- **Whether an evidence ID resolving today still resolves later.** All 42 were
  resolved at `eab69cdb`. 30-04 must re-check.
- **Whether the Delta column's peer half is still true.** Every Delta is
  static-source, bound to `BASE-2026-07-13`, and rests partly on the UNRESOLVED
  `PEER-PROBE-2026-07-26`. The ledger flags eight Delta clauses as possibly-stale
  **about Core** and leaves them for F30; this plan confirms all eight are worth
  adjudicating but adjudicates none — 30-02 and 30-03 own that.
- **Per-family maturity judgements cannot be reduced to a command.** Where §4
  reasons from status text, the exact text is named so any reader can overturn it.

## 6. Findings binding the rest of Phase 30

**CRITICAL: none.**

**HIGH (6):**
1. `PEER-PROBE-2026-07-26` names no openable artifact, yet carries half the Delta
   column in six families. Any 30-03 claim resting on a peer comparison inherits it.
2–6. STALE-01/02/11/12/13 — PORT-\* and REACH-\* materially understate what landed.
   **Phase 30 must not position from the ledger's Limitation columns as written.**

**MEDIUM (8):** STALE-03..08 (NATIVE-\*, SUPPLY-\* dated by ~13 hours); the six
unowned shipped surfaces (§6 of the inventory); the walker's alias blind spot.

**LOW (2):** `F05-TRUTH-{n}` template ID; `F28-MATRIX-651` bare-filename citation.

**REFUTED: none.** No finding I raised turned out not to hold — but four candidate
findings were withdrawn before filing because they were **my instrument's defects**,
and those are recorded in §1 rather than deleted.
