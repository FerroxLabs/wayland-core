# D1 — Core Producer Contract (CORE HALF)

**Status: the CORE half of D1 is complete. D1 itself is NOT complete.**

`.planning/intel/DESKTOP-PROTOCOL-CHECKPOINT.md` defines D1 as two obligations:

- **(a) CORE** — publish a pinned, digest-bound Core producer contract. **This document
  discharges (a).**
- **(b) DESKTOP** — a linked Desktop plan plus a real consumer/reducer conformance suite
  replaying this pinned corpus through the actual Desktop reducer. **Not discharged. It
  cannot be discharged from this repository.** See [§9](#9-what-the-desktop-lane-must-still-supply).

The checkpoint is explicit that "Core cannot claim Desktop behavior from Core tests alone."
Nothing below claims Desktop behavior. Every Core-side receipt in [§8](#8-what-core-has-actually-proven)
is a Core-side receipt. D1 flips from "blocked" to "Desktop lane has everything it requires"
— not to "done".

---

## 1. Pinned commit

Everything in this document describes exactly one tree state.

| | |
|---|---|
| **Pinned SHA** | `b6936299d9c3a7d3110e9ba03c36e5debe965b85` |
| Commit | `chore(protocol): re-pin Desktop contract provenance digests` |
| Authored | 2026-07-25 20:00:17 +0700 |
| Repository | `FerroxLabs/wayland-core` |
| Branch observed | `plan/f20-unified-audit-repair` |
| Observation HEAD | `2cc1a285ffd3f3b0fb41b177bd9a1317654cb350` (2026-07-26) |

**Why the pin is `b6936299` and not the observation HEAD.** This checkout is shared by
concurrently running agents; HEAD moved during authoring. `b6936299` is the last commit that
touched *any* contract-relevant path — the 40 generator source inputs or the 156-file corpus.
All contract-relevant paths are byte-identical between `b6936299`, the observation HEAD, and
the working tree, verified by:

```bash
git diff --stat b6936299d9c3a7d3110e9ba03c36e5debe965b85 HEAD -- \
  crates/wcore-protocol/contracts <the 40 SOURCE_INPUTS paths>
# → no output (byte-identical)
git diff --stat b6936299d9c3a7d3110e9ba03c36e5debe965b85 -- <same paths>
# → no output (working tree also byte-identical)
```

The corpus commit (`b6936299`, 2026-07-25 20:00) is **later than** the last commit touching
any generator source input (`a1085597`, 2026-07-25 17:22). The corpus is therefore not stale
relative to its inputs, and [§3](#3-digests) proves it independently.

Desktop pins against `b6936299` **or** any later Core commit whose `ready.contract` descriptor
still reports the three digests in [§3](#3-digests). The digests, not the SHA, are the real
compatibility boundary — a Core commit that changes neither sources nor corpus is contract-identical.

---

## 2. Contract identity

Emitted by Core in the `ready` event as the `contract` object, and recorded in
`crates/wcore-protocol/contracts/desktop/v1/manifest.json`.

| Field | Value |
|---|---|
| `name` | `wayland-desktop-core` |
| `major` | `1` |
| `minor` | `8` |
| `generator` | `wcore-desktop-contract-gen/11` |
| Corpus root | `crates/wcore-protocol/contracts/desktop/v1/` |
| Corpus size | 156 files (all git-tracked) |
| Inventory | 18 commands, 49 events, 3 durable-child types, 151 fixtures |

Source of truth for these constants: `crates/wcore-protocol/src/contract/generate.rs`
lines 22–26 (`CONTRACT_NAME`, `CONTRACT_MAJOR`, `CONTRACT_MINOR`, `GENERATOR_VERSION`,
`CONTRACT_ROOT`).

### Sub-contract versions

From `manifest.json` → `subcontracts`. Each is versioned independently of the `1.8` corpus minor.

| Sub-contract | Version |
|---|---|
| `anvil_receipts` | 1.0 |
| `durable_child` | 1.0 |
| `execution_policy` | 1.0 |
| `operator_tool_effect_resolution` | 1.0 |
| `runtime_diagnostics` | 1.0 |
| `semantic_failover_receipts` | 1.0 |
| `turn_recovery` | 1.0 |
| `workflow_lifecycle` | 1.0 |

### Capability statuses

From `manifest.json` → `capabilities`, mirrored into `ready.contract.capabilities`. The host
observer requires an **exact** match on this whole map (`CapabilityStatusMismatch` otherwise).

| Capability | Status | Meaning |
|---|---|---|
| `contract_negotiation` | `available` | Live and proved by serialized replay |
| `durable_child_model_v1` | `available` | |
| `effective_execution_policy_revisions` | `available` | |
| `host_delegated_delivery` | `available` | |
| `operator_tool_effect_resolution_v1` | `available` | |
| `runtime_diagnostics_v1` | `available` | |
| `runtime_mcp_lifecycle_v1` | `available` | |
| `semantic_failover_receipts` | `available` | |
| `turn_recovery_v1` | `available` | |
| `workflow_lifecycle_v1` | `available` | |
| `anvil_receipts` | `publication_bound` | Producer binds the serialized verdict body and *immediate* post-publication artifact state only. Later filesystem mutation over the receipt lifetime is **not** watched. |
| `browser_events` | `shape_only` | Fixture shape is pinned; **no production emitter is proven** at this baseline |
| `cua_events` | `shape_only` | Same |
| `plugin_events` | `shape_only` | Same |

**`shape_only` is a load-bearing warning.** Desktop may pin the wire shape of
`browser_event`, `cua_event`, and `plugin_event`, but must not assume a production emitter
exists. Building UI that requires these to arrive is unsupported at v1.8.

---

## 3. Digests

**All three digests below were computed by this author from the actual files. None are copied
on trust from the manifest — each was recomputed independently and then compared.**

| Digest | Value |
|---|---|
| `fixture_digest` | `sha256:42f142abf6e534e0bcb33ef7e6d9ec00c53c57938fa467f46d643d9e80e451e4` |
| `schema_digest` | `sha256:e5d1744aa6cadc46d2707a1fa190ac80ee74f13477d685bb9146a71b3fff2e54` |
| `source_inputs_digest` | `sha256:d8b1a8b5645707225bdaafea617452fe1cf2f99556be53d7dcd35891d5e92f28` |

### 3.1 Digest algorithm

Defined in `crates/wcore-protocol/src/contract/canonical.rs::digest_named_bytes`:

> SHA-256 over entries sorted by relative path, each contributed as
> `path_bytes || 0x00 || exact_file_bytes`. Rendered as `sha256:<64 lowercase hex>`.

Inputs per digest:

- **`source_inputs_digest`** — the 40 `.rs` files listed in `manifest.json` → `source_inputs`,
  read from the workspace root. All 40 are Rust sources; **no documentation file is an input**
  (verified — see [§7](#7-doc-vs-code-drift)).
- **`schema_digest`** — the 3 files under `schema/`.
- **`fixture_digest`** — the 151 files under `commands/`, `events/`, `types/`, `compat/`,
  `adversarial/`. Four of these are self-referential (they embed the descriptor), so before
  hashing, `contract.fixture_digest` is normalized to `sha256:` + 64 zeros in
  `events/ready.json`, `adversarial/events/version-mismatch.jsonl`,
  `adversarial/events/schema-mismatch.jsonl`, `adversarial/events/fixture-mismatch.jsonl`,
  and the value is re-serialized as canonical JSON (recursively key-sorted, compact
  separators, one trailing `\n`).

### 3.2 Reproduction — exact command run, exact output

The canonical Rust reproduction is
`cargo run -p wcore-protocol --bin wcore-contract -- digest`
(`crates/wcore-protocol/src/bin/wcore-contract.rs`). **This author did NOT run it** — the
authoring machine (macOS) has no Rust toolchain, and per project policy Cargo must not be run
there. Authoritative Cargo proof runs on `hetzner-dsm:/root/wayland`.

Instead the digests were reproduced independently, from the working-tree files, with a
toolchain-free reimplementation of `digest_named_bytes`. Script:
`scratchpad/verify_digests.py` (session scratchpad, not committed).

Command:

```bash
cd /Users/seandonahoe/dev/waylandcore-ferrox && python3 verify_digests.py
```

Verbatim output:

```
source_inputs_digest  files=40
  computed: sha256:d8b1a8b5645707225bdaafea617452fe1cf2f99556be53d7dcd35891d5e92f28
  manifest: sha256:d8b1a8b5645707225bdaafea617452fe1cf2f99556be53d7dcd35891d5e92f28
  MATCH: True

schema_digest  files=3
  computed: sha256:e5d1744aa6cadc46d2707a1fa190ac80ee74f13477d685bb9146a71b3fff2e54
  manifest: sha256:e5d1744aa6cadc46d2707a1fa190ac80ee74f13477d685bb9146a71b3fff2e54
  MATCH: True

fixture_digest  files=151
  computed: sha256:42f142abf6e534e0bcb33ef7e6d9ec00c53c57938fa467f46d643d9e80e451e4
  manifest: sha256:42f142abf6e534e0bcb33ef7e6d9ec00c53c57938fa467f46d643d9e80e451e4
  MATCH: True

corpus_tree_digest (all 156 files, not in manifest)
  computed: sha256:8b519511a12637cd7c54b0eb24450c8e1e2f13b06d1d919f016a5faedcbd617d

declared fixture count: 151 | recomputed: 151
OVERALL: PASS
```

**What this proves:** the checked-in `manifest.json` (and therefore the `ready.contract`
descriptor Core emits) is byte-consistent with the checked-in generator sources and the
checked-in corpus at the pinned SHA. The corpus is not stale and was not hand-edited.

### 3.3 Plain-tool digests (no Rust, no Python)

For Desktop CI that wants a cheap integrity pin, verifiable with `shasum` alone:

```bash
cd crates/wcore-protocol/contracts/desktop/v1
shasum -a 256 manifest.json schema/*.json events/ready.json events/execution_policy.json
```

```
03b1fedfca8aa4e5ec0f8c1aefc92670b769393441c0fb0e6fba9f0124d187de  manifest.json
d1e1036fe944a8af58618daa8a95ad994b7a4d4f07c46b6ad7ea44296ca12e5a  schema/core-event.schema.json
161054163449922c549e671d527ee186c7e0d89fdadfd5ef86aa0193722b0709  schema/host-command.schema.json
e5d9803e90779834bae99ccf1e62d7a82b4ebf519ec4ac096ee68e2186de9a56  schema/producer-complete.schema.json
ccbc87c254d53228528557bbe399dcaa3353e86063f91cc2a258c2632adaa219  events/ready.json
d5a1c7fde15448891a543f0bb996bd0bf8592d5729404618ba5e09f304ca2bac  events/execution_policy.json
258d04495ed9b80059e771577db66a071b5a3c0cbae7a351f5cfebae0560f054  DEFERRED.md
```

Whole-corpus pin over all 156 files:

```bash
cd crates/wcore-protocol/contracts/desktop/v1
find . -type f | sed 's|^\./||' | LC_ALL=C sort | xargs shasum -a 256 | shasum -a 256
# → a39c13794669e1afca2218ddf3437ba967b4dceb25d9c7e669974358495821e6  -
```

Note this `a39c1379…` value is a *convenience* pin using a different construction than
`digest_named_bytes`; it is not the `fixture_digest` and must not be compared to it.

---

## 4. Negotiation — the fail-closed handshake

Reference implementation:
`crates/wcore-protocol/src/contract/observation.rs::HostContractObserver::observe_json_line`.
Desktop is expected to implement equivalent semantics; this is the behavior Core's own
reference observer enforces, and the behavior the adversarial corpus is built to exercise.

1. **`ready` must come first.** Any event before a successful `ready` → `ReadyRequired`.
2. **`ready` occurs exactly once.** A second `ready` → `DuplicateReady`.
3. **`ready` required fields**, in schema order: `type`, `version`, `capabilities`,
   `contract`, `execution_policy`. `session_id` is optional. Missing → `MissingReadyField` /
   `MissingContractDescriptor`; malformed → `InvalidReadyField` / `InvalidContractDescriptor`.
4. **`capabilities.current_mode` must appear in `capabilities.modes`**, else
   `InvalidReadyField { field: "capabilities" }`.
5. **Descriptor structural validation**: non-empty `name` and `generator`, `major != 0`,
   non-empty `capabilities`, and all three digests matching `^sha256:[0-9a-f]{64}$`.
6. **Descriptor pin comparison, each its own error** — `name` → `UnsupportedContractName`;
   `major` → `UnsupportedContractMajor`; `minor` → `ContractMinorMismatch`; `generator` →
   `GeneratorMismatch`; `schema_digest` → `SchemaDigestMismatch`; `fixture_digest` →
   `FixtureDigestMismatch`; `source_inputs_digest` → `SourceInputsDigestMismatch`;
   `capabilities` map → `CapabilityStatusMismatch`.
   **Note the strictness:** `minor` mismatch is an error, not a tolerated forward-compatible
   skew. At v1.8 the contract is pinned exactly, both directions.
7. **Launch policy validation** — `execution_policy` must have `critical: true`,
   `revision == 0`, `reason ∈ {launch, resume}`, an accepted `contract_version` major, and an
   internally consistent policy (see [§5.4](#54-internal-consistency-invariants)).

Only after all of that does the observer set `negotiated = true`.

### 4.1 Unknown events after negotiation

| Line | Disposition |
|---|---|
| `type` in the 49-event producer inventory | `HostObservation::Event(value)` |
| Unknown `type` with `"critical": false` | **Dropped** — `DroppedUnknownNonCritical` |
| Unknown `type` with `"critical": true` | **Fail closed** — `UnknownCriticalEvent` |
| Unknown `type` with **no** `critical` field | **Fail closed** — `UnknownCriticality` |

The third row is the important one: absence of `critical` is *not* treated as
non-critical. An unclassified unknown event halts the consumer. This is deliberate and is a
requirement on the Desktop reducer, not merely on Core.

Transport-level failures are distinguished from unknown-type handling: `MalformedJson`,
`NonObject`, `MissingType`, `NonStringType`.

---

## 5. `EffectiveExecutionPolicy` — actual semantics from the code

Authority type: `crates/wcore-types/src/execution_policy.rs`.
Wire sequencing: `crates/wcore-protocol/src/execution_policy.rs`.

### 5.1 The core invariant

`EffectiveExecutionPolicy` **implements `Serialize` but deliberately NOT `Deserialize`**
(`crates/wcore-types/src/execution_policy.rs` line 262). It is output-only. There is no code
path by which a serialized wire value becomes live Core authority.

The only shape accepted from lower-trust serialized input is `ExecutionPolicyRequest`
(line 66), which is `#[serde(deny_unknown_fields)]` and carries exactly one optional field:
`approvals: Option<ApprovalPolicyRequest>`. `posture` and `sandbox` are **absent from the
request type entirely**. A wire peer can ask for an approval posture; it cannot mint a managed
floor, and it cannot mint a sandbox bypass. Proved by the unit test
`serialized_input_cannot_request_dangerous_or_managed`.

**Desktop consequence: echoing a policy snapshot back to Core changes nothing.** Any Desktop
UI that presents policy as an editable object is misrepresenting it. It is a receipt.

### 5.2 Fields — every one, with widen/narrow direction

Serialized field set of `EffectiveExecutionPolicy`:

| Field | Type / values | Meaning | Direction |
|---|---|---|---|
| `posture` | `smart` \| `managed` \| `dangerous` | User-facing posture after applying any managed floor | `dangerous` widens; `managed` narrows |
| `approvals` | `prompt` \| `auto_edit` \| `bypass` | Approval gate | **Strictness `prompt`(2) > `auto_edit`(1) > `bypass`(0)**. `prompt` narrowest, `bypass` widest |
| `sandbox` | `required` \| `bypass` | Shell/process sandbox | `required` narrow, `bypass` widest. **Only reachable via `dangerous`** |
| `source` | `default`, `managed`, `user_config`, `project`, `environment`, `local_cli_launch`, `desktop_local_launch`, `protocol`, `acp`, `tui`, `resume`, `child` | Provenance of the effective decision | Not an authority axis — provenance only. But **only `local_cli_launch` and `desktop_local_launch` can resolve `dangerous`** |
| `managed_floor_active` | bool | A managed floor is installed | `true` narrows; sticky, survives a dangerous grant |
| `dangerous_activation_id` | string, omitted when absent | Identity of the one-shot dangerous grant | Present **iff** `posture == dangerous` |
| `dangerous_expires_at_unix_ms` | integer, omitted when absent | **Audit/display only** | Present **iff** `posture == dangerous` |

Both `dangerous_*` fields are `#[serde(skip_serializing_if = "Option::is_none")]` — absent
from the wire, not `null`, in every non-dangerous snapshot.

### 5.3 What widens, what narrows — the enforced rules

- **Sandbox never widens except through `dangerous`.** `EffectiveExecutionPolicy::baseline()`
  hardcodes `sandbox: SandboxPolicy::Required` regardless of the baseline's own field
  (line 280). The only constructor producing `sandbox: bypass` is
  `EffectiveExecutionPolicy::dangerous()`, which requires a resolver-produced
  `DangerousSessionGrant`. Test `force_equivalent_bypasses_approval_but_retains_sandbox`
  proves `approvals: bypass` still yields `sandbox: required`.
- **A managed floor cannot be weakened by any request from any source.**
  `BaselineExecutionPolicy::with_requested_approvals` routes managed sessions through
  `stricter_approval_policy` and forces `source: Managed`. Test
  `managed_approval_floor_cannot_be_weakened_by_any_request_source` sweeps all 10
  non-default sources × `{auto_edit, bypass}` and asserts the floor holds.
- **`dangerous` requires an explicit local process launch.** `resolve_dangerous_launch`
  rejects every source except `LocalCliLaunch` and `DesktopLocalLaunch`
  (`DangerousRequiresLocalLaunch`). Test `only_explicit_local_launch_can_resolve_dangerous`
  sweeps the other 10 sources. **`Protocol` is in the rejected set — Desktop cannot activate
  dangerous mode over the JSON stream.** It must relaunch the Core process.
- **Managed deny beats everything, and is checked first.** `DangerousDeniedByManagedPolicy`
  is returned before source validation (test `managed_deny_wins_before_source_validation`).
- **A managed floor survives a dangerous grant.** `managed_floor_active` stays `true` through
  `EffectiveExecutionPolicy::dangerous()` (test
  `local_dangerous_grant_retains_managed_floor_provenance`).
- **TTL bounds**: default 900s, max 3600s. `ttl_secs == 0` or `> 3600` →
  `DangerousTtlOutOfRange`. Empty/whitespace activation id → `DangerousActivationIdEmpty`.
- **Expiry authority is monotonic, not wall-clock.** `DangerousSessionGrant` holds a
  `#[serde(skip)]` `monotonic_deadline: Instant`. `dangerous_expires_at_unix_ms` is audit
  metadata. **Desktop must not compute remaining authority from the Unix timestamp** — it can
  display it, but Core's monotonic deadline governs, and a clock change does not move it.
- **`with_runtime_approvals` narrows only the approvals view.** It clones the snapshot and
  replaces `approvals`, keeping sandbox grant, source, expiry, and activation identity
  byte-for-byte. It creates no authority; the caller must already have resolved through the
  live approval manager.

### 5.4 Internal consistency invariants

Enforced by `ObservedExecutionPolicy::validate` and reflected in the schema:

- If `posture == dangerous`: `approvals == bypass` **and** `sandbox == bypass` **and** a
  non-empty `dangerous_activation_id` **and** a present `dangerous_expires_at_unix_ms`.
- Otherwise: `sandbox == required` **and** both `dangerous_*` fields absent, **and** if
  `posture == managed` then `managed_floor_active == true`.

A snapshot violating any of these must be rejected, not normalized.

### 5.5 Revision sequencing (`execution_policy` sub-contract 1.0)

Envelope (all required): `critical`, `contract_version`, `revision`, `reason`,
`effective_at_unix_ms`, `policy`.

- `critical` is **always `true`** — schema pins `{"const": true}`. This is an
  authority-critical event; a contract-aware host that does not understand it must fail
  closed rather than drop it.
- `contract_version` is `"1.0"`. Only **major `1`** is accepted; the minor is ignored
  (`validate_execution_policy_contract_version` accepts `"1.7"`, rejects `"2.0"`).
- `revision` is session-monotonic, starting at `0` in the `ready` snapshot.
- **`advance_if_changed` advances only when the serialized `policy` actually changed.**
  An accepted no-op `set_mode` does **not** consume a revision. Desktop must not assume a
  revision bump per accepted command.
- `reason` ∈ `launch` | `mode_change` | `resume` | `expiry`. `revision 0` carries `launch` or
  `resume` only.

Reducer dispositions (`ExecutionPolicySequence::accept`):

| Input | Result |
|---|---|
| `revision == current`, byte-identical | `Duplicate` (idempotent) |
| `revision == current`, different bytes | **fail** `ConflictingDuplicate` |
| `revision == current + 1` | `Advanced` |
| Any other revision (gap or stale) | **fail** `OutOfOrder { expected, actual }` |
| Unsupported `contract_version` major | **fail** `UnsupportedContractVersion` |
| `critical: false` | **fail** `NonCriticalSnapshot` |
| `current + 1` overflows `u64` | **fail** `RevisionOverflow` |

---

## 6. Lifecycle, correlation, ordering, duplicates, terminals, failure

### 6.1 Criticality vocabulary

`manifest.json` classifies every event. Distribution across the 49 Desktop events:
**33 `safety`, 15 `observational`, 1 `required`**. 48 of 49 carry a non-`noncritical`
classification. Desktop must not treat the corpus as a set of droppable notifications.

### 6.2 Correlation keys — the full table

There is no single universal correlation id. Desktop needs 22 distinct correlation classes:

| Correlation key | Events |
|---|---|
| `call_id` | `browser_event`, `cua_event`, `host_send_message_request`, `tool_cancelled`, `tool_chunk`, `tool_panicked`, `tool_request`, `tool_result`, `tool_running` |
| `msg_id` | `browser_policy_denied`, `cua_policy_denied`, `info`, `stream_end`, `stream_start`, `text_delta`, `thinking`, `trace_event` |
| `msg_id_or_session` | `error` |
| `request_id` | `budget_grant_result`, `mcp_removal_result`, `runtime_diagnostics_snapshot`, `runtime_diagnostics_unavailable` |
| `request_id_and_cursor` | `session_recovery_replay`, `session_recovery_snapshot` |
| `request_id_and_session_id` | `session_recovery_unavailable` |
| `resume_token` | `approval_required`, `approval_resume`, `suspend` |
| `revision` | `execution_policy` |
| `run_id_and_sequence` | `workflow_started`, `workflow_node_event`, `workflow_finished` |
| `child_run_id_and_child_sequence` | `sub_agent_event` |
| `run_id` | `evolution_event` |
| `session_id` | `ready`, `session_cost` |
| `session_id_and_sequence` | `anvil_receipt`, `anvil_receipt_invalidated` |
| `session` | `budget_exceeded`, `config_changed` |
| `turn_id_and_cursor` | `turn_recovery_lifecycle` |
| `session_turn_tool_and_cursor` | `unknown_tool_effect_resolved` |
| `name` | `mcp_ready`, `mcp_failed` |
| `connection` | `pong` |
| `primary` | `provider_circuit_event` |
| `failed_provider_and_selected_provider` | `provider_failover_receipt` |
| `plugin_name` | `plugin_event` |
| `plugin_name_and_surface` | `plugin_registration_failed` |

### 6.3 Ordering and duplicate rules by sub-contract

**Execution policy** — see [§5.5](#55-revision-sequencing-execution_policy-sub-contract-10).

**Workflow lifecycle 1.0** (`crates/wcore-protocol/src/workflow.rs`).
Node states: `queued`, `running`, `succeeded`, `failed`, `blocked`. Terminal run states:
`succeeded`, `failed`.
Acceptance: `Advanced`, `Duplicate`, `IgnoredAfterChildTerminal`, `IgnoredAfterNodeTerminal`,
`IgnoredAfterRunTerminal`, `Unrelated`.
**Post-terminal events are absorbed as ignored, not errors** — that asymmetry matters for the
Desktop reducer, which must not fail closed on a late node event after a run terminal.
Failures: `UnsupportedContractVersion`, `Malformed{field}`, `ConflictingDuplicate{event_id}`,
`DuplicateRun`, `UnknownRun`, `InvalidStartSequence`, `OutOfOrder{run_id,expected,actual}`,
`ChildOutOfOrder`, `ChildCorrelationChanged`, `InvalidChildParent`, `ConflictingChildTerminal`,
`ChildTerminalTypeMismatch`, `ChildAfterNodeTerminal`, `NodeChildCorrelationChanged`.

**Anvil receipts 1.0** (`crates/wcore-protocol/src/anvil.rs`).
`ANVIL_RECEIPT_ORIGIN = "core/anvil"`, `ANVIL_DIGEST_ALGORITHM = "sha256"`.
Receipt statuses: `Active`, `Invalidated`, `Superseded`. Apply outcomes: `Applied`,
`Duplicate`, `Inert`.
Invalidation reasons: `artifact_mutated`, `gate_revoked`, `superseded`.
Failures: `Malformed`, `VersionMismatch`, `InvalidOrigin`, `InvalidField`,
`SequenceGap{expected,observed}`, `OutOfOrder{expected,observed}`, `EventConflict`,
`ReceiptConflict`, `UnknownReceipt`, `CorrelationMismatch`, `UnknownCriticalExtension`,
`ReceiptBodyDigestMismatch`, `InvalidationBodyDigestMismatch`.
**Trust is never resurrected**: replaying a receipt after its invalidation is `Inert`, proved
by the `adversarial/anvil/stale-replay.jsonl` vector.
**A receipt nested inside `sub_agent_event` or `plugin_event` is not authority** — only a
top-level `anvil_receipt` is. Vector: `adversarial/anvil/nested-receipt-inert.jsonl`;
Core tests `forged_receipt_nested_in_sub_agent_event_is_not_top_level`,
`forged_receipt_nested_in_plugin_event_is_not_top_level`.
`required_extensions` carrying an unknown value → `UnknownCriticalExtension` (fail closed).

**Turn recovery v1.** `recovery_version` is pinned `{"const": 1}`. Cursors are
`{journal_digest: ^[0-9a-f]{64}$, journal_sequence?}`. `session_recovery_unavailable.reason` ∈
`session_not_found`, `unsupported_version`, `cursor_invalid`, `cursor_ahead`,
`cursor_digest_mismatch`, `history_gap`, `journal_corrupt`, `snapshot_unavailable`,
`unknown_critical_state`. Lifecycle ∈ `ready`, `streaming`, `awaiting_approval`,
`tool_in_flight`, `reconciliation_required`, `suspended`, `completed`, `cancelled`, `failed`.
The replay stream is **sanitized and content-free** — it restores lifecycle position, not
message content.

**Legacy ordinary turn/tool events have NO producer event id and NO monotonic sequence.**
Recorded in `DEFERRED.md` under `ordinary_turn_tool_replay_reducer`. Desktop cannot build a
deduplicating reducer over `text_delta`/`tool_*` from producer identity at v1.8. Recovery v1
is the supported mechanism for interrupted-turn restoration.

### 6.4 Closed-shape commands

These 11 wire types set `additionalProperties: false` — unknown fields are rejected, not
ignored: `continue_with_budget`, `budget_grant_result`, `session_resync`, `resume_turn`,
`resolve_interrupted_approval`, `resolve_unknown_tool_effect`, `unknown_tool_effect_resolved`,
`get_runtime_diagnostics`, `runtime_diagnostics_snapshot`, `remove_mcp_server`,
`mcp_removal_result`. Everything else is `additionalProperties: true` (additive-tolerant).

`continue_with_budget` additionally requires at least one of `additional_tokens >= 1` or
`additional_cost_usd > 0` (`anyOf`). `budget_grant_result` requires `refusal_reason` iff
`outcome == refused`, and forbids it when `granted` (`allOf`/`if`/`then`).

### 6.5 Ownership boundary

The pinned corpus describes **18 commands and 49 events** as the Desktop-consumed surface.
The producer emits 8 further wire types that are **explicitly outside** the Desktop contract
and appear in `schema/producer-complete.schema.json` only as an inventory discriminator:

`capability_activation`, `compact_offload`, `grant_workspace_capability`,
`mid_flight_monitor_decision`, `provider_attempt`, `provider_failure`, `provider_retry`,
`workspace_policy`.

Desktop must tolerate these on the stream but must not build control-plane behavior on them —
they are not covered by `fixture_digest` conformance and may change without a contract minor
bump. **`workspace_policy` is the trap**: it is prose-documented in
`docs/json-stream-protocol.md` §1.1b as though it were a normal event, but it is *not* one of
the 49. See finding [F-6](#f-6-info--workspace_policy-documented-as-in-contract-but-is-inventory-only).

Authority ownership, unchanged by this document: **Core is the enforcement authority.**
Desktop is the GUI/control plane. Nothing Desktop sends can mint policy, sandbox bypass, a
managed floor, or a dangerous session over the protocol.

---

## 7. Doc-vs-code drift

The brief required reporting drift between `docs/json-stream-protocol.md` and the
implementation rather than papering over it. Six findings. Two were HIGH and — per the
termination rule (CRITICAL/HIGH must be fixed or disproved; MEDIUM and below are logged and
do not block) — **both are fixed in this change**. The remaining four are logged.

**The fixes touch only `docs/json-stream-protocol.md`.** That file is **not** one of the 40
`source_inputs`, so no digest changed. Verified by re-running the digest reproduction after
editing: still `OVERALL: PASS`, all three digests identical, and `git status --porcelain crates`
still empty.

### F-1 (HIGH — FIXED) `ready` documented without its two required fields

`docs/json-stream-protocol.md` §1.1 showed a `ready` example and field table with **no
`contract` object and no `execution_policy` object**. Both are `required` in
`schema/core-event.schema.json`
(`required = ["type","version","capabilities","contract","execution_policy"]`), and the
reference observer fails closed before negotiation on either — `MissingContractDescriptor`,
`MissingReadyField{field:"execution_policy"}`.

Impact: a Desktop consumer implemented from the doc alone would emit or expect a `ready` that
never negotiates. This is precisely the "doc disagrees with code, downstream consumer breaks"
case.

Fix: the example now carries both objects with the real pinned descriptor values, the table
has rows for both, and a note names the observer and the corpus as normative.

### F-2 (HIGH — FIXED) `execution_policy` documented without its authority envelope

§1.1a showed only `{type, policy}`. The schema requires six fields:
`type`, `critical`, `contract_version`, `revision`, `reason`, `effective_at_unix_ms`,
`policy` — with `critical` pinned `{"const": true}`. The doc also said "Unknown hosts must
drop this additive event", which **inverts the actual rule**: this is an authority-critical
event, and a contract-aware host that does not understand it must fail closed
(`UnknownCriticalEvent` / `NonCriticalSnapshot`), not drop it.

Impact: a Desktop reducer built from the doc would (a) emit/accept snapshots missing every
sequencing field, and (b) silently drop policy changes it should have failed closed on. The
second is a security-relevant misread of the authority model.

Fix: example now shows the full envelope; added the revision-sequencing rules, the
"no-op change does not consume a revision" rule, the four reducer dispositions, the
`contract_version` major-only rule, the monotonic-vs-audit-clock distinction, and the
`dangerous_*` presence invariant. Removed the incorrect "must drop" sentence.

### F-3 (MEDIUM — logged, mitigated) 15 wire types absent from the prose doc

Verified by substring search over `docs/json-stream-protocol.md` — **0 occurrences** each:

- Events (12): `anvil_receipt_invalidated`, `mcp_failed`, `mcp_removal_result`,
  `plugin_registration_failed`, `provider_failover_receipt`, `runtime_diagnostics_snapshot`,
  `runtime_diagnostics_unavailable`, `tool_panicked`, `unknown_tool_effect_resolved`,
  `workflow_started`, `workflow_node_event`, `workflow_finished`
- Commands (3): `get_runtime_diagnostics`, `remove_mcp_server`, `resolve_unknown_tool_effect`

That is 15 of the 67 Desktop-consumed wire types (22%) with no prose at all. This is an
**omission, not a contradiction** — the machine-readable corpus covers all 67 correctly, and
the corpus is what D1 pins. Narrating 15 event families is out of scope for a protocol
checkpoint and would be unreviewable drive-by scope.

Mitigation applied: added a "Normative source" callout to the doc Overview stating that the
digest-pinned corpus is normative, that it covers 18 commands and 49 events, that the doc does
not yet narrate all of them, and that the corpus wins on any disagreement.

Recommendation: file a Core follow-up to narrate the 15. Not a D1 blocker — **Desktop should
implement against the corpus, not the prose.**

### F-4 (LOW — FIXED opportunistically) stale values in the edited `ready` section

Same lines as F-1, so corrected in the same edit rather than left knowingly wrong:
`version` example `"0.2.0"` → `"0.12.25"`; `modes` example `["default","auto_edit","yolo"]` →
`["default","auto_edit","force"]` (matching `events/ready.json`; note `yolo` remains an
accepted `set_mode` input, it is simply not what Core advertises); added the three capability
flags present in the pinned fixture but missing from the table — `memory_enabled`,
`online_evolution`, `user_model_backend`.

### F-5 (LOW — logged, not fixed) generated `DEFERRED.md` self-describes as v1.7

`crates/wcore-protocol/contracts/desktop/v1/DEFERRED.md` line 3 reads "This v1.7 corpus"
while `CONTRACT_MINOR = 8` and `manifest.json` reports `minor: 8`. The string is a hardcoded
literal in `generate.rs` (the `DEFERRED` const, line 28+) that was not updated with the minor
bump.

No machine impact: `DEFERRED.md` is excluded from both `fixture_digest` (not under
`commands/`, `events/`, `types/`, `compat/`, `adversarial/`) and `schema_digest` (not under
`schema/`). Cosmetic only.

**Not fixed deliberately.** `generate.rs` *is* a `source_inputs` file — editing it changes
`source_inputs_digest`, which changes the descriptor Core emits, which invalidates the pin
this document exists to publish. Fixing a cosmetic typo is not worth re-pinning the contract
mid-checkpoint. Fold it into the next intentional minor bump.

### F-6 (INFO) `workspace_policy` documented as in-contract, but is inventory-only

§1.1b documents `workspace_policy` as a first-class event. It is not among the 49
`EVENT_SPECS`; it is one of the 8 producer-inventory-only types
([§6.5](#65-ownership-boundary)). Likewise §1.1c is headed "contract v1.1" while the corpus is
v1.8. Left as-is: the doc content is not wrong about what Core emits, only about the event's
contract status, and the new Overview callout plus [§6.5](#65-ownership-boundary) resolve the
ambiguity for Desktop.

---

## 8. What Core has actually proven

Core-side only. Every item is a test in `crates/wcore-protocol/tests/`, all replaying the
**serialized** corpus — not in-process struct assertions.

**`desktop_contract_corpus.rs`** — `checked_corpus_matches_real_serializers_byte_for_byte`;
`inventory_is_exactly_eighteen_commands_and_forty_nine_events`;
`manifest_pins_generator_and_all_three_digests`;
`manifest_ready_and_schema_titles_share_one_contract_identity`;
`every_command_fixture_deserializes_through_protocol_command`;
`every_json_artifact_is_canonical_and_lf_terminated`;
`manifest_criticality_uses_only_the_normative_typed_vocabulary`;
`event_schema_distinguishes_correlated_and_legacy_child_shapes`;
`generated_schemas_reject_malformed_authority_types_and_enums`;
`budget_grant_adversarial_fixtures_fail_closed`;
`contradictory_budget_grant_results_cannot_cross_the_wire`;
`durable_child_type_fixtures_round_trip_without_a_parallel_model`;
`malformed_durable_child_fixtures_are_rejected`;
`producer_complete_schema_keeps_current_and_non_desktop_variants_visible`;
`authority_fixtures_pin_correlated_current_shapes`.

**`desktop_contract_adversarial.rs`** — `canonical_ready_advertises_the_embedded_generated_contract`;
`ready_replay_fails_closed_on_major_schema_and_fixture_mismatch`;
`negotiated_host_drops_serialized_unknown_noncritical_event`;
`negotiated_host_rejects_unknown_critical_and_unclassified_events`;
`serialized_policy_reducer_accepts_valid_revisions_and_duplicate_replay`;
`serialized_policy_reducer_fails_closed_on_conflict_gap_version_and_criticality`;
`serialized_recovery_reducer_accepts_pinned_cursor_replay`;
`serialized_recovery_reducer_fails_closed_on_version_cursor_and_state_drift`;
`serialized_workflow_reducer_replays_correlated_node_and_child_lifecycle`;
`serialized_workflow_reducer_detects_conflict_gaps_and_absorbs_terminals`;
`production_workflow_reducer_replays_the_checked_corpus`;
`serialized_anvil_reducer_replays_invalidation_and_never_resurrects_trust`;
`serialized_anvil_reducer_proves_duplicate_inert_and_fail_closed_vectors`;
`negotiated_observer_accepts_authoritative_anvil_invalidation`;
`runtime_diagnostics_is_closed_versioned_correlated_and_redacted`;
`malformed_and_unknown_commands_never_deserialize_for_dispatch`;
`remaining_deferrals_exclude_live_negotiation_guarantees`.

**`host_decoder_contract.rs`** — 30+ tests on additive tolerance, capability-gated decoding,
malformed-vs-unknown discrimination, and nested-receipt forgery rejection.

**CI gate.** `wcore-contract check` (`crates/wcore-protocol/src/bin/wcore-contract.rs`)
regenerates every artifact in memory and compares byte-for-byte against the checked-in corpus.
A source change that alters the wire without regenerating fails CI.

**Explicitly NOT proven by any of the above:** Desktop reducer behavior, Desktop UI behavior,
durable Desktop replay across restart, and the persistent Anvil artifact-mutation watcher.
`DEFERRED.md` names the last two as `anvil_desktop_replay_reducer` and
`anvil_persistent_mutation_watcher`.

**Compile/test status at this SHA: VERIFIED GREEN — §8 is a receipt, not a citation.**

The author of §8 could not run Cargo (no toolchain on the authoring machine; project policy
forbids Cargo there) and correctly recorded its evidence as cited-from-source rather than
observed. That gap is now closed by an actual run on the authoritative Linux host:

```
host    : hetzner-dsm:/root/wayland
HEAD    : b6936299d9c3a7d3110e9ba03c36e5debe965b85   (the pinned contract SHA)
worktree: clean (git status --porcelain = 0 lines)
command : cargo nextest run -p wcore-protocol --no-fail-fast
result  : 302 tests run: 302 passed, 0 skipped   (EXIT=0)
```

Included in that run and passing: `desktop_contract_corpus::checked_corpus_matches_real_serializers_byte_for_byte`,
`::manifest_ready_and_schema_titles_share_one_contract_identity`,
`::every_json_artifact_is_canonical_and_lf_terminated`,
`::generated_schemas_reject_malformed_authority_types_and_enums`,
`::malformed_durable_child_fixtures_are_rejected`, and
`::producer_complete_schema_keeps_current_and_non_desktop_variants_visible`.

Those are the tests that bind this document's claims to the real serializers, so the corpus
Desktop consumes is proved byte-identical to what Core actually emits at this SHA — not merely
consistent with a checked-in manifest.

Note on environment: `cargo` is not on the PATH of a non-login shell on `hetzner-dsm`. Use
`/root/.cargo/bin/cargo` or export `PATH=/root/.cargo/bin:$PATH` explicitly; a bare `cargo`
over `ssh` fails with `cargo: not found` / exit 127, which is a PATH artifact and not a
build failure.

---

## 9. What the Desktop lane must still supply

**Core evidence does not close any of these.** The checkpoint requires the linked Desktop lane
to prove consumer replay and UI/control behavior; both receipts are required for a
whole-Wayland claim.

1. **A linked Desktop plan** in the Wayland Desktop repository, referencing pinned SHA
   `b6936299d9c3a7d3110e9ba03c36e5debe965b85` and the three digests in [§3](#3-digests).
2. **A host conformance suite** that replays the **serialized** corpus at
   `crates/wcore-protocol/contracts/desktop/v1/` through the **real Desktop consumer/reducer**.
   The checkpoint is explicit that deserialization alone is insufficient — the reducer must
   produce observable state transitions that are asserted.
3. **Negotiation conformance** — Desktop pins `name`/`major`/`minor`/`generator` + all three
   digests, and fails closed on each of the eight distinct mismatches in
   [§4](#4-negotiation--the-fail-closed-handshake) step 6. Vectors provided:
   `adversarial/events/version-mismatch.jsonl`, `schema-mismatch.jsonl`,
   `fixture-mismatch.jsonl`.
4. **Unknown-event conformance** — all three dispositions in [§4.1](#41-unknown-events-after-negotiation),
   including the non-obvious one: an unknown event with **no** `critical` field must fail
   closed, not be dropped. Vectors: `adversarial/events/unknown-critical.jsonl`,
   `unknown-noncritical.jsonl`, `unknown-criticality.jsonl`.
5. **Policy reducer conformance** — all six vectors under `adversarial/policy/`:
   `valid-revisions`, `duplicate-identical`, `duplicate-conflict`, `revision-gap`,
   `version-mismatch`, `noncritical`. Plus an assertion that Desktop treats
   `EffectiveExecutionPolicy` as a **receipt**, never as an editable object, and never
   computes remaining dangerous-session authority from `dangerous_expires_at_unix_ms`.
6. **Workflow reducer conformance** — the seven vectors under `adversarial/workflow/`,
   including the post-terminal absorption asymmetry ([§6.3](#63-ordering-and-duplicate-rules-by-sub-contract)):
   late events after a terminal are *ignored*, not errors.
7. **Anvil reducer conformance** — the ten vectors under `adversarial/anvil/`, and
   specifically that (a) invalidation is never reversed by a later stale receipt replay, and
   (b) a receipt nested inside `sub_agent_event`/`plugin_event` renders no trust affordance.
   `DEFERRED.md` names `anvil_desktop_replay_reducer` as deferred **pending exactly this**.
8. **Recovery reducer conformance** — the five vectors under `adversarial/recovery/`, and an
   acknowledgement that the replay stream is content-free: Desktop must not expect message
   content restoration from it.
9. **Correlation conformance** — Desktop demultiplexes using all 22 correlation classes in
   [§6.2](#62-correlation-keys--the-full-table). A single-id assumption will silently
   mis-correlate `sub_agent_event` and the workflow family.
10. **Ownership acknowledgement** — Desktop does not build control-plane behavior on the 8
    inventory-only types in [§6.5](#65-ownership-boundary), and does not treat the three
    `shape_only` capabilities (`browser_events`, `cua_events`, `plugin_events`) as having a
    proven production emitter.
11. **A named renegotiation policy** — what Desktop does when a Core upgrade changes any
    digest. Because `minor` mismatch is a hard error ([§4](#4-negotiation--the-fail-closed-handshake)
    step 6), every contract bump is a coordinated release, not a silent upgrade. Desktop must
    state its intended behavior: refuse to launch, degrade, or prompt.

**Until items 1–2 exist with a green run, D1 is not complete.** This document makes it
Desktop-blocked rather than Core-blocked.

---

## 10. Provenance of this document

| | |
|---|---|
| Authored | 2026-07-26 |
| Checkout | `/Users/seandonahoe/dev/waylandcore-ferrox`, branch `plan/f20-unified-audit-repair` |
| Pinned SHA | `b6936299d9c3a7d3110e9ba03c36e5debe965b85` |
| Observation HEAD | `2cc1a285ffd3f3b0fb41b177bd9a1317654cb350` |
| Digests | Independently recomputed from working-tree files; all three matched the manifest ([§3.2](#32-reproduction--exact-command-run-exact-output)) |
| Cargo | **NOT RUN** — no toolchain on the authoring machine; project policy forbids it there |
| Code changes | `docs/json-stream-protocol.md` only (F-1, F-2, F-4 fixes + Overview callout). No file under `crates/` was modified; no digest changed |
