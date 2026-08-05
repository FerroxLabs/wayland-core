# UAT-DESKTOP-CONTRACT — running notes

Lane `uat-desktop-contract`. Worktree
`/Users/seandonahoe/dev/waylandcore-frontier-worktrees/lane-uat-desktop-contract`,
branch `lane/uat-desktop-contract`, base integration `e9bed1af`.

Append-only. Re-committed after every measurement (LANE-BRIEF §6b-i).

---

## T+0 — premise verification

Brief claim: `events/ready.json` `contract` block declares major 1, minor 10,
generator `wcore-desktop-contract-gen/11`.

**HELD.** Read from
`crates/wcore-protocol/contracts/desktop/v1/events/ready.json` (captured to
`/tmp/uat-dc-ready.json`, read with the Read tool, not through Bash):

```
"contract":{... "generator":"wcore-desktop-contract-gen/11","major":1,"minor":10,
  "name":"wayland-desktop-core",
  "fixture_digest":"sha256:eb3f7207...c123e2",
  "schema_digest":"sha256:217c15c1...b23a1c4",
  "source_inputs_digest":"sha256:da3aa114...b5d413c"}
```

## T+0 — structural fact that reframes the whole lane

Desktop's `app/src/process/agent/wcore/protocol.ts` is **355 lines of pure
TypeScript type declarations**. There is no `zod`, no `io-ts`, no runtime
validator, no schema check. TypeScript types are **erased at compile time**.

Consequence: "does Desktop's decoder accept what Core emits" is the wrong
question — at the JSON.parse level Desktop accepts *literally anything that is
valid JSON*. The decoder cannot reject. The real question is the one with
teeth: **which event types does Desktop actually act on, and which does it
silently drop?**

So the deliverable is a *handled-set* differential, not a validation pass.

## T+0 — first counts (UNVERIFIED — derived from FILENAMES, must re-derive from `type` fields)

- Corpus event fixtures: 52 (`events/*.json`)
- Corpus command fixtures: 23 (`commands/*.json`)
- Desktop `case '...'` arms in `agent/wcore/index.ts`: 37 total, of which 33 are
  in the event switch (lines 674–1135) and 4 (`edit`/`exec`/`mcp`/`info`, lines
  1194–1216) belong to a *different* switch on tool category.

Filename-derived candidate gap (19 event types in corpus, absent from Desktop
switch): anvil_receipt, anvil_receipt_invalidated, budget_grant_result,
execution_policy, goal_control_refused, goal_snapshot, goal_transition,
mcp_removal_result, provider_failover_receipt, runtime_diagnostics_snapshot,
runtime_diagnostics_unavailable, session_recovery_replay,
session_recovery_snapshot, session_recovery_unavailable,
turn_recovery_lifecycle, unknown_tool_effect_resolved, workflow_finished,
workflow_node_event, workflow_started.

**Not yet trustworthy.** A fixture filename is not the `type` field. Must
re-derive from the JSON itself, and must grep the *whole* Desktop app for each
name before claiming absence (LANE-BRIEF §3b-i: an absence is the easiest claim
to pass without doing work).

## T+0 — already-visible field-level divergence in `ready`

`ready.json` carries, at top level, `contract` and `execution_policy` objects.
Desktop's `WCoreEvent` `ready` variant declares only
`{type, version, session_id?, capabilities}` — **neither `contract` nor
`execution_policy` is in the type**.

Inside `capabilities`, the fixture carries `memory_enabled`, `online_evolution`
and `user_model_backend`; Desktop's `WCoreCapabilities` declares none of those
three.

## T+1 — derivation complete, both directions (VERIFIED)

Filename == `type` field for **all 75 fixtures** (52 events, 23 commands) —
checked by parsing content, so the earlier filename shortcut is now retired
rather than merely lucky.

Desktop `index.ts` has **two** switches, correctly separated:
`switch#1 on event.type` = 33 arms (L674–L1135); `switch#2 on tool.category` =
4 arms (`edit`/`exec`/`mcp`/`info`, L1194–L1216). Only switch#1 is the decoder.
`default:` arms at L1152 and L1217.

Authoritative Core side read from Rust, not the corpus:
`ProtocolEvent` (events.rs L558–1314) = **62 variants → 59 distinct wire tags**.
Three variants deliberately share a tag via `#[serde(rename)]`:
`CorrelatedSubAgentEvent`→`sub_agent_event`, `CorrelatedWorkflowStarted`→
`workflow_started`, `CorrelatedWorkflowFinished`→`workflow_finished`.
`ProtocolCommand` (commands.rs L262–403) = **24 tags**, no dupes.
(Extractor self-test: 3/3 — known-positive parses, known-negative enum yields
None, repaired version carries line numbers so a dupe is explainable.)

### Cardinalities

| set | n |
|---|---|
| Core distinct event wire tags | **59** |
| Corpus event fixtures | 52 |
| Desktop handled event types | **33** |
| Core command tags | 24 |
| Corpus command fixtures | 23 |
| Desktop declared command types | 11 |

### Differentials

- **A. Core emits, corpus does NOT cover — 7**: `capability_activation`,
  `compact_offload`, `mid_flight_monitor_decision`, `provider_attempt`,
  `provider_failure`, `provider_retry`, `workspace_policy`.
- **B. Corpus covers, Core does not emit — 0** (no dead fixtures).
- **C. Core emits, Desktop does NOT handle — 26** (44% of Core's surface).
- **D. Desktop handles, Core does not emit — 0** (no dead arms).
- **E. Corpus covers, Desktop does not handle — 19.**
- **F. Core accepts, corpus does not cover (command) — 1**:
  `grant_workspace_capability`.
- **G. Desktop sends, Core does NOT accept — 0.**
- **H. Core accepts, Desktop never sends — 13.**

Whole-tree absence sweep over **1866** Desktop `.ts/.tsx/.js` files (node_modules
and vendor excluded): all 19 set-E names return **0 files**. Instrument controls
in the same invocation: `browser_policy_denied` 3 files, `host_send_message_request`
4 files, sentinel `zzz_wayland_uat_sentinel_absent_zzz` 0 files. So the absence
is measured, not assumed — these types are not handled elsewhere in Desktop.

## T+2 — LIVE DRIVE of the real binary (this is the circularity break)

hetzner `/root/wayland` HEAD = `e9bed1af` = my lane base, exactly. Binary
`/root/wayland/target/release/wayland-core` reports **0.12.25**, matching the
`ready` fixture's `version`.

Driven for real: `wayland-core --json-stream --session-id abcdef123456
--provider anthropic --api-key <dummy> --model claude-sonnet-4-5`, fed 21
commands on stdin. **No real credential was used or needed** — a dummy key
satisfies the startup presence check and none of the 21 commands triggers an
LLM call, so nothing was spent and nothing sensitive exists in the capture.
`WLRC=0`, 38 stdout frames, 44 stderr lines.

Two false starts, both real product behaviour worth recording:
1. Without any key the engine refuses to start `--json-stream` at all —
   `ready` is never emitted.
2. `--session-id uat-dc-live-001` was rejected: *"must be 6-40 hex characters"*.
   Note the corpus `ready.json` ships `session_id: "session-desktop-001"`,
   which that validator would reject. Different direction (engine→host vs the
   CLI flag), so not a contract violation — but the corpus's own sample id is
   not one the engine would accept.

Capture retained at `.planning/evidence/uat-desktop-contract/live-stdout.jsonl`
(+ `live-commands.jsonl`).

### The result

38 frames, **10 distinct event types**. Control: live types absent from Core's
enum = **[]** (empty, as it must be — confirms the enum extraction is complete).

| | |
|---|---|
| live types Desktop does NOT handle | `budget_grant_result`, `capability_activation`, `execution_policy`, `workspace_policy` |
| live types NOT in the contract corpus | `capability_activation`, `workspace_policy` |
| **frames Desktop would drop** | **27 of 38 = 71%** |

Dropped: `capability_activation` ×24, `execution_policy` ×1,
`workspace_policy` ×1, `budget_grant_result` ×1.
Handled: `info` ×5, `pong` ×2, `ready`/`config_changed`/`error`/`approval_resume` ×1.

`capability_activation` alone is 24 of 38 frames — the most frequent event in an
ordinary session — and it is in **neither** Desktop's decoder **nor** the
corpus. `execution_policy` and `workspace_policy` are emitted immediately after
`ready`, i.e. in the first moments of *every* session.

### Field-level divergences observed on the live wire

- `error` frames carry **no `msg_id` key at all** (Rust:
  `#[serde(skip_serializing_if="Option::is_none")]`). Desktop's type declares
  `msg_id: string | null` — a *required* key. The declared type is wrong.
  Runtime is nonetheless safe: the handler uses
  `event.msg_id ?? this.activeMsgId ?? ''` (L793), and `??` absorbs `undefined`.
  So: type-declaration defect, not a runtime defect. Graded accordingly.
- `config_changed.capabilities` carries `user_model_backend` and
  `memory_enabled`; `ready.capabilities` additionally carries `online_evolution`.
  None of the three is in Desktop's `WCoreCapabilities`.
- `ready` carries top-level `contract` and `execution_policy` objects that
  Desktop's `ready` variant does not declare.
- `set_mode` with **both** `"force"` and `"yolo"` was **refused** — *"an
  auto-approving mode (auto_edit/force) requires a local-operator opt-in
  (launch with --force or WAYLAND_ALLOW_WIRE_FORCE=1)"*. The refusal arrives as
  an `info` frame, not an `error`. Desktop's declared union is
  `'default'|'auto_edit'|'yolo'`; `SessionMode` accepts `yolo` as an alias for
  `Force`, so the *parse* succeeds and the *authorization* then fails.
- `add_mcp_server` after session start → `error` *"only allowed before first
  Message"*. Desktop's type exposes no such ordering constraint.

## T+3 — COMPLETE

All legs closed. Full result in `.planning/UAT-DESKTOP-CONTRACT.md`.

- tsc gate against Desktop's real `protocol.ts`: built, proven both directions
  (pos rc=0; neg rc=2 with 4 distinct reasons). 30/38 live frames and 24/52
  corpus fixtures fail Desktop's declared types.
- Field extractor cross-validated 5/5 against tsc before its extra findings
  were trusted; found 16 undeclared fields across 5 handled event types.
- Adversarial: Core's reference observer rejects for the exact stated typed
  reason; falsified by a one-field mutation (17/17 → 16/1), then restored,
  tree clean. Desktop has no rejection path at all — the three poisoned
  `ready` fixtures are byte-indistinguishable from golden to its decoder.
- Doc vs code: §4.1 documents a `protocol_error` Core never emits (0/5,
  controls green); Desktop's own comments misstate the rule-3 clause they cite;
  44/59 events and 16/24 commands documented.
- Two instrument defects found and REPAIRED in-lane (enum extractor; a dead
  `grep` control whose spacing didn't match the file).

hetzner worktree `/root/wayland-uat-desktop-contract` and branch
`hz/uat-desktop-contract` removed; `/tmp` lane files cleaned; tree was clean
(0 porcelain) before removal. Nothing was written to
`/Users/seandonahoe/dev/wayland` (0 files modified).
4. Drive the real `wcore-cli` binary on hetzner, capture a real session stream,
   diff its event types against both the corpus and Desktop's handled set.
   This is the leg that breaks the corpus's self-referential circularity.
5. Adversarial fixtures: confirm rejection **for the stated reason**.
6. `docs/json-stream-protocol.md` vs code.
