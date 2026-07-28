# Phase 30 — Continuous Scorecard and Frontier Review: PHASE VERDICT

**Graded** 2026-07-29 at `32cc7ac8f64d43d9c7d937c105013c13e9bbce95`, lane `lane/30-04`, on
`hetzner-dsm`.

**Machine-verified.** `evidence/30-04/phase-verdict.json` driven through the shipped
`wayland-scorecard verify` → `SCORECARD_VERIFY=OK criteria=4 surfaces=0`, rc=0. That path
refuses `MET` and `MET_WITH_STATED_EXCEPTIONS` when any cited evidence reference fails to
resolve or is marked unproven, so a grade here is enforced by code rather than caught by a
reader. Re-proved at the end of the phase: see §3.

---

## 0. The verdict in one paragraph

**Phase 30's goal was not achieved.** Two of the four Success Criteria are NOT MET, one is
PARTIAL, and one — the claims criterion — is MET WITH STATED EXCEPTIONS. The phase built four
good instruments and then ran them honestly, and what they returned was mostly *refusals*: zero
peer comparisons publishable in either direction, two of five comparative dimensions never
measured, two of six per-surface truths unproven on 100% of rows, and every reserved action
structurally unreachable.

**That is a real result, not a failed one.** The single most valuable thing this phase produced
is the demonstration that the refusal machinery works against its own interest: 30-03 declined
to publish an *unflattering* comparison it could have shipped for credibility, because the
number did not measure the dimension it was named after.

**But the phase goal — "Sean can make a defensible release-positioning decision from repeated,
comparable, independently reviewed evidence" — is NOT ACHIEVED**, and §5 says why plainly.

---

## 1. The four Success Criteria, quoted verbatim from `.planning/ROADMAP.md`

Each is reproduced **verbatim**. Quietly rewording a criterion until the evidence in hand
satisfies it is the specific forgery a verdict is most exposed to. **The criteria are fixed;
the verdict moves.**

---

### Criterion 1 — **NOT MET**

> 1. Every surface has versioned activation, operator-completeness, maturity, security-owner, evidence, and peer-delta truth refreshed at each phase.

**Evidence.** `30-01-SURFACE-INVENTORY.md`; `evidence/30-01/surface-truths.tsv`, 148 data rows,
driven through the real `SurfaceRowV1` verifier. Measured per column, directly from the
committed table:

| Truth | Unproven on |
|---|---|
| versioned activation | **0 / 148** |
| operator-completeness | **148 / 148** |
| maturity | 15 / 148 |
| security-owner | 15 / 148 |
| peer-delta | **148 / 148** |

**Two of the six required truths are unproven on every single row.** The criterion is
universally quantified — *"Every surface has …"* — so a truth unproven on 100% of rows fails it
outright.

Three further clauses fail independently:

- **"Every surface" is not what was measured.** The walk takes the binary's *advertised* command
  tree. `forgeflows` is a hidden clap alias that **runs live**, has no inventory row, and
  therefore has no security owner. 30-01 found and recorded this against its own strongest
  artifact rather than papering over it.
- **15 rows / 6 top-level commands are owned by no coverage family at all** — `init`,
  `mcp-serve`, `models`, `profile`, `project-context`, `setup`. Three are first-run and
  credential-adjacent, which is where unowned surface matters most.
- **"refreshed at each phase" has no evidence.** This is a *first* refresh. One refresh is not a
  demonstrated cadence.

**What genuinely landed**, and it is not nothing: a byte-deterministic inventory taken off the
shipped binary by running it (regeneration diff `IDENTICAL`, 149 lines each side, binary sha256
asserted before and after), the closed maturity enum, and `MaturityTruthV1` — which exists
because 30-01 refused to add an enum member meaning "nobody has graded this".

**Dissent recorded.** Four of six truths hold on 133/148 rows and the instrument now exists; a
reader who takes the quantifier as per-truth rather than per-surface would reach PARTIAL. The
numbers above are given so that reader can overturn this grade.

---

### Criterion 2 — **NOT MET**

> 2. Wayland, Hermes, and OpenClaw complete common correctness, recovery, security, cost, and cognitive-tax trials with pinned baselines and confidence bounds.

**Evidence.** `30-02-TRIAL-RESULTS.md`; `evidence/30-02/legs.tsv` — 15 legs, **9 RUN, 6
UNPROVEN**, each accounted for exactly once.

**Two of the five named dimensions have zero legs, for all three tools:**

- **security 0/3** — the shared loopback meter records `body_sha256`, `semantic_body_sha256` and
  per-leaf hashes; it does not retain bodies, so the frozen canary byte-search was never
  performable. A strictly narrower exact-leaf extraction was **deliberately not substituted**,
  because silently narrowing an extraction after the fact is how a protocol stops meaning what
  it said.
- **cognitive tax 0/3** — all four panel members independently refused to proxy it, *before any
  trial ran*. Codex: *"configuration-line counts, command counts and elapsed time are not
  cognitive tax — encode `NOT_MEASURED`, not zero."*

**And the nine legs that did run are confounded.** The canonical script emits a tool call named
`write_file`, a name only Hermes exposes; Wayland Core's equivalent is `Write`; OpenClaw scored
0/30 on the identical script at its own pinned commit. Two of three harnesses failed one script.
The numbers are real and they do not measure the dimension they are named after.

The consequence is mechanical, not advisory: 30-03's rule
`confounded_leg_supports_no_comparison` refuses **every** comparison resting on those legs, in
either direction, and **zero peer comparisons were published**.

**What genuinely landed:** both peer baselines re-verify exactly at their pinned commits;
real confidence bounds on all nine RUN legs (Wilson, and `ZERO_EMPIRICAL_VARIANCE` reported as
such rather than dressed up as a zero-width interval); a fourth verdict state `INCONCLUSIVE`
that earned its keep on a real row; and OpenClaw obtained the hard way after a 42% transfer
failure — the leg that converted "Wayland is broken" into the far better-supported "two of three
harnesses do not expose a tool named `write_file`".

**Why not PARTIAL** (the grade the plan itself flagged as arguable): three of five dimensions
produced numbers with pinned baselines and bounds, which is literally partial completion. It is
NOT MET because **zero of the five dimensions produced a usable comparative** — two produced
nothing, and three produced numbers the phase's own checker refuses to compare. The criterion
exists to yield comparable evidence. On this run it yielded none.

---

### Criterion 3 — **MET WITH STATED EXCEPTIONS**

> 3. Published claims and limitations match raw redacted evidence and contain no unsupported superiority language.

**Evidence.** `30-03-CLAIMS-ALLOWED.md`, `30-03-CLAIMS-PROHIBITED.md`, `30-03-LIMITATIONS.md`,
`evidence/30-03/attack-corpus.tsv`. All four resolve; all four are marked proven; the asymmetric
verifier was paid for this grade.

**"match raw redacted evidence" — publication is a code path, proved both ways:**

| | |
|---|---|
| re-render all three documents from the committed register and diff | **byte identical** |
| tamper test — append one flattering sentence, re-diff | **DETECTED** |
| publish against a register with one broken reference | **REFUSED**, wrote nothing, named the offender |

The tamper test is what stops the clean diff passing vacuously, and the refusal gate lets the
remote side decide so that ssh being down cannot satisfy it.

**"no unsupported superiority language"** — twelve typed refusal rules, **12 distinct rules
actually fired** across a 24-row paired corpus (12 accepted, 12 refused). No rule is unattacked.
Pairing is *structural*: `AttackCase::new` takes the pristine and the mutation positionally, so a
case without its control cannot be constructed.

**The stated exceptions**, all three named by 30-03 itself and none discovered afterwards:

- **E1** The checker proves a claim points at something that exists, is bounded, is scoped
  correctly and is classified consistently. **It cannot prove the sentence means what the
  evidence shows.** That residual is a reading task.
- **E2** The comparative lexicon is finite; a sufficiently creative sentence can compare without
  matching it. Scope containment and the mandatory evidence pointer do more work than the
  lexicon.
- **E3** The evidence-bundle secret scan holds **zero** secrets, so in production it cannot fail
  and proves only that the mechanism ran — stated in `MANIFEST.tsv` itself.

**The finding that makes this criterion worth the grade.** `ATK-11` takes a real ledger
fragment, quotes it **verbatim** — *"This is Core's clearest unique capability"* — and severs it
from the `runtime certification required` qualifier its family carries. Same evidence, same
scope, same source document. With the qualifier it is accepted; without it the checker refuses
it as `unbounded_superiority`. **A quotation can be word-for-word accurate and still fabricate a
claim, by dropping the clause that withheld it.** This verdict is exactly the kind of document
that defect attacks, and the counter-measure is that every criterion above is quoted *whole*.

---

### Criterion 4 — **PARTIAL**

> 4. No main merge, issue closure, release, deployment, or frontier positioning occurs without Sean's explicit approval.

**Evidence.** `30-04-AUTHORITY-PROOF.md`; `evidence/30-04/authority-audit.tsv` — 10
determinations, 10 well-formed of 10, each naming a capture that exists.

**Affirmatively proven:**

- The mechanism exists and fails closed. Nine reserved actions as a **closed** enum backed by a
  compiler-checked exhaustive array; **one** principal and it is not the agent; nine **distinct**
  per-action signature domains; subject-bound approvals. 14/14 contract tests on hardware.
- The bundled root is the all-zeros placeholder and **refuses every approval**, proved on
  hardware, with an error naming its own substitution point.
- The verifier is not merely one that refuses everything: a throwaway root generated at run time
  **accepts** a valid approval. Positive control, run first.
- **Frontier positioning demonstrably did not occur.** 30-03 published zero peer comparisons in
  either direction; this plan's packet is gate-checked free of readiness vocabulary. This is the
  single action the phase exists to protect, and it is *proven*, not merely unobserved.
- This lane merged nothing: HEAD is **not** an ancestor of `main` on the GitHub remote, measured
  by read-only `ls-remote` with a falsification control proving the check can answer YES. No tag
  points at HEAD. Retained evidence refs intact at exactly 37.

**Not observable, for any actor, over the whole phase:** issue closure, release publication and
deployment write **no git object at all**; pull requests are GitHub state and `refs/pull/*` is
not fetched here; and no baseline snapshot of `main` was taken at phase start, so *"did some
other lane merge to main during this phase"* is unanswered.

**Cross-audit panel** (captures in `evidence/30-04/panel/`): codex `gpt-5.6-sol` **PARTIAL**,
gemini `3.1-pro` **PARTIAL**, kimi K3 **UNPROVEN**. An internal adversarial pass argued *for*
MET WITH STATED EXCEPTIONS and failed on its own premise — it required assuming that across
roughly twenty concurrent lanes nobody closed an issue, cut a release or deployed anything,
which is precisely the self-report the audit was built to replace.

**Why the majority carries.** All three agree MET WITH STATED EXCEPTIONS overclaims: the
unobservability of half the criterion's surface is not a marginal exception. The split is only
PARTIAL versus UNPROVEN. Kimi's dissent — that PARTIAL implies a known partial *failure*, and no
clause is proven violated — is well made. It does not carry because **UNPROVEN would erase the
affirmatively proven half**, and above all would say we do not know whether frontier positioning
occurred. We do know. It did not, and that is measured.

---

## 2. The phase goal, graded

> **Goal**: Sean can make a defensible release-positioning decision from repeated, comparable, independently reviewed evidence.

**NOT ACHIEVED.** Clause by clause:

| Clause | State |
|---|---|
| **independently reviewed** | **Achieved.** 30-01 reviewed CTRL-01 by resolving 42 evidence IDs with commands, found 6 HIGH, and was structurally barred from repairing what it graded. |
| **repeated** | **Not achieved.** Every measurement in this phase is a first measurement. No dimension has a second data point, and the per-phase refresh cadence Criterion 1 requires has no evidence. |
| **comparable** | **Not achieved.** Zero of five dimensions produced a usable comparative: two never ran, three are confounded. |
| **defensible release-positioning decision** | **Not supported.** A positioning decision needs comparative evidence. There is none — by the phase's own refusal, which was correct. |

**The frontier position cannot yet be stated, and this is what it would take to state it**, in
ascending cost:

1. **Per-tool dialect compilation and a re-pre-registered protocol v2** (KEY-08). Needs no
   credential, no account and no authorisation — only a new pre-registration and a re-run of the
   nine legs. **Without it Criterion 2 cannot be re-graded at all.** This is the cheapest and
   most consequential item in the whole packet.
2. **Request-body retention or leaf-hash exposure in the shared meter** (KEY-02), which unblocks
   the three security legs. Release-coordinated; seam request open.
3. **A per-surface comparative pass** against both pinned peer trees, which is the only thing
   that moves `peer_delta` off UNPROVEN on 148/148 rows.
4. **A second refresh at a later phase**, which is the only thing that can turn one measurement
   into the cadence Criterion 1 asks for.
5. **A cognitive-tax study outside the scripted tier** (no fixture substitution can produce it),
   and **a live-provider tier with real billing** for real dollar cost.

Items 1–4 need no credential from anybody.

---

## 3. How to check this verdict is worth what it claims

**The grades are enforced by code.** `evidence/30-04/phase-verdict.json` verifies through the
shipped binary. And the asymmetry was re-proved at the end of the phase rather than assumed from
the start: forcing every `NOT_MET` upward to `MET` is **refused**, naming the reference that
refused it —

```
FORCED_COUNT=2
REFUSED_AS_REQUIRED
wayland-scorecard: criterion `CRIT-01` is graded MET but evidence reference
`OPERATOR-COMPLETENESS-148-OF-148` is marked UNPROVEN
```

— and the gate is structured so that ssh failing, the checkout failing or the binary being
missing cannot satisfy it: the remote side decides, an unexpected acceptance exits 9, and only
the refusal exits 0.

**Note the plan's own forcing gate would not have worked.** It seds `"verdict": "NOT_MET"`, but
the field `CriterionV1` actually carries is `grade`. Run literally it is a **no-op**, which would
have produced an unchanged document, an accepted verify, and a reported `UNEXPECTED_MET_ACCEPTED`
— failing for the wrong reason. Measured (`PLAN_SED_IS_A_NO_OP=YES`) and corrected. Filed as
`F-30-04-003`.

---

## 4. Findings

### HIGH — carried, with owners

| ID | Finding | Owner |
|---|---|---|
| `F-30-01-001` | `PEER-PROBE-2026-07-26` names **no openable artifact** yet carries roughly half the Delta column in six coverage families. A reader cannot check a single probe finding. | CTRL-01 row owners |
| `F-30-01-002/003` | STALE-01/02 — PORT-\* asserts *"the entire import half is unbuilt"* and *"the migration security boundary has never been crossed"*. **Both false**: 26-02 and 26-04 are complete with requirements claimed. | Phase 26 row owner |
| `F-30-01-004/005/006` | STALE-11/12/13 — REACH-\* asserts three unmet criteria and two Sean-reserved blockers. **All false**: `25-PHASE-STATUS.md` carries four bolded MET, the cloud credential was minted, and the second-host lane closed C2 and C4. | Phase 25 row owner |
| `F-30-03-002` | **The truncated-hedge defect class.** A verbatim quotation can fabricate a claim by dropping the clause that withheld it. Carried as `ATK-11`; the checker refuses it as `unbounded_superiority`. | closed mechanically in 30-03 |
| `F-30-04-001` | **The ROADMAP status column is stale against the tree.** Its Phase 28 row states *"no phase verdict exists yet — 28-04 … has not started"* and its Phase 29 row states *"29-03 and 29-04 not started"*. **All four artifacts exist on disk** (`28-04-PHASE-VERDICT.md`, `28-04-SUMMARY.md`, `29-PHASE-VERDICT.md`, `29-04-SUMMARY.md`). This is 30-01's STALE-06/07/08 still unrepaired, and it matters here because this verdict's own criteria are quoted from that file. **The criteria text itself is current; only the progress table is stale.** | ROADMAP owner |

### MEDIUM — filed to BACKLOG, non-blocking

| ID | Finding |
|---|---|
| `F-30-04-002` | The plan's ref-count gate counts `refs/remotes`. It reads **275** against a floor of **37**, so it could not detect the deletion of 238 refs. The *tight* count (tags + `refs/f20a/*`) is **37**, exactly the baseline. |
| `F-30-04-003` | The plan's MET-forcing gate seds `"verdict"`; the field is `"grade"`. Run literally it is a no-op. |
| `F-30-04-004` | The plan's `read_first` states *"this repository has NO remote-tracking refs at all, which is what bounds the audit."* **False** — there are 238, and `gh/main` is among them. The audit ceiling is narrower than the plan assumed. |
| `F-30-04-005` | The plan's verdict-verify gate passes `--root`; the CLI argument is `--repo-root`. |
| `F-30-04-006` | AUTH-03 ("no local `main` contains HEAD") passes **vacuously** — there is no local `main` branch at all. It would pass at base and would pass after a remote merge. AUTH-07 is what actually carries that determination. |
| `F-30-01-M*` | Six shipped top-level commands owned by no coverage family; the walker's clap-alias blind spot. |
| `F-30-01-L*` | `F05-TRUTH-{n}` template ID; `F28-MATRIX-651` bare-filename citation. |

### LOW — closed in this plan

| ID | Finding | Disposition |
|---|---|---|
| `F-30-03-001` | `30-02-SUMMARY.md` §Gates transcribed its own authoritative gate as `run=6 unproven=9 comparatives=3` — inverted and halved. Its own capture, `legs.tsv` and its own frontmatter all say `run=9 unproven=6 comparatives=6`. | **FIXED at source** in this plan (`b8556eb7`). Prose only; no measurement changed; `evidence/30-02/` gate-checked untouched. |

### Instrument defects measured in this lane

Recorded because on this program the instrument that hunts a defect class keeps carrying it.

1. **My own falsification harness manufactured a self-passing gate.** `git show BASE:path > file`
   creates the file even when `git show` fails, so an absent-at-base file appeared
   present-and-empty and `test -f` passed. Six of seven gates still went red on their grep legs,
   which is exactly what hid it. Re-run with `git archive | tar -x`, **all seven are red**.
2. **`rtk` silently filtered `git for-each-ref`** — an interactive listing printed nothing while
   `grep -c` on the identical pipeline counted 2. Same class 30-03 measured on `git log`. Not
   fabricated, *filtered*, which is worse because it looks complete.
3. **My own anchored regex lost every match** — adding `%(objectname)` put a SHA after the
   refname, so `/(main|master)$` matched zero where the unanchored form matched two. Verbatim the
   trap the lane brief names, walked into while writing the check meant to catch it.
4. **The panel's first `GRADE=` match is the echoed prompt** and extracts as an empty vote.
   Taking the first match would have produced a silent 0-0-0 panel. The last match is the answer.
5. **`codex exec` blocks reading stdin** when not given `< /dev/null` — it produced 39 bytes
   ("Reading additional input from stdin...") and timed out at 400s, twice, before the cause was
   found by byte-counting the capture.

---

## 5. Requirement closure position

**No F30 requirement is marked complete by this plan.** Stated here rather than by editing the
traceability table from inside the phase being graded.

| Req | Position |
|---|---|
| **F30-01** — independently review the versioned capability ledger | **MET.** 30-01 resolved all 42 evidence IDs by running something, re-verified both peer baselines read-only, checked all ten families against all seven clauses, and filed 6 HIGH without editing the row it graded. |
| **F30-02** — the scorecard is refreshed at every admitted phase | **NOT MET.** One refresh, not a cadence. F30 did independently review rather than first-discover peer gaps, so the second clause holds; the first does not. |
| **F30-03** — common five-dimension trials with pinned baselines, repeated trials and confidence bounds | **NOT MET.** 9/15 legs; two dimensions never ran; the nine that ran are confounded. Pinned baselines and confidence bounds *are* satisfied. |
| **F30-04** — claims allowed / prohibited / limitations / raw redacted evidence published without unsupported superiority language | **MET WITH STATED EXCEPTIONS.** Published, machine-enforced, re-render-verified, tamper-detected. Three exceptions named in Criterion 3. |
| **F30-05** — Sean explicitly approves any source push, frontier positioning, main merge, issue closure, release or deployment | **PARTIAL.** The mechanism is landed and proved both ways and positioning is structurally unreachable; the issue-closure, release and deployment surfaces are unobservable from here. |

---

## 6. What this phase did NOT do

- **It did not position.** No plan in Phase 30 contains a recommendation, a readiness statement
  or a market comparison. `30-04-POSITIONING-PACKET.md` says so at the top and its index is
  gate-checked free of readiness vocabulary.
- **It published no peer comparison**, favourable or unfavourable, hedged or otherwise.
- **It took no reserved action.** No merge to main, no pull request, no tag, no release, no
  deployment, no issue closure, no deletion of a retained evidence ref, no `wcore-contract
  generate`. Committing and pushing the lane branch is not a reserved action.
- **It used no credential.** Sean's approval key was never obtained, requested or simulated. No
  gate in this phase requires a secret to pass, and none can be passed by supplying one.
- **It did not launder anything unproven into proven.** `security ×3` remain UNPROVEN with the
  meter seam as their substitution point; `peer_delta` remains measured-UNPROVEN on 148/148;
  the confounded legs remain confounded.
- **It did not weaken a test to reach green.** No `#[ignore]`, no `#[allow]`, no re-gating, no
  deletion, no raised timeout. Where an inline test failed, the **oracle** was corrected and the
  code was left alone, because the code was right.
- **It did not assert "zero known defects."** Amendment A3 binds, and §4 lists what is open.

---

## 7. Known unknowns, recorded rather than resolved

- Whether the confounded legs would show a different result under a dialect-compiled protocol.
  **Nobody knows, and this phase deliberately did not guess.**
- Whether the six unowned shipped surfaces have security-relevant behaviour. Unreviewed is not
  the same as unsafe, and neither is claimed.
- Whether the `PEER-PROBE-2026-07-26` findings are correct. They may well be; the finding is that
  **nobody can check**.
- Whether any actor closed an issue, cut a release or deployed anything during this phase. Not
  observable from this repository, for anybody.
- What Sean concludes from the packet — **the one thing this phase was built never to answer.**
