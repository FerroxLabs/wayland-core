# SEAM REQUEST — per-child attribution on the host protocol (F21-04-01)

**Status:** OPEN — fenced. Requires Core+Desktop release coordination.
**Raised by:** Phase 21, plan 21-04. HIGH, still open after 21-04's two permitted harness iterations.
**Author:** core lane, 2026-07-27. Evidence executable; see §5.
**Fixture regeneration is NOT performed by this request.** `wcore-contract generate` is a
release-coordination action. This document specifies the change; it does not make it.

---

## 1. The gap, stated exactly

A Wayland session may run several `Spawn` siblings at once. The host protocol cannot tell
which one it is looking at. Four of the six events that carry per-child meaning have no actor
field, and the one command that acts on a child acts on the whole turn instead.

Verified against `crates/wcore-protocol/src/` at `de977949`:

| Surface | Location | Today | Gap |
|---|---|---|---|
| `ProtocolEvent::ApprovalRequired` | `events.rs:969` | `call_id`, `resume_token`, `correlation_id`, `reason`, `context`, `plan` | **no field naming the sibling.** Two siblings suspended at once and the host cannot tell whose gate it is answering. |
| `ProtocolEvent::BudgetExceeded` | `events.rs:1009` | `reason`, `observed`, `limit` | no actor — a sibling's cap trip is indistinguishable from the parent's |
| `ProtocolEvent::BudgetGrantResult` | `events.rs:1017` | `#[serde(flatten)] result` | no actor; see the flatten caveat in §3 |
| `ProtocolCommand::Stop` | `commands.rs:142` | unit variant | whole-turn only. No way to stop one sibling. |
| `ChannelSink` relay | — | text, thinking, stream lifecycle, error, info | `emit_tool_call` / `emit_tool_result` are deliberate no-ops, so child tool activity never reaches a channel host at all |

This is the exact shape of 21-04's own threat **T-21-04-02**. It is an *observability* gap —
no misattribution has been demonstrated in the product, and this request should not be written
up as one.

## 2. What is being asked for

**No new type crosses a crate boundary.** `ChildId` already exists
(`wcore-types/src/spawner.rs:24`, a validated newtype over `String`) and is **already
re-exported into `wcore-protocol`** via `child.rs:7`. That crate exists precisely so the
protocol does not grow a second child model. This request consumes what is already there.

Additive on four surfaces:

```rust
// events.rs — ApprovalRequired, BudgetExceeded, BudgetGrantResult
#[serde(default, skip_serializing_if = "Option::is_none")]
child: Option<ChildId>,

// commands.rs — Stop becomes a struct variant with a defaulted field
Stop {
    #[serde(default)]
    child: Option<ChildId>,
},
```

`None` means "the session/parent" and is the current meaning of every one of these events, so
existing single-agent traffic keeps its exact wire shape.

This follows a precedent already set in this file, not a new convention: `ToolApprove.answer`
was added additively in v0.9.3 (`commands.rs:147-153`) with the same reasoning, and
`ApprovalRequired.correlation_id` / `.plan` both already use `skip_serializing_if`.

`ChannelSink`'s no-op `emit_tool_call` / `emit_tool_result` is a **separate, larger** question
(it is a relay-policy decision, not a field addition) and is deliberately NOT bundled here.

## 3. Compatibility — measured, not assumed

Both enums are internally tagged (`#[serde(tag = "type")]`, `rename_all = "snake_case"` —
`commands.rs:132`, `events.rs:556`). `ProtocolCommand` derives **only** `Deserialize`;
`ProtocolEvent` derives **only** `Serialize`. Direction therefore matters and is not symmetric.

Probe run on hetzner, 2026-07-27 (source in §5):

```
legacy-stop-on-new-shape  = OK -> Stop { child: None }
scoped-stop-on-new-shape  = OK -> Stop { child: Some("c-1") }
old-decoder-on-scoped     = ACCEPTS, ignores unknown field -> Stop
```

So converting `Stop` from a unit variant to a struct variant with one defaulted field is
**wire-compatible for decoding**: `{"type":"stop"}` still decodes. That is the non-obvious
result and it is why this can be additive rather than a new `StopChild` command.

**Two hazards fall out of the same probe, and both must be in the release note:**

- **Version skew, host newer than Core (line 3 of the probe).** An old Core decoder *accepts*
  `{"type":"stop","child":"c-1"}` and silently ignores `child` — so a scoped stop from an
  upgraded host **stops the entire turn** on an un-upgraded Core. Silent over-broad kill, no
  error. This must be gated on a capability flag the host checks before sending a scoped stop,
  in the manner of `capabilities.hitl_suspend`.
- **`BudgetGrantResult` uses `#[serde(flatten)]` over a type with a hand-written `Serialize`
  impl** (`events.rs:1018`, impl at `events.rs:394`). Adding a sibling field next to a flatten
  is only safe if the inner map emits no `child` key. Verify at implementation time; do not
  assume. If it collides, put the actor on the inner struct instead.

**Unknown-field tolerance is not the same rule as unknown-event tolerance.** The D1 producer
contract pins `critical` as `const true`, requiring hosts to **fail closed** on an unknown
additive *event* (see `.planning/intel/D1-CORE-PRODUCER-CONTRACT.md`, revision 2 — docs
previously drifted to "drop" and that was a security-relevant HIGH). These are unknown
*fields on known events*, which a host ignores and degrades to current behaviour. Nothing here
changes the fail-closed rule for events, and the release note should say so explicitly so the
next reader does not re-derive the wrong precedent from this change.

## 4. Why this is fenced rather than just done

Every one of these edits moves `fixture_digest` and `schema_digest` in the generated contract
manifest (`contract/generate.rs:1201-1203`), which is a coordinated Core↔Desktop release, and
the capability flag in §3 needs a Desktop-side consumer to be worth anything. A Core-only
landing would produce a field no host reads plus a scoped-stop command that is *actively
dangerous* against an un-upgraded Core.

**Recommended disposition:** `minor` bump, both sides in one release, capability-gated.

## 5. Reproducing the compatibility evidence

Self-contained, no repo dependency — two-crate `serde`/`serde_json` probe modelling the exact
before/after variant shapes and both skew directions:
`.planning/intel/evidence/F21-04-01-serde-probe/`.

```
cd .planning/intel/evidence/F21-04-01-serde-probe && cargo run -q
```

Expected final line: `PROBE_DONE`. The probe **asserts** cases 1-3 (it fails loudly rather
than printing a wrong answer) and *reports* case 4, because case 4's outcome is the finding
rather than a requirement.

## 6. What this request does NOT claim

- No misattribution bug is demonstrated in the shipped product. This is an observability gap.
- `ChannelSink`'s no-op tool relay is named but not solved here.
- No fixtures were regenerated and no contract digest was recomputed.
