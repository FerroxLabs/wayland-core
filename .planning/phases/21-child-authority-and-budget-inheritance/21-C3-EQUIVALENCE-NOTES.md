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

---

## Phase 1 — BASELINE MEASURED (hetzner-dsm, `/root/wayland-21c3eq`, SHA `27ca2d2a`)

`cargo test -p wcore-cli --test child_authority_corpus`, unproxied
(`/root/.cargo/bin/cargo`):

```
test result: ok. 29 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 24.19s
```

`WLRC=0` + `WLDONE` both present. Log: `/tmp/21c3eq-base.log`, table from
`/tmp/21c3eq-base-nocap.log`.

### P-5. The measured 11×4 table at HEAD, against the table in my brief

Extracted with
`grep -oE "^COMBINATION :: corpus_[a-z_]+ :: linux :: [a-z-]+ :: [a-z-]+ :: [A-Z-]+"`.

| Dimension | SA in-proc | HP in-proc | SA live | HP live | vs brief |
|---|---|---|---|---|---|
| tool | **NOT-EXPR** | NOT-EXPR | **REFUSED** | **NOT-EXPR** | **3 of 4 differ** |
| filesystem | REFUSED | REFUSED | REFUSED | REFUSED | held |
| egress | REFUSED | NOT-EXPR | REFUSED | REFUSED | held |
| secret | REFUSED | REFUSED | REFUSED | REFUSED | held |
| depth | REFUSED | REFUSED | **REFUSED** | **REFUSED** | **2 differ** (brief: NOT-EXPR live) |
| fan-out | REFUSED | NOT-EXPR | **REFUSED** | **REFUSED** | **2 differ** (brief: NOT-EXPR live) |
| time | REFUSED | REFUSED | NOT-EXPR | NOT-EXPR | held |
| token | REFUSED | REFUSED | NOT-EXPR | NOT-EXPR | held |
| cost | REFUSED | REFUSED | NOT-EXPR | NOT-EXPR | held |
| provider | NO-CHANNEL | **REFUSED** | NO-CHANNEL | NO-CHANNEL | **1 differs** |
| approval | NO-CHANNEL | REFUSED | NO-CHANNEL | NO-CHANNEL | held |

**Zero ALLOWED on any dimension, any surface, any mode — the brief's claim on this HELD.**

### P-6. Brief claim 4 (fan-out undetermined live on both surfaces) — **FALSE at HEAD**

Measured: `corpus_fan_out :: standalone :: live :: REFUSED` and
`corpus_fan_out :: host-protocol :: live :: REFUSED`. Lane `21-c3-hostile` closed this with an
at-cap control. Nothing for me to determine here — it is already determined. The remaining
fan-out gap is the **host-protocol IN-PROCESS** cell only.

### P-7. The real remaining gap is narrower than the brief, and it is TOOL

`is_decisive` (`surfaces.rs:135-137`) = `Refused | Allowed | NoChannel`. `NOT-EXPRESSIBLE` is
**not** decisive, and `assert_surface_equivalence` (`child_authority_corpus.rs:341-343`)
`continue`s when either side is non-decisive. So every NOT-EXPR cell makes the equivalence
assertion **vacuous for that pairing**.

Applying that to the measured table, the equivalence pairs that ACTUALLY RUN:

| Dimension | in-process pair | live pair |
|---|---|---|
| tool | **SKIPPED** (both NOT-EXPR) | **SKIPPED** (HP NOT-EXPR) |
| egress | SKIPPED (HP NOT-EXPR) | runs |
| fan-out | SKIPPED (HP NOT-EXPR) | runs |
| time / token / cost | runs | SKIPPED (both NOT-EXPR) |
| filesystem / secret / depth / provider / approval | runs | runs |

⇒ **`tool` is the ONLY dimension with zero running equivalence pairs in either mode.** That is
the sharpest statement of the unmet criterion, and it is not the statement the brief or the
ledger makes.

### P-8. Why each tool cell is non-decisive (verbatim causes, from the run)

- **HP in-process** — *"the host child-spawn request type carries the fields [name, prompt,
  max_turns, max_tokens, system_prompt, provider, model, temperature] and none of them expresses
  a tool-authority request; `spawn_host_child` hardcodes `ForkOverrides::default()`"*. ← the one
  cell my remit can fix by extending the request type.
- **HP live** — *"the delegated child's shell never ran"*.
- **SA in-process** — *"the KNOWN-POSITIVE arm failed: … could not write a sentinel inside its
  own workspace and read it back"* (the bwrap overlapping-deny defect, 21-C3-01).
- **SA live** — REFUSED, but *"ATTRIBUTED TO WORKSPACE CONTAINMENT, NOT TOOL AUTHORITY"*.

**Consequence for design.** Three of those four are blocked by reading an EFFECT (a file on
disk, a shell that ran). `f21_02_01_child_tool_authority.rs` already proves tool authority by
reading the child's **own registry off the wire** instead, and passes. A registry-reading probe
sidesteps 21-C3-01 (bwrap), 21-C3-03 (confirmer) and 21-C3-04 (unknown checkout root) at once.
That, not an effect probe, is the shape the tool differential needs.
