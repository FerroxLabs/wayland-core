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

Populated by Task 2 Step 1. Every candidate applied ALONE in the throwaway
Hetzner worktree `/root/wayland-p21-probe`, reverted between candidates, each
emitting its own verdict line into
`evidence/21-03-t2-blastradius.log`.

## 4. The panel

Populated by Task 2 Steps 2-5.

## 5. Authorization, per finding

Populated by Task 2 Step 4.

## 6. Post-repair delta

Populated by Task 3.
