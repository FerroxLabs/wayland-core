# 21-03 — Repair Set: triage, authorization, and the post-repair delta

Phase 21, plan 03. Field separator ` :: `.

The sole authorized input to this triage is `21-02-CORPUS-RESULTS.md`, produced
by an independent pass that was forbidden from fixing anything. Nothing else
enters the repair set — not a defect noticed in passing, not an adjacent
improvement, not a census dimension the corpus did not actually flag.

```
BASE-SHA :: 2d5a3d550dcd1e117b03249f6a5e20b66360f64b
BRANCH :: plan/f20-unified-audit-repair
CORPUS-INPUT :: 21-02-CORPUS-RESULTS.md at RUN-SHA 4a3dd3756efec29f91fa99ce4a68500c485adc1f
PHASE-BASE :: dd02a624e99ac061cc38a070c1a99719c80f2f68
AUTHORISATION :: 21-01-ADMISSION-GATE.md :: SCOPE-LIMIT :: 21-03 :: PROCEED
```

## 0. The admission gate, read and not re-verified

`21-01-ADMISSION-GATE.md:172` records:

> `SCOPE-LIMIT :: 21-03 :: PROCEED :: Triage and authorized repair are bounded to
> the four Core-internal HIGH findings recorded in the census, namely HIGH-1 tool
> replacement, HIGH-2 absent provider intersection, HIGH-3 orphan PolicyGate and
> HIGH-4 non-managed approval replacement. BINDING CONSTRAINT carried from the
> adversarial dissent, which every downstream reader must honour. No repair may
> land a change to the 40 generator source inputs or the 156-file Desktop
> contract corpus without re-running wcore-contract digest and check, recording
> the new digests, and re-pinning D1 section 3 as an explicit contract bump.
> HIGH-4 sits directly on this surface so the constraint is live, not
> hypothetical.`

The gate is OPEN for this plan. 21-01 owns that determination and it is not
re-verified here.

**The binding constraint is materially larger than the gate's author expected,
and this is the single most important thing the triage discovered.**
`crates/wcore-protocol/src/contract/spec.rs:833-874` lists the 40 generator
source inputs verbatim. Three of them are the edit sites of the three HIGH
findings' proposed repairs:

```
CONSTRAINT-HIT :: F21-02-01 :: crates/wcore-agent/src/bootstrap.rs :: SOURCE_INPUTS[21]
CONSTRAINT-HIT :: F21-02-02 :: crates/wcore-types/src/execution_policy.rs :: SOURCE_INPUTS[16]
CONSTRAINT-HIT :: F21-02-03 :: crates/wcore-agent/src/engine.rs :: SOURCE_INPUTS[22]
CONSTRAINT-CLEAR :: crates/wcore-agent/src/spawner.rs :: NOT a generator source input
```

`source_digest()` (`contract/generate.rs:1080`) hashes the **contents** of every
one of the 40, so any byte change to those three moves `source_inputs_digest`,
which moves the `ready` descriptor, which D1 records as a hard renegotiation
error rather than a tolerated upgrade. Every candidate repair is therefore
measured for its digest effect in the Task 2 probe, not assumed.

## 1. Triage — one disposition per corpus red

`FINDING :: [id] :: [severity] :: [FIX | DISPROVE | BACKLOG] :: [seam file] :: [seam symbol]`

Severity is copied from the independent measurement and is never softened here.
FIX and DISPROVE are available only to CRITICAL and HIGH; BACKLOG is the only
disposition the amended rules give MEDIUM and below.

```
FINDING :: F21-02-01 :: HIGH :: FIX :: crates/wcore-agent/src/spawner.rs :: build_tool_registry
FINDING :: F21-02-02 :: HIGH :: FIX :: crates/wcore-types/src/execution_policy.rs :: with_requested_approvals
FINDING :: F21-02-03 :: HIGH :: FIX :: crates/wcore-agent/src/engine.rs :: policy_gate
FINDING :: F21-02-04 :: MEDIUM :: BACKLOG :: crates/wcore-cli/tests/child_authority_corpus/cases.rs :: corpus_tool
FINDING :: F21-02-05 :: MEDIUM :: BACKLOG :: crates/wcore-cli/tests/child_authority_corpus/cases.rs :: corpus_token
FINDING :: F21-02-06 :: MEDIUM :: BACKLOG :: crates/wcore-agent/src/egress/policy.rs :: AgentEgressPolicy
FINDING :: F21-02-07 :: LOW :: BACKLOG :: crates/wcore-cli/src/main.rs :: trailing_var_arg
FINDING :: F21-02-08 :: LOW :: BACKLOG :: crates/wcore-cli/tests/support/vault.rs :: vault
FINDING :: F21-02-09 :: LOW :: BACKLOG :: .planning/phases/21-child-authority-and-budget-inheritance/21-02-PLAN.md :: json-stream
FINDING :: F21-02-10 :: MEDIUM :: BACKLOG :: crates/wcore-cli/tests/deterministic_openai_loop.rs :: packaged_core_cancels_an_active_stream
```

`CRITICAL` count in the corpus: 0. `HIGH` count: 3. `MEDIUM` count: 4. `LOW`
count: 3. Total 10, which is every `FINDING` row in `21-02-CORPUS-RESULTS.md`.

### Why HIGH-2 (provider) carries no row

The admission gate's scope limit names four census HIGH findings, but the
independent corpus produced `FINDING` rows for only three. HIGH-2 is discussed
at `21-02-CORPUS-RESULTS.md:244-250` — *"confirmed as recorded, not closed"* —
and deliberately given no finding row, because the corpus measured the provider
dimension `NO-CHANNEL` on all eight combinations across both platforms and added
a live structural canary that reads the real `input_schema()` of the production
`Delegate` and `Spawn` tools and fails the day either grows a provider-naming
property. The scope limit is a CEILING on what may be repaired, not a floor on
what must be. The repair set is bounded by the corpus and by nothing else, so
HIGH-2 is recorded here as in-ceiling but unflagged, with its canary already in
the suite. This is stated rather than silently dropped.

### F21-02-01 — HIGH — FIX

**Seam:** `crates/wcore-agent/src/spawner.rs :: build_tool_registry`
(census `DIMENSION :: tool :: ABSENT :: crates/wcore-agent/src/spawner.rs :: build_tool_registry`).

**What the corpus left open.** Every tool combination recorded REFUSED, but none
reached `build_tool_registry` with a live child: a `toolsets: ["Bash"]` request
classifies as `RequestedChildWorkspace::IsolatedMutation` and durable workspace
preparation refuses first in a hermetic non-repository workspace. The corpus
measured the *second* of the census's three mitigations, not the absence of
intersection. 21-03 must reach the seam directly or disprove it.

**Reached directly, at the seam, and it is CONFIRMED — not disprovable.**
`build_tool_registry` (`spawner.rs:2396-2445`) takes `allowed: &[String]`, a
fixed six-tool array, the requested workspace class, the read-deny list and the
sandbox runtime. It never receives, and the `AgentSpawner` struct
(`spawner.rs:719-800`) never carries, any representation of the parent's own
tool authority. The permit expression is:

```rust
let permitted = if allowed.is_empty() { SHARED_READ_ONLY_CHILD_TOOLS.contains(name) }
                else { allowed.iter().any(|a| a.as_str() == *name) };
let permitted = permitted
    && (requested_workspace == RequestedChildWorkspace::IsolatedMutation
        || SHARED_READ_ONLY_CHILD_TOOLS.contains(name));
```

The codebase's **own existing unit test** proves the widening at the seam
without any new harness:
`spawner.rs:4357 tc_7_42_build_tool_registry_destructive_requires_opt_in` calls
`build_tool_registry(&["Bash","Write"], IsolatedMutation, …)` and asserts
`registry.get("Bash").is_some()`. Nothing in that call path consults a parent.
A parent restricted to Delegate + Read/Grep/Glob therefore hands its child
`Bash` whenever an isolated worktree can be prepared. DISPROVE is not available
for this finding: the question the corpus asked is one production can be asked,
and the answer is that authority is a replacement.

**Concrete change.** Give `AgentSpawner` a `parent_tool_authority:
Option<Arc<BTreeSet<String>>>` field with a `with_parent_tool_authority(…)`
builder, propagate it in `clone_for_spawn`, pass it into `build_tool_registry`
as a new final parameter, and intersect **inside that function** so no route
into the seam can skip it:

```rust
let permitted = permitted
    && parent_tool_authority.is_none_or(|authority| authority.contains(*name));
```

Wire the authority at the single production construction site,
`crates/wcore-agent/src/bootstrap.rs:2202`, where the parent's own `registry` is
already in scope (`.with_sandbox_runtime(registry.sandbox_runtime())`) and
`ToolRegistry::tool_names()` (`crates/wcore-tools/src/registry.rs:292`) already
exists. Five in-file test call sites gain a trailing `None`.

**Flags.** Not a behaviour change for a correctly-configured session — a parent
that holds Bash still delegates Bash. It IS a behaviour change for any session
whose parent registry is narrower than the child's request, which is precisely
the amplification. **CONSTRAINT-HIT: the bootstrap wiring line touches
`SOURCE_INPUTS[21]`, so the contract digest moves.** `spawner.rs` itself is
clear.

**Residual this fix does NOT close, stated up front.** `None` means "no parent
authority recorded" and applies no intersection, so legacy and test spawners are
unaffected. That is fail-open by default — the same shape as the PolicyGate
defect in F21-02-03. It is chosen deliberately because a fail-closed default
would deny every tool to every non-bootstrap spawner, and it is mitigated by a
reachability test asserting the production bootstrap path wires it. The residual
is recorded, not hidden.

### F21-02-02 — HIGH — FIX

**Seam:** `crates/wcore-types/src/execution_policy.rs :: with_requested_approvals`
(census `DIMENSION :: approval :: VACUOUS :: crates/wcore-types/src/execution_policy.rs :: with_requested_approvals`).

**Confirmed executably by the corpus** on every platform and both surfaces:
`BaselineExecutionPolicy::smart(Prompt, LocalCliLaunch).with_requested_approvals(Bypass, PolicySource::Child)`
=> posture Smart, approvals **Bypass**, source Child, managed false. Read at
`execution_policy.rs:138-154`: the managed branch ratchets via
`stricter_approval_policy`; the non-managed branch is a bare
`Self::smart(requested, source)` — a replacement.

**Concrete change.** Insert a child-sourced ratchet between the two existing
branches, leaving every other source byte-identical:

```rust
} else if matches!(source, PolicySource::Child) {
    Self::smart(stricter_approval_policy(self.approvals, requested), source)
} else {
    Self::smart(requested, source)
}
```

`stricter_approval_policy` is already a `const fn` (`:169`), so the enclosing
`const fn` still compiles.

**Flags.** Not an architecture change. Behaviour change ONLY for
`PolicySource::Child`, which has no production constructor today, so no existing
session's posture moves — the corpus's own NO-CHANNEL measurement is the
evidence for that. This is the cheapest of the three fixes and the one that best
matches the phase's purpose: it converts a property that holds *by absence of a
channel* into one that holds *by enforcement*, which is exactly the vacuity the
phase was created to eliminate. **CONSTRAINT-HIT: `execution_policy.rs` is
`SOURCE_INPUTS[16]`, so the contract digest moves. The admission gate names this
finding specifically as the live case.**

### F21-02-03 — HIGH — FIX

**Seam:** `crates/wcore-agent/src/engine.rs :: policy_gate` (census seam S3,
`crates/wcore-agent/src/policy_gate.rs` + `crates/wcore-permissions/src/policy.rs`;
the reachability defect is at the engine's field initialisers, which is where a
repair would land).

**Confirmed, and re-measured here rather than trusted.**
`git grep -nF 'set_policy_gate' -- crates/` returns exactly two hits, both in
`engine.rs` — the doc comment at `:2679` and the definition at `:4064`. **Zero
callers.** Every agent-path initialiser is `None` (`:3147`, `:3381`, and the
seven test constructors at `:15307 :16986 :17300 :18688 :19135 :19992 :21042`,
plus `orchestration/node_executor.rs:418`). `policy_gate.rs`'s own header states
the gate is opt-in and that a session without one sees "every tool runs".

**Concrete change (the only one that makes the gate reachable).** Replace
`policy_gate: None` at the two production constructors, `engine.rs:3147` and
`engine.rs:3381`, with a constructed `PolicyGate`.

**Flags — BEHAVIOUR CHANGE and ARCHITECTURE CHANGE, both, and this is not a
patch.** `PolicyEngine::check` (`wcore-permissions/src/policy.rs:120-157`) is
**deny-by-default**: it walks its grant list and returns
`Err(DenyReason::NoMatchingGrant)` when nothing matches. A gate constructed over
a fresh `PolicyEngine::new()` therefore denies **every tool for every existing
session**. Making it useful instead of catastrophic requires populating grants
from the parent's registry — which is an inheritance model, in a crate whose own
header (`policy.rs:1-5`) states its scope is *"explicit grants only. No role
hierarchy, no inheritance"*. That is the exact architecture change census OOP-2
routed OUT of this phase. **CONSTRAINT-HIT: `engine.rs` is `SOURCE_INPUTS[22]`.**

**And it is the secondary mechanism, not the primary.** The census is explicit:
`build_tool_registry` — F21-02-01's seam — is the mechanism that actually runs
for the tool dimension. HIGH-3 is "the policy layer that would express
parent/child intersection is not consulted", not "tools are ungated". If
F21-02-01 lands, the intersection exists at the seam that runs, and HIGH-3's
remaining content is a second, unused expression of the same property.

The triage proposes FIX because the amended rules give a HIGH only FIX or
DISPROVE, and DISPROVE is unavailable — the finding is confirmed true. The
honest expected outcome at the Task 2 authorization is **DECLINED**, recorded
with its evidence and left open. The blast-radius probe measures the cost so
that decision is made against a number.

### F21-02-04 … F21-02-10 — MEDIUM and LOW — BACKLOG

The amended rules give these exactly one disposition and there is no second
option for them, including for the ones that look quick. All are logged in
`.planning/BACKLOG.md` marked explicitly non-blocking with a pointer back to
their corpus case. F21-02-04 through F21-02-09 were logged by 21-02 itself;
F21-02-10 is appended by this plan.

`F21-02-10` additionally carries an **exclusion**: it is a PRE-EXISTING red,
already recorded at `.planning/TEST-AUDIT.md:171` as flaky 2/3 with the `ci`
profile's `retries=2` turning it green. It is excluded from this plan's bounded
repair budget under the plan's known-red exclusion rule, and logged so it is
retained rather than spent twice.

```
EXCLUDED :: F21-02-10 :: pre-existing :: TEST-AUDIT.md:171 records wcore-cli::deterministic_openai_loop packaged_core_cancels_an_active_stream as flaky 2/3 before Phase 21 existed; repairing it here would spend a bounded budget on a red another pass already triaged
```

### Surface-equivalence findings

Triaged separately as the plan requires, and the honest answer is that there are
none. `21-02-CORPUS-RESULTS.md:188-189` records
`MODE-EQUIVALENCE :: CONSISTENT` and `SURFACE-EQUIVALENCE :: CONSISTENT` over
88 executions, with no in-process-REFUSED against live-ALLOWED anywhere, and the
corpus explicitly states "No SURFACE-equivalence failure and no MODE-equivalence
failure was observed, so neither produces a finding of its own". No row is
invented for a failure that did not occur.

## 2. Deliverability against the two-iteration cap

```
DELIVERABILITY :: EXCEEDS :: 3 :: The full proposed set does not fit. F21-02-01 and F21-02-02 are each a contained edit at a named seam and together plausibly fit one edit-build-run iteration with a second in reserve. F21-02-03 does not fit at any iteration count: PolicyEngine is deny-by-default over an empty grant list, so the only change that makes the gate reachable denies every tool for every existing session, and the only change that makes it useful is a grant-inheritance model in a crate whose header states it deliberately has none - census OOP-2 routed exactly that out of Phase 21. Additionally all three edit sites touch the 40 pinned generator SOURCE_INPUTS, so each carries a wcore-contract digest re-run, a check, and a D1 section 3 re-pin under the admission gate's binding constraint. A subset excluding F21-02-03 is stated FITS at 2.
DELIVERABILITY-SUBSET :: FITS :: 2 :: F21-02-01 and F21-02-02 only, with F21-02-03 declined and left honestly open for 21-04 to state as an exception.
```

## 3. Blast radius, measured

Every candidate was applied ALONE at the exact file and symbol Task 1 named, in
the throwaway Hetzner worktree `/root/wayland-p21-probe` created from the triage
commit and **deleted afterwards**, reverted between candidates so each is
measured alone. Baseline and post-change counts over the five crates the three
seams touch: `wcore-agent`, `wcore-types`, `wcore-permissions`, `wcore-protocol`,
`wcore-budget`. Full transcript: `evidence/21-03-t2-blastradius.log`.

```
BLAST-RADIUS :: F21-02-01 :: WIDE :: 3421 :: 3420 :: -1
BLAST-RADIUS :: F21-02-02 :: WIDE :: 3421 :: 3420 :: -1
BLAST-RADIUS :: F21-02-03 :: WIDE :: 3421 :: 3398 :: -23
```

**Read the composition, not the label.** Baseline 3421 passed with ONE
pre-existing non-passing test unrelated to all three candidates
(`wcore-agent::workflow_limits_test fix1_dispatch_budget_aborts_with_partial_result`,
a 60s timeout under concurrent build load that passed on a separate quiet run).

- **F21-02-01 — 1 verdict-changing test**, and it is only
  `wcore-protocol::desktop_contract_corpus checked_corpus_matches_real_serializers_byte_for_byte`.
  `source_inputs_digest` moved `d8b1a8b5…` → `9c7b98e2…`. **No functional test
  changed verdict.**
- **F21-02-02 — 1 verdict-changing test**, the same contract pin.
  `source_inputs_digest` moved `d8b1a8b5…` → `5b0263bc…`. **No functional test
  changed verdict**, which independently CORROBORATES the corpus's NO-CHANNEL
  measurement: nothing in production constructs `PolicySource::Child`.
- **F21-02-03 — 23 verdict-changing tests**: the contract pin plus **22
  functional tests** across `dangerous_lease_e2e_test` (2),
  `engine::audit_2026_05_22_tests` (4), `execution_posture_e2e_test` (1),
  `json_stream_approval_test` (3), `output_compaction_test` (2),
  `runaway_loop_test` (5), `typed_execution_policy_e2e_test` (3) and
  `w9_1_skill_drafting_per_turn` (2). This is the deny-by-default catastrophe
  arriving as a number: turning the gate on with an empty grant list stops tool
  dispatch across the engine.

Nothing returned BREAKS-BUILD, so no candidate was disqualified on that ground.

**The contract-pin cost, measured rather than feared.** The baseline digest
`sha256:d8b1a8b5…` matches the checked-in fixture exactly, so the tree is
in-contract and any SOURCE_INPUT edit moves it. `schema_digest` did NOT move for
any candidate — only the source-provenance fingerprint. `git log` over
`crates/wcore-protocol/contracts/desktop/v1/` shows 16 regenerations, three of
them titled verbatim `chore(protocol): re-pin Desktop contract provenance
digests`, and commit `b6936299`'s message documents this exact situation and its
resolution. The repo ships `just desktop-contract-check` and a
`wcore-contract generate` subcommand for it. The pin failure is therefore
**established routine maintenance with a documented procedure**, not a wire
break.

**A defect in the binding constraint itself, recorded not resolved.** The
admission gate requires "re-pinning D1 section 3 as an explicit contract bump".
`.planning/intel/DESKTOP-PROTOCOL-CHECKPOINT.md` is four paragraphs, **has no
section 3, and pins no digests at all**. That clause names a thing that does not
exist; the digests it is about live in this repo's own contract corpus. The
satisfiable parts of the constraint — re-run `digest`, run `check`, record the
new digests — are honoured by this plan. D1's own closing sentence reads: "Both
receipts are required for a whole-Wayland claim; neither blocks Core-only engine
claims outside the shared contract."

## 4. The panel

One shared bundle (`21-03-t2-panel/panel-prompt.txt`, 24276 bytes) carrying the
question verbatim, all four options in rotated order with pros and cons copied
unaltered, the full triage table with both change-class flags, the two crate
headers quoted from source, the deliverability estimate and every probe verdict
line. Each member's response captured verbatim, stdout and stderr together.

```
PANEL-VOTE :: codex-sol :: authorize-partial :: codex-sol.raw.txt
PANEL-VOTE :: gemini-pro :: authorize-partial :: gemini-pro.raw.txt
PANEL-VOTE :: kimi-k3 :: authorize-partial :: kimi-k3.raw.txt
PANEL-VOTE :: claude-adversarial :: disprove-and-correct :: claude-adversarial.raw.txt
PANEL-DECISION :: authorize-partial :: MAJORITY
PANEL-RATIONALE :: Three of four members independently selected authorize-partial on one shared bundle, a strict 3-1 majority over the adversarial member's disprove-and-correct, so the basis is MAJORITY and no evidentiary tiebreak was required. The set-level choice was never close: authorize-full requires F21-02-03, whose probe measured 22 functional tests changing verdict because PolicyEngine::check is deny-by-default over an empty grant list, and whose useful form needs a grant-inheritance model in a crate whose header states it deliberately has none - the change census OOP-2 routed out of this phase. decline-all discards a measured, functionally contained repair for a cost the repository already pays as routine maintenance three commits running. disprove-and-correct is unavailable as a matter of rule, because the plan permits DISPROVE only where the triage proposed it and the triage proposed FIX for all three. What WAS close, and what the per-finding rows below turn on, is the split inside authorize-partial. gemini-pro and kimi-k3 authorized both F21-02-01 and F21-02-02. codex-sol authorized only F21-02-02 and declined F21-02-01, on the ground that the proposed bootstrap.rs wiring covers one production spawner construction site while several others remain fail-open. That claim was NOT taken on the panel's word - it was independently re-verified against the live tree, and it is TRUE: crates/wcore-cli/src/workflow.rs:173 and crates/wcore-cli/src/crucible.rs:36 have no cfg(test) attribute anywhere above them, and crates/wcore-agent/src/engine.rs:12061 and crates/wcore-agent/src/orchestration/anvil/seat.rs:91 are likewise production. At all four, parent_tool_authority stays None and the intersection is skipped, so the candidate closes one of five doors. Worse, three of those four construct their spawner from a Config with no parent ToolRegistry in scope, so there is nothing available there to intersect against without making parent tool authority a first-class concept across the spawner API - an architecture change, not a wiring line, and not one that fits a two-iteration cap. The per-finding outcome therefore follows the verified evidence rather than the headcount, which is what a measured panel is for. BLAST-RADIUS-ACCEPTED: F21-02-02 is authorized at a measured radius of exactly 1 verdict-changing test, TESTS_BEFORE=3421 TESTS_AFTER=3420, and that one test is the Desktop contract provenance pin rather than any functional test. That cost is worth paying because the repository's own history discharges it as routine - three recent commits titled chore(protocol) re-pin Desktop contract provenance digests, a documented just desktop-contract-check target, and schema_digest unmoved - while the benefit is converting a property that currently holds only by the absence of a request channel into one that holds by enforcement, which is the precise vacuity Phase 21 was created to eliminate.
PANEL-DISSENT :: disprove-and-correct :: The adversarial member made the case nobody else would: that F21-02-02 is textbook DISPROVE on the plan's own definition, because three independent measurements agree the input cannot be constructed - the corpus records NO-CHANNEL on all eight combinations, PolicySource::Child has no production constructor, and the probe changed zero functional tests - so repairing it ships a real wire-provenance bump for a defect nothing can reach. It did not carry for two reasons the member itself recorded. Phase 21 exists precisely because the property holds VACUOUSLY; treating unreachability as the defence inverts the phase's purpose and would forbid ever converting a vacuous property into an enforced one. And the plan permits DISPROVE only over a finding the triage proposed DISPROVE for, so selecting it here would be authorizing a corpus edit over a finding triaged FIX - the exact forgery the RED-over-GREEN rule names.
PANEL-DISSENT :: authorize-full :: Argued by no member. Recorded with the reason it could not be argued: its own cons make it available only if the deliverability estimate says the set fits two iterations, and the triage said EXCEEDS before the probe ran, after which the probe measured F21-02-03 at 22 functional tests changing verdict. Choosing it would have been a waiver dressed as a verdict.
PANEL-DISSENT :: decline-all :: Argued by no member, and rejected explicitly by three. Its strongest form is that with F21-02-01 declined, the phase closes with two of three HIGH findings open anyway, so the marginal value of the one remaining repair is small against a Desktop contract bump the consumer lane cannot renegotiate from this checkout. It did not carry because the bump is provenance-only with schema_digest unmoved, the repository discharges that class of change as routine maintenance, and F21-02-02 is the one finding in the set whose repair is both measured functionally inert and squarely on the phase's stated purpose.
```

### Independence caveat, recorded because concealing it would be worse

The four captures were written into the same directory the members were invoked
from. `gemini-pro` touched no sibling file (0 references). `kimi-k3` observed
that the sibling captures existed and **explicitly declined to read them**,
stating so in its transcript. `codex-sol` ran a ripgrep across the parent
directory that surfaced both `gemini-pro.raw.txt:127` — gemini's position
paragraph — and `21-03-PLAN.md` including this task's own gate scripts. Its
set-level vote is therefore **not independent** and is recorded as such.

Two things bound the damage, and neither is offered as an excuse. `codex-sol`
reached a per-finding conclusion that DIFFERS from gemini's, so it did not
simply copy. And the fact its dissent turns on — four unwired production spawner
construction sites — was re-verified directly against the live source tree
before being acted on, so the decision rests on ground truth rather than on a
vote that may have been contaminated. Even discarding `codex-sol`'s vote
entirely, `authorize-partial` still leads 2-1 and remains the modal position.
The procedure defect is real and belongs in any future plan's panel setup: give
each member its own working directory.

## 5. Authorization, per finding

```
AUTHORIZED :: F21-02-01 :: DECLINED
AUTHORIZED :: F21-02-02 :: FIX
AUTHORIZED :: F21-02-03 :: DECLINED
AUTHORIZED :: F21-02-04 :: BACKLOG
AUTHORIZED :: F21-02-05 :: BACKLOG
AUTHORIZED :: F21-02-06 :: BACKLOG
AUTHORIZED :: F21-02-07 :: BACKLOG
AUTHORIZED :: F21-02-08 :: BACKLOG
AUTHORIZED :: F21-02-09 :: BACKLOG
AUTHORIZED :: F21-02-10 :: BACKLOG
```

**F21-02-01 — DECLINED, and it stays honestly open.** Not because the finding is
wrong; it is confirmed by the product's own unit test. Because the repair Task 1
designed does not close it. `parent_tool_authority` wired at `bootstrap.rs`
leaves the intersection skipped at `crates/wcore-cli/src/workflow.rs:173`,
`crates/wcore-cli/src/crucible.rs:36`, `crates/wcore-agent/src/engine.rs:12061`
and `crates/wcore-agent/src/orchestration/anvil/seat.rs:91`, and three of those
four have no parent `ToolRegistry` in scope to intersect against at all.
Shipping it would place a fail-open guard at one caller and leave the seam
reachable through four other production routes — which the plan's own
"REPAIR AT THE SEAM THE CENSUS NAMED, NOT WHEREVER IT IS EASIEST" rule forbids
by name, and which reproduces the exact PolicyGate fail-open shape this same
decision is declining one finding later. Closing it properly requires making
parent tool authority a first-class concept across the spawner API so every
construction site must supply it — an architecture change that was not on the
table at this checkpoint and does not fit the two-iteration cap. **21-04 must
state this as an explicit exception: Success Criterion 1 cannot be claimed for
the tool dimension.**

**F21-02-02 — FIX.** The only production change this plan ships. Measured radius
1, and that one is the provenance pin.

**F21-02-03 — DECLINED, and it stays honestly open.** 22 functional tests, a
deny-by-default engine, and an inheritance model in a crate that declares it has
none. **21-04 must state this as an explicit exception.**

## 6. Post-repair delta

Populated by Task 3.
