---
phase: 21-child-authority-and-budget-inheritance
work: F21-02 vacuity — decide (A) vs (B), then implement and prove non-vacuously
branch: lane/f21-02-vacuity
base: plan/f20-unified-audit-repair @ aab3c353
evidence_host: hetzner-dsm /root/wayland-f21v (Linux)
platform_coverage: "Linux only. No Windows run was made and none is inherited."
decision: B (capability gap) — 3/3 panel, internal adversarial pass lost but carried one point
status: complete
---

# F21-02 — from "nothing can ask" to "asked, resolved, enforced"

## 1. Stage 1 — the decision

**Panel: 3/3 for (B).** `codex-sol`, `gemini-3.1-pro` and `kimi-k3` each returned
`PANEL_POSITION=B` on a question that gave them the strongest available form of
(A), the measured facts that cut against (B), and five named points they were
required to engage rather than skip.

**One fact I found while framing the question materially changed it, and I put it
to the panel rather than suppressing it.** The premise "nothing can ask" is not
fully true. `crates/wcore-tools/src/delegate.rs:302` exposes `max_iterations` as
an LLM-fillable integer with no clamp; it flows to `config.max_turns` at
`spawner.rs:2283` and is never compared against the requester's own allowance. A
delegated child holding `Delegate` can hand a grandchild 200x its own turn
budget. Turn count is not one of F21-02's six named dimensions and the ancestor
rollup still binds tokens/cost/time, so this does **not** falsify F21-02's letter
— but it does falsify (A)'s premise. This codebase already admits child-fillable
envelope fields. The absence of a *budget* channel is an omission that happens to
be safe, not a design in which widening is unrepresentable.

All three legs reached the same verdict on that point independently.

**Internal adversarial pass, arguing FOR (A): lost, but won one point I have
carried into the implementation and into the claims below.**

* It lost on capability. Sibling starvation is real and unaddressed: every child
  drew from one undifferentiated parent envelope, so one runaway child could
  drain the subtree. `sub_budget(Some(..))` and `intersect_execution_budget` both
  already existed and were unreachable from production. (A) would have enshrined
  dead code as "the design".
* It lost on gate discipline. An (A)-style canary asserts *"no production caller
  passes `Some(..)`"* — green against an untouched tree, asserting a negative,
  and with a success condition ("nobody ever wires the seam") directly opposed to
  what the fleet phase needs. That is a self-passing gate by the lane brief's own
  §3.2 taxonomy.
* **It won this, and I state it plainly rather than dressing it up:** the
  *widening* direction was ALREADY unamplifiable. `first_exceeded_reason`,
  `try_enter_process` and `try_reserve_tool_runtime` all walk the ancestor chain
  regardless of what the child's leaf names, so `sub_budget(Some(wider))` could
  never grant anything. **A test that "refuses a widening request" is therefore
  largely theatre, and I have not built the canary on it.** The canary is built
  on the NARROWING differential, which is the leg that cannot pass vacuously.
  Measured confirmation below: under the full revert-to-vacuous control, the
  widening test still passes while the narrowing tests all go red.

## 2. Stage 2 — what landed

| Layer | Change |
|---|---|
| `wcore-budget` | `effective_budget()` — pointwise min across leaf + every ancestor. `sub_budget_narrowed(requested)` — intersects the ask with `effective_budget()`, the first production-reachable `sub_budget(Some(..))`. |
| `wcore-types` | `ChildBudgetRequest` (7 provider-neutral scalars); `ForkOverrides.budget`, sited beside `allowed_tools` because both are what a delegator *asks* and both resolve by intersection. |
| `wcore-agent` | `enter_child_budget(requested)` resolves the ask on both the plain and durable-authority paths. `inherit_budget_authority(authority, narrowed)` + `AgentEngine::narrowed_execution_budget` + `current_run_budget()` honour it. |
| `wcore-tools` | `Delegate` advertises a `budget` object over all seven dimensions and parses it. This is the untrusted request channel. |

**Narrowing is monotonic by construction.** The requested envelope is intersected
with the caps that actually bind the requester, so there is no arithmetic path
from a larger requested number to a larger effective envelope. The worst an
adversarial delegator achieves is under-allocating its own descendant. That is
why exposing the field to an LLM adds no widening surface.

### 2.1 The trap, caught

Sean's brief warned that `intersect_execution_budget` is not the spawn primitive.
It is worse than that. On the **durable-authority path — the production path —**
`bind_child_budget` called `inherit_budget_authority(authority)`, which sets the
child engine's envelope from `current_execution_view()`: the **parent's** active
turn. The child view built by `enter_child_budget` was used only as a depth guard
and then dropped. `current_run_budget()` re-derived from the coordinator too.

A sub-allocation wired only at `enter_child_budget` would have computed correctly,
looked correct in the guard, and bound **nothing the child ran against**. It is
pinned by a dedicated test whose RED control is recorded below.

I also confirmed Sean's `limit_for` note and resolved it at the root rather than
at the reader: because the intersection happens at the seam, the child's own leaf
caps ARE the binding ones, so `limit_for` — which the run loop uses to render
`BudgetExceeded` at `engine.rs:11900` — reports the narrowed number rather than
the ask. `effective_budget()` uses the `minimum_remaining` fold pattern for
anything that needs the whole chain.

## 3. Evidence

### 3.1 Live, on the shipped binary (`hetzner-dsm`, Linux)

`crates/wcore-cli/tests/f21_02_child_budget_live.rs` — two runs of real
`wayland-core acp serve`, hermetic home, wiremock provider, differing **only** by
the `budget` object the **parent's own model** puts on its `Delegate` call.

```
F21-02 LIVE: control child served 8 turns, narrowed child served 3 turns
under a 900-token sub-allocation of a 100000-token root.
test f21_02_a_delegated_child_is_bound_by_the_envelope_its_delegator_sub_allocated ... ok
test f21_02_a_delegator_cannot_request_a_wider_envelope_than_the_session_root ... ok
test result: ok. 10 passed; 0 failed; finished in 4.71s
```

The child charges 400 input tokens a turn. It was permitted two (800) and
**refused the third** (1200 > 900) — an actual request for more resource, actually
refused, observably, while the 100 000-token root was nowhere near binding. The
control child ran its full 8-turn script from the same root, which is what makes
the narrowed number attributable to the sub-allocation rather than to a harness
that never reached the seam.

### 3.2 Gate-can-fail — every gate has a demonstrated red

| Control | Mutation | Result |
|---|---|---|
| **LIVE** | spawn seam → unconditional `sub_budget(None)` (the exact pre-existing state) | **RED.** *"the NARROWED child was served **8** turns at 400 input tokens each, against a sub-allocated envelope of 900. The envelope its delegator requested did not bind it."* 8 vs 3 — the differential collapses. |
| `wcore-budget` sub-allocation suite | `sub_budget_narrowed` → `sub_budget(None)` | **RED, 5 of 6.** The 6th is the widening test — measured confirmation of the adversarial point in §1. |
| engine-binding trap test | `inherit_budget_authority` discards `narrowed` | **RED.** `left: Some(10000), right: Some(100)`. |
| agent seam tests | seam reverted to vacuous | **RED, 2 of 4** (the control and the widening test survive, by design). |
| inverted no-channel canary | same revert | **RED**, printing *"NO PRODUCTION CALLER forwards a requested envelope into sub_budget_narrowed. F21-02 has reverted to holding by the ABSENCE of a request channel."* |

### 3.3 Regression (`hetzner-dsm`, load ~14)

```
wcore-agent  --lib -j 2 -- --test-threads=1   2102 passed; 0 failed; 3 ignored (136.89s)
wcore-tools  --lib                             989 passed; 0 failed; 3 ignored
wcore-types                                    134 + 5 passed; 0 failed
wcore-budget --lib                              57 passed; 0 failed
child_authority_corpus (phase 21's own)         27 passed; 0 failed (24.38s)
f21_02_no_channel_canary                         3 passed; 0 failed
cargo fmt --all -- --check                     clean
cargo clippy (4 touched crates, --all-targets) no errors, no new warnings
```

The reverification's baseline was 2096 passed + 2 failed; the 2 were EMFILE under
load 146. They did not recur at load 14. I added 4 tests: 2098 → 2102.

## 4. How the canary fails if the property reverts to vacuous

The canary is **three instruments, none of which can pass by nothing asking**:

1. **The live differential.** Control 8 turns / narrowed 3. Remove the channel and
   both are 8. Verified red above. A suite that only asserted "the child could not
   exceed the parent" would pass either way — that indistinguishability is the
   defect this phase kept hitting.
2. **The seam differential** (`f21_02_a_requested_narrow_envelope_binds_the_child`
   + its unrequested control). The parent's root is 10 000 and the child spends
   500; the *only* thing that can stop it is the requested 100. Revert to `None`
   and the pair contradict each other.
3. **The inverted source canary.** The old canary asserted no production caller
   exists — green at base, and opposed to the capability. The new one asserts the
   channel exists, is reachable by a delegating actor (Delegate schema, all seven
   dimensions, `ForkOverrides.budget`, `ChildBudgetRequest`), and is resolved by
   intersection against `effective_budget()`. It reads only `crates/*/src` and
   asserts its own crawl collected >100 files, so a broken walk cannot make it
   vacuous.

## 5. Findings

**F-1 (HIGH, measurement — the phase corpus's budget canary is now blind).**
`child_authority_corpus/surfaces.rs:794 budget_no_channel_canary` greps for the
literal `sub_budget(Some(` while excluding `crates/wcore-budget/`. The production
caller now uses `sub_budget_narrowed(...)`, and the `sub_budget(Some(narrowed))`
it delegates to lives inside the excluded crate. **The canary therefore still
reports "NO-CHANNEL canary intact" although a live, LLM-reachable production
channel exists.** The corpus is green (27/27) and its budget rows will read
NO-CHANNEL, which is now false. I deliberately did **not** edit it: it encodes the
prior grading frame ("no channel = good"), and reversing that is a verifier
decision, not an executor's. A fourth grading must not read that row as current.

**F-2 (MEDIUM, product — `max_iterations` is unclamped).** See §1.
`delegate.rs:96` → `spawner.rs:2283`, no comparison against the requester's own
`max_turns`. Not one of F21-02's six dimensions and bounded by the ancestor rollup
wherever token/cost/time caps are configured, so per the brief's severity policy
this is BACKLOG, not blocking. It is the shape (B) was accused of creating, found
already shipped.

**F-3 (INFO).** `Spawn` and `spawn_host_child` cannot carry a budget request:
`Spawn` takes no `ForkOverrides` and `spawn_host_child` hardcodes
`ForkOverrides::default()`. Delegate is the only surface. This is the same
host-protocol expressiveness gap SC3 already records as fenced.

## 6. Honest verdict

**F21-02's vacuity is closed, and the phase goal is not thereby achieved.**

What is now true and proven live: a delegating actor can request a narrower
envelope, the request is resolved by intersection with the caps that bind it, the
narrowed envelope binds the child *engine* on the production durable-authority
path, and a child that tries to spend past it is refused — 3 turns against 8,
with the vacuous state measured red at 8.

What I am **not** claiming:

* The **widening** direction was already unamplifiable, and my test of it is
  largely theatre. §1 and §3.2 record the measurement that proves it.
* Only the **token** dimension is live-driven. Fan-out, time, cost and depth now
  have a request channel and the same intersection, but are exercised only
  in-process. The channel makes them *obtainable*; I did not obtain them.
* **Linux only.** No Windows evidence was produced and none is inherited.
* SC3 and F21-04 are untouched. This lane closes one requirement's vacuity, not
  the phase.
