---
phase: 21-child-authority-and-budget-inheritance
verified: 2026-07-27T01:30:00Z
verified_at_sha: ac94b1d57cc95b577f60b8b3da3be0d536a6d7ad
branch: plan/f20-unified-audit-repair
status: gaps_found
score: 1/3 success criteria newly upgraded; 1/3 still NOT MET
requirements: "F21-01 MET · F21-02 NOT MET · F21-03 FENCED · F21-04 NOT MET"
verifier_stance: adversarial (goal-backward, FORCE), third grading of this phase
platform_coverage: "Linux only (hetzner-dsm). NO Windows evidence was produced by this re-grade."
re_verification:
  previous_status: gaps_found
  previous_score: 0/3
  previous_verification: VERIFICATION-2.md at df63a4af (code at 359ce2bf)
  gaps_closed:
    - "SC1's deciding falsifier — the tool guard confirmed ABSENT — is repaired and LIVE-proven at HEAD (6b0083b0 + 3e23b83d + merge 9c3e3687)."
    - "F21-04-02 (reservation does not survive a restart) is DISPROVED with executable counter-evidence (e879206e); attribution_refund's in-process MODE moves NOT-OBSERVABLE -> CORRECT at HEAD."
    - "F21-04-03 (parallel Spawn siblings die on a journal-head CAS collision) is repaired (1eb9b5ca); 6 of 6 live two-sibling json-stream runs at HEAD produced two distinct parent_call_ids with both siblings serving turns."
    - "F-V9 is moot for this document: the Criterion-3 grade is re-derived here from measurement, not inherited."
  gaps_remaining:
    - "SC3 — standalone and host-protocol corpora prove EQUIVALENT enforcement. Unchanged and still NOT MET."
    - "F21-02 — four of its six dimensions (fan-out, time, token, cost) have NO live evidence and cannot obtain any, because no shipped surface carries a child-fillable budget field."
  regressions: []
  new_findings:
    - "N1 (LOW, documentation) — the corpus's own CENSUS row still reads `corpus_tool :: tool :: ABSENT :: expectation REFUSED :: canary false` at HEAD. The census artifact is stale relative to the repaired product."
    - "N2 (INFO, evidence quality) — the phase corpus's tool REFUSED remains jointly attributable to tool authority and workspace containment, by its own statement. Only the NEW live test separates them."
    - "N3 (unchanged product fact) — `ToolConfirmer::check_for` still returns `Denied` unconditionally when stdin is not a terminal (crates/wcore-agent/src/confirm.rs:125-131). Not repaired, not required to be. The evidence in this report is NOT that artifact — see §6."
gaps:
  - truth: "SC3 — Standalone and host-protocol hostile corpora prove EQUIVALENT enforcement."
    status: failed
    reason: "None of the three clauses that made this NOT-MET was touched by the repairs. Re-measured at HEAD on Linux, not inherited from the prior verdict."
    artifacts:
      - path: "crates/wcore-cli/tests/child_authority_corpus/surfaces.rs (host-protocol driver)"
        issue: "tool and fan-out are still NOT-EXPRESSIBLE on the host-protocol in-process surface. Measured at HEAD: the host child-spawn request type carries [name, prompt, max_turns, max_tokens, system_prompt, provider, model, temperature] and none expresses a tool-authority or breadth request; `spawn_host_child` hardcodes `ForkOverrides::default()`."
      - path: "corpus_fan_out live rows"
        issue: "NOT-EXPRESSIBLE on BOTH live surfaces at HEAD — 1 provider request served, 0 of them by a delegated child. Fan-out remains undetermined live."
      - path: "Windows standalone live surface"
        issue: "Not exercised by this re-grade at all. No ConPTY-drivable PTY exists, and no Windows run was produced here. Windows equivalence is asserted by nobody at HEAD."
    missing:
      - "A host-protocol expression for the tool and fan-out dimensions, or an accepted decision that those two are permanently standalone-only."
      - "A live fan-out control that separates 'the cap bound' from 'nothing ran'."
      - "A Windows live actor, or an accepted decision that Windows equivalence stands on the in-process modes plus host-protocol-live only."
  - truth: "F21-02 — Nested children cannot exceed parent depth, fan-out, concurrency, token, cost, or time reservations."
    status: failed
    reason: "The vacuity this phase was written to catch is INTACT at HEAD and was re-measured, not assumed. F21-02 is enforced in-process for every dimension and holds live only by the ABSENCE of a request channel for four of six."
    artifacts:
      - path: "crates/wcore-agent/src/engine.rs:6173"
        issue: "`begin_active_turn(turn_id, None)` — the sole production caller, still None."
      - path: "crates/wcore-budget/src/execution.rs:936"
        issue: "The only `sub_budget(Some(..))` caller in the crate sits inside `#[cfg(test)] mod tests`. Zero production callers pass Some(..) at HEAD."
      - path: "crates/wcore-types/src/spawner.rs:518-537"
        issue: "`SubAgentConfig` carries no budget, depth, breadth or tool-authority field. Confirmed by reading the struct, and independently by the corpus's exhaustive-destructuring guard."
    missing:
      - "Live evidence for fan-out, time, token or cost. It is not obtainable without ADDING a child-fillable budget channel — i.e. adding the attack surface in order to test it."
      - "An explicit accepted decision that these four dimensions close on in-process enforcement plus a red-able NO-CHANNEL canary, which is what the evidence actually supports."
  - truth: "F21-04 — Hostile child tests prove no authority or resource amplification across standalone AND host protocol paths."
    status: failed
    reason: "Three of the four blockers named in the original OPEN text are resolved. The fourth is not: F21-04 requires equivalence across both paths and SC3 is still NOT MET."
    artifacts:
      - path: "corpus_tool / corpus_fan_out host-protocol in-process rows"
        issue: "WITHHELD as NOT-EXPRESSIBLE rather than compared. 2 of 11 dimensions have no cross-surface comparison at HEAD."
    missing:
      - "Whatever closes SC3."
deferred:
  - truth: "F21-04-01 — the host protocol carries no per-child observable for reservation, refund, escalation or cancellation, and no sibling identity on `approval_required`."
    addressed_in: "A coordinated Core/Desktop release"
    evidence: ".planning/SEAM-REQUESTS/F21-04-01.md, specified at dcd34c08. Every one of these edits moves fixture_digest and schema_digest. Measured rather than assumed: both enums are internally tagged, so `Stop` can gain a defaulted field additively and still decode {\"type\":\"stop\"}. Graded here as the fenced seam it is, NOT as a gradeable failure of product code."
behavior_unverified_items:
  - truth: "F21-04-03 does not recur on Windows."
    test: "Run two parallel `Spawn` siblings against the shipped binary on SEANDESKTOP, 24 iterations."
    expected: "0/24 leave the losing sibling's budget authority PERMANENTLY FAULTED. At the pre-fix SHA it was 23/24."
    why_human: "Windows hardware is not reachable from this lane. The fix is platform-neutral (an atomic capture-and-append inside one writer-lock acquisition) and Linux moved 3/8 -> 0/6 live, but that is inference, not measurement, on Windows."
  - truth: "SC3's Windows leg."
    test: "Drive the standalone live surface on Windows with a delegated child that takes its own provider turn."
    expected: "A live actor exists, or the impossibility is re-affirmed at HEAD."
    why_human: "Requires SEANDESKTOP. No Windows evidence was produced by this re-grade and none is inherited."
human_verification:
  - test: "Accept or reject the SC1 upgrade recorded in §2 of this document (NOT-MET -> MET WITH STATED EXCEPTIONS)."
    expected: "A recorded decision. The 4-way panel was unanimous for the upgrade, and §2.4 records the discount that unanimity deserves plus the strongest surviving case against it."
    why_human: "This is the phase's most consequential grade and it moves in the permissive direction for the first time. It should move on a recorded decision, not on a verifier's say-so."
  - test: "Decide whether F21-02 may close on in-process enforcement plus a red-able NO-CHANNEL canary, given that live evidence is unobtainable without adding the attack surface."
    expected: "Either the requirement stays OPEN until a channel exists, or a recorded acceptance that this is the terminal evidence shape for a dimension with no request channel."
    why_human: "It is a standing-rule question (\"never complete on in-process evidence alone\") applied to a case the rule did not anticipate, not a measurement question."
---

# Phase 21 — Third Grading (Re-Verification against the repaired product)

**Phase Goal (ROADMAP.md:75):** Every delegated actor remains inside the parent's
authority and resource envelope.

**Graded at:** `ac94b1d57cc95b577f60b8b3da3be0d536a6d7ad`, branch
`plan/f20-unified-audit-repair`.
**Evidence host:** `hetzner-dsm`, worktree `/root/wayland-21rv`, Linux. **Every
number below was produced by this re-grade at this SHA.** Nothing is inherited
from `21-04-PHASE-VERDICT.md`, `VERIFICATION.md` or `VERIFICATION-2.md`.

**Overall verdict: PHASE GOAL NOT ACHIEVED — for the third time, and for the
first time on materially narrower grounds.**

---

## 0. Headline

The phase graded itself NOT ACHIEVED twice. Both gradings were correct. The
product defects those gradings found have since been repaired, and this
re-grade confirms the repairs are real, wired, and live-proven — but it does
not turn the phase green, because two of the four requirements fail for
reasons the repairs never claimed to address.

| | Prior (at `359ce2bf`) | This grading (at `ac94b1d5`) |
|---|---|---|
| SC1 | NOT MET | **MET WITH STATED EXCEPTIONS** (upgraded) |
| SC2 | MET WITH STATED EXCEPTIONS | **MET WITH STATED EXCEPTIONS** (one exception discharged, one fenced) |
| SC3 | NOT MET | **NOT MET** (unchanged, re-measured) |
| F21-01 | OPEN | **MET** |
| F21-02 | OPEN | **NOT MET** |
| F21-03 | OPEN | **FENCED** on F21-04-01 |
| F21-04 | OPEN | **NOT MET** |

---

## 1. What actually landed, verified in the code and not from the commit messages

| Finding | Claim | Verified at HEAD |
|---|---|---|
| F21-02-01 | child tool registries intersected with parent authority | **CONFIRMED.** `spawner.rs:2649` — `let permitted = permitted && parent_tool_authority.contains(*name);` is unconditional; `ParentToolAuthority` (`spawner.rs:737`) has no skip arm, defaults to `CHILD_ELIGIBLE_TOOLS`, and `narrow_to` takes an intersection so no path can widen. Declared at `bootstrap.rs:2595` (session, after `apply_posture` and the persona `retain`), `engine.rs:3996` (transients), `bootstrap.rs:282` (roots). |
| F21-02-03 | dispatch-time `PolicyGate` from the same snapshot | **CONFIRMED.** `spawner.rs:2233` — `engine.set_policy_gate(PolicyGate::from_parent_tools(authority.iter()))` inside `execute_resolved_launch`, built from the ONE snapshot taken at `spawner.rs:2172` that also builds the registry. This is the first production caller of `set_policy_gate` on the agent path; the prior verification measured ZERO callers. |
| F21-04-03 | atomic journal-head capture | **CONFIRMED.** `build_and_append` now calls `journal.append_built_from_head(...)`, moving the head read inside the append's writer-lock acquisition. The read-modify-write that spanned two acquisitions is gone. |
| F21-04-02 | disproved, corpus binding at fault | **CONFIRMED.** No product file changed; the corpus probe was rebound onto the charged meter alongside the reserved one. |

**Enumeration guard is real and it can fail.** `spawner_authority_enumeration`
re-derives the production construction sites from source. Its own commit records
four RED controls (gate removed, both layers neutralised, a second snapshot
injected, a 7th spawner site injected). At HEAD it runs 3 passed / 0 failed.

---

## 2. Success Criteria, re-graded

### SC1 — "A child cannot widen ANY provider, tool, filesystem, egress, secret, approval, depth, fan-out, time, token, or cost restriction."

**MET WITH STATED EXCEPTIONS** (was NOT MET).

**2.1 The deciding falsifier is gone.** The prior grade did not rest on breadth
of proof; it rested on one measured falsification. `21-04-PHASE-VERDICT.md:50`
recorded the product's own unit test showing `build_tool_registry` registering
Bash "without ever consulting a parent". A child COULD widen tool authority.
That instance no longer exists, and its absence is proved on the shipped binary,
not in process:

```
crates/wcore-cli/tests/f21_02_01_child_tool_authority.rs   (hetzner-dsm, HEAD)
  f21_02_01_delegated_child_cannot_obtain_a_tool_the_parent_lacks ... ok
  f21_02_01_control_unnarrowed_parent_still_delegates_bash        ... ok
  test result: ok. 10 passed; 0 failed; finished in 7.59s
```

That test spawns the real `wayland-core` binary over `acp serve`, narrows the
parent through the only externally drivable production mechanism (a persona
`allowed_tools` roster), delegates a child that requests `toolsets: ["Bash"]`,
and reads the CHILD'S OWN REGISTRY out of the real wire traffic — not an effect
on disk. It asserts a child provider turn was served before it reads the
verdict, so "no Bash" cannot be produced by "no child ran". **The control is the
differential**: an unnarrowed parent's child DOES get Bash, so a blanket denial
or a harness that never reaches the seam fails it.

**2.2 What the eleven dimensions actually read at HEAD** (Linux, from
`child_authority_corpus`, 27 passed / 0 failed, 110.31s):

| Dimension | standalone in-proc | host-proto in-proc | standalone live | host-proto live |
|---|---|---|---|---|
| tool | REFUSED | NOT-EXPRESSIBLE | **REFUSED** (2 child turns) | **REFUSED** (2 child turns) |
| filesystem | REFUSED | REFUSED | **REFUSED** (2) | **REFUSED** (2) |
| egress | REFUSED | NOT-EXPRESSIBLE | **REFUSED** (2) | **REFUSED** (2) |
| secret | REFUSED | REFUSED | **REFUSED** | **REFUSED** |
| depth | REFUSED | REFUSED | NOT-EXPRESSIBLE | NOT-EXPRESSIBLE |
| fan-out | REFUSED (8 requested vs cap 5 ⇒ 0 children; control batch of 5 admitted 5) | NOT-EXPRESSIBLE | NOT-EXPRESSIBLE | NOT-EXPRESSIBLE |
| time | REFUSED | REFUSED | NOT-EXPRESSIBLE | NOT-EXPRESSIBLE |
| token | REFUSED | REFUSED | NOT-EXPRESSIBLE | NOT-EXPRESSIBLE |
| cost | REFUSED | REFUSED | NOT-EXPRESSIBLE | NOT-EXPRESSIBLE |
| provider | NO-CHANNEL | NO-CHANNEL | **NO-CHANNEL** (1) | **NO-CHANNEL** (1) |
| approval | NO-CHANNEL | REFUSED | **NO-CHANNEL** (1) | **NO-CHANNEL** (2) |

Zero ALLOWED verdicts on any dimension, any surface, any mode.

**2.3 Why this is now a proof gap and no longer a guard gap.** The phase's own
line (`21-04-PHASE-VERDICT.md:86`) is *"Criteria 2 and 3 have gaps in PROOF;
Criterion 1 has a guard confirmed absent."* At HEAD no guard is confirmed
absent. Every dimension either refuses, or has no child-fillable request field
in the shipped product — and NO-CHANNEL is a stronger form of "cannot widen"
than a guard, not a weaker one. What is missing is live drive for five
dimensions, and for four of those it is missing because obtaining it would
require ADDING the attack surface in order to test it.

**2.4 Cross-audit, and the discount it deserves.** Four-way panel, framed with
the phase's own PROOF-vs-GUARD line:

* `codex-sol` — **MET-WITH-STATED-EXCEPTIONS**
* `gemini-3.1-pro` — **MET-WITH-STATED-EXCEPTIONS**
* `kimi-k3` — **MET-WITH-STATED-EXCEPTIONS**
* internal adversarial pass, arguing FOR keeping NOT-MET — **lost, but not cheaply**

The unanimity is discounted for the same reason the phase discounted its own
panel: all three received the same framing, and that framing embedded the
PROOF/GUARD distinction, which pre-decides the question. The adversarial case
is recorded because it is not weak: the criterion says *any*, the phase's own
standard is *"a criterion that says ANY is not satisfied by MOST"*, and the
original NOT-MET rested on TWO legs — the absent tool guard **and** "six of
eleven hold by absence of a channel rather than by enforcement". Only the first
leg was repaired. Grading up now means accepting that the second leg was never
sufficient on its own.

It lost on a point checkable without the panel: a safety property of the form
*"X cannot happen"* is falsified by an instance of X, and satisfied — not
weakened — by X being unreachable. The phase had an instance. It no longer does.
Every remaining dimension is unreachable rather than unguarded, and the
unreachability is pinned by canaries that were proved red-able by injecting a
production file naming a child-sourced request type
(`the_no_channel_canary_goes_red_on_a_realised_approval_widening` ... ok,
`the_no_channel_canary_passes_once_the_widening_is_removed` ... ok, both at HEAD).

**Stated exceptions carried on this grade:** fan-out, time, token and cost hold
live by absence of a request channel; depth and fan-out have no live drive;
Windows is unmeasured at this SHA.

### SC2 — "Nested reservation, refund, escalation, approval, cancellation, and result delivery remain attributable to the correct parent/session."

**MET WITH STATED EXCEPTIONS** — one exception discharged, one fenced.

`child_attribution_corpus` at HEAD: **20 passed / 0 failed, 61.08s.**

| Event | in-process | live |
|---|---|---|
| reservation | **CORRECT** (A: 100/$0.10, B: 250/$0.25, each on its own books) | NOT-OBSERVABLE (host protocol) |
| **refund (crash + restart)** | **CORRECT — was NOT-OBSERVABLE** | NOT-OBSERVABLE (host protocol) |
| escalation | **CORRECT** (sibling's second extension returned `Err(NoExhaustedBudget)`) | NOT-OBSERVABLE |
| approval | **CORRECT** (2 outstanding before the answer, 1 after) | **CORRECT on the rendered TUI screen**; NOT-OBSERVABLE on json-stream |
| cancellation | **CORRECT** (alpha `Cancel`, beta `Run`) | **CORRECT on the rendered TUI screen**; NOT-OBSERVABLE on json-stream |
| result delivery | **CORRECT** | **CORRECT on the real wire** — A's result reached only `spawn:0:anon`, B's only `spawn:1:anon` |

**Zero MISATTRIBUTED verdicts, anywhere, in any mode.** Unchanged and re-measured.

**Exception 2 (F21-04-02) is DISCHARGED.** The refund row now reads, at HEAD:
*"the crash left sibling A's reserved books at (100, 0.1) and sibling B's at
(250, 0.25) in the journal; the restart posted (100, 0.1) to sibling A's charged
books and (250, 0.25) to sibling B's ... the refund reported true ... the
reservations survived the crash on the siblings that made them, the restart
charged each sibling only its own, and the refund reduced only the sibling whose
reservation it released."* The finding's fear was that a crash silently returns
spent budget; the measurement is that it CHARGES it, which is the safe
direction. The disproof is executable, drives the real binary under SIGKILL, and
was proved non-vacuous by mutation (inverting restart reconciliation to refund
every recovered reservation makes all three artefacts fail).

**Exception 1 (F21-04-01) is FENCED, not failed.** Four of six events have no
per-child observable on the host protocol. Specified in
`.planning/SEAM-REQUESTS/F21-04-01.md` as a coordinated Core/Desktop release
because every edit moves `fixture_digest` and `schema_digest`. Graded as a
fenced seam per instruction; it is an observability gap, and nothing in this
corpus caught the product putting a nested event on the wrong actor.

**Exception 3 (windows-tui, MEDIUM) stands**, re-emitted by the corpus at HEAD.

### SC3 — "Standalone and host-protocol hostile corpora prove equivalent enforcement."

**NOT MET.** Unchanged, and re-measured rather than inherited.

The repairs did not touch, and did not claim to touch, any of the three clauses:

1. **tool and fan-out have no host-protocol expression at all.** Measured at
   HEAD: the host child-spawn request type carries
   `[name, prompt, max_turns, max_tokens, system_prompt, provider, model, temperature]`
   and none of them expresses a tool-authority or breadth request;
   `spawn_host_child` hardcodes `ForkOverrides::default()`. Both verdicts are
   correctly WITHHELD rather than borrowed from the standalone driver — but a
   withheld verdict is not an equivalence.
2. **Fan-out is undetermined live**, on both surfaces: 1 provider request served,
   0 by a delegated child.
3. **Windows is unmeasured at this SHA by anyone**, including this re-grade.

One clause did improve: the tool dimension's REFUSED was previously jointly
attributable to tool authority and workspace containment, and the new live test
separates them by reading the registry instead of an effect. That is not enough
to move the grade.

---

## 3. Requirements

**F21-01 — MET.** *Every child receives the intersection of parent and requested
provider, model, tool, filesystem, egress, secret, and approval authority.*
Deciding evidence: `f21_02_01_delegated_child_cannot_obtain_a_tool_the_parent_lacks`
plus its control, on the real binary; `filesystem`, `egress` and `secret`
REFUSED live on both surfaces with 2 child provider turns each; `provider` and
`approval` NO-CHANNEL live with a real actor, canaries red-able. The exact
sentence that kept it open — *"Marking this complete would claim an intersection
the product does not compute"* — is no longer true: the product computes it, at
construction and again at dispatch, from one snapshot, at all six production
spawner sites, with a source-derived guard against a seventh.

**F21-02 — NOT MET.** *Nested children cannot exceed parent depth, fan-out,
concurrency, token, cost, or time reservations.* Depth and fan-out refuse
in-process with real numbers; time, token and cost refuse at the ancestor-rollup
seam. **Not one of the six has live evidence.** Four of them cannot obtain any,
because no shipped surface carries a child-fillable budget field. This is the
phase's standing rule applied exactly as written — *a requirement is never
marked complete on in-process evidence alone* — and it is the honest red.

**F21-03 — FENCED.** *Approval, escalation, cancellation, reservation, refund,
and result delivery remain attributable to the correct parent/session actor.*
All six now CORRECT at the real in-process seam; refund CORRECT across a real
crash and restart; result delivery CORRECT on the shipped wire; approval and
cancellation CORRECT on the live rendered TUI screen. Zero misattributions. Its
sole remaining blocker is F21-04-01, which is a specified, deferred protocol
seam and not a defect in product code. Graded FENCED rather than MET because the
requirement's own words are "remain attributable", and four of six events cannot
be addressed or audited per child by a host — which is what F21-04-01 exists to
fix.

**F21-04 — NOT MET.** *Hostile child tests prove no authority or resource
amplification across standalone and host protocol paths.* Three of the four
blockers named in its OPEN text are gone: tool authority is present and
live-proven, F21-04-03 is repaired, F21-04-02 is disproved. The fourth is not:
the requirement demands proof across BOTH paths, and SC3 is NOT MET.

---

## 4. The vacuity question, answered in the words it was asked in

**F21-02's budget dimensions are STILL MERELY UNREQUESTABLE. They are not
enforced-and-driven.** Re-measured at HEAD, three independent ways:

1. `crates/wcore-agent/src/engine.rs:6173` — `begin_active_turn(turn_id, None)`,
   the sole production caller, still `None`.
2. `crates/wcore-budget/src/execution.rs:936` — the only `sub_budget(Some(..))`
   call site in the crate sits inside `#[cfg(test)] mod tests`. Zero production
   callers pass `Some(..)`.
3. The corpus re-asserts it mechanically on every budget row at HEAD:
   *"NO-CHANNEL canary intact: no `crates/*/src` file forwards a `Some(..)`
   override into `sub_budget`."*

`SubAgentConfig` (`crates/wcore-types/src/spawner.rs:518`) carries no budget,
depth or breadth field — read directly, and independently pinned by the corpus's
exhaustive-destructuring guard, which stops compiling if a field is added.

**What DID change is the tool dimension, and it changed in the opposite
direction.** Tool authority was never vacuous — it was requestable
(`ForkOverrides.allowed_tools`) and unenforced, which is the worst of the four
possible states. It is now requestable and enforced, at construction and at
dispatch, and driven live against the shipped binary. That is a genuine move
from *unguarded* to *enforced*, not a move from *unenforced* to *unrequestable*.

---

## 5. Gate-can-fail discipline

Every gate this re-grade relied on has a demonstrated red:

| Gate | Can it fail? |
|---|---|
| `f21_02_01_delegated_child_..._parent_lacks` | Yes — its sibling CONTROL asserts the opposite outcome on the same script, so a blanket denial or an unreached seam fails one of the pair. |
| `spawner_authority_enumeration` | Yes — four RED controls recorded at `9c3e3687` (gate removed, both layers neutralised, second snapshot injected, 7th spawner site injected). |
| `concurrent_journal_writer_never_faults_budget_authority` | Yes — 200 commits against a concurrent conversation writer; it fails before the atomic-capture repair. |
| NO-CHANNEL canaries | Yes — proved by injecting a production file naming the child-sourced policy request type; `the_no_channel_canary_goes_red_on_a_realised_approval_widening` and `..._passes_once_the_widening_is_removed` both green at HEAD. |
| F21-04-02 disproof | Yes — mutation-proved: inverting restart reconciliation fails the live SIGKILL test, the unit test, and flips the corpus verdict to MISATTRIBUTED. |

Nothing was weakened, ignored, re-gated, deleted, or given a longer timeout to
reach any result in this document. No production file was modified by this
re-grade. `cargo fmt --all -- --check` is clean.

---

## 6. The `ToolConfirmer` artefact — why this report is not another instance of it

`crates/wcore-agent/src/confirm.rs:125-131` still returns `Denied`
unconditionally when `io::stdin().is_terminal()` is false. It is unchanged at
HEAD and this re-grade does not claim otherwise. **None of the live evidence
above is that artefact:**

* Every standalone live row ran over a **real PTY** (`headless-pty` / `tui`
  transports) and the transcripts record *"approval prompts answered: 1"* and
  *"approval prompts answered: 3"* — a human-equivalent approver acted.
* Every host-protocol live row ran over `--json-stream`, whose `ready` frame
  advertises `"tool_approval":true`; approvals are answered on the protocol, not
  on stdin.
* Every decisive live row carries **1 or 2 delegated child provider turns**, and
  the corpus states the attribution explicitly: *"a child existed and took its
  own turn — the refusal is attributable to what that child was given rather
  than to the child never having acted."*
* The one row that still shows the artefact is
  `corpus_tool :: standalone :: in-process`, whose evidence string reads *"What
  the child's own tool call returned: Tool execution denied by user"*. That row
  is IN-PROCESS, is not load-bearing for any grade above, and the corpus itself
  flags the joint attribution.

---

## 7. New findings

**N1 (LOW, documentation).** The corpus's own census row is stale relative to the
repaired product: at HEAD it still prints
`CENSUS :: corpus_tool :: tool :: ABSENT :: expectation REFUSED :: seam S2-spawn-seam :: canary false`.
The guard is present; the census that says otherwise is the artefact of a
measurement taken in 21-01. Not a product defect. Worth correcting before Phase
22 reads it as current.

**N2 (INFO, evidence quality).** The phase corpus's tool REFUSED remains jointly
attributable to tool authority and workspace containment — its own words. Only
`f21_02_01_child_tool_authority.rs` separates the two, by reading the child's
registry off the wire rather than grading an effect on disk. Any future reader
should cite the latter, not the former, for the tool dimension.

**N3 (unchanged product fact).** `ToolConfirmer::check_for` fail-closed
non-terminal denial is unrepaired. It was disclosed by `VERIFICATION-2.md` as a
Windows question (session-0 scheduled tasks report `is_terminal() == true` and
then block forever on `read_line`). Nothing here bears on it; it is restated so
it is not lost.

---

## 8. What this phase is now blocked on

1. **SC3's three clauses** — a host-protocol expression for tool and fan-out, a
   live fan-out control that separates "the cap bound" from "nothing ran", and
   a Windows live actor or an accepted decision that Windows equivalence stands
   on three of four combinations.
2. **F21-02's evidence shape** — a recorded decision on whether a dimension with
   no request channel may close on in-process enforcement plus a red-able
   canary. This is not a measurement question and no further measurement will
   settle it.
3. **F21-04-01** — fenced, specified, awaiting a coordinated Core/Desktop
   release. Not a gradeable failure of Core.
4. **Windows re-proof of F21-04-03** — 0/24 is claimed and is unverified here.

---

## 10. Regression suite at HEAD, and the two reds it produced

Run once, per the one-full-run rule, on `hetzner-dsm`:

```
cargo test -p wcore-agent --lib -j 2 -- --test-threads=1
test result: FAILED. 2096 passed; 2 failed; 3 ignored; finished in 147.99s
```

**The two reds are host resource exhaustion, not code.** Both are
`file_watcher_notifier::tests::*`, and both panic with
`Notify(Error { kind: Io(Os { code: 24, ... message: "Too many open files" }) })`
— EMFILE, raised while the box was at load average **146.59 with 842 logged-in
sessions** and five other lanes building concurrently. They are reported red
here rather than dismissed; nothing was ignored, re-gated, retried to green, or
given a longer timeout.

A first, PARALLEL run of the same suite reported **14 failed**. Twelve of those
were `session journal writer lease is already held` — a shared-journal
serialisation artefact of running these tests concurrently, and they all cleared
under `--test-threads=1`. That is recorded because the parallel number is the
one an unwary reader would quote, and it is wrong. `2096 + 2 = 2098`, matching
the count the reconciling merge (`9c3e3687`) recorded.

---

## 11. Platform honesty

This re-grade produced **Linux evidence only**, on `hetzner-dsm` under a load
average that peaked at 149 with five other lanes building. No Windows run was
made, none is inherited, and every Windows statement above is explicitly marked
as unmeasured. Two of the three SC3 clauses and one behaviour-unverified item
turn on Windows.

---

_Graded: 2026-07-27 at `ac94b1d5` · Linux (hetzner-dsm) · Verifier: Claude (ferrox-verifier), third grading_

---

# ADDENDUM — 2026-07-29, lane/record-truth

**This addendum does not rewrite anything above it.** The body of this document,
its frontmatter and its §4 are left exactly as the third grading wrote them.
Re-writing a prior verification's conclusions from a later lane is a hazard this
program has already had to correct once; the record of what was believed at
`ac94b1d5`, and why, is worth more than a tidy document.

**What this addendum establishes: F21-02 is carried NOT MET on a premise that is
false at HEAD, and the phase was graded before its own work landed.**

Graded at HEAD of `lane/record-truth`, base `ef1d97be`
(`plan/f20-unified-audit-repair`). Mac-side source measurement only; no build.

## A1. The grading predates the work — measured on commit timestamps

This document's `verified_at_sha` is `ac94b1d5`, committed **2026-07-27
07:58:42 +0700**. Four commits landed the same day, after it, that build the
budget request channel:

| SHA | committed (+0700) | subject |
|---|---|---|
| `10947402` | 2026-07-27 **08:47:45** | feat(21-02): sub-allocate a narrowed execution envelope to delegated children |
| `373599ea` | 2026-07-27 **08:54:18** | test(21-02): invert the no-channel canary to assert the channel exists |
| `d12d7d48` | 2026-07-27 **09:17:17** | test(21): unblind the budget no-channel canary |
| `d29413c1` | 2026-07-27 **21:07:51** | fix(corpus): grade the budget legs on enforcement, not on channel absence |

The frontmatter's `verified: 2026-07-27T01:30:00Z` is 08:30 +0700 — 17 minutes
before `10947402`. Either reading puts the grade before the work. **Nothing here
is a criticism of the grading: it was correct for the tree it read.**

## A2. §4's three measurements, re-measured at HEAD — 1 survives, 2 are false

§4 asserted the budget dimensions are "STILL MERELY UNREQUESTABLE", supported by
three independent measurements. At HEAD:

1. **SURVIVES.** `begin_active_turn(turn_id, None)` is still the sole production
   caller — now `engine.rs:6203` (§4 cited 6173). Every other hit is inside
   `budget_authority.rs`'s own tests. **But this measures the per-turn engine
   path, not the child-spawn path**, so it no longer supports the conclusion it
   was used for.
2. **FALSE.** §4: *"the only `sub_budget(Some(..))` call site in the crate sits
   inside `#[cfg(test)] mod tests`. Zero production callers pass `Some(..)`."*
   At HEAD `crates/wcore-budget/src/execution.rs:591` is
   `self.sub_budget(Some(narrowed))`, in the body of `sub_budget_narrowed`. The
   first `#[cfg(test)]` in that file is line **964** — line 591 is production.
3. **FALSE.** §4: *"no `crates/*/src` file forwards a `Some(..)` override into
   `sub_budget`."* `crates/wcore-agent/src/spawner.rs:1350` and `:1377` forward a
   caller-supplied `ChildBudgetRequest` into `sub_budget_narrowed`, which
   forwards `Some(..)` on. The first `#[cfg(test)]` in `spawner.rs` is line
   **1448** — both callers are production.

## A3. The channel exists, is child-fillable, and cannot widen

- `pub struct ChildBudgetRequest` — `crates/wcore-types/src/spawner.rs:555`;
  `pub budget: Option<ChildBudgetRequest>` on `ForkOverrides` at `:597`.
- `pub fn sub_budget_narrowed` — `crates/wcore-budget/src/execution.rs:586`.
- **Untrusted request surface:** `crates/wcore-tools/src/delegate.rs:105`
  `fn parse_budget(input: &Value) -> Option<ChildBudgetRequest>` — the delegating
  model fills it. This is precisely the "child-fillable budget field" §4 and the
  `gaps:` block say no shipped surface carries.
- **Monotonic by construction:** `sub_budget_narrowed` intersects the request
  with `self.effective_budget()` before passing it down, so a larger request
  cannot amplify — the worst an adversarial delegator achieves is
  under-allocating its own descendant.

## A4. The live evidence the gaps block says cannot exist

`21-02-VACUITY-SUMMARY.md` §3.1, from `crates/wcore-cli/tests/f21_02_child_budget_live.rs`
on `hetzner-dsm` — two runs of real `wayland-core acp serve`, hermetic home,
wiremock provider, differing **only** by the `budget` object the parent's own
model put on its `Delegate` call:

```
F21-02 LIVE: control child served 8 turns, narrowed child served 3 turns
under a 900-token sub-allocation of a 100000-token root.
test result: ok. 10 passed; 0 failed; finished in 4.71s
```

The child charges 400 input tokens a turn, was permitted two (800) and **refused
the third** (1200 > 900) while the 100 000-token root was nowhere near binding.
The control child ran its full 8-turn script from the same root, which is what
makes the narrowed number attributable to the sub-allocation rather than to a
harness that never reached the seam.

**The gate is proven red-able**, which is the part that matters here: reverting
the spawn seam to unconditional `sub_budget(None)` serves the narrowed child
**8** turns and the differential collapses (§3.2 of that document, with four
further mutation controls).

## A5. §4's own instrument was blind, and has since been repaired

§4's third measurement leaned on the corpus canary. `21-02-VACUITY-SUMMARY.md` §5
finding **F-1 (HIGH)** records that `budget_no_channel_canary` grepped the literal
`sub_budget(Some(` while excluding `crates/wcore-budget/` — so once the caller
moved to `sub_budget_narrowed(...)` it reported "NO-CHANNEL canary intact"
against a live, LLM-reachable channel. That lane deliberately did not edit it,
calling it a verifier's decision.

**It has since been repaired:** `d29413c1` ("grade the budget legs on enforcement,
not on channel absence", +218/−42 across the corpus). `budget_no_channel_canary`
returns **0 hits** in `crates/wcore-cli/tests/child_authority_corpus/surfaces.rs`
at HEAD. F-1 is CLOSED.

The replacement canary `crates/wcore-agent/tests/f21_02_no_channel_canary.rs` is
inverted: it asserts the channel EXISTS and is resolved by intersection, reads
only `crates/*/src`, and asserts its own crawl collected >100 files so a broken
walk cannot make it vacuous.

## A6. Re-grade

**F21-02 — NOT MET ⇒ MET WITH STATED EXCEPTIONS.**

The requirement is *"nested children cannot exceed parent depth, fan-out,
concurrency, token, cost, or time reservations."* Its NOT MET rested entirely on
the vacuity argument — that the property held because nothing could ask. Nothing
can ask is no longer true; something can ask, the ask is resolved by intersection
against the caps that bind the parent, and a child that tries to spend past the
result is refused live on the shipped binary with a differential control.

**Stated exceptions, carried honestly and taken from the implementing lane's own
"what I am not claiming":**

1. **Only the token dimension is live-driven.** Fan-out, time, cost and depth now
   have a request channel and the same intersection, but are exercised
   **in-process only**. The channel makes them obtainable; nobody has obtained
   them.
2. **The widening direction was already unamplifiable**, so a test that "refuses a
   widening request" is largely theatre. The narrowing differential is the leg
   that cannot pass vacuously, and it is the one that was built.
3. **Linux only.** No Windows evidence exists and none is inherited.
4. **`Spawn` and `spawn_host_child` cannot carry a budget request** — `Spawn`
   takes no `ForkOverrides` and `spawn_host_child` hardcodes
   `ForkOverrides::default()`. `Delegate` is the only surface (F-3, INFO).

**Unchanged by this addendum: SC3 and F21-04 remain NOT MET**, F21-03 remains
FENCED, and **the phase goal remains NOT ACHIEVED.** This addendum moves one
requirement on measurement; it does not move the phase. The two open questions in
§8 items 1 and 3 are untouched. §8 item 2 — *"a recorded decision on whether a
dimension with no request channel may close on in-process enforcement plus a
red-able canary"* — is now **moot**: the dimension has a request channel, so the
question it posed no longer arises.

## A7. What I did not do

- No build, no test execution. Every claim above is a source or git measurement
  on the Mac, or a quotation of a number another lane measured on `hetzner-dsm`
  and recorded with its command. **I did not re-run `f21_02_child_budget_live.rs`
  myself** — the 8-vs-3 figure is `21-02-VACUITY-SUMMARY.md`'s, attributed, not
  re-measured here.
- I did not touch the frontmatter. A reader consuming `requirements:` will still
  see `F21-02 NOT MET`; **the machine-readable header and this addendum disagree
  on purpose**, because silently editing a prior verifier's frontmatter is the
  hazard named at the top. Whoever next re-grades the phase should reconcile it.

_Addendum: 2026-07-29 · base `ef1d97be` · lane/record-truth · source measurement only_
