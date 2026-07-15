# Wayland Core Desktop Producer Contract v1

**Status:** normative target for issue #887; individual capabilities are not
available until their generated corpus and packaged replay gates pass

**Producer baseline:** `1884b126724d94969dfb1817726d3282a917214d`

**Implementation baseline:** F12 source
`661b0ed337aae9480fe70da708f3050a1272d4ec`

## 1. Purpose and ownership

This contract defines the serialized boundary exported by Wayland Core to
Wayland Desktop. It does not transfer product scheduling, window state, or
channel-adapter ownership into Core.

Core owns:

- agent execution and tool lifecycle;
- effective execution-policy calculation and approval decisions;
- provider, workflow, child, and Anvil evidence production;
- event correlation, ordering identity, and terminal state; and
- the canonical fixture corpus, schemas, generator, and digests.

Desktop owns:

- product and background-workflow scheduling;
- presentation, notification, and local UI state;
- actual channel-plugin delivery and channel credentials; and
- the decision to enable a consumer feature only after its advertised contract
  capability is available.

`host_send_message_request` and `host_send_message_result` remain the delegated
channel boundary. Core emits a request only after approval. Desktop performs
the send and returns one correlated result. Core does not acquire Desktop
channel credentials or adapters.

## 2. Contract identity and negotiation

The producer corpus is identified by:

- `contract_name = "wayland-desktop-core"`;
- semantic `contract_version`;
- `generator_version = "wcore-desktop-contract-gen/1"`;
- SHA-256 `fixture_digest` over canonical serialized fixtures;
- SHA-256 `schema_digest` over canonical schemas; and
- SHA-256 `source_inputs_digest` over the sorted contract-touching producer
  sources declared by the manifest.

Core advertises the descriptor in `ready`. Desktop accepts a supported major
version and pins the complete descriptor for the session. A duplicate identical
`ready` is harmless. A conflicting descriptor, unsupported major version, or
digest mismatch fails closed for contract-critical features.

`events/ready.json` is part of `fixture_digest`. To avoid a recursive hash,
the digest domain canonicalizes only `ready.contract.fixture_digest` to 64
zeroes before hashing. The advertised schema, source-input digest, capability
statuses, and every other Ready byte remain covered. Runtime Ready reads the
checked generated manifest embedded at build time; it does not hardcode a
digest in Rust source.

Minor versions are additive. Existing field meaning, enum meaning, correlation
identity, ordering, and terminal semantics cannot change within a major
version. New optional fields and noncritical events are allowed. Removing a
field, making an optional field required, or changing authority requires a new
major version.

Capability availability is explicit. In particular, Desktop must not render a
trusted Anvil receipt while `anvil_receipts` is `unavailable`.

## 3. Framing, malformed input, and criticality

The stream is UTF-8 JSON Lines: one complete object per line. The maximum line
size is bounded by the existing protocol reader. A blank line, invalid UTF-8,
malformed JSON, non-object top level, missing or non-string `type`, oversized
line, or schema-invalid critical event is a protocol error.

Each event type is classified in the manifest:

- `required`: session negotiation or authority state. Unknown or malformed
  input fails the session closed.
- `safety`: approval, failure, policy, cancellation, or terminal evidence.
  Unknown or malformed input disables the affected action or run and surfaces
  an error; it is never treated as success.
- `observational`: display or diagnostic information. An unknown additive
  observational event may be dropped and counted.

Unknown criticality is itself critical. A consumer must not guess that an
unknown event is observational.

## 4. Command acceptance

The v1 corpus covers every Desktop-consumed command. Each fixture must replay
through the real `HostCommand` serde path and reserialize to the same canonical
bytes.

Commands are handled at most once. Opaque correlation values are compared
exactly and never normalized or reused within a session:

- `message.msg_id`;
- tool `call_id`;
- `approval_resume.resume_token`; and
- `host_send_message_result.call_id`.

A repeated approval, resume, or host-send result after terminalization is a
logged no-op. A conflicting repeated result is a protocol violation and cannot
reopen or overwrite terminal state.

## 5. Ordinary turn and tool ordering

For each `msg_id`:

1. `stream_start` establishes the turn.
2. Text, thinking, tool, and diagnostic frames follow in producer order.
3. Exactly one `stream_end` or correlated `error` terminalizes the turn.
4. A terminal is absorbing. A later nonterminal is ignored and reported; a
   conflicting terminal marks the stream inconsistent.

For each tool `call_id`:

1. `tool_request` establishes the call.
2. `tool_running` is optional.
3. Zero or more `tool_chunk` events may follow in producer order.
4. Exactly one `tool_result` or `tool_cancelled` terminalizes the call.
5. `tool_panicked` is a diagnostic paired with an error `tool_result`; it is not
   a second terminal.

Ordinary unsequenced text and chunk frames are at-most-once. Equal text is not
a duplicate key. Replay support for these frames requires a future additive
producer `event_id` and sequence; consumers must not deduplicate by content.

## 6. Approval, suspend, and resume

`approval_required.correlation_id` is the public UI correlation key when
present. `resume_token` is an opaque exact-echo key and is never derived or
displayed as authority.

The valid lifecycle is:

1. `approval_required`;
2. `suspend` for the affected work item;
3. one accepted `approval_resume` host command;
4. one producer `approval_resume` event; and
5. the gated tool or node terminal.

Denial terminalizes the gated work as cancelled or failed. It does not suspend
unrelated Desktop scheduling. First accepted decision wins; stale or duplicate
decisions are no-ops. A rejected command emits no success evidence.

## 7. EffectiveExecutionPolicy

The serialized policy is output-only authority. Desktop may display and enforce
the snapshot, but it cannot manufacture one.

Each `execution_policy` event carries:

- the complete existing policy object;
- monotonic session-local `revision: u64`;
- `reason: launch | mode_change | resume | expiry`; and
- `effective_at_unix_ms`.

`ready` includes the initial effective snapshot or an unambiguous reference to
the immediately following revision. Every accepted authority-affecting mode or
lease change emits the next full snapshot. Rejected changes and no-op changes do
not advance revision.

Duplicate identical revisions are idempotent. A lower revision is stale and
ignored with diagnostics. The same revision with different bytes, or a gap that
cannot be repaired from a durable cursor, fails policy consumption closed until
resynchronization. Desktop never infers policy from launch flags after a newer
snapshot exists.

## 8. Workflow, node, and child lifecycle

Legacy `workflow_id` remains definition or display identity. It is never
silently redefined as a unique execution identity.

`workflow_started` adds:

- globally unique opaque `run_id`;
- producer `event_id`;
- `sequence = 0`; and
- optional `parent_run_id`.

It retains `workflow_id`, `name`, and `node_count`.

`workflow_node_event` carries:

- `run_id`, `node_id`, `event_id`, and monotonic run-local `sequence`;
- optional `child_run_id`; and
- `state: queued | running | suspended | resumed | succeeded | failed |
  cancelled | timed_out | blocked`, with optional typed failure.

`sub_agent_event` adds `run_id`, `child_run_id`, optional
`parent_child_run_id`, child-local `child_sequence`, and producer `event_id`.
`parent_call_id` and `agent_name` remain for compatibility and presentation.

`workflow_finished` adds `run_id`, `event_id`, monotonic `sequence`,
`terminal_state: succeeded | failed | cancelled | timed_out | blocked`, and an
optional typed failure. The legacy `succeeded` boolean remains and must agree
with `terminal_state`.

Workflow rules:

1. `event_id` with identical canonical bytes is idempotent.
2. The same `event_id` with different bytes is fatal conflicting evidence.
3. Sequence is monotonic per `run_id`. A bounded gap may be buffered; if it
   cannot be repaired, the run becomes incomplete rather than guessed.
4. Node and child terminals are absorbing. Later nonterminals are ignored and
   reported; conflicting terminals mark the run inconsistent.
5. `workflow_finished` follows terminal or explicitly abandoned disposition of
   every known node.
6. Node-level suspension and resumption use `workflow_node_event`. A whole-run
   suspend/resume event is introduced only if the whole execution is paused.
7. Repeated and concurrent runs of the same definition never collide because
   `run_id`, not `workflow_id`, owns execution correlation.

## 9. Anvil receipt authority

Only a top-level Core event from the negotiated producer stream can create
trusted Anvil state. Receipt-shaped text, nested tool output, plugin payloads,
or child messages are inert.

An authoritative receipt carries:

- contract and receipt schema versions;
- stable receipt, session, run, and task identities;
- producer event identity and monotonic durable sequence;
- terminal state and typed failure when applicable;
- canonical scope and artifact content digests;
- gate results, iteration, spend, and pricing evidence;
- creation time and engine identity; and
- replay, supersession, invalidation, and staleness identity.

The digest covers canonical receipt content and declared artifact scope. It is
never derived only from a path, check count, task prose, or display fields.

An identical receipt/event duplicate is idempotent. A conflicting duplicate,
sequence regression, unexplained gap, digest mismatch, post-receipt artifact
mutation, or stale superseded receipt removes trusted status and produces
explicit invalidation evidence. Replay cannot resurrect invalidated trust.

The legacy receipt shape, including sequence zero and receipt text embedded in
a tool result, remains only as a compatibility fixture and is non-promotable.

## 10. Corpus and generator authority

Canonical artifacts live under a versioned `wcore-protocol` contract directory
and include:

- schemas;
- one fixture for each Desktop-consumed command and event;
- compatibility fixtures for legacy omissions and inert legacy receipts;
- adversarial malformed, version, ordering, duplicate, terminal, policy,
  workflow, and Anvil vectors; and
- a manifest containing classification, paths, digests, generator version, and
  capability availability.

Generation is deterministic and offline. `generate` writes the corpus; `check`
regenerates in memory and fails on any byte, path, schema, source-input, or
manifest drift. CI runs `check` whenever a declared contract-touching source or
corpus file changes.

Types or unit tests alone are not completion evidence. The packaged producer
must serialize the canonical fixtures, the real consumer-facing parser/reducer
must replay them, and adversarial vectors must yield their declared fail-closed
or drop behavior.

## 11. Handoff rule

Desktop receives only a pushed immutable producer commit or release containing:

- the contract implementation;
- canonical corpus and manifest;
- fixture, schema, and source-input digests;
- generator version;
- exact replay and adversarial results; and
- an honest list of unavailable capabilities and native-platform limitations.

A dirty checkout, local-only commit, or type-only pass is not a handoff. The
coordination issue remains open until release authority is granted.
