# Phase 28 — Native Cross-Platform Certification: PHASE VERDICT

**Adjudicated** 2026-07-28 at base `cf48b349d3aa84f85168511431b0a248fd50ded9`, lane `lane/28-04`.
**Receipt** `28-04-CERTIFICATION-RECEIPT.json`, body digest
`2037352cff1c2f2c8f8b35e59289ba73b514cd56977c8e22d599ed45e49e0fbb`.
**Phase-scoped key** `phase-28-certification-2026-07-28`, public half
`Ks20+wo/p7Jeaa0c5DY4ex6ylMrIDfhs4TsWQ/6apIE=`, fingerprint
`f0ef7d06c620b23c1ad84cc083d0a3a01c0c1ca7270a1cfdd5e46c9b050ed466`.

---

## 0. What this document is, and what it is NOT

**THIS IS NOT A SEAL. THIS IS NOT A TRUST ROOT. THIS IS NOT A RELEASE.**

The receipt is **EVIDENCE, NOT AUTHORIZATION**. The key above is phase-scoped: it was minted
for this certification run and is bound to no release trust root. A phase-scoped signature says
exactly one thing —

> *this evidence was assembled by this certification run and has not been altered since.*

It does **not** say *this build is released*, *this build is approved*, or *this build is safe*.
Binding a key to a release trust root, rotating one, publishing one, tagging, releasing, opening
a pull request, merging to main and closing an issue are all **outside this phase**: trust-root
work belongs to Phase 29, and the outward actions are reserved to Sean. None was performed.

**What the certification asserts is exactly three things**, and amendment A3 is enforced by two
independent verifiers rather than by good intentions:

| Claim | Value |
|---|---|
| `zero_skipped_critical_cases` | **true** |
| `zero_undispositioned_findings` | **false** |
| `zero_unresolved_critical_or_high` | **false** |

**It does NOT assert "zero known defects" and it does NOT assert "zero findings."** A receipt
attempting either is rejected with `F28R-OVERCLAIM` / `F28V-OVERCLAIM`, demonstrated against a
mutated copy of this very receipt in `evidence/28-04/verifier-rejections.log`.

**The acceptance gate did NOT pass.** Two of the three claims are false, and the receipt says so
rather than claiming otherwise.

---

## 1. The four Success Criteria, quoted verbatim from `.planning/ROADMAP.md`

Each is reproduced **verbatim** and machine-checked against ROADMAP.md by
`f28-verify-bindings.py --check-verdict`. Quietly narrowing a criterion until the evidence in
hand satisfies it is the specific forgery a verdict plan is most exposed to. **The criteria are
fixed; the verdict moves.**

---

### Criterion 1 — **MET WITH STATED EXCEPTIONS**

> 1. Native macOS, Linux, and Windows pass the required hostile platform matrix with no skipped critical case.

**Evidence.** 651 E5 matrix cells across all three families over all nine required dimensions
(sandbox probes, Unicode, long paths, UNC/reparse/symlink, process cleanup, suspend/resume,
offline, disk-full/read-only, hostile inputs): **linux 216/216 pass, macos 216/216 pass, windows
219/219 pass. 0 red. 0 skipped. 147 critical cells, 0 skipped critical cases.** Every count in
that sentence was recomputed by counting the raw cell list, not read from a summary.

Every `sandbox-probes` cell additionally required a **positive activeness observation** — a
containment differential — so a green cannot be indistinguishable from a silently disabled
sandbox. Linux: PID/mount/network namespace differences. Windows: `0xC0000022` on
`\BaseNamedObjects`, which AppContainer confines by construction. macOS: DNS resolves outside and
not inside, `/etc` readable outside and denied inside.

The Windows observability question that conditioned all of this was **settled by control before
any sandbox cell was graded**: 3 positive and 3 negative directional controls, all caught, at
both scheduled-task and SSH session types. The `observation-blocked` skip class was **never
authorised and never used**.

**LIVE evidence, stated separately.** All 651 cells were produced by executing the real
digest-bound `wayland-core` release binary on three physical/hosted machines — `hetzner-dsm`, the
certification Mac, and `seandesktop`. In addition, the two macOS members of the `wcore-sandbox`
live acceptance surface executed on a real macOS host for the **first time ever**: GitHub Actions
run **30364529551** at this exact base SHA `cf48b349`, both cases `running 1 test` →
`test result: ok. 1 passed; 0 failed`, and — the load-bearing check — **no `skip:` line**, which
is what separates a real run from the two early returns each test opens with. I verified
independently that zero output lines begin with `skip:`; the only three occurrences in the
workflow log are the gate echoing its own source.

**I read those two test bodies rather than accepting their green,** because 0.05 s and 0.04 s is
fast for a live containment case. They are substantive, and the speed is load-bearing rather than
suspicious: `hard_process_containment_macos` runs `/bin/sh -c '/bin/sleep 45 & exit N'` under
sandbox-exec twice and asserts wall clock **< 20 s**, so a backend that failed to reap would leave
the detached `sleep` holding the stdout pipe and `execute` would block to 45 s or the 30 s manifest
timeout. **A non-reaping backend physically cannot produce 0.05 s — fast IS the pass condition**,
and the bound is one-sided in the safe direction, because load can only produce a false FAIL.
`live_integrity_macos` is a matched pair whose inside-write half is the built-in control
separating "the sandbox contained it" from "the sandbox failed to launch". The `1 filtered out` in
each run is fully accounted for: each binary contains exactly two functions, the `#[ignore]`d case
and the always-running `zero_execution_guard`, and `--ignored` selects only the former.

**Stated exceptions — three, and they are why this is not a bare MET:**

1. **The coverage spans TWO candidates.** Linux and Windows are 28-02's run at `32e2f57d`; the
   macOS matrix re-run is at `e4a3f5fc`. **No single-candidate full matrix exists for this
   phase.** Eleven merges landed between them. (`F-28-04-004`)
2. **The macOS activeness observation is run-level** — one containment differential applied to
   all 24 macOS sandbox cells rather than one per cell. Raised by 28-02, carried by 28-03, still
   true. (`F-28-04-007`)
3. **The two macOS matrix members use no containment differential** — one infers reaping from a
   wall-clock bound and the other from a matched write pair — so macOS evidence rests on two
   different instruments rather than one. (`F-28-04-011`)

The "no skipped critical case" clause is **met outright and unconditionally.**

---

### Criterion 2 — **MET WITH STATED EXCEPTIONS**

> 2. The 1,000-session/concurrent-child soak has no secret leak, orphan process, unbounded resource use, or unacceptable quality/performance delta.

**Evidence.** **3,000 of 3,000 sessions** at concurrency 4 — linux, macos and windows each
completed 1,000/1,000 against a digest-bound binary asserted before the first session.

| Observable | Result | Its positive control |
|---|---|---|
| secret canaries | **0 detections**, six channels scanned on every family | control canary planted in all six and **detected in all six**, every family |
| orphan processes | **0**, every family | a deliberately orphaned **product** process **FOUND** by every census |
| resource slopes | every metric in band | a deliberately growing lane **flagged** by the same evaluator |
| quality / latency drift | every band green; both families got *faster* late in the run | — |

**Every observable carries a positive control, and a missed control makes the observable VOID
rather than green.** That is the difference between a clean scan and an absent scanner, and it is
the rule the whole soak was built on. `--json-stream` was tried as the orphan control and
**replaced**, because it exits immediately without a credential and a control that has already
died proves nothing about whether a census can see a live one.

The delta bands were decided by **unanimous four-way cross-audit and committed at `1dea6437`,
before any soak session existed**; the record landed at `a0ca3ecf`, and the ordering is provable
from `git log`. `bands.json` declares `numbers_are_measured: false` and the validator rejects the
file if that is flipped. **No band was widened after measurement.**

**LIVE evidence, stated separately.** All 3,000 sessions drove the real binary on real hardware.
`KR-01` — the carried Windows descendant-reap red that 28-03 predicted would force this criterion
NOT MET — was **tested on `seandesktop` and DISPROVED**: 12/12 serial passes across competing load
from 4 to 32 processes, witnessed by host-side CIM as
`DESCENDANTS_ALIVE_BEFORE_DROP=[31360]` → `SURVIVORS_AFTER_DROP=0 of 1`. **The witness was
STRENGTHENED, not the assertion relaxed** — from heartbeat-file length, which cannot separate
"reaped" from "starved" and biases toward a false pass under load, to fixed-`ProcessId` liveness,
with a panic if no descendant is ever observed so a run that proves nothing now fails as
unmeasurable.

**Stated exceptions — two:**

1. **The macOS orphan census is NON-AUTHORITATIVE.** It observes a process group and a hostile
   descendant can leave one, so its zero is a zero *observation*, not a containment guarantee.
   Linux (cgroup-v2) and Windows (job object) are authoritative. The instrument is weaker, not
   absent — it found its planted control. (`F-28-04-005`)
2. **The workload is read-only by construction**, so `state_dir_bytes` was 301 bytes at the first
   sample and 301 at the last. A green there means *"a thousand read-only sessions wrote
   nothing"*, **not** *"the product does not accumulate state under use"*. The bands document
   recorded the related limit before any number existed: in a soak of 1,000 fresh short-lived
   processes a per-process leak cannot accumulate, so detection weight sits on the slope bands
   rather than the drift bands. (`F-28-04-006`)

Also carried: the soak ran at `e4a3f5fc` while the Linux and Windows matrix ran at `32e2f57d`
(`F-28-04-004`).

---

### Criterion 3 — **MET WITH STATED EXCEPTIONS**

> 3. Signed receipts bind exact candidate, platform, posture, corpus, environment, artifacts, logs, and skip policy.

**Evidence.** All eight bindings are present in `28-04-CERTIFICATION-RECEIPT.json`: 2 candidates
(per scope), 3 platforms, 4 postures, 2 fixture corpora, 5 environments, 23 artifacts, 42 logs
and the skip policy, plus the 63-finding ledger as a first-class section. The schema is versioned
and every struct is `deny_unknown_fields`, so an older reader **fails closed** rather than
silently ignoring a section it does not understand.

**The receipt is verified TWICE, by different means, and both must pass:**

- the **Rust** `CertificationVerifier` checks the body digest, the phase-scoped signature and the
  schema rules — and it reads **the real produced artifact off disk**, not only fixtures:
  `49 tests run: 49 passed` on `hetzner-dsm`, printing
  `verified 63 findings, gate_passed=false, unresolved CRITICAL/HIGH = ["F-28-02-002"]`;
- the **independent** `f28-verify-bindings.py --verify` **RECOMPUTES** every digest and count
  from the raw evidence — cells counted from the cell list, digests computed off disk, hosts read
  out of run records, the three claims recomputed from the raw ledger — and rejects any
  disagreement, naming the field.

**Two independent checkers that must agree is materially stronger than one checker run twice**,
and they agree. Their A3 allowlists are cross-read from each other's source so they cannot drift.

**The verifier has been seen to say NO**, against mutated copies of this very receipt
(`evidence/28-04/verifier-rejections.log`), each with the correct specific code:

| Mutation | Rejection |
|---|---|
| assert `zero_known_defects` | `F28V-OVERCLAIM` |
| assert `zero_findings` | `F28V-OVERCLAIM` |
| record a skipped critical case | `F28V-SKIPCRIT` + `F28V-SKIP` |
| flip a false claim to true | `F28V-CLAIM` — *recomputed False from the raw ledger* |
| drop one enumerated finding | `F28V-ENUM` |
| inflate a cell count by one | `F28V-PLATFORM` — *claims 217, recomputed 216* |
| rewrite a candidate commit | `F28V-CANDIDATE` |
| forge a log digest | `F28V-LOG` |
| flip one byte of the body | `F28R-DIGEST` |
| control: the real receipt | **all four gates OK** |

**The verifier caught two real problems on its first run against the real receipt, and both were
fixed at cause rather than by loosening the rule.** An empty-log rule fired on a zero-byte stderr
capture whose emptiness is the good news; it was **sharpened**, not relaxed — it now fires exactly
when an empty file is *cited as finding evidence*, which is the only place one could carry a
claim. And a tamper fixture set `cells_red` to 0 on a family already at 0 — a no-op mutation
asserting that nothing changes the digest — so every mutation now asserts it actually mutated
before its verdict is trusted. That second one is this phase's own defect class appearing inside
the checker built to catch it, and it is recorded rather than quietly corrected.

**Stated exception — one.** "Exact candidate" is honoured by binding **both** candidates, each
per scope with its own commit, tree and per-target binary digests, rather than picking one and
calling it "the" candidate. **The binding is exact and honest; what is split is the coverage.**
(`F-28-04-004`)

---

### Criterion 4 — **NOT MET**

> 4. Zero findings remain at every severity before acceptance.

**"Resolved" is implemented as DISPOSITIONED**, per the `c4-disposition` decision taken 4-0 at
planning time with three binding amendments, recorded with its dissent in
`28-01-decision-evidence/`. The gate is *zero findings at any severity lack an explicit,
evidence-backed terminal disposition*, with FIXED and DISPROVED the only options at CRITICAL and
HIGH. **Criterion 4 is emphatically NOT satisfied by an empty ledger** — a program that fixed
nothing but silently deleted its MEDIUM findings would satisfy a literal empty-ledger gate while
destroying exactly what the criterion protects.

**63 findings adjudicated. 62 carry a terminal disposition. ONE does not.**

> **`F-28-02-002` — HIGH — OPEN.** The stale AppContainer lease wedge is a **persistent denial of
> service**: a file nobody knows to look for permanently refuses all sandboxed execution, with a
> message that reads like a platform limitation.

At HIGH the only available dispositions are FIXED and DISPROVED. The finding is real and
CONFIRMED by control, so DISPROVED is unavailable; repairing a production defect is outside plan
28-04's scope by design, so FIXED is unavailable here. **It therefore remains OPEN, the acceptance
gate does not pass, and Criterion 4 is NOT MET.**

**The re-score that would have made this criterion pass was declined deliberately, and a later
reader is entitled to know that.** A MEDIUM reading is genuinely arguable under the contract's
§3.1 bands: `F-28-02-002` contradicts no criterion, the Windows matrix passed 219/219 in the
as-found state, and the wedged state was reached only by the control. **MEDIUM would open
ACCEPTED and DEFERRED and this criterion would read MET.** Re-scoring a finding downward so its
accept path opens is one of the three named forgeries an adjudication plan is most exposed to, so
the row keeps the severity **Phase 28's own plan 02** gave it. A later reader may reopen the
score deliberately; they should read the ledger row first.

**What the ledger does show, and it is not nothing:**

| | |
|---|---|
| Findings adjudicated | **63** |
| CRITICAL / HIGH / MEDIUM / LOW | 1 / 8 / 41 / 13 |
| FIXED / DISPROVED / ACCEPTED / DEFERRED / **OPEN** | 9 / 7 / 28 / 18 / **1** |
| A2 crossings (subject matter *is* a criterion's) | **7**, every one FIXED or DISPROVED |
| Downgrades from inherited severity | **0** |
| Completeness, upstream → ledger | machine-checked over 4 artifacts, 25 upstream ids, **0 missing** |

Both A2 crossings the contract named in advance were driven to a terminal disposition on the only
two paths open to them: **`KR-01` DISPROVED** (the reap works, 12/12) and **`KR-05` DISPROVED**
(the wedge is a denial of service, not an elevation of privilege — in both wedged observations the
product **refused to execute**, with no uncontained High-integrity label). `KR-05`'s residual, the
bash-tool path under a wedged lease, is **named as its own finding** (`F-28-04-002`) rather than
absorbed into the DISPROVED row.

---

## 2. Requirements

| Requirement | Verdict |
|---|---|
| **F28-01** | **Complete** — nine dimensions, three families, 651 cells, 0 red, 0 skipped, 147 critical cells, 0 skipped critical cases. |
| **F28-02** | **Complete** — 3,000/3,000 sessions, canaries intact with 6/6 controls caught, 0 orphans with the control orphan found, all deltas in pre-registered bands. |
| **F28-03** | **Complete** — all eight bindings, independently recomputed, verified by two checkers that must agree. |
| **F28-04** | **Open** — first clause met (0 skipped critical cases); second clause not met (`F-28-02-002` has no terminal disposition). |

---

## 3. How to check this acceptance is worth what it claims

**Read the dissent first: `28-01-decision-evidence/decision-dissent.txt`.** It names its own
reversal condition, and it says in terms:

> *If a future plan, executor or reviewer keeps c4-disposition's four dispositions but drops or
> softens A2 — by scoring findings at their inherited severity, or by letting a
> criterion-contradicting finding take the ACCEPT path — then this decision has silently become
> c4-standing with paperwork, the dissent becomes correct, and the certification is worth
> materially less than its own text claims. **Check A2 before trusting any Phase 28 acceptance.***

**The A2 check, concretely.** Every one of the 7 A2 crossings is on FIXED or DISPROVED; none took
the accept path; and no finding was re-scored below the severity it arrived with. Falsify that
yourself:

```bash
P=.planning/phases/28-native-cross-platform-certification
python3 .planning/scripts/f28-ledger.py --check-a2          $P/evidence/28-04/findings.tsv
python3 .planning/scripts/f28-ledger.py --check-downgrades   $P/evidence/28-04/findings.tsv
python3 .planning/scripts/f28-ledger.py --validate           $P/evidence/28-04/findings.tsv   # MUST fail: F28L-002
python3 .planning/scripts/f28-verify-bindings.py --verify    $P/28-04-CERTIFICATION-RECEIPT.json
```

The strict `--validate` **must fail with exactly one `F28L-002` on `F-28-02-002`**. A run in which
it passes means either the finding was repaired or it was laundered, and the difference is the
whole point.

**The counter-evidence to this very acceptance rule is carried inside the signed receipt**, not
buried in an appendix. `F-28-01-003` records that commit `d0837aa7` — whose being the "later
instrument" is the entire load-bearing structure of the `c4-disposition` decision — ends its own
message with *"Phase 28's criteria are untouched (different phase)."* That is the strongest
available argument for the losing `c4-literal` position and it appears in no captured panel
response. **Anyone reopening the Phase 28 acceptance rule should start there.**

---

## 4. What Phases 29 and 30 inherit

**Phase 29 must consume the machine-readable list**, and this is a real dependency rather than a
courtesy. The receipt is an **accounting control, not a technical one**: it records that a defect
was known; it does not prevent that defect from reaching a user. The 46 accepted and deferred
findings are enumerated inside the signed receipt at `body.findings` with severity, rationale,
owner and backlog id, mirrored in `.planning/BACKLOG.md` under `BL-F28-*`.

> **If Phase 29 does not read that list, the accounting control has no consumer and this
> acceptance rule is worth less than it looks.** That is stated rather than assumed, and it was
> the open risk the acceptance decision itself carried forward.

Named for Phase 29 specifically: **the two-candidate split (`BL-F28-TWO-CANDIDATES`)** — decide
whether a single-candidate full matrix is a release prerequisite; **the acceptance rule's own
counter-evidence (`BL-F28-C4`)**; and **the trust root**, which this phase deliberately did not
create.

Named for Phase 30: the hardening backlog — `BL-F28-BWRAP-ETC`, `BL-F28-ACL-COST`,
`BL-F28-TEMP-SCRATCH`, `BL-F28-WEDGE-BASHPATH`, `BL-F28-MACOS-CENSUS`, `BL-F28-SOAK-WORKLOAD`,
`BL-F28-WIN-PARALLEL`, `BL-F28-VACUOUS-GREENS`, `BL-F28-COUNT-INFLATION`, `BL-F28-FLAVOUR-D` and
the rest.

**And the one blocker: `F-28-02-002` is OPEN at HIGH.** It must be fixed or disproved before any
phase can claim the Phase 28 acceptance gate passed.

---

## 5. Known unknowns, recorded rather than resolved

- **Whether a later restatement of Criterion 4 by its author, made in knowledge of the current
  severity policy, supersedes the planning-time decision.** The dissent says it does, immediately
  and without further panel. No such restatement exists today.
- **Whether the unproven-control corollary should have been applied**, which would have moved
  `KR-02` and `KR-03` across the A2 line — four crossings instead of two. Considered, deliberately
  not applied, recorded as `F-28-01-001`.
- **Whether the `wedge-clearable` verdict generalises off `seandesktop`.** It was not generalised
  and neither AppContainer intel file is cited as evidence for anything.
- **Whether a state-accumulating soak workload would show what the read-only one cannot.**

---

## 6. What this phase did NOT do

- **Nothing was repaired by plan 28-04.** No production defect was fixed and no file under
  `crates/*/src` outside `wcore-eval-scenarios` was touched.
- **No existing test was modified**, including `tests/receipt_contract.rs`, which is unchanged and
  still passes alongside the new contract in the same run (49/49).
- **No measurement was re-run to obtain a better number.** The matrix and the soak are consumed as
  produced.
- **No test was weakened**, `#[ignore]`d, `#[allow]`ed, re-gated or deleted, and no timeout was
  raised. One clippy lint was resolved by **naming a type**, not by suppressing it.
- **No seal, no trust root, no release was claimed.** No PR, no merge to main, no tag, no release,
  no issue closed, no retained evidence ref deleted, no credential supplied.
- **`wcore-contract generate` was NOT run**, and `desktop_contract_corpus` was not chased —
  `CLASS-CONTRACT-01`, structural, recorded as `F-28-04-008`.
- **The acceptance rule was not softened.** A1, A2 and A3 are enforced as written, by two
  independent checkers.
- **No fifth Phase 28 plan.** The phase is capped at four and this is the fourth.

---

# ADDENDUM — 2026-07-29 — `F-28-02-002` is FIXED, and a superseding receipt has been issued

**Appended by lane `lane/28-receipt`. Nothing above this line has been altered.**

The body of this verdict is left exactly as 28-04 wrote it. It was correct when written, and a
later lane rewriting a prior plan's verdict in place is the specific hazard that made an
independent adjudication lane necessary in the first place. What follows is what changed
afterwards, recorded as an addition.

## What changed

`F-28-02-002` was repaired (`15821c03` + `3f3f93dc`) and then **independently adjudicated FIXED**
by lane `28-adj`, which did not author the repair. The finding ledger and
`evidence/28-04/findings.tsv` both read `FIXED`.

The signed receipt did not, and could not: it was signed on 2026-07-28 over a ledger in which the
finding was `OPEN`, and **a signed receipt is never edited**. Its signature is correct over what
was true when it was made; rewriting it to follow the ledger would destroy the only property the
signature provides.

So the original receipt stands, byte-identical, and a **superseding receipt** has been issued
beside it.

| | Original | Superseding |
|---|---|---|
| file | `28-04-CERTIFICATION-RECEIPT.json` | `28-04-CERTIFICATION-RECEIPT-SUPERSEDING-001.json` |
| `body_sha256` | `2037352cff1c2f2c8f8b35e59289ba73b514cd56977c8e22d599ed45e49e0fbb` | `8db1ef07600f644166b422956b13b4f9b5d75af5dc7d0822aa7a4a16746116fb` |
| `key_id` | `phase-28-certification-2026-07-28` | `phase-28-certification-supersession-001-2026-07-29` |
| `F-28-02-002` | `OPEN` | `FIXED` |
| acceptance gate | `false` | **`true`** |

Both verify under the Rust `CertificationVerifier`, each against its own recorded key, in the same
run: `28 passed; 0 failed; 0 ignored; 0 filtered out` on `hetzner-dsm`.

## Which lines above are superseded

These read correctly as of 2026-07-28 and are superseded as of this addendum. **They are left in
place deliberately** — the verdict is a record of what was concluded when, not a live dashboard:

- **line 181** — `gate_passed=false, unresolved CRITICAL/HIGH = ["F-28-02-002"]`. Still the exact
  output for the original receipt, and still printed by the test today. The superseding receipt
  prints `gate_passed=true` in the same run.
- **line 236** — `F-28-02-002 — HIGH — OPEN`. Now `FIXED`.
- **line 281** — `F28-04 | Open`. Its second clause ("every finding resolved") is now met.
- **line 308** — the strict ledger `--validate` "must fail with exactly one `F28L-002`". That
  expectation belonged to the OPEN state; with no non-terminal row left it no longer applies.
- **line 343** — "the one blocker: `F-28-02-002` is OPEN at HIGH". That blocker is cleared.

## What has NOT changed, and must not be read as changed

- **This is still not a seal, not a trust root, and not a release.** A superseding receipt is the
  artifact most likely to be misread as a re-seal. It is not one. It confers no authority the
  original lacked, and tagging, releasing, merging and issue closure remain reserved to Sean.
- **Amendment A3 still binds, and three true claims are NOT "zero known defects".** The
  adjudication itself opened two MEDIUM findings, `F-28-ADJ-001` and `F-28-ADJ-002`. Neither is a
  row in `evidence/28-04/findings.tsv`, so neither is covered by any claim in either receipt. Both
  are named explicitly in the superseding receipt's `posture` binding so that the accounting is
  not silently improved by their absence. `F-28-ADJ-002` matters most to a reader of this section:
  it is **the same permanent-wedge shape as `F-28-02-002`, surviving through a different door**
  (a 0-byte lease `.toml` from a crash between create and write). `F-28-02-002` being FIXED does
  **not** mean the wedge class is eliminated.
- **The dissent recorded above stands.**
