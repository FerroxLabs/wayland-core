# 21-01 — Broad-Execution Admission Gate

Phase 21, plan 01, Task 1 (verdicts) and Task 3 (cross-audited authorization).

This document records the state of the Phase 21 broad-execution admission gate **as read at
execution time from the real intel artifacts**, and the authorization taken over that state by a
four-way cross-audit panel. Nothing here is recalled from a summary; every verdict quotes the
artifact it came from and pins that artifact by its live last-commit SHA.

---

## 0. Machine-readable rows

Field separator is ` :: `. Downstream plans 21-02, 21-03 and 21-04 read the `SCOPE-LIMIT` rows
below rather than re-deriving the gate.

```
BASE-SHA :: 3d80f14662c3df9bd63aeb7ecffc144fe643a553
GATE-VERDICT :: CTRL-01 :: OPEN :: .planning/intel/COMPETITIVE-LEDGER.md
GATE-VERDICT :: CTRL-02 :: NOT-OPEN :: .planning/intel/DESKTOP-PROTOCOL-CHECKPOINT.md
EVIDENCE :: .planning/intel/COMPETITIVE-LEDGER.md :: **Status: CLOSED for admission purposes
EVIDENCE :: .planning/intel/COMPETITIVE-LEDGER.md :: Both peer baselines are **PINNED** as of 2026-07-26
EVIDENCE :: .planning/intel/DESKTOP-PROTOCOL-CHECKPOINT.md :: Before Phase 21 broad execution, publish a linked Desktop plan and host conformance suite consuming a pinned clean Core producer.
EVIDENCE :: .planning/intel/DESKTOP-PROTOCOL-CHECKPOINT.md :: Both receipts are required for a whole-Wayland claim; neither blocks Core-only engine claims outside the shared contract.
```

Artifact pins (live `git log -1 --format=%H -- <path>` at the time of reading):

```
ARTIFACT-PIN :: .planning/intel/COMPETITIVE-LEDGER.md :: d06a60513e1b41507ae41a7513e892ddb6cf6fc6 :: 2026-07-26 10:45:39 +0700
ARTIFACT-PIN :: .planning/intel/DESKTOP-PROTOCOL-CHECKPOINT.md :: 738977ee1f3e84a121855f4a98c874576c18abb7 :: 2026-07-23 20:51:04 +0700
ARTIFACT-PIN :: .planning/intel/D1-CORE-PRODUCER-CONTRACT.md :: 65339c4ed84dd4f78b1fc0dba2ee3a2ea0c340e4 :: 2026-07-26 11:19:57 +0700
ARTIFACT-PIN :: .planning/intel/FIELD-REGRESSIONS.md :: 738977ee1f3e84a121855f4a98c874576c18abb7 :: 2026-07-23 20:51:04 +0700
```

The reading was taken on branch `plan/f20-unified-audit-repair` in
`/Users/seandonahoe/dev/waylandcore-ferrox`. `git status --porcelain -- crates/` was empty
before and after: this plan modified nothing under `crates/`.

---

## 1. The standard being applied, verbatim

From `.planning/ROADMAP.md` line 83, quoted exactly:

> **Admission prerequisite**: before broad Phase 21 execution — (CTRL-02 / D1) the linked Desktop plan and consumer/reducer conformance harness are pinned to the exact Core producer contract, AND (CTRL-01) the Competitive Capability Ledger has pinned exact Hermes + OpenClaw baselines with F03/F05 evidence mapped.

From `.planning/REQUIREMENTS.md`, the two control requirements, quoted exactly:

> - [x] **CTRL-01**: A schema-complete versioned capability/maturity ledger with pinned Hermes/OpenClaw baselines exists before Phase 21 and is refreshed at every admitted phase; F30 independently reviews it. — **ESTABLISHED 2026-07-26** (`d06a6051`).

> - [ ] **CTRL-02**: D1 publishes a pinned Core producer contract, linked Desktop plan, and real consumer/reducer conformance suite before Phase 21 broad execution.

Note the checkbox asymmetry in `REQUIREMENTS.md` itself: CTRL-01 is `[x]`, CTRL-02 is `[ ]`.
That is corroboration, not the verdict; the verdicts below are derived from the artifacts.

---

## 2. CTRL-01 — VERDICT: OPEN

**Standard applied.** Two clauses, taken together: (a) the ROADMAP clause — *"the Competitive
Capability Ledger has pinned exact Hermes + OpenClaw baselines with F03/F05 evidence mapped"*;
and (b) the ledger's own self-stated close condition, quoted from its Admission rule section:

> - CTRL-01 remains open until every active row uses the declared maturity enum and has a pinned peer baseline, security owner, exact evidence IDs, delta, limitation, and refresh phase.

**Evidence read from `.planning/intel/COMPETITIVE-LEDGER.md`.**

- Pinned exact peer baselines, ROADMAP clause (a): the file states *"Both peer baselines are
  **PINNED** as of 2026-07-26"* and carries a table giving Hermes Agent **0.17.0** at
  `dbe734beff0caf5e8ee2acbe4277db7f6cf84a21` with pin source `git show dbe734be:pyproject.toml`
  line 10, and OpenClaw **2026.6.2** at `11a0ad10e91a50d5a0e636494eea4d7ad3eaf9fc` with pin
  source `git show 11a0ad10:package.json` line 3. Both are exact versions, not `UNPINNED`.
- F03/F05 evidence mapped, ROADMAP clause (a): the file carries a section headed *"F03/F05
  retroactive evidence map"* which maps `F03-RECEIPT@1c644ccd` to the AUTH-* and SUPPLY-*
  families and maps **all eight** F05 audited capability identities to ledger rows in a table,
  including the honest negatives (`Delegate isolation — Unavailable: isolation not enforced`).
- Self-stated close condition, clause (b): the file's own disposition section states
  *"**Status: CLOSED for admission purposes — with two carried limitations, neither of which is a
  missing external input.**"* and discharges each of the seven close-condition clauses in a table
  where every cell reads **MET**.

**Verdict: OPEN.** Every clause of the stated standard is met at the moment of reading. This is
not an intent or an in-progress claim — the ledger's rows carry the values the condition names.

**Recorded, not treated as unmet:** the ledger carries two limitations it explicitly labels
*"tracked, not blocking"* — the `delegate_isolation` F05 identity has not been re-gated after
Phase 20 (owner: Phase 21), and every delta is static-source rather than runtime-measured
(owner: Phase 30, by design). Neither is a clause of the admission standard, so neither changes
this verdict. The first is a genuine input to Phase 21 and is carried into the census as an
out-of-phase observation.

**Staleness check.** Ledger last commit `d06a6051` (2026-07-26 10:45:39 +0700); the file's own
footer reads *"CTRL-01 refreshed 2026-07-26 against Phase 20 seal `01a5b0ae` and Phase 20A seal
`9821ef76`."* Its self-declared refresh date and its git commit date agree. Not stale.

---

## 3. CTRL-02 / D1 — VERDICT: NOT-OPEN

**Standard applied.** The ROADMAP clause: *"(CTRL-02 / D1) the linked Desktop plan and
consumer/reducer conformance harness are pinned to the exact Core producer contract"*. The
checkpoint document states the same obligation in its own words:

> Before Phase 21 broad execution, publish a linked Desktop plan and host conformance suite consuming a pinned clean Core producer. Record protocol version, fixture/schema digests, generator version, EffectiveExecutionPolicy semantics, lifecycle/correlation/ordering/duplicate/terminal/failure behavior, and ownership boundaries. Issue coordination is not completion.

**Evidence read from `.planning/intel/DESKTOP-PROTOCOL-CHECKPOINT.md` and, for the Core half,
from `.planning/intel/D1-CORE-PRODUCER-CONTRACT.md`.**

The obligation is **two-part and asymmetric**, and the two parts are in different states:

| D1 half | State | Evidence |
|---|---|---|
| (a) pinned, digest-bound Core producer contract | **DISCHARGED** | `D1-CORE-PRODUCER-CONTRACT.md` §1 pins SHA `b6936299d9c3a7d3110e9ba03c36e5debe965b85`; §3 publishes all three `digest_named_bytes` digests; §8 records `302 tests run: 302 passed, 0 skipped` for `wcore-protocol` at that SHA on `hetzner-dsm`. Its own header: *"**Status: the CORE half of D1 is complete. D1 itself is NOT complete.**"* |
| (b) linked Desktop plan + real consumer/reducer conformance suite | **NOT DISCHARGED** | `D1-CORE-PRODUCER-CONTRACT.md` §9 enumerates eleven items the Desktop lane must still supply and closes with *"**Until items 1–2 exist with a green run, D1 is not complete.**"* No Desktop plan or conformance-suite receipt exists in this repository or is referenced from either intel artifact. |

**Verdict: NOT-OPEN.**

**Precise unmet clause.** The ROADMAP clause requires *"the linked Desktop plan **and**
consumer/reducer conformance harness"* to be *"pinned to the exact Core producer contract"*.
The **pinning target** exists and is exact — SHA `b6936299`, three digests, published. What does
not exist is the **pinned party**: no linked Desktop plan referencing that SHA, and no
consumer/reducer conformance harness replaying the serialized corpus at
`crates/wcore-protocol/contracts/desktop/v1/` through the real Desktop reducer. Concretely,
items 1 and 2 of `D1-CORE-PRODUCER-CONTRACT.md` §9 are unmet.

**Why this cannot be closed here, and why that is a fact rather than a permission to wait.**
The Desktop half lives in the Wayland Desktop repository. It is not on the reserved-to-Sean list
(merge, PR, tag, release, issue closure, evidence-ref deletion, real credentials); it is simply
not reachable from this checkout. Waiting on it from here is a decision to stall indefinitely,
not a decision to be careful. That is exactly the question Task 3's panel decides.

**The clause the panel is required to weigh.** The checkpoint document's own **last sentence**,
confirmed at execution time to still be the file's last sentence:

> Both receipts are required for a whole-Wayland claim; neither blocks Core-only engine claims outside the shared contract.

**Staleness check.** Checkpoint last commit `738977ee` (2026-07-23 20:51:04 +0700). The file
carries no internal version or date marker of its own — it is a standing contract document, not
a refreshed ledger. Its D1 section is unchanged since 2026-07-23; `D1-CORE-PRODUCER-CONTRACT.md`
(`65339c4e`, 2026-07-26 11:19:57 +0700) is the newer artifact and is the one that reports
progress against it. Nothing here is being read as current that is not.

**Neither control was worked on.** No attempt was made to close CTRL-01 or CTRL-02 in this plan.

---

## 4. CTRL-03 — context only, does not gate this phase

`.planning/intel/FIELD-REGRESSIONS.md` (last commit `738977ee`, 2026-07-23) carries CTRL-03, the
live regression register. `ROADMAP.md:83` names only CTRL-01 and CTRL-02 as Phase 21 admission
prerequisites, so CTRL-03's state is recorded as context and produces no verdict row here.

---

## 5. Task 3 — the measured evidence and the four-way cross-audited authorization

### 5.1 Machine-readable decision rows

```
MEASURED :: contract-corpus-pin :: a39c13794669e1afca2218ddf3437ba967b4dceb25d9c7e669974358495821e6
MEASURED :: contract-drift :: STABLE :: .planning/phases/21-child-authority-and-budget-inheritance/evidence/21-01-t3-linux.log
PANEL-VOTE :: codex-sol :: proceed-scope-limited :: codex-sol.raw.txt
PANEL-VOTE :: gemini-pro :: proceed-scope-limited :: gemini-pro.raw.txt
PANEL-VOTE :: kimi-k3 :: proceed-scope-limited :: kimi-k3.raw.txt
PANEL-VOTE :: claude-adversarial :: hold :: claude-adversarial.raw.txt
PANEL-DECISION :: proceed-scope-limited :: MAJORITY
PANEL-RATIONALE :: Three of four members independently picked proceed-scope-limited on one shared bundle, a strict 3-1 majority, so the basis is MAJORITY and no evidentiary tiebreak was needed. CTRL-01 measured OPEN and CTRL-02 measured NOT-OPEN, which removes proceed-broad by that option's own terms. The choice was therefore between hold and proceed-scope-limited, and the checkpoint document that DEFINES the CTRL-02 obligation ends by stating that neither receipt blocks Core-only engine claims outside the shared contract. Every one of the census's four HIGH findings sits on a Core-internal seam, and the half of CTRL-02 that is missing is the Desktop consumer half, which lives in another repository and cannot be closed from this checkout, so hold converts an external-lane dependency into an indefinite stall for phases 21 through 23 with no Core-side safety gain. The named cost of proceeding was not argued but measured on real hardware at the decision commit in four legs that each emitted their own verdict line, including the canonical wcore-contract digest run that the D1 document itself records as never having been performed, and a live observation of the shipped wayland-core binary emitting a ready contract descriptor byte-identical to the pinned digests. The adversarial dissent did not carry the count but its substance did carry into the scope limit, and plan 21-03 is bound by it.
PANEL-DISSENT :: hold :: The adversarial member argued that the four legs measured whether the contract has ALREADY drifted, which is a statement about the past, whereas the option's stated risk is whether it WILL drift before Desktop adopts it. Worse, it identified a concrete mechanism by which the authorized work causes the drift the measurement was used to rule out. HIGH-4 is with_requested_approvals on EffectiveExecutionPolicy, and execution_policy is a versioned SUB-CONTRACT of the Desktop corpus with its own event fixture and adversarial vectors, so a 21-03 repair to that seam can move source_inputs_digest or fixture_digest, which changes the ready descriptor, which D1 records as a hard renegotiation error rather than a tolerated upgrade. It did not carry because it does not defeat proceed-scope-limited as such, only an unbounded version of it, and the member conceded as much in writing. It is answered by binding it into the 21-03 scope row rather than by dismissing it.
PANEL-DISSENT :: proceed-broad :: No member argued for it and the executor did not select it. Recorded for completeness with the reason it could not be argued. CTRL-02 measured NOT-OPEN and this option's own cons state that a single NOT-OPEN verdict removes it, so choosing it would have been a waiver dressed as a verdict rather than a gate that was passed. No WAIVER row appears in this document because no NOT-OPEN control was overridden.
SCOPE-LIMIT :: 21-02 :: PROCEED :: The dual-surface hostile corpus may be built and run against the Core producer contract as pinned and measured at this commit. Both surfaces are authorized because the host-protocol surface here is the Core PRODUCER side, which is the half of CTRL-02 that IS discharged. Excluded: no Desktop consumer or reducer equivalence may be claimed or implied by any corpus case.
SCOPE-LIMIT :: 21-03 :: PROCEED :: Triage and authorized repair are bounded to the four Core-internal HIGH findings recorded in the census, namely HIGH-1 tool replacement, HIGH-2 absent provider intersection, HIGH-3 orphan PolicyGate and HIGH-4 non-managed approval replacement. BINDING CONSTRAINT carried from the adversarial dissent, which every downstream reader must honour. No repair may land a change to the 40 generator source inputs or the 156-file Desktop contract corpus without re-running wcore-contract digest and check, recording the new digests, and re-pinning D1 section 3 as an explicit contract bump. HIGH-4 sits directly on this surface so the constraint is live, not hypothetical.
SCOPE-LIMIT :: 21-04 :: PROCEED :: Attribution and the phase verdict may be proven and rendered as a CORE verdict against the standalone surface and the Core producer contract as pinned. Excluded and reserved until CTRL-02 closes: any whole-Wayland claim, any statement about Desktop consumer or reducer behavior, and any assertion that D1 section 9 items 1 and 2 have been discharged. Recorded minority within the majority: codex-sol argued 21-04 must hold entirely; the executor did not adopt that because no Phase 21 Success Criterion requires the Desktop consumer, so blocking the verdict outright would leave the phase unresolvable for a reason its own criteria do not ask for.
```

### 5.2 Step 1 — the measurement, taken on real hardware before anyone saw the bundle

Host `hetzner-dsm`, phase-dedicated worktree `/root/wayland-p21` created from `/root/wayland`
so no checkout another agent may hold was disturbed. Pinned to the decision commit; the
transcript's first line is `RUN_SHA=3d80f14662c3df9bd63aeb7ecffc144fe643a553`. Full transcript:
`.planning/phases/21-child-authority-and-budget-inheritance/evidence/21-01-t3-linux.log`.

Each leg's script emitted its own verdict line — these are tool output, not an executor's
impression of what scrolled past:

```
CONTRACT-DIGEST RESULT=MATCH RC=0
CONTRACT-CHECK RESULT=CURRENT RC=0
LIVE-READY RESULT=EMITTED RC=0
CORPUS-PIN RESULT=MATCH RC=0
```

- **CONTRACT-DIGEST** — `cargo run -p wcore-protocol --bin wcore-contract -- digest` printed
  `fixture_digest=sha256:42f142ab…`, `schema_digest=sha256:e5d1744a…`,
  `source_inputs_digest=sha256:d8b1a8b5…`, all three equal to D1 §3. **This closes a gap D1
  records against itself**: §3.2 states *"This author did NOT run it"* of the canonical Rust
  reproduction, having substituted a throwaway Python script. This is the canonical run.
- **CONTRACT-CHECK** — the same binary's `check` regenerated every artifact in memory and
  compared byte-for-byte to the checked-in corpus. Exit 0: not stale, not hand-edited.
- **LIVE-READY** — the real shipped binary was built **and driven**. `wayland-core --json-stream`
  was spawned against a hermetic `WAYLAND_HOME`, and the `ready` event it emitted carried
  `name=wayland-desktop-core, major=1, minor=8, generator=wcore-desktop-contract-gen/11` and all
  three digests identical to D1 §3 *and* to `manifest.json`. Phase 20A shipped CI-green with
  nobody ever launching the binary; this decision did not repeat that.
- **CORPUS-PIN** — the toolchain-free whole-corpus pin over all **156** files re-derived to
  `a39c13794669e1afca2218ddf3437ba967b4dceb25d9c7e669974358495821e6`, matching D1 §3.3. It was
  independently re-derived a second time on the Mac and agreed.

### 5.3 Step 2 — the one shared bundle

`21-01-t3-panel/panel-prompt.txt`, first line `BUNDLE-ID=3d80f14662c3df9bd63aeb7ecffc144fe643a553`.
It carries the question, all three options verbatim with their ids/names/pros/cons in **rotated**
order (`hold`, `proceed-broad`, `proceed-scope-limited`), Task 1's two `GATE-VERDICT` rows with
quoted evidence, the verbatim last sentence of `DESKTOP-PROTOCOL-CHECKPOINT.md` with an explicit
instruction that the answer must engage it, the fact that the Desktop half is in another
repository, Task 2's eleven-row census with severities and the four HIGH findings, and all four
measured verdict lines. **Every member received these exact bytes and nothing else.**

### 5.4 Step 3 — the four responses, quoted verbatim

The three measured vote-loss traps were handled: `--skip-trust` for gemini (without it it exits
55 with a trusted-directory refusal); the absolute path for kimi plus **unanchored** extraction
(kimi indents its final lines by two spaces, so `^PANEL_POSITION=` returns empty and silently
drops the vote); and **last-match** extraction for codex, whose capture contains the marker three
times because it echoes the prompt and repeats its final block. Extraction used
`grep -oE 'PANEL_POSITION=[a-z-]+' <capture> | tail -1` for all four. Unedited bytes, stdout and
stderr together, are in the four `.raw.txt` files; quoted below are each member's substantive
answers verbatim from those captures.

**codex-sol — `proceed-scope-limited`** (verbatim from `codex-sol.raw.txt`):

> Phase 21 is partly both. The four HIGH findings and standalone enforcement are “Core-only engine claims outside the shared contract,” which the checkpoint’s final sentence explicitly says CTRL-02 does not block. Desktop reducer equivalence, cross-wire approval semantics, and final attribution claims are inside the shared contract and remain blocked.
>
> Pick: `proceed-scope-limited`.
>
> Binding limit:
>
> - 21-02 may run only Core standalone and Core producer-side protocol enforcement tests pinned to the measured contract.
> - 21-03 may triage and repair the four Core-internal HIGH findings.
> - 21-04 must hold; no Desktop consumer/reducer equivalence, whole-Wayland attribution, or final Phase 21 verdict may be claimed until CTRL-02 opens.
>
> The strongest argument against this pick is ROADMAP.md’s literal “before broad Phase 21 execution” prerequisite: 21-02 is explicitly dual-surface, so careless execution could become a de facto gate bypass. The named exclusions prevent that, while the four MATCH/CURRENT/EMITTED measurements remove the stated contract-drift reason to halt Core-only work.

**gemini-pro — `proceed-scope-limited`** (verbatim from `gemini-pro.raw.txt`):

> The decision is to proceed under a named scope limit.
>
> CTRL-01 is OPEN, but CTRL-02 is explicitly NOT-OPEN because the Desktop lane's consumer/reducer conformance suite is missing. However, we must engage the checkpoint's explicit clause: *"Both receipts are required for a whole-Wayland claim; neither blocks Core-only engine claims outside the shared contract."*
>
> Phase 21 is partly a Core-only engine claim and partly a shared contract claim. The four HIGH findings from the authority census (tool replacement, missing provider intersection, orphan PolicyGate, replacing approvals) are fundamentally Core-internal enforcement seams. Repairing these (Plan 21-03) and generating the hostile corpus for them (Plan 21-02) are Core-only claims that do not depend on the Desktop consumer being pinned. Only Plan 21-04 (attribution/correlation) crosses deeply into a whole-Wayland claim. Therefore, the checkpoint's own language explicitly unblocks the Core-internal execution of 21-02 and 21-03. Furthermore, the sole named risk of proceeding—that the Core producer contract might drift before Desktop pins it—has been empirically measured on hardware as STABLE.
>
> The strongest argument AGAINST this pick is that ROADMAP.md line 83 demands the Desktop plan be pinned "before broad Phase 21 execution," with no exceptions for partial execution. One could argue that interpreting Phase 21 as divisible into "Core-only" and "whole-Wayland" claims is a semantic loophole designed to bypass a hard gate, which risks integration divergence when Plan 21-04 eventually needs to run.

**kimi-k3 — `proceed-scope-limited`** (verbatim from `kimi-k3.raw.txt`; kimi's own bullet and
two-space indentation preserved in the capture, stripped here only for readability):

> ## Reading of the checkpoint's last sentence
>
> "Both receipts are required for a whole-Wayland claim; neither blocks Core-only engine claims outside the shared contract."
>
> Phase 21 proves that a child cannot widen authority or budget inside the engine. That is a Core-only engine claim. The authority census confirms it structurally: all four HIGH findings are Core-internal seams (Delegate tool's `toolsets` replacement into `ForkOverrides.allowed_tools`, no provider intersection channel, `PolicyGate` orphaned on the agent path, non-managed `with_requested_approvals` replacing instead of ratcheting). None of them requires a Desktop consumer to prove or disprove. The seams that do touch the shared contract — approval revisions crossing the wire, and attribution/correlation in 21-04 — are a minority of the phase's surface, and the verdict document that *defines* CTRL-02 says explicitly that the missing receipts do not block this class of claim.
>
> ## The pick and the scope limit
>
> **proceed-scope-limited.** proceed-broad is off the table by its own terms — CTRL-02 returned NOT-OPEN, and selecting it anyway would be bypassing the gate. hold costs the phase everything while protecting nothing Core-side: the Desktop half lives in a different repository, is unreachable from this checkout, and the gate document says it doesn't block this work.
>
> The binding limit, named exactly:
>
> - **21-02 (dual-surface hostile corpus): proceeds.** The named risk — corpus encoding a contract that later changes — was measured at this commit, not argued: CONTRACT-DIGEST MATCH, CONTRACT-CHECK CURRENT, LIVE-READY EMITTED against the real shipped binary, CORPUS-PIN MATCH, all at SHA b6936299/bundle 3d80f146. The pinning *target* is exact and byte-stable; only the pinned *party* is missing. A corpus built against that target does not change when Desktop adopts it.
> - **21-03 (triage + authorized repair): proceeds.** All four HIGH findings are Core-internal; repairs land inside Core authority logic. Constraint: no repair may change the serialized contract surface (fixtures, schema, digests) — if one must, it is deferred to 21-04 and the digests re-pinned.
> - **21-04 (attribution + phase verdict): proceeds only as a Core-only claim.** Attribution, correlation, and the phase verdict may be proven against the standalone surface and the Core producer contract as pinned. Any whole-Wayland claim — i.e., any verdict statement about the Desktop consumer path or cross-repo conformance — is reserved until CTRL-02 closes. 21-04 may not assert what Section 9 items 1–2 have not yet demonstrated.
>
> ## Strongest argument against my pick
>
> The ROADMAP:83 text says "before broad Phase 21 execution" with no Core/Desktop carve-out, and the checkpoint itself says "Issue coordination is not completion" — a warning precisely against treating a published Core contract as discharge. Reading the last sentence as license to proceed is exactly the kind of scoping leniency a gate is meant to resist, and if the Desktop conformance suite later reveals the pinned contract is wrong (not merely unadopted), 21-02's protocol-surface cases and 21-04's approval-dimension proofs would need re-proving despite today's STABLE measurement. I weigh that and still pick scope-limited: the drift risk is the one thing that was measured byte-exact rather than reasoned about, the limit reserves the only claims the receipts actually cover, and hold converts an external-lane dependency into an indefinite stall for phases 21–23 with no Core-side safety gain.

**claude-adversarial — `hold`** (verbatim from `claude-adversarial.raw.txt`, §1, §2 and §5, the
sections carrying the argument that did not appear in any other transcript):

> The `cons` of `proceed-scope-limited`, quoted from the bundle, is: "the risk is that a corpus built before the producer contract is pinned encodes a contract that later changes, so any protocol-surface case may need re-proving."
>
> The four legs measured that the contract has not changed AS OF commit 3d80f146. That is a statement about the past. The option's risk is a statement about the future: will the contract change between now and Desktop's adoption of it? A STABLE reading today is consistent with both answers, so it does not discharge the risk. All three members treated `MEASURED :: contract-drift :: STABLE` as though it did. Codex wrote that the measurements "remove the stated contract-drift reason to halt Core-only work." They do not. They remove the reason to believe drift has ALREADY happened.
>
> […]
>
> HIGH-4 is `with_requested_approvals` in `EffectiveExecutionPolicy`. The bundle also records, from D1, that the Desktop contract corpus carries `execution_policy` as a versioned SUB-CONTRACT, that the corpus contains an `execution_policy` event fixture and a family of adversarial policy vectors, and that the contract's `source_inputs_digest` is taken over the generator's Rust source inputs.
>
> Chain the two. Plan 21-03 is authorized — by all three members — to repair HIGH-4. Repairing HIGH-4 means changing `EffectiveExecutionPolicy` approval resolution. If that type or its serialization is among the generator's source inputs, the repair changes `source_inputs_digest`; if the fixtures move, it changes `fixture_digest`; either changes the `ready.contract` descriptor the running binary emits — which the LIVE-READY leg just proved is the byte-exact thing Desktop would pin. And D1 records that a `minor` mismatch is a HARD renegotiation error, not a tolerated upgrade.
>
> So the panel has authorized, as the safe Core-internal work, precisely the repair most likely to move the contract that the same panel just certified as stable. The measurement did not merely answer the wrong question — it was used to license the activity that falsifies it.
>
> […]
>
> I am required to default to refuted when uncertain, and I am not certain the majority is wrong. My §3 argument is the weakest of the three: SC3's host-protocol leg is the Core PRODUCER surface, and the producer half of the contract is the half that IS discharged, so a reasonable reader can say the producer-side corpus is outside the missing receipt's scope. I concede that. My §1 and §2 arguments I do not concede […]
>
> I therefore refuse to ratify. If the executor commits to `proceed-scope-limited` anyway on a 3-1 count, the minimum this dissent obliges is that the scope limit is not a label but a constraint with an executable consequence: plan 21-03 must not land any repair that changes the 40 generator source inputs or the 156-file corpus without re-running `wcore-contract digest` and `check`, re-pinning D1 section 3, and recording the new digests as a contract bump.

### 5.5 Step 4 — the decision and its arithmetic

Four votes: `proceed-scope-limited` ×3, `hold` ×1. Strict majority, so the basis is `MAJORITY`
and no evidentiary tiebreak was required. Every recorded vote was verified against the **last**
`PANEL_POSITION=` line in that member's own captured transcript.

`proceed-broad` was not available on the evidence: CTRL-02 measured NOT-OPEN, and that option's
own `cons` state a single NOT-OPEN verdict removes it. **No `WAIVER` row appears in this
document, because no NOT-OPEN control was overridden.** The gate was not bypassed; it was
scope-limited.

**The dissent lost the count and won a clause.** The adversarial member's §2 argument — that
21-03's authorized repair of HIGH-4 is itself the most likely cause of the contract drift the
measurement was used to rule out — was unanswered by any of the three majority transcripts and
is, on the evidence in the bundle, correct in mechanism. It does not defeat
`proceed-scope-limited`, only an unbounded version of it, which the member conceded in writing.
It is therefore answered rather than dismissed: the binding constraint in the `21-03`
`SCOPE-LIMIT` row above is that dissent, made executable.

**Where the majority itself split, and how it was resolved.** On plan 21-04, codex-sol said it
*"must hold"*; kimi-k3 said it *"proceeds only as a Core-only claim"*; gemini-pro said only that
it *"crosses deeply into a whole-Wayland claim"* without naming a disposition. The executor
adopted kimi-k3's position and recorded 21-04 as `PROCEED` with an explicit exclusion, because no
Phase 21 Success Criterion requires the Desktop consumer — SC1 and SC2 are Core-internal and SC3
names the *producer* surface, which is the discharged half — so blocking the verdict outright
would leave the phase unresolvable for a reason its own criteria do not ask for. Codex's stricter
reading is recorded above as a minority within the majority so it is not lost.

**Sean was not reached.** There was no deadlock: the count was 3-1, not 2-2, and all four members
answered. Nothing in this question is on the reserved list.

### 5.6 Termination state

**State 2 — Complete, gate not open, scope-limited authorization.** CTRL-02 verified NOT-OPEN;
the panel authorized a scope-limited continuation naming, plan by plan, exactly which of 21-02,
21-03 and 21-04 may proceed and under what limit; the limit is recorded as three machine-readable
`SCOPE-LIMIT` rows. This plan writes its SUMMARY and stops.
