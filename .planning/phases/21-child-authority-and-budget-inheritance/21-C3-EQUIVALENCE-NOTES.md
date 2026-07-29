# 21-C3-EQUIVALENCE — running notes

**Lane** `21-c3-equivalence` · branch `lane/21-c3-equivalence` · base
`5be910561f688c75d39492e7b982d6e100772a64` on `plan/f20-unified-audit-repair`
(asserted against `/usr/bin/git ls-remote gh plan/f20-unified-audit-repair`).

Criterion: *"Standalone and host-protocol hostile corpora prove equivalent enforcement."*

---

## Phase 0 — brief-premise verification (in progress)

### P-1. The brief is STALE: lane `21-c3-hostile` already landed and moved fan-out

My brief says *"Fan-out is reported undetermined live on **both** surfaces (0 provider requests
by a delegated child)"*, sourced from `21-REVERIFICATION.md:264-284` at `ac94b1d5`/`873cc389`.

`21-C3-SUMMARY.md` is present in this tree, i.e. lane `21-c3-hostile` is merged into my base.
It records fan-out standalone-live and host-protocol-live both moving
**NOT-EXPRESSIBLE → REFUSED** behind an at-cap control, on Linux and (host-proto) Windows.
It simultaneously moved every **tool** cell **REFUSED → NOT-EXPRESSIBLE**.

⇒ Brief claim on fan-out: **provisionally FALSE at HEAD** (verify in code, not from the summary).
⇒ The table in my brief (11 dimensions) predates that lane and cannot be quoted as HEAD state.

### P-2. "the host child-spawn request type in `wcore-protocol`" — FALSE

`CRITERIA-GAP-LEDGER.md` (21-C3 row) says closing this needs
*"adding tool-authority and breadth fields to the host child-spawn request type in
`wcore-protocol` (a schema change ⇒ fenced seam, Desktop must re-pin in the same train)"*.

Measured at base:

- The 8-field type is `SubAgentConfig`, defined at
  `crates/wcore-types/src/spawner.rs:518-539` — in **`wcore-types`**, not `wcore-protocol`.
- Search run: `/usr/bin/grep -rn "SubAgentConfig\|ForkOverrides" crates/wcore-protocol/`
  → **0 hits**. Instrument alive: the same pattern over `crates/` returns 20+ files.
- `ProtocolCommand` (`crates/wcore-protocol/src/commands.rs:262-400`) has **no child-spawn
  variant at all** — no `SpawnChild`, no `Delegate`. The full variant list is Message, Stop,
  ToolApprove, ToolDeny, InitHistory, SetMode, SetConfig, ContinueWithBudget, SessionResync,
  ResumeTurn, ResolveInterruptedApproval, ResolveUnknownToolEffect, GetRuntimeDiagnostics,
  GoalOpen, GoalDeclareTask, GoalAdvance, GoalCancel, GoalResync, AddMcpServer, RemoveMcpServer,
  GrantWorkspaceCapability, ApprovalResume, HostSendMessageResult, Ping.
- `SubAgentConfig` derives no `Serialize`.

⇒ **Consequence for the orchestrator's contract instruction.** The desktop contract corpus pins
`COMMAND_SPECS.len() == 23` / `EVENT_SPECS.len() == 52`
(`crates/wcore-protocol/tests/desktop_contract_corpus.rs:217-218,318-321`). Those are driven by
`COMMAND_SPECS`/`EVENT_SPECS`, which enumerate wire commands and events. **Extending
`SubAgentConfig` touches neither**, so the prediction that the corpus "will go RED on the count
pins" is expected to be **wrong**, and I must NOT manufacture a protocol command merely to make
it come true. To be re-confirmed by running the corpus after the change.

### P-3. "`spawn_host_child` hardcodes `ForkOverrides::default()`" — NOT literal; trace pending

`crates/wcore-agent/src/spawner.rs:1110-1113`:

```rust
pub async fn spawn_host_child(&self, sub_config: SubAgentConfig) -> SubAgentResult {
    self.spawn_one_with_origin(sub_config, ChildOrigin::Host).await
}
```

No `ForkOverrides` literal at that site. The hardcode, if it exists, is downstream in
`spawn_one_with_origin`. **Open — trace next.** The corpus asserts the claim at
`child_authority_corpus/surfaces.rs:1966,2013`.

### P-4. What "host-protocol expression" means here

The corpus's host surface (`HostProtocolInProcess`, `surfaces.rs:1972`) is not a wire command —
it is the production `AgentBootstrap` object graph plus `HostChildController::spawn_child` →
`spawn_host_child`. So "expressible on the host surface" == "fillable in `SubAgentConfig` at the
`spawn_host_child` seam". That is where the fix belongs, and it is a `wcore-types` +
`wcore-agent` change, not a wire-schema change.

`ForkOverrides` (`wcore-types/src/spawner.rs:584-598`) already carries `allowed_tools` (tool
authority) and `budget: Option<ChildBudgetRequest>` (breadth/limits) — the standalone surface's
request vocabulary. The asymmetry is that `SubAgentConfig` cannot carry it.

---

## Still to establish

- [ ] Trace `spawn_one_with_origin` — is `ForkOverrides::default()` really hardcoded there?
- [ ] Re-run `child_authority_corpus` at base on hetzner; read the actual 11×4 table back from
      the product, not from any summary.
- [ ] Confirm fan-out's current live verdicts in code.
- [ ] Design the additive `SubAgentConfig` extension; confirm no wire shape moves.
- [ ] Build a genuine *differential* gate (same corpus entry, both surfaces, compare verdicts) —
      per brief, a single-surface policy-string assertion proves nothing.
- [ ] Known-negative with three assertions per §6b-ii.
- [ ] Windows: state plainly if unmeasured.
