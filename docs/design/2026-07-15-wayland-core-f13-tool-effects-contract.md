# Wayland Core F13 Tool-Effect Contract

**Status:** adversarially amended implementation contract; verification pending

**Baseline:** `d0aa0abc75afe056cc5434fcd652efa6d474ab0c`

**Coordination:** FerroxLabs/wayland#889

## 1. Safety invariant

Wayland Core must never repeat an externally visible operation after a crash,
timeout, cancellation, panic, transport loss, or host restart unless durable
evidence proves one of the following:

1. the operation did not start;
2. the operation is repeat-safe under the same stable idempotency key;
3. a reconciler authoritatively proves that the intended effect is already
   committed; or
4. a deterministic filesystem transaction can be completed or restored with
   a compare-and-swap guard.

All other started operations become `unknown`. An unresolved `unknown` effect
blocks automatic continuation and requires a durable reconciliation or
operator decision. A display-level error, timeout, cancelled future, dropped
task, or process exit is not evidence that no external effect occurred.

## 2. Durable state machine

Tool effects use a tool-specific state machine. Provider, child, and delivery
states retain the F12 contract until their owning milestones change them.

| State | Meaning | Automatic dispatch allowed |
|---|---|---:|
| `prepared` | Intent, effective input, contract, and idempotency key are durable; physical execution has not been authorized | no |
| `running` | The start record is durable and the physical boundary may be crossed | only the original dispatch |
| `succeeded` | Authoritative success and result receipt are durable | no |
| `failed` | Authoritative terminal failure is durable | only as a new linked attempt when policy permits |
| `not_started` | A typed gate denied dispatch before the physical boundary | yes, as a new attempt |
| `unknown` | The physical outcome cannot be proven | no |

Allowed transitions:

- `prepared -> running`
- `prepared -> not_started`
- `running -> succeeded`
- `running -> failed` only with authoritative terminal evidence
- `running -> unknown`
- `unknown -> succeeded|failed|not_started` only through a durable validated
  reconciliation/operator-resolution event

There is no `unknown -> running` transition. A permitted retry is a new linked
attempt which reuses the original provider-enforced idempotency key and retains
the old attempt as evidence.

## 3. Write-ahead boundaries

The production boundary is
`AgentNodeExecutor -> dispatch_once -> execute_single_with_streaming`.

1. Policy, approval, pre-hook input rewriting, availability, circuit-breaker,
   and budget admission complete before `prepared`.
2. `prepared` is fsynced with requested/effective input digests, effect
   contract, provider call identity, turn ordinal, and stable idempotency key.
3. Any pre-dispatch failure is durably `not_started`; it never fabricates a
   physical start.
4. `running` is fsynced immediately before the tool future is first polled or a
   process/network request can be spawned.
5. A normal authoritative result is terminalized before it is exposed as a
   committed tool result.
6. Timeout, panic, dropped future, lost transport, or non-authoritative error
   is durably `unknown` before control returns to the engine.
7. On recovery, `running` is first converted to `unknown`; unresolved unknown
   effects are surfaced and automatic continuation is refused.

Post-tool hooks are not part of the tool's terminal receipt. A side-effecting
post-hook needs its own journaled effect boundary; it cannot make the tool
receipt retroactively uncertain.

## 4. Stable identity

The v1 idempotency key is a domain-separated SHA-256 digest over the exact
session identity, turn identity, provider tool-call identity, ordinal, tool
identity, and effective-input digest. The same logical recovery attempt must
reuse the same key; a restart must not mint a random replacement.

The journal execution ID may be derived from this key. Duplicate preparation
of the same key is rejected or returns the existing attempt; it never creates a
second physical authority.

## 5. Effect contracts

The neutral tool layer exposes a defaulted, versioned effect contract:

| Contract | Recovery rule |
|---|---|
| read-only/repeat-safe | may replay only with the same key after proving no hidden mutation surface |
| deterministic filesystem transaction | reconcile pre/post identities and digests, then complete or restore using CAS |
| provider idempotent | forward the stable key and query/accept the provider's authoritative receipt |
| opaque | operator resolution required after any uncertain start |

The default is `opaque`. Tool name, UI category, or a successful-looking text
message is never sufficient classification. MCP, plugins, remote executors,
Script, and delegated dispatch remain opaque until their adapter declares and
proves a stronger contract.

Version 1 has no generic adapter-defined reconciler contract and advertises no
automatic filesystem reconciler for ordinary host paths. Provider-idempotent
adapters may propagate a stable key but still require authoritative provider
evidence before terminalization. Adding a generic status probe requires a
separately versioned contract, validated receipt shape, and startup dispatcher
before it may be advertised.

## 6. Ordinary filesystem effects

Write and Edit remain functional but are deliberately `opaque` on every
platform. Their normal successful result is journaled, while timeout,
cancellation after start, panic, host loss, or missing terminal evidence becomes
`unknown` and is never automatically replayed.

The adversarial audit rejected the earlier filesystem-transactional claim.
Linux `RENAME_NOREPLACE` and macOS `RENAME_EXCL` authoritatively protect only
an absent destination. Neither platform exposes a general operation meaning
"replace this pathname only if it still refers to object/content X" against a
non-cooperating writer. Advisory locks do not constrain ordinary editors, and
exchange-then-rollback publishes speculative bytes and has crash and second-
writer windows. Matching postimage bytes also cannot prove that Wayland, rather
than an external writer, created the current object.

Consequently, production `RealFs` does not advertise authoritative
compare-exchange for ordinary host files. The in-memory compare-exchange model
is retained only for deterministic fixture backends; it is not release evidence
for a host filesystem. Write/Edit do not emit checkpoint or reconciliation
receipts, do not claim metadata/ACL/xattr preservation, and do not treat
matching bytes as authoritative success after recovery.

A future filesystem transaction must use either a cooperative single-writer
workspace protocol or revisioned storage whose commit pointer is exclusively
owned by Wayland. A create-only surface may use no-replace primitives after it
also pins containment, parent identity, prepared object identity, metadata, and
crash recovery. Replacement, delete, symlink/reparse, hard-link, parent-swap,
and metadata preservation remain unadvertised until native proof exists.

The current `FileHistory`/`RollbackTool` implementation is not constructed or
registered by a production path, and no authoritative Write/Edit completion
path records its postimage guard. Its durable guard and compare-and-swap tests
therefore prove the isolated rollback primitive only, not end-to-end Rollback;
until separate wiring lands, production rollback is unavailable and fails
closed rather than fabricating authority.

Native filesystem transaction, containment, object-identity, metadata, and ACL
proof remains an F28 gate. Until then, Windows, macOS, and Linux use the same
honest opaque Write/Edit recovery contract rather than platform-dependent false
authority.

## 7. External and nested effects

- Bash/process containment proves process ownership and termination, not the
  outcome of commands already executed. Timeout/cancel/panic is `unknown`
  unless a supervisor receipt proves the process never spawned.
- MCP/plugin/remote adapters receive the stable key. Without a declared
  idempotency or reconciliation protocol, a lost response is `unknown`.
- Script substeps and delegated child tools must traverse the same dispatcher
  or the entire parent operation remains opaque. Top-level journaling may not
  claim exactly-once behavior for hidden nested mutations.
- Read-only declarations require explicit adapter ownership and tests. Plugin
  metadata such as `Info` is not trusted as an effect contract.

Native plugin invocations receive a versioned optional capability containing
the durable `tool_execution_id` and stable idempotency key. MCP `tools/call`
forwards the same identity, including through the subprocess MCP bridge, only
through namespaced
`_meta["wayland/durable-effect"]`; it never mutates tool arguments. Direct and
legacy calls omit both identities rather than fabricating one. Native
subprocess SDK plugins receive the identity through the versioned
`call_tool_v2` verb only after advertising the reserved Init capability
`wayland.subprocess.call_tool_v2`; the legacy `call_tool` wire shape is
unchanged. Plugins that do not advertise the extension remain functional
through the opaque legacy call and receive no fabricated identity. A transport
loss restarts the worker only for later calls and never replays the ambiguous
invocation. The fixed WASM
WIT request remains unchanged because adding record fields would break the
existing component ABI: the host-side invocation retains the durable identity,
but a WASM guest cannot consume it until a separately versioned WIT surface is
introduced. All of these adapters remain `opaque` from propagation alone.

## 8. Operator resolution

An operator resolution is a durable journal event containing the operator
decision, evidence, timestamp/source, and the original unknown effect ID. It
may confirm succeeded, confirm authoritative failure/not-applied, or abandon
automatic continuation. Resolution cannot delete or rewrite the original
unknown evidence, and it cannot silently authorize a new random-key retry.

Host/TUI presentation may be completed with F14 resynchronization, but F13
must expose a typed engine/journal API and block new automatic work until the
unknown is durably resolved.

## 9. Mandatory proof

- reducer property tests for every allowed and forbidden transition;
- deterministic-key stability, collision-domain, and restart tests;
- live dispatcher crash cuts before/after `prepared`, before/after `running`,
  after physical effect, before terminal append, and after terminal append;
- prepared crash performs zero physical calls;
- running/unknown crash never performs a second opaque call;
- policy/approval/budget/circuit denials never enter `running`;
- timeout, panic, cancellation, MCP loss, plugin loss, and opaque Script become
  `unknown`;
- ordinary Write/Edit remain functional under the opaque contract and an
  interrupted started call is never automatically repeated;
- production host files refuse the experimental compare-exchange authority;
- same-key propagation and duplicate replay against a scripted remote fixture;
- exact-source Linux nextest and strict clippy through the Hetzner harness;
- native macOS and Windows receipts remain an explicit later F28 gate unless
  obtained during F13.

## 10. Release boundary

F13 advances the private session journal to schema version 4. The legacy
`ToolIntentRecorded` event remains replayable and is reduced conservatively as
an opaque effect; new writers emit `ToolIntentRecordedV2`. The public Rust
`SessionEvent` and `ToolNotStartedReason` enums are now non-exhaustive, so
downstream source matches must include a wildcard arm. This is an intentional
journal-model API revision, not a silent compatible change. In contrast, F13
does not add required fields to the public `ToolContext` or
`AgentExecutorConfig`; durable authority uses defaulted trait entry points and
private adapter state.

F13 is not complete from journal types or unit tests alone. Completion requires
live dispatcher wiring, recovery refusal, honest opaque filesystem/external
defaults, exact-source runtime proof, independent security review, and a
public-safe receipt on issue #889. No push, merge, release, or issue closure is
authorized by this contract.
