use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use crate::anvil::{AnvilReceipt, anvil_receipt_body_digest};
use crate::commands::{
    BUDGET_GRANT_REQUEST_ID_MAX_BYTES, BUDGET_GRANT_REQUEST_ID_PATTERN, ProtocolCommand,
};

use super::ContractResult;
use super::canonical::{canonical_json, digest_named_bytes};
use super::observation::{ContractCapabilityStatus, ContractDescriptor};
use super::spec::{
    COMMAND_SPECS, EVENT_SPECS, PRODUCER_COMMAND_TYPES, PRODUCER_EVENT_TYPES, SOURCE_INPUTS,
    WireSpec, anvil_invalidation, anvil_receipt, command_fixture_values,
    compatibility_event_values, durable_child_fixture_values, event_fixture_values,
    workflow_lifecycle_events,
};

pub const CONTRACT_NAME: &str = "wayland-desktop-core";
pub const CONTRACT_MAJOR: u64 = 1;
// Bumped with the seven newly declared producer events. The event vocabulary a
// host can validate got strictly wider, which is exactly what a minor bump is
// for; `protocol_sink.rs` already states the converse rule (a change that adds
// no manifest entry implies no bump).
// 13 -> 14: `call_announced` added. Additive only - no field on an existing
// event changed shape, and `major` therefore holds at 1. Hosts that pin the
// descriptor must re-pin; hosts that ignore unknown types are unaffected.
// 15 -> 16: two additive capabilities land together.
//
// `path_boundary_prompt_v1` (#1099): `tool_request.tool` may now carry an
// optional `escalation` object. The field is skipped when absent and `tool`
// already published `additionalProperties: true`, so a host that has never
// heard of it validates and renders exactly as before.
//
// `render_artifact_v1` (#1098): a new event. No field on an existing event
// changed shape, so `major` holds at 1 — the vocabulary a host can validate
// got strictly wider, which is what a minor bump is for.
// 17 -> 18: `path_write_grants_v1` (#1104). No wire type or field changed
// shape — `ApprovalScope::AlwaysPath::write` and `PathGrantAccess::Write` were
// already published and already refused — so `major` holds at 1. What changed
// is that the engine can now say YES to them, and a pinned host has no way to
// learn that from the shapes alone. Without the bump a host would have to
// discover write support by sending a grant and reading the refusal text,
// which is exactly the button-that-lies this feature exists to prevent.
// 16 -> 17: `grant_workspace_capability`, `grant_path` and `revoke_path` are
// declared (#314). Three new command wire types; no field on an existing
// command or event changed shape, so `major` holds at 1 and the command union
// a host can emit got strictly wider. The wire-shape gate refuses this
// regeneration without the bump - three `added=` entries under a standing
// 1.16 - which is that gate deciding the version question it exists to force.
// 17 -> 18: TWO capabilities land in the same release; one bump carries both.
//
// `inline_reasoning_split_v1` (#1129). No wire SHAPE moved - this is the first
// bump here that the wire-shape gate does NOT force, and the entry says so
// plainly rather than implying a refusal that never happened. What moved is the
// MEANING of two already-published types: `text_delta` no longer carries inline
// `<think>`/`<thinking>`/`<reasoning>` bodies from open-weights models, and
// `thinking` now carries them. A host pinned to 1.17 has no way to learn that
// from the shapes, and rendering is exactly what it changes - the reasoning was
// previously indistinguishable from the answer.
//
// `path_write_grants_v1` (#1104). Every shipped Core accepts `write: true` and
// every one before this refused it, so a host CANNOT feature-detect by sending
// one - it would have to parse refusal prose. The version and the capability
// are the only honest signals.
//
// 18 -> 19: `turn_abandon_v1` (#326, Desktop-side FerroxLabs/wayland#1116). `resume_turn`
// gains a fourth `action`. The command union does not widen and no field
// changes shape, so `major` holds at 1; what widens is one closed enum, which a
// pinned host validates against and would otherwise reject before the frame
// ever reached the wire. Desktop reported exactly that: it cannot offer "give
// up on this stuck turn" because its own outbound validation refuses the value.
// A host cannot feature-detect this by sending one and reading the refusal —
// the refusal happens inside the host, against the pinned corpus.
//
// `contract.minor` plus the named capabilities are the only signals a pinned
// host reads, so the version moves once and all three capabilities name
// themselves.
//
// 19 -> 20: `route_info_v1` (#372). A new always-on event carrying the route a
// turn dispatched against — provider, model, the scrubbed endpoint, and a
// derived local-vs-cloud flag. The reporter ran the same task against a local
// Ollama server and a cloud OpenRouter gateway; both are driven as
// `provider = "openai"`, so every route-bearing field already published read
// identically for the two runs and the endpoint was absent from the protocol
// entirely. A host cannot feature-detect a new event type by sending anything —
// it either sees the frame or it does not — so the version is the only way a
// pinned host learns the route is now answerable.
//
// 20 -> 21: `provider_retry_count_v1` (#372). `provider_attempt` gains
// `attempt` and `provider_retry` gains `retry`, each the 1-based ordinal within
// the turn. Two already-published shapes move, so the wire-shape gate refuses
// the regeneration under a standing 1.20 and forces this bump — which is that
// gate deciding the version question it exists to force. `major` holds at 1:
// both events already published `additionalProperties: true`, so a host that
// has never heard of the fields validates and renders exactly as before. The
// ticket asks for a retry count by name, and counting frames cannot supply one:
// these events are additive, so a host pinned below the minor that introduced
// them drops them, and a host attaching mid-run never saw the earlier ones.
//
// ON THE GAP BETWEEN 16 AND 19. The last TAGGED contract is 1.16 — v0.13.5,
// v0.13.6 and `main` all publish it. 17 and 18 were assembled on this branch
// and never reached a tag, so no host has ever pinned them. The entries above
// are kept as the decision log they are, rather than renumbered to close the
// gap: each records why a specific widening needed a signal, and rewriting them
// to look consecutive would destroy that reasoning to tidy a sequence no host
// reads. A pinned host moves 1.16 -> 1.19 and finds every capability named.
// 19 -> 21: wayland#896's quiesced snapshot lease (three commands, five
// events) and wayland#372's dispatched-route receipt (one event). Both are
// additive and nothing existing changes shape, so this whole integration is a
// single MINOR move: a host pinned to 1.19 keeps working, and the bump is the
// only way it can learn the new capabilities exist to ask for.
// 21 -> 22: wayland#1088 adds one event, `set_mode_refused`. The gate that
// refuses a wire `set_mode` without the local operator opt-in (GHSA-8r7g) used
// to announce itself only as an `info` frame of prose, so a host could not tell
// its requested mode had been rejected and kept rendering a mode the session was
// never in. The event is purely additive and nothing existing changes shape, so
// `major` holds at 1 — but the minor has to move, because an added type is
// undiscoverable to a host pinned below the version that introduced it.
pub const CONTRACT_MINOR: u64 = 22;
pub const GENERATOR_VERSION: &str = "wcore-desktop-contract-gen/22";
pub const CONTRACT_ROOT: &str = "contracts/desktop/v1";

const DEFERRED: &str = r#"# Deferred Desktop contract adversarial cases

This v1.7 corpus records the current producer wire. Contract negotiation,
unknown-critical rejection, and unknown-noncritical dropping are live and
proved by serialized replay through the reference host observer.

Policy, workflow, and Anvil sub-contract vectors exercise their current
producer identities and reducer rules.

Anvil receipts are publication-bound: the producer binds the serialized
verdict body and immediate post-publication artifact state. Durable Desktop
replay and a persistent later-mutation watcher remain deferred.

- `ordinary_turn_tool_replay_reducer`: legacy ordinary turn and tool events
  still have no producer event ID or monotonic sequence. Recovery v1 instead
  exposes a sanitized, content-free journal cursor and replay stream for
  interrupted-turn restoration; it does not retroactively make legacy event
  payloads authoritative.
- `anvil_desktop_replay_reducer`: deferred until Desktop consumes the Core
  reducer and proves restart/replay against this corpus.
- `anvil_persistent_mutation_watcher`: deferred because Core currently checks
  immediate post-publication mutation, not later filesystem changes over the
  full receipt lifetime.

## Producer events with NO Desktop payload schema

None. This section previously listed seven `ProtocolEvent` variants —
`workspace_policy`, `capability_activation`, `provider_attempt`,
`provider_retry`, `provider_failure`, `mid_flight_monitor_decision` and
`compact_offload` — that the production sink emits but that no shipped artifact
declared. All seven now have a `WireSpec`, a fixture generated from the real
serializer, and a payload branch in `core-event.schema.json`.

The gap was not cosmetic. A Desktop host knows only what this corpus ships, and
the documented rule for an event type it does not recognise, with no `critical`
field, is to hard error — so `workspace_policy`, which arrives on every session
immediately after `ready`, tore down a corpus-only host. That is now proved from
the consumer side by `tests/desktop_contract_corpus_only_host.rs`, which builds
a host whose known-event set comes only from `manifest.json` and therefore
cannot be fooled by the producer-side `PRODUCER_EVENT_TYPES` constant.

`generated_artifacts()` now refuses to build a corpus whose `EVENT_SPECS` and
`PRODUCER_EVENT_TYPES` disagree, so this hole cannot reopen silently.

CLOSED, same class, command direction (#314): `grant_workspace_capability`,
`grant_path` and `revoke_path` were in `PRODUCER_COMMAND_TYPES` and absent from
`manifest.json`'s `commands`. The blast radius was different — commands travel
host to Core, so an undeclared command does not hard error a host — but a host
that derives its emitter or its conformance check from the published union
cannot send a command that union does not contain, and that failure reads as
"folder grants do not persist" rather than as a contract gap. All three now
carry a `WireSpec`, a fixture generated from the real deserializer, and a branch
in `host-command.schema.json`, and `generated_artifacts()` refuses to build a
corpus whose `COMMAND_SPECS` and `PRODUCER_COMMAND_TYPES` disagree — the parity
gate the event direction already had, which previously covered events only.

Malformed command fixtures and the current unknown-type behavior are proved by
`desktop_contract_adversarial.rs`. Browser, CUA, and plugin event fixtures are
shape-only because no production emitter is proven at this source baseline.
Runtime diagnostics v1 is production-backed by correlated serialized replay;
its executable readiness is non-spawning, launch-environment exact, and
redacted before entering protocol state.
"#;

fn json_lines(values: impl IntoIterator<Item = Value>) -> ContractResult<Vec<u8>> {
    let mut bytes = Vec::new();
    for value in values {
        bytes.extend(canonical_json(&value)?);
    }
    Ok(bytes)
}

fn event_value(event: &crate::events::ProtocolEvent) -> ContractResult<Value> {
    Ok(serde_json::to_value(event)?)
}

fn refresh_anvil_receipt_body_digest(value: &mut Value) -> ContractResult<()> {
    let mut receipt_value = value.clone();
    receipt_value
        .as_object_mut()
        .expect("receipt event fixture must be an object")
        .remove("type");
    let mut receipt: AnvilReceipt = serde_json::from_value(receipt_value)?;
    receipt.receipt_body_digest = anvil_receipt_body_digest(&receipt)?;
    value["receipt_body_digest"] = json!(receipt.receipt_body_digest);
    Ok(())
}

fn inferred_schema(value: &Value) -> Value {
    match value {
        Value::Null => json!({"type": "null"}),
        Value::Bool(_) => json!({"type": "boolean"}),
        Value::Number(number) if number.is_i64() || number.is_u64() => {
            json!({"type": "integer"})
        }
        Value::Number(_) => json!({"type": "number"}),
        Value::String(_) => json!({"type": "string"}),
        Value::Array(values) => {
            let mut item_schemas = Vec::new();
            for value in values {
                let schema = inferred_schema(value);
                if !item_schemas.contains(&schema) {
                    item_schemas.push(schema);
                }
            }
            // `anyOf`, NOT `oneOf`. Every object schema this function infers is
            // permissive by construction — `additionalProperties: true` with no
            // `required` (see the `Value::Object` arm below) — so no two of them are
            // ever mutually exclusive: any object matching one matches them all.
            // `oneOf` demands EXACTLY one branch, so a `oneOf` over inferred object
            // schemas is unsatisfiable by construction, and the published schema then
            // rejects the very fixture it was inferred from. That is not theoretical:
            // `goal_snapshot`'s `goal.tasks` array has two distinct task shapes, and
            // the emitted `oneOf` made `core-event.schema.json` reject
            // `events/goal_snapshot.json`, so any host validating against our own
            // published contract rejected a valid Core frame.
            //
            // This is inference, not assertion — it describes the shapes that were
            // observed, and claims no exclusivity between them. `anyOf` says exactly
            // that. The genuinely exclusive unions are built elsewhere and keep
            // `oneOf`: `nullable_schema()` (a value or null) and the `Ok`/`Err`
            // disposition in `provider_failover_receipt_schema()`, both of which pin
            // `required` and `additionalProperties: false` and so really are disjoint.
            let items = match item_schemas.as_slice() {
                [] => json!({}),
                [schema] => schema.clone(),
                _ => json!({"anyOf": item_schemas}),
            };
            json!({"items": items, "type": "array"})
        }
        Value::Object(object) => {
            let properties = object
                .iter()
                .map(|(field, value)| (field.clone(), inferred_schema(value)))
                .collect::<serde_json::Map<_, _>>();
            json!({
                "additionalProperties": true,
                "properties": properties,
                "type": "object"
            })
        }
    }
}

fn constrained_property_schema(wire_type: &str, field: &str, value: &Value) -> Value {
    match (wire_type, field) {
        (_, "type") => json!({"const": wire_type}),
        ("continue_with_budget" | "budget_grant_result", "additional_tokens") => {
            json!({"minimum": 0, "maximum": u64::MAX, "type": "integer"})
        }
        ("continue_with_budget" | "budget_grant_result", "additional_cost_usd") => {
            json!({"minimum": 0, "type": "number"})
        }
        (
            "session_resync"
            | "resume_turn"
            | "resolve_interrupted_approval"
            | "resolve_unknown_tool_effect"
            | "session_recovery_snapshot"
            | "session_recovery_replay"
            | "session_recovery_unavailable"
            | "turn_recovery_lifecycle"
            | "unknown_tool_effect_resolved",
            "recovery_version",
        ) => json!({"const": 1, "type": "integer"}),
        ("get_runtime_diagnostics" | "runtime_diagnostics_snapshot", "diagnostics_version") => {
            json!({"const": 1, "type": "integer"})
        }
        ("runtime_diagnostics_unavailable", "diagnostics_version") => {
            json!({"minimum": 0, "maximum": 65535, "type": "integer"})
        }
        ("runtime_diagnostics_unavailable", "supported_version") => {
            json!({"const": 1, "type": "integer"})
        }
        ("runtime_diagnostics_unavailable", "reason") => {
            json!({"enum": ["unsupported_version", "invalid_request"], "type": "string"})
        }
        // `ready`'s correlation key, always present, `null` when this run has
        // no durable session. Inference would read the ONE `events/ready.json`
        // fixture, see a string, and publish `{"type": "string"}` — a schema
        // that rejects the degraded frame Core really emits, i.e. the published
        // contract calling a valid Core frame malformed.
        ("ready", "session_id") => nullable_schema(json!({"type": "string"})),
        // CLOSED, deliberately: a future value must not be able to arrive as
        // free text and be accepted by a host that has never heard of it. The
        // cost of that choice is that widening the vocabulary is a contract
        // event, which is why `session_persistence_v2` exists below — the
        // widening is announced rather than smuggled.
        //
        // `disabled_by_host` stays in the enum although this producer can no
        // longer emit it. The schema is what a host validates FRAMES against,
        // and an older Core still sends that value; dropping it would make the
        // published contract call a genuine 0.12.x frame malformed. Emission
        // and acceptance are different questions and this is the one where the
        // legacy value still belongs.
        ("ready", "session_persistence") => json!({
            "enum": [
                "durable",
                "journaled_without_replay",
                "disabled_by_operator",
                "disabled_by_host"
            ],
            "type": "string"
        }),
        // CLOSED, for exactly the reason `ready.session_persistence` above is:
        // a future value must not arrive as free text and be accepted by a
        // host that has never heard of it and cannot render it. The enum is
        // generated from `RenderMime::all()` so the schema can never drift
        // from the type that produces the frames.
        ("render_artifact", "mime") => json!({
            "enum": crate::events::RenderMime::all(),
            "type": "string"
        }),
        // The classification is the whole forward-compat mechanism: a host
        // that does not know `render_artifact` drops it BECAUSE the frame says
        // false. Pinning the const means a producer that ever emitted `true`
        // would fail its own published schema instead of silently costing a
        // host its connection.
        ("render_artifact", "critical") => json!({"const": false, "type": "boolean"}),
        ("render_artifact", "title") => json!({
            "maxLength": crate::events::RENDER_ARTIFACT_TITLE_LIMIT_BYTES,
            "type": "string"
        }),
        ("continue_with_budget" | "budget_grant_result", "request_id") => json!({
            "minLength": 1,
            "maxLength": BUDGET_GRANT_REQUEST_ID_MAX_BYTES,
            "pattern": BUDGET_GRANT_REQUEST_ID_PATTERN,
            "type": "string"
        }),
        (
            "get_runtime_diagnostics"
            | "runtime_diagnostics_snapshot"
            | "runtime_diagnostics_unavailable",
            "request_id",
        ) => {
            json!({"minLength": 1, "maxLength": 128, "type": "string"})
        }
        ("remove_mcp_server" | "mcp_removal_result", "request_id" | "name") => {
            json!({
                "minLength": 1,
                "maxLength": 256,
                "type": "string",
                "x-maxUtf8Bytes": 256
            })
        }
        ("remove_mcp_server" | "mcp_removal_result", "lifecycle_version") => {
            json!({"minimum": 0, "maximum": 65535, "type": "integer"})
        }
        ("resume_turn", "action") => {
            json!({"enum": ["continue", "reconcile", "cancel", "abandon"], "type": "string"})
        }
        ("resolve_interrupted_approval", "decision") => {
            json!({"enum": ["approve", "deny"], "type": "string"})
        }
        // CLOSED on the wire (`PathGrantAccess`), so closed in the published
        // schema too. Inference would read `"read"` from the fixture and
        // publish `{"type": "string"}` - a contract that calls a frame valid
        // which the deserializer rejects outright.
        ("grant_path", "access") => {
            json!({"enum": ["read", "write"], "type": "string"})
        }
        ("session_recovery_snapshot" | "turn_recovery_lifecycle", "lifecycle") => {
            recovery_lifecycle_schema()
        }
        ("session_recovery_unavailable", "reason") => json!({
            "enum": [
                "session_not_found",
                "unsupported_version",
                "cursor_invalid",
                "cursor_ahead",
                "cursor_digest_mismatch",
                "history_gap",
                "journal_corrupt",
                "snapshot_unavailable",
                "unknown_critical_state"
            ],
            "type": "string"
        }),
        ("session_recovery_snapshot" | "turn_recovery_lifecycle", "reconcile_reason") => {
            recovery_reconcile_reason_schema()
        }
        ("session_recovery_snapshot", "state_digest") => raw_recovery_digest_schema(),
        ("resolve_unknown_tool_effect" | "unknown_tool_effect_resolved", "outcome") => {
            operator_resolution_outcome_schema()
        }
        ("set_mode", "mode") => json!({
            "enum": [
                "default",
                "auto_edit",
                "force",
                "yolo",
                "dangerously_skip_permissions",
                "dangerously-skip-permissions"
            ],
            "type": "string"
        }),
        ("tool_approve", "scope") => json!({
            "oneOf": [
                {"enum": ["once", "always"], "type": "string"},
                {
                    "additionalProperties": false,
                    "properties": {
                        "always_prefix": {
                            "additionalProperties": false,
                            "properties": {"prefix": {"type": "string"}},
                            "required": ["prefix"],
                            "type": "object"
                        }
                    },
                    "required": ["always_prefix"],
                    "type": "object"
                },
                // The published schema has to ADMIT `always_path`, or a host
                // that validates its own outgoing commands against this file
                // (Desktop does, and fails closed) could never send the scope
                // that `path_grants_v1: available` tells it is supported. A
                // capability the schema forbids is not a capability.
                {
                    "additionalProperties": false,
                    "properties": {
                        "always_path": {
                            "additionalProperties": false,
                            "properties": {
                                "root": {"type": "string"},
                                "write": {"type": "boolean"}
                            },
                            "required": ["root"],
                            "type": "object"
                        }
                    },
                    "required": ["always_path"],
                    "type": "object"
                }
            ]
        }),
        ("stream_end", "finish_reason") => {
            json!({"enum": ["stop", "length", "error", "max_turns"], "type": "string"})
        }
        ("tool_result", "status") => {
            json!({"enum": ["success", "error"], "type": "string"})
        }
        ("tool_result", "output_type") => {
            json!({"enum": ["text", "diff", "image"], "type": "string"})
        }
        ("budget_grant_result", "outcome") => {
            json!({"enum": ["granted", "refused"], "type": "string"})
        }
        ("budget_grant_result", "refusal_reason") => json!({
            "enum": [
                "host_not_authorized",
                "managed_policy",
                "no_exhausted_budget",
                "invalid_grant",
                "budget_tracker_unavailable",
                "persistence_failure",
                "request_id_conflict",
                "ledger_capacity_exceeded",
                "turn_in_progress"
            ],
            "type": "string"
        }),
        ("execution_policy", "reason") => json!({
            "enum": ["launch", "mode_change", "resume", "expiry"],
            "type": "string"
        }),
        ("execution_policy", "critical") => json!({"const": true, "type": "boolean"}),
        ("workflow_node_event", "state") => json!({
            "enum": ["queued", "running", "succeeded", "failed", "blocked"],
            "type": "string"
        }),
        ("workflow_finished", "terminal_state") | ("sub_agent_event", "terminal_state") => {
            json!({"enum": ["succeeded", "failed"], "type": "string"})
        }
        ("anvil_receipt_invalidated", "reason") => json!({
            "enum": ["artifact_mutated", "gate_revoked", "superseded"],
            "type": "string"
        }),
        ("anvil_receipt", "terminal_state") => {
            json!({"const": "verified", "type": "string"})
        }
        ("anvil_receipt", "origin") | ("anvil_receipt_invalidated", "origin") => {
            json!({"const": "core/anvil", "type": "string"})
        }
        ("anvil_receipt", "digest_algorithm") => {
            json!({"const": "sha256", "type": "string"})
        }
        _ => inferred_schema(value),
    }
}

fn prefixed_sha256_digest_schema() -> Value {
    json!({"pattern": "^sha256:[0-9a-f]{64}$", "type": "string"})
}

fn raw_recovery_digest_schema() -> Value {
    json!({"pattern": "^[0-9a-f]{64}$", "type": "string"})
}

fn recovery_cursor_schema() -> Value {
    json!({
        "additionalProperties": false,
        "properties": {
            "journal_digest": raw_recovery_digest_schema(),
            "journal_sequence": {"type": "integer"}
        },
        "required": ["journal_digest"],
        "type": "object"
    })
}

fn operator_resolution_cursor_schema() -> Value {
    json!({
        "additionalProperties": false,
        "properties": {
            "journal_digest": raw_recovery_digest_schema(),
            "journal_sequence": {"type": "integer"}
        },
        "required": ["journal_digest"],
        "type": "object"
    })
}

fn recovery_lifecycle_schema() -> Value {
    json!({
        "enum": [
            "ready",
            "streaming",
            "awaiting_approval",
            "tool_in_flight",
            "reconciliation_required",
            "suspended",
            "completed",
            "cancelled",
            "failed"
        ],
        "type": "string"
    })
}

fn recovery_reconcile_reason_schema() -> Value {
    json!({
        "enum": [
            "approval_expired",
            "provider_outcome_unknown",
            "tool_outcome_unknown",
            "effect_requires_operator",
            "budget_exhausted",
            "context_unrestorable",
            "cancellation_ambiguous",
            "unknown_critical_state"
        ],
        "type": "string"
    })
}

fn recovery_turn_snapshot_schema() -> Value {
    json!({
        "additionalProperties": true,
        "properties": {
            "lifecycle": recovery_lifecycle_schema(),
            "msg_id": {"type": "string"},
            "pending_call_id": {"type": "string"},
            "reconcile_reason": recovery_reconcile_reason_schema(),
            "turn_id": {"type": "string"}
        },
        "required": ["turn_id", "lifecycle"],
        "type": "object"
    })
}

fn recovery_budget_schema() -> Value {
    json!({
        "additionalProperties": true,
        "properties": {
            "cost_limit_usd": {"type": "number"},
            "cost_used_usd": {"type": "number"},
            "token_limit": {"type": "integer"},
            "tokens_used": {"type": "integer"}
        },
        "required": ["tokens_used", "cost_used_usd"],
        "type": "object"
    })
}

fn recovery_replay_item_schema() -> Value {
    json!({
        "additionalProperties": true,
        "properties": {
            "cursor": recovery_cursor_schema(),
            "kind": {
                "enum": [
                    "state_advanced",
                    "turn_started",
                    "stream_started",
                    "stream_committed",
                    "approval_requested",
                    "approval_resolved",
                    "tool_started",
                    "tool_committed",
                    "effect_uncertain",
                    "cancellation_requested",
                    "turn_completed",
                    "turn_cancelled",
                    "turn_failed"
                ],
                "type": "string"
            },
            "turn_id": {"type": "string"}
        },
        "required": ["cursor", "kind"],
        "type": "object"
    })
}

fn operator_resolution_outcome_schema() -> Value {
    json!({
        "enum": ["succeeded", "failed", "not_started"],
        "type": "string"
    })
}

fn operator_resolution_evidence_schema() -> Value {
    json!({
        "additionalProperties": false,
        "properties": {
            "digest": prefixed_sha256_digest_schema(),
            "observed_at_unix_ms": {"minimum": 1, "type": "integer"},
            "reference_id": {"maxLength": 256, "minLength": 1, "type": "string"},
            "source": {
                "enum": [
                    "tool_receipt",
                    "provider_receipt",
                    "process_observation",
                    "external_system_record"
                ],
                "type": "string"
            }
        },
        "required": ["source", "reference_id", "observed_at_unix_ms", "digest"],
        "type": "object"
    })
}

fn workflow_failure_schema() -> Value {
    json!({
        "additionalProperties": true,
        "properties": {
            "code": {"type": "string"},
            "message": {"type": "string"},
            "retryable": {"type": "boolean"}
        },
        "required": ["code", "message", "retryable"],
        "type": "object"
    })
}

fn contract_descriptor_schema() -> Value {
    json!({
        "additionalProperties": true,
        "properties": {
            "capabilities": {
                "additionalProperties": {
                    "enum": ["available", "publication_bound", "shape_only", "unavailable"],
                    "type": "string"
                },
                "type": "object"
            },
            "fixture_digest": {"type": "string"},
            "generator": {"type": "string"},
            "major": {"type": "integer"},
            "minor": {"type": "integer"},
            "name": {"type": "string"},
            "schema_digest": {"type": "string"},
            "source_inputs_digest": {"type": "string"}
        },
        "required": [
            "name",
            "major",
            "minor",
            "generator",
            "fixture_digest",
            "schema_digest",
            "source_inputs_digest",
            "capabilities"
        ],
        "type": "object"
    })
}

fn effective_execution_policy_schema() -> Value {
    json!({
        "additionalProperties": true,
        "properties": {
            "approvals": {"enum": ["prompt", "auto_edit", "bypass"], "type": "string"},
            "dangerous_activation_id": {"type": "string"},
            "dangerous_expires_at_unix_ms": {"type": "integer"},
            "managed_floor_active": {"type": "boolean"},
            "posture": {"enum": ["smart", "managed", "dangerous"], "type": "string"},
            "sandbox": {"enum": ["required", "bypass"], "type": "string"},
            "source": {
                "enum": [
                    "default",
                    "managed",
                    "user_config",
                    "project",
                    "environment",
                    "local_cli_launch",
                    "desktop_local_launch",
                    "protocol",
                    "acp",
                    "tui",
                    "resume",
                    "child"
                ],
                "type": "string"
            }
        },
        "required": ["posture", "approvals", "sandbox", "source", "managed_floor_active"],
        "type": "object"
    })
}

fn runtime_diagnostics_snapshot_schema() -> Value {
    json!({
        "additionalProperties": false,
        "properties": {
            "process": {
                "additionalProperties": false,
                "properties": {
                    "profile_binding": {"enum": ["unknown", "default_home", "explicit_home", "bound_profile", "unbound_profile"], "type": "string"},
                    "profile_name": {"type": "string"},
                    "engine_mode": {"enum": ["unknown", "standard", "raw"], "type": "string"},
                    "workspace_kind": {"enum": ["unknown", "none", "project", "temporary", "profile_home"], "type": "string"}
                },
                "required": ["profile_binding", "engine_mode", "workspace_kind"],
                "type": "object"
            },
            "config_sources": {
                "items": {
                    "additionalProperties": false,
                    "properties": {
                        "role": {"enum": ["global", "project", "profile", "cli", "environment", "credential_store", "desktop_launch"], "type": "string"},
                        "disposition": {"enum": ["loaded", "absent", "ignored", "unreadable", "invalid", "overridden", "restricted"], "type": "string"},
                        "precedence": {"minimum": 0, "maximum": 65535, "type": "integer"},
                        "display_path": {"type": "string"},
                        "content_digest": prefixed_sha256_digest_schema()
                    },
                    "required": ["role", "disposition", "precedence"],
                    "type": "object"
                },
                "type": "array"
            },
            "unsupported_overrides": {
                "items": {
                    "additionalProperties": false,
                    "properties": {
                        "name": {"type": "string"},
                        "disposition": {"enum": ["loaded", "absent", "ignored", "unreadable", "invalid", "overridden", "restricted"], "type": "string"}
                    },
                    "required": ["name", "disposition"],
                    "type": "object"
                },
                "type": "array"
            },
            "mcp_servers": {
                "items": {
                    "additionalProperties": false,
                    "properties": {
                        "name": {"type": "string"},
                        "origin": {"enum": ["effective_config", "global_config", "project_config", "profile_config", "runtime_command", "plugin"], "type": "string"},
                        "transport": {"enum": ["stdio", "sse", "streamable_http"], "type": "string"},
                        "connection": {"enum": ["configured", "deferred", "connecting", "ready", "failed", "timed_out", "skipped", "stopping", "stopped"], "type": "string"},
                        "exposure": {"enum": ["not_attempted", "not_applicable", "exposed", "resource_only", "resource_only_unavailable", "hidden_no_tools", "blocked"], "type": "string"},
                        "deferred": {"type": "boolean"},
                        "tool_count": {"minimum": 0, "maximum": 4294967295_u64, "type": "integer"},
                        "resources_declared": {"type": "boolean"},
                        "resources_exposed": {"type": "boolean"},
                        "assistant_scoped": {"type": "boolean"},
                        "executable_basename": {"type": "string"},
                        "executable_readiness": {"enum": ["not_applicable", "unchecked", "resolved", "missing_effective_path", "not_found", "invalid_absolute_path", "invalid_executable", "invalid_effective_environment", "permission_denied", "not_executable", "probe_timed_out", "unsupported_transport"], "type": "string"},
                        "working_directory": {"enum": ["inherited_process", "project_root", "profile_home", "explicit"], "type": "string"},
                        "failure": {"enum": ["missing_executable", "launch_failed", "connection_refused", "timeout", "protocol_mismatch", "authentication_required", "authorization_denied", "invalid_configuration", "transport_closed", "unknown"], "type": "string"},
                        "remediation": {"items": {"enum": ["open_active_config", "restart_desktop", "fix_gui_launch_path", "install_executable", "fix_executable_permissions", "review_server_config", "retry_connection", "retry_diagnostics", "check_assistant_scope", "restart_to_load_resources"], "type": "string"}, "type": "array"}
                    },
                    "required": ["name", "origin", "transport", "connection", "exposure", "deferred", "tool_count", "resources_declared", "resources_exposed", "assistant_scoped", "executable_readiness", "working_directory", "remediation"],
                    "type": "object"
                },
                "type": "array"
            }
        },
        "required": ["process", "config_sources", "unsupported_overrides", "mcp_servers"],
        "type": "object"
    })
}

fn execution_policy_snapshot_schema() -> Value {
    json!({
        "additionalProperties": true,
        "properties": {
            "contract_version": {"type": "string"},
            "critical": {"const": true, "type": "boolean"},
            "effective_at_unix_ms": {"type": "integer"},
            "policy": effective_execution_policy_schema(),
            "reason": {
                "enum": ["launch", "mode_change", "resume", "expiry"],
                "type": "string"
            },
            "revision": {"type": "integer"}
        },
        "required": [
            "critical",
            "contract_version",
            "revision",
            "reason",
            "effective_at_unix_ms",
            "policy"
        ],
        "type": "object"
    })
}

fn child_terminal_conditions() -> Value {
    json!([
        {
            "if": {
                "properties": {"terminal_state": {"const": "succeeded"}},
                "required": ["terminal_state"]
            },
            "then": {
                "properties": {
                    "inner": {
                        "additionalProperties": true,
                        "properties": {
                            "message": {"type": "string"},
                            "msg_id": {"minLength": 1, "type": "string"},
                            "type": {"const": "info"}
                        },
                        "required": ["type", "msg_id", "message"],
                        "type": "object"
                    }
                }
            }
        },
        {
            "if": {
                "properties": {"terminal_state": {"const": "failed"}},
                "required": ["terminal_state"]
            },
            "then": {
                "properties": {
                    "inner": {
                        "additionalProperties": true,
                        "properties": {
                            "error": {
                                "additionalProperties": true,
                                "properties": {
                                    "code": {"type": "string"},
                                    "message": {"type": "string"},
                                    "retryable": {"type": "boolean"}
                                },
                                "required": ["code", "message", "retryable"],
                                "type": "object"
                            },
                            "type": {"const": "error"}
                        },
                        "required": ["type", "error"],
                        "type": "object"
                    }
                }
            }
        }
    ])
}

fn failover_reason_schema() -> Value {
    json!({
        "enum": [
            "auth",
            "auth_permanent",
            "format",
            "rate_limit",
            "overloaded",
            "billing",
            "timeout",
            "model_not_found",
            "session_expired",
            "context_overflow",
            "unknown"
        ],
        "type": "string"
    })
}

fn nullable_schema(schema: Value) -> Value {
    json!({"oneOf": [schema, {"type": "null"}]})
}

fn provider_failover_receipt_schema() -> Value {
    let disposition = json!({
        "oneOf": [
            {
                "additionalProperties": false,
                "properties": {"Ok": {"type": "null"}},
                "required": ["Ok"],
                "type": "object"
            },
            {
                "additionalProperties": false,
                "properties": {
                    "Err": {
                        "enum": [
                            "provider_not_allowed",
                            "provider_denied",
                            "region_not_allowed",
                            "organization_mismatch",
                            "tools_unsupported",
                            "vision_unsupported",
                            "structured_output_unsupported",
                            "context_window_unknown",
                            "context_window_too_small",
                            "pricing_stale",
                            "pricing_unavailable",
                            "cooldown_active",
                            "budget_denied"
                        ],
                        "type": "string"
                    }
                },
                "required": ["Err"],
                "type": "object"
            }
        ]
    });
    let pricing = json!({
        "additionalProperties": false,
        "properties": {
            "source": {"type": "string"},
            "age_seconds": nullable_schema(json!({"minimum": 0, "type": "integer"})),
            "stale": {"type": "boolean"},
            "priced": {"type": "boolean"},
            "estimated_microcents": nullable_schema(json!({"minimum": 0, "type": "integer"}))
        },
        "required": ["source", "age_seconds", "stale", "priced", "estimated_microcents"],
        "type": "object"
    });
    let candidate = json!({
        "additionalProperties": false,
        "properties": {
            "provider": {"type": "string"},
            "model": {"type": "string"},
            "region": nullable_schema(json!({"type": "string"})),
            "disposition": disposition,
            "failure_reason": nullable_schema(failover_reason_schema()),
            "cooldown_reason": nullable_schema(failover_reason_schema()),
            "retry_after_ms": nullable_schema(json!({"minimum": 0, "type": "integer"})),
            "pricing": pricing
        },
        "required": [
            "provider",
            "model",
            "region",
            "disposition",
            "failure_reason",
            "cooldown_reason",
            "retry_after_ms",
            "pricing"
        ],
        "type": "object"
    });
    json!({
        "additionalProperties": false,
        "properties": {
            "reason": failover_reason_schema(),
            "failed_provider": {"type": "string"},
            "failed_model": {"type": "string"},
            "candidates": {"items": candidate, "type": "array"},
            "selected_provider": nullable_schema(json!({"type": "string"})),
            "selected_model": nullable_schema(json!({"type": "string"}))
        },
        "required": [
            "reason",
            "failed_provider",
            "failed_model",
            "candidates",
            "selected_provider",
            "selected_model"
        ],
        "type": "object"
    })
}

fn schema_branch(spec: &WireSpec, fixture: &Value) -> Value {
    let object = fixture
        .as_object()
        .expect("canonical contract fixture must be an object");
    let mut properties = object
        .iter()
        .map(|(field, value)| {
            (
                field.clone(),
                constrained_property_schema(spec.wire_type, field, value),
            )
        })
        .collect::<serde_json::Map<_, _>>();
    match spec.wire_type {
        "ready" => {
            properties
                .entry("contract")
                .or_insert_with(contract_descriptor_schema);
            properties
                .entry("execution_policy")
                .and_modify(|schema| *schema = execution_policy_snapshot_schema());
        }
        "execution_policy" => {
            properties
                .entry("policy")
                .and_modify(|schema| *schema = effective_execution_policy_schema());
        }
        "runtime_diagnostics_snapshot" => {
            properties
                .entry("snapshot")
                .and_modify(|schema| *schema = runtime_diagnostics_snapshot_schema());
        }
        "session_resync" => {
            properties
                .entry("after")
                .and_modify(|schema| *schema = recovery_cursor_schema());
        }
        "resume_turn" => {
            properties
                .entry("cursor")
                .and_modify(|schema| *schema = recovery_cursor_schema());
        }
        "resolve_interrupted_approval" => {
            properties
                .entry("cursor")
                .and_modify(|schema| *schema = recovery_cursor_schema());
        }
        "resolve_unknown_tool_effect" | "unknown_tool_effect_resolved" => {
            properties
                .entry("cursor")
                .and_modify(|schema| *schema = operator_resolution_cursor_schema());
            properties
                .entry("evidence")
                .and_modify(|schema| *schema = operator_resolution_evidence_schema());
        }
        "session_recovery_snapshot" => {
            properties
                .entry("cursor")
                .and_modify(|schema| *schema = recovery_cursor_schema());
            properties
                .entry("pending_turn")
                .and_modify(|schema| *schema = recovery_turn_snapshot_schema());
            properties
                .entry("budget")
                .and_modify(|schema| *schema = recovery_budget_schema());
        }
        "session_recovery_replay" => {
            properties
                .entry("from")
                .and_modify(|schema| *schema = recovery_cursor_schema());
            properties
                .entry("through")
                .and_modify(|schema| *schema = recovery_cursor_schema());
            properties.entry("items").and_modify(|schema| {
                *schema = json!({"items": recovery_replay_item_schema(), "type": "array"});
            });
        }
        "turn_recovery_lifecycle" => {
            properties
                .entry("cursor")
                .and_modify(|schema| *schema = recovery_cursor_schema());
        }
        "workflow_started" => {
            properties
                .entry("parent_run_id")
                .or_insert_with(|| json!({"type": "string"}));
        }
        "workflow_node_event" | "workflow_finished" => {
            properties
                .entry("failure")
                .or_insert_with(workflow_failure_schema);
        }
        "sub_agent_event" => {
            properties
                .entry("parent_child_run_id")
                .or_insert_with(|| json!({"type": "string"}));
            properties
                .entry("terminal_state")
                .or_insert_with(|| json!({"enum": ["succeeded", "failed"], "type": "string"}));
        }
        "provider_failover_receipt" => {
            properties
                .entry("receipt")
                .and_modify(|schema| *schema = provider_failover_receipt_schema());
        }
        "anvil_receipt" => {
            properties
                .entry("supersedes_receipt_id")
                .or_insert_with(|| json!({"type": "string"}));
        }
        "anvil_receipt_invalidated" => {
            properties
                .entry("observed_artifact_digest")
                .or_insert_with(|| json!({"type": "string"}));
        }
        "budget_grant_result" => {
            properties.entry("refusal_reason").or_insert_with(|| {
                constrained_property_schema(
                    "budget_grant_result",
                    "refusal_reason",
                    &Value::String(String::new()),
                )
            });
        }
        _ => {}
    }
    let mut branch = json!({
        "additionalProperties": true,
        "properties": properties,
        "required": spec.required,
        "type": "object"
    });
    if spec.wire_type == "sub_agent_event" {
        branch["allOf"] = child_terminal_conditions();
    }
    if spec.wire_type == "continue_with_budget" {
        branch["anyOf"] = json!([
            {
                "properties": {"additional_tokens": {"minimum": 1}},
                "required": ["additional_tokens"]
            },
            {
                "properties": {"additional_cost_usd": {"exclusiveMinimum": 0}},
                "required": ["additional_cost_usd"]
            }
        ]);
    }
    if spec.wire_type == "budget_grant_result" {
        branch["allOf"] = json!([
            {
                "if": {
                    "properties": {"outcome": {"const": "granted"}},
                    "required": ["outcome"]
                },
                "then": {"not": {"required": ["refusal_reason"]}}
            },
            {
                "if": {
                    "properties": {"outcome": {"const": "refused"}},
                    "required": ["outcome"]
                },
                "then": {"required": ["refusal_reason"]}
            }
        ]);
    }
    if matches!(
        spec.wire_type,
        "continue_with_budget"
            | "budget_grant_result"
            | "session_resync"
            | "resume_turn"
            | "resolve_interrupted_approval"
            | "resolve_unknown_tool_effect"
            | "unknown_tool_effect_resolved"
            | "get_runtime_diagnostics"
            | "runtime_diagnostics_snapshot"
            | "remove_mcp_server"
            | "mcp_removal_result"
    ) {
        branch["additionalProperties"] = json!(false);
    }
    branch
}

/// Build the published schema and, beside it, the per-wire-type shape branches
/// keyed by fixture path.
///
/// The branches are the version-independent half of the schema: the document
/// title carries `vMAJOR.MINOR`, the branches carry only structure. That split
/// is what lets `wire_shape_refusal` ask "did the shape move?" without the
/// question being confounded by a version bump that moved the title.
fn schema_for(
    specs: &[WireSpec],
    fixtures: &BTreeMap<String, Value>,
    legacy_child: Option<&Value>,
    title: &str,
) -> (Value, BTreeMap<String, Value>) {
    let mut one_of = Vec::with_capacity(specs.len() + 1);
    let mut shapes = BTreeMap::new();
    for spec in specs {
        let fixture = fixtures
            .get(spec.path)
            .unwrap_or_else(|| panic!("missing canonical fixture {}", spec.path));
        let branch = schema_branch(spec, fixture);
        shapes.insert(spec.path.to_string(), branch.clone());
        one_of.push(branch);
        if spec.wire_type == "sub_agent_event" {
            let legacy = legacy_child.expect("legacy sub-agent fixture must be present");
            let legacy_spec = WireSpec {
                wire_type: "sub_agent_event",
                path: "compat/events/sub_agent_event.legacy.json",
                required: &["type", "parent_call_id", "agent_name", "inner"],
                criticality: spec.criticality,
                correlation: spec.correlation,
                capability: spec.capability,
            };
            let mut legacy_branch = schema_branch(&legacy_spec, legacy);
            legacy_branch["not"] = json!({
                "anyOf": [
                    {"required": ["run_id"]},
                    {"required": ["child_run_id"]},
                    {"required": ["child_sequence"]},
                    {"required": ["event_id"]},
                    {"required": ["terminal_state"]}
                ]
            });
            legacy_branch["title"] =
                json!("Legacy non-authoritative sub-agent compatibility event");
            shapes.insert(legacy_spec.path.to_string(), legacy_branch.clone());
            one_of.push(legacy_branch);
        }
    }
    (
        json!({
            "$schema": "https://json-schema.org/draft/2020-12/schema",
            "oneOf": one_of,
            "title": title
        }),
        shapes,
    )
}

fn producer_complete_schema(command_schema: &Value, event_schema: &Value) -> Value {
    let mut one_of = command_schema["oneOf"]
        .as_array()
        .expect("command schema must contain oneOf")
        .clone();
    one_of.extend(
        event_schema["oneOf"]
            .as_array()
            .expect("event schema must contain oneOf")
            .iter()
            .cloned(),
    );
    let desktop_types = COMMAND_SPECS
        .iter()
        .chain(EVENT_SPECS)
        .map(|spec| spec.wire_type)
        .collect::<BTreeSet<_>>();
    let inventory_only = PRODUCER_COMMAND_TYPES
        .iter()
        .chain(PRODUCER_EVENT_TYPES)
        .copied()
        .filter(|wire_type| !desktop_types.contains(wire_type))
        .collect::<Vec<_>>();
    if !inventory_only.is_empty() {
        one_of.push(json!({
            "additionalProperties": true,
            "properties": {"type": {"enum": inventory_only}},
            "required": ["type"],
            "title": "Non-Desktop producer inventory discriminator",
            "type": "object"
        }));
    }
    json!({
        "$schema": "https://json-schema.org/draft/2020-12/schema",
        "anyOf": one_of,
        "title": "Complete current Core producer inventory"
    })
}

fn contract_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join(CONTRACT_ROOT)
}

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("wcore-protocol must remain inside the workspace crates directory")
        .to_path_buf()
}

fn source_digest() -> ContractResult<String> {
    let root = workspace_root();
    let mut sources = Vec::with_capacity(SOURCE_INPUTS.len());
    for relative in SOURCE_INPUTS {
        let bytes = fs::read(root.join(relative))?;
        sources.push((*relative, bytes));
    }
    Ok(digest_named_bytes(
        sources
            .iter()
            .map(|(path, bytes)| (*path, bytes.as_slice())),
    ))
}

fn specs_manifest(specs: &[WireSpec]) -> Vec<Value> {
    specs
        .iter()
        .map(|spec| {
            json!({
                "capability": spec.capability,
                "correlation": spec.correlation,
                "criticality": spec.criticality,
                "path": spec.path,
                "type": spec.wire_type
            })
        })
        .collect()
}

fn fixtures_digest(artifacts: &BTreeMap<String, Vec<u8>>) -> ContractResult<String> {
    let mut normalized = Vec::new();
    for (path, bytes) in artifacts {
        let included = ["commands/", "events/", "types/", "compat/", "adversarial/"]
            .iter()
            .any(|prefix| path.starts_with(prefix));
        if !included {
            continue;
        }
        let mut bytes = bytes.clone();
        if path == "events/ready.json"
            || path == "compat/events/ready.journaled-without-replay.json"
            || path == "compat/events/ready.disabled-by-host.legacy.json"
            || path == "adversarial/events/version-mismatch.jsonl"
            || path == "adversarial/events/schema-mismatch.jsonl"
            || path == "adversarial/events/fixture-mismatch.jsonl"
        {
            let mut value: Value = serde_json::from_slice(&bytes)?;
            if let Some(fixture_digest) = value
                .get_mut("contract")
                .and_then(Value::as_object_mut)
                .and_then(|contract| contract.get_mut("fixture_digest"))
            {
                *fixture_digest = Value::String(format!("sha256:{}", "0".repeat(64)));
                bytes = canonical_json(&value)?;
            }
        }
        normalized.push((path.as_str(), bytes));
    }
    Ok(digest_named_bytes(
        normalized
            .iter()
            .map(|(path, bytes)| (*path, bytes.as_slice())),
    ))
}

fn schemas_digest(artifacts: &BTreeMap<String, Vec<u8>>) -> String {
    digest_named_bytes(artifacts.iter().filter_map(|(path, bytes)| {
        path.starts_with("schema/")
            .then_some((path.as_str(), bytes.as_slice()))
    }))
}

/// One digest per published wire type, over the canonical bytes of its schema
/// branch.
///
/// Value-independent (a branch records types, enums and nesting, never fixture
/// values) and version-independent (a branch carries no title), so this moves
/// when, and only when, the shape a host validates against moves.
fn wire_shape_digests(
    shapes: impl IntoIterator<Item = (String, Value)>,
) -> ContractResult<BTreeMap<String, String>> {
    let mut digests = BTreeMap::new();
    for (path, branch) in shapes {
        let bytes = canonical_json(&branch)?;
        let digest = digest_named_bytes([(path.as_str(), bytes.as_slice())]);
        digests.insert(path, digest);
    }
    Ok(digests)
}

/// The wire shape surface the checked-in corpus published, and the contract
/// version it published it under.
struct PublishedWireShapes {
    major: u64,
    minor: u64,
    shapes: BTreeMap<String, String>,
}

/// Read the checked-in manifest's published wire shapes.
///
/// `None` means there is no baseline to compare against: the corpus is absent,
/// truncated, or predates `wire_shapes`. That is not a state a regeneration may
/// silently pass through - see `WireShapeBaseline`.
fn published_wire_shapes() -> ContractResult<Option<PublishedWireShapes>> {
    let Ok(bytes) = fs::read(contract_root().join("manifest.json")) else {
        return Ok(None);
    };
    let manifest: Value = serde_json::from_slice(&bytes)?;
    let (Some(published), Some(major), Some(minor)) = (
        manifest.get("wire_shapes").and_then(Value::as_object),
        manifest.pointer("/contract/major").and_then(Value::as_u64),
        manifest.pointer("/contract/minor").and_then(Value::as_u64),
    ) else {
        return Ok(None);
    };
    let shapes = published
        .iter()
        .filter_map(|(path, digest)| Some((path.clone(), digest.as_str()?.to_string())))
        .collect();
    Ok(Some(PublishedWireShapes {
        major,
        minor,
        shapes,
    }))
}

/// Whether a corpus that publishes no wire shapes may be regenerated at all.
///
/// A missing baseline is the one state in which the gate cannot do its job, so
/// it is an error rather than a skip: the commit that introduces `wire_shapes`
/// is exactly the commit a wire change is most likely to ride along in
/// unnoticed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WireShapeBaseline {
    /// The published corpus must carry a baseline. Everything except an
    /// operator typing the bootstrap flag uses this, `check_contract`
    /// included - CI has no legitimate bootstrap.
    Required,
    /// Permit a MISSING baseline, and nothing else. Where a baseline exists it
    /// is compared exactly as under `Required`, so the escape can create a
    /// contract root but can never bless a change to a shape already published.
    Bootstrap,
}

/// The whole gate decision, with the disk read hoisted out so every branch is
/// reachable from a test.
fn wire_shape_verdict(
    published: Option<&PublishedWireShapes>,
    current: &BTreeMap<String, String>,
    major: u64,
    minor: u64,
    baseline: WireShapeBaseline,
) -> Option<String> {
    match (published, baseline) {
        (Some(published), _) => wire_shape_refusal(published, current, major, minor),
        (None, WireShapeBaseline::Bootstrap) => None,
        (None, WireShapeBaseline::Required) => Some(
            "The checked-in Desktop contract corpus publishes no wire shapes, so this \
             regeneration has nothing to compare against and could bless a changed wire shape \
             under an unchanged contract version. That is legal only while a contract root is \
             being created: if that is what this is, run `wcore-contract generate \
             --bootstrap-wire-shapes` once and commit the manifest it writes, after which every \
             regeneration is gated. Otherwise contracts/desktop/v1 is truncated or hand-edited - \
             restore manifest.json from git rather than bypassing this."
                .to_string(),
        ),
    }
}

/// Refuse a corpus whose `approval_required` rows contradict the behaviour
/// `ProtocolEvent::ApprovalRequired` documents.
///
/// A corpus row is the only thing a real integrator reads, so a row that
/// disagrees with the engine is worse than no row: the Desktop lane built its
/// approval reply on `resume_token` because the shipped row carried
/// `"resume-001"` and the manifest named that field as the correlation key,
/// while the engine emits the EMPTY string there for every ordinary tool gate
/// and correlates on `call_id`. Answering such a gate with `approval_resume`
/// resolves nothing and the tool hangs until its TTL (wayland#1088).
///
/// Deliberately inside `generated_artifacts` rather than beside the wire-shape
/// gate: the wire-shape gate exists to make a BLESSED break impossible to
/// bless accidentally, whereas this is an invariant no corpus may ever hold,
/// so no caller — generator, checker, test or tool — should be able to
/// materialise one.
fn enforce_approval_gate_contract(
    events: &BTreeMap<String, Value>,
    compatibility: &BTreeMap<String, Value>,
) -> ContractResult<()> {
    let declared = EVENT_SPECS
        .iter()
        .find(|spec| spec.wire_type == "approval_required")
        .ok_or_else(|| std::io::Error::other("EVENT_SPECS must declare approval_required"))?;
    if declared.correlation != "call_id" {
        return Err(std::io::Error::other(format!(
            "approval_required declares correlation={:?}, but `resume_token` is the bridge \
                secret and is EMPTY on an ordinary tool gate. The public handle is `call_id` (`correlation_id` always equals it), and an ordinary gate is answered with \
                tool_approve/tool_deny keyed by call_id.",
            declared.correlation
        ))
        .into());
    }

    for (path, fixture) in events.iter().chain(compatibility.iter()) {
        if fixture.get("type").and_then(Value::as_str) != Some("approval_required") {
            continue;
        }
        let call_id = fixture
            .get("call_id")
            .and_then(Value::as_str)
            .ok_or_else(|| {
                std::io::Error::other(format!("{path}: approval_required must carry a call_id"))
            })?;
        // Omitted from the JSON when empty, so only a row that publishes it is
        // constrained - but a row that publishes a DIFFERENT value teaches a
        // host to correlate on the wrong field.
        if let Some(correlation_id) = fixture.get("correlation_id").and_then(Value::as_str)
            && correlation_id != call_id
        {
            return Err(std::io::Error::other(format!(
                "{path}: correlation_id {correlation_id:?} != call_id {call_id:?}. \
                 `ProtocolEvent::ApprovalRequired::correlation_id` always equals \
                 `call_id`."
            ))
            .into());
        }
    }

    // The canonical row is the case a host meets on nearly every session: an
    // ordinary tool gate, which has no bridge entry and therefore no token. The
    // bridge-backed kind keeps its own minimal compat row.
    let canonical = events.get("events/approval_required.json").ok_or_else(|| {
        std::io::Error::other("the corpus must publish events/approval_required.json")
    })?;
    let token = canonical
        .get("resume_token")
        .and_then(Value::as_str)
        .unwrap_or_default();
    if !token.is_empty() {
        return Err(std::io::Error::other(format!(
            "events/approval_required.json publishes resume_token={token:?}, but the canonical \
                row is an ordinary tool gate and an ordinary gate has NO bridge entry, so its \
                resume_token is the empty string. A host that echoes a token it read here answers \
                with approval_resume, which resolves nothing, and the tool hangs until its TTL."
        ))
        .into());
    }
    Ok(())
}

/// Refuse a regeneration that would move a published wire shape while the
/// contract version stands still.
///
/// `contract.major`/`contract.minor` is the only compatibility signal a pinned
/// Desktop build reads, and regeneration does not move it. Without this gate a
/// renamed correlation key - `tool_request.call_id`, say - regenerates straight
/// back to green under an unchanged `1.16`, and the host that pinned `1.16`
/// then accepts frames it cannot correlate: every tool call renders as an
/// orphan, with no version error anywhere to explain it.
///
/// The baseline is the checked-in corpus, not a hand-maintained constant, so
/// the only way past the gate is the version decision itself.
fn wire_shape_refusal(
    published: &PublishedWireShapes,
    current: &BTreeMap<String, String>,
    major: u64,
    minor: u64,
) -> Option<String> {
    let altered = published
        .shapes
        .iter()
        .filter(|(path, digest)| current.get(path.as_str()).is_some_and(|now| now != *digest))
        .map(|(path, _)| path.as_str())
        .collect::<Vec<_>>();
    let removed = published
        .shapes
        .keys()
        .filter(|path| !current.contains_key(*path))
        .map(String::as_str)
        .collect::<Vec<_>>();
    let added = current
        .keys()
        .filter(|path| !published.shapes.contains_key(*path))
        .map(String::as_str)
        .collect::<Vec<_>>();
    if altered.is_empty() && removed.is_empty() && added.is_empty() {
        return None;
    }
    if (major, minor) > (published.major, published.minor) {
        return None;
    }
    let (was_major, was_minor) = (published.major, published.minor);
    // The justification has to match the finding. On an added-only refusal
    // nothing a pinned host already correlates or renders has moved, and
    // telling the author otherwise points them at CONTRACT_MAJOR for a change
    // that is additive - #314 walked into exactly that sentence.
    let consequence = if altered.is_empty() && removed.is_empty() {
        format!(
            "a host pinned to {was_major}.{was_minor} has no way to learn the added wire \
             types exist: the version has to move for the addition to be discoverable"
        )
    } else {
        format!(
            "a host pinned to {was_major}.{was_minor} would accept an engine whose frames it \
             can no longer correlate or render"
        )
    };
    Some(format!(
        "Desktop contract wire shape changed while the contract version stayed at \
         {was_major}.{was_minor}: altered={altered:?}, removed={removed:?}, added={added:?}. \
         Regenerating cannot bless this. `contract.major`/`contract.minor` in manifest.json is \
         the only compatibility signal a pinned Desktop build reads and regeneration does not \
         move it, so {consequence}. Decide the version in \
         crates/wcore-protocol/src/contract/generate.rs first, then regenerate: bump \
         CONTRACT_MINOR for a new wire type or a new optional field on an existing one, bump \
         CONTRACT_MAJOR for a field renamed, removed, retyped or newly required, and move \
         GENERATOR_VERSION with it."
    ))
}

fn contract_capabilities() -> BTreeMap<String, ContractCapabilityStatus> {
    BTreeMap::from([
        (
            "anvil_receipts".into(),
            ContractCapabilityStatus::PublicationBound,
        ),
        ("browser_events".into(), ContractCapabilityStatus::ShapeOnly),
        (
            "contract_negotiation".into(),
            ContractCapabilityStatus::Available,
        ),
        ("cua_events".into(), ContractCapabilityStatus::ShapeOnly),
        (
            "effective_execution_policy_revisions".into(),
            ContractCapabilityStatus::Available,
        ),
        (
            "durable_child_model_v1".into(),
            ContractCapabilityStatus::Available,
        ),
        // #1129. Core splits inline reasoning out of the visible stream on the
        // JSON-stream path: `text_delta` carries answer text only, and the
        // `<think>`/`<thinking>`/`<reasoning>` bodies that open-weights models
        // inline in that same stream ride `thinking` instead. `Available`, not
        // `ShapeOnly`: both event types already existed and already round-trip
        // - what this declares is that the producer now populates them this
        // way, which is the part a host renders off.
        (
            "inline_reasoning_split_v1".into(),
            ContractCapabilityStatus::Available,
        ),
        // #326. `resume_turn` accepts a fourth action, `abandon`, which ends a
        // stuck turn permanently and tolerates the three disagreements
        // `cancel` refuses: a stale cursor, a session with nothing interrupted,
        // and a turn id the engine does not hold. `Available`, not `ShapeOnly`:
        // the enum value is published AND the dispatcher answers it, in the
        // same change. Declared because a pinned host validates its own
        // outbound frames against this corpus, so before the value is
        // published the host refuses it internally and the verb is
        // unreachable — which is the state FerroxLabs/wayland#1116 reports.
        (
            "turn_abandon_v1".into(),
            ContractCapabilityStatus::Available,
        ),
        // F22-C1. Promoted ShapeOnly -> Available in the SAME change that
        // added the five Goal control commands AND their dispatcher in
        // `crates/wcore-cli/src/main.rs`, never before it. The round trip a
        // host now completes is: `goal_open`/`goal_declare_task`/
        // `goal_advance`/`goal_cancel`/`goal_resync` in, `goal_snapshot` or
        // `goal_control_refused` out. While only the events existed this was
        // correctly ShapeOnly, because nothing answered a command.
        (
            "durable_goals_v1".into(),
            ContractCapabilityStatus::Available,
        ),
        // wayland#896. `Available`, not `ShapeOnly`, and only because the
        // dispatcher lands in the SAME change as the wire types: the round trip
        // a host completes is `quiesce_acquire`/`quiesce_release`/
        // `quiesce_status` in, `quiesce_lease_granted`/
        // `quiesce_lease_released`/`quiesce_lease_expired`/
        // `quiesce_status_report`/`quiesce_refused` out. Declaring it while
        // only the types existed would tell a host it can stop faking
        // quiescence with filesystem timestamps before anything answered.
        (
            "quiesced_snapshot_lease_v1".into(),
            ContractCapabilityStatus::Available,
        ),
        (
            "host_delegated_delivery".into(),
            ContractCapabilityStatus::Available,
        ),
        // The feature-detect for `ApprovalScope::AlwaysPath` — "always allow
        // this folder" on a `tool_approve`.
        //
        // A host MUST check this before sending the scope, and the reason is
        // sharper than the usual additive-field case. `scope` carries
        // `#[serde(default)]`, so an ABSENT scope is harmless on any Core; but
        // an unknown VARIANT is not a missing field, it fails the whole
        // `tool_approve` frame. On an older Core the approval is then never
        // resolved and the pending call sits until the TTL reaper denies it —
        // the host sees a hang, not a rejection. Undeclared therefore means
        // "send `once` or `always` only", never "try it and see".
        //
        // Available, not ShapeOnly: an approved grant is honoured end to end
        // by the same Core that declares this — `readable_roots()` for the OS
        // sandbox and `SandboxedFs` for the in-process file tools. It says
        // nothing about who raises the prompt; that is
        // `path_boundary_prompt_v1` below.
        ("path_grants_v1".into(), ContractCapabilityStatus::Available),
        // The feature-detect for the WRITE half of a path grant (#1104):
        // `always_path.write: true` on a `tool_approve`, and
        // `grant_path.access: "write"`.
        //
        // Separate from `path_grants_v1` for the same reason
        // `path_boundary_prompt_v1` is: they are separate promises and a host
        // must be able to hold one without the other. Every shipped Core
        // accepts both frames — the field and the enum variant were on the wire
        // from the start — and every Core before this one REFUSED the write and
        // granted nothing. So a host cannot feature-detect write support by
        // sending it: the frame is valid either way and the only difference is
        // an `info` line. A host that renders a "grant write access" button
        // without checking this ships a button that silently does nothing on
        // three quarters of the installed base.
        //
        // Available, not ShapeOnly: an approved write grant is honoured end to
        // end by the same Core that declares this — `writable_roots()` for the
        // OS sandbox manifest and `SandboxedFs`'s mutating operations for the
        // in-process file tools. It does NOT promise that any given folder will
        // be accepted: the write grant applies strictly more refusals than the
        // read grant (an unconfined sandbox backend, an auto-run location, a
        // folder holding an executable or a secret), and a host must still
        // render the refusal it gets back.
        (
            "path_write_grants_v1".into(),
            ContractCapabilityStatus::Available,
        ),
        // The feature-detect for `tool_request.tool.escalation` (#1099): Core
        // itself raises the approval when a read names a path outside every
        // reachable root, instead of letting the call fail with an
        // out-of-sandbox tool error.
        //
        // Separate from `path_grants_v1` because they are separate promises and
        // a host must be able to hold one without the other. `path_grants_v1`
        // says an `always_path` scope will be honoured; this says Core will
        // ASK. A host that has only the first has to attach `always_path` to
        // some unrelated pending approval, which is what shipping it alone
        // meant in practice.
        //
        // Declared => when `escalation` is present with `kind:
        // "path_boundary"`, answering that approval with
        // `always_path { root: suggested_root }` is guaranteed to be accepted:
        // the producer dry-runs that exact grant before emitting the frame.
        // Undeclared => the field never appears, and its absence means nothing.
        (
            "path_boundary_prompt_v1".into(),
            ContractCapabilityStatus::Available,
        ),
        // #1098 — the feature-detect for `render_artifact`: the engine can
        // hand the host CONTENT to display instead of asking the OS to open a
        // path. A host that declares a render surface gets one; a host that
        // does not simply never sees the event (it is droppable by
        // construction — see the `critical` const above).
        //
        // Available, not ShapeOnly: the event is emitted by a real registered
        // tool whose content comes through the same vfs/policy path as an
        // ordinary `read`, and the payload is capped and truncation-marked
        // before it reaches the wire. What is NOT promised is that any host
        // renders it — that is the host's half of #1098.
        (
            "render_artifact_v1".into(),
            ContractCapabilityStatus::Available,
        ),
        ("plugin_events".into(), ContractCapabilityStatus::ShapeOnly),
        (
            "semantic_failover_receipts".into(),
            ContractCapabilityStatus::Available,
        ),
        // The feature-detect a host needs to trust `ready.session_id`'s
        // absence. Declared => `session_id` is always on the wire (`null` when
        // there is no session) and `session_persistence` states the cause.
        // Undeclared => an older Core, whose missing `session_id` means
        // nothing in particular.
        //
        // RETAINED, not replaced by v2, and the distinction is the whole
        // argument. v1's declared guarantee is about the FRAME SHAPE — the
        // correlation key never vanishes, and a sibling always states the
        // cause — and both of those are still exactly true. Retiring v1 would
        // revoke a promise this producer still keeps, and a host pinned on it
        // would see the capability disappear and fall back to "a missing
        // session_id means nothing in particular", which is strictly worse than
        // where it started.
        (
            "session_persistence_v1".into(),
            ContractCapabilityStatus::Available,
        ),
        // v2 is the VOCABULARY, and it is additive for the same reason v1 is
        // retained. What changed is not the shape but the value set: a host
        // that feature-detected on v1 minted its switch when the enum had three
        // values, and `journaled_without_replay` did not exist. Since the enum
        // is closed, such a host's own validator would reject a frame this
        // producer now legitimately emits, so it needs a way to ask "does this
        // Core use the wider vocabulary?" that is distinct from "does this Core
        // publish the field at all?".
        //
        // Declared => `session_persistence` may be `journaled_without_replay`:
        // a real, resumable, fully journaled session whose interrupted turns do
        // NOT resume themselves. A host that treats that as `durable` will wait
        // for an auto-recovery that never comes.
        // Undeclared but v1 declared => the three-value vocabulary, on a
        // producer where a keyless host journaled nothing.
        (
            "session_persistence_v2".into(),
            ContractCapabilityStatus::Available,
        ),
        (
            "turn_recovery_v1".into(),
            ContractCapabilityStatus::Available,
        ),
        (
            "operator_tool_effect_resolution_v1".into(),
            ContractCapabilityStatus::Available,
        ),
        (
            "runtime_diagnostics_v1".into(),
            ContractCapabilityStatus::Available,
        ),
        (
            "runtime_mcp_lifecycle_v1".into(),
            ContractCapabilityStatus::Available,
        ),
        // wayland#605 -- the feature-detect for `mcp_ready.already_connected`:
        // an `add_mcp_server` that names an already-connected server is skipped
        // and acknowledged with `already_connected: true`, so a host can tell a
        // no-op re-add from a real reconnect.
        //
        // Separate from `runtime_mcp_lifecycle_v1` rather than folded into it,
        // for the reason `session_persistence_v2` is separate from v1: a host
        // that feature-detected on the lifecycle capability minted its handling
        // when every `mcp_ready` was a connect, and widening that capability's
        // meaning in place gives such a host no way to ask which producer it is
        // talking to.
        //
        // The flag is omitted when false, so its ABSENCE is exactly what a host
        // cannot read without this: on a Core that predates the annotation it is
        // absent on skips too. Declared => absent means a real connect.
        // Undeclared => absent means nothing in particular, and a host must keep
        // treating a duplicate `mcp_ready` as ambiguous.
        //
        // Available, not ShapeOnly: the producer sets it on the skip path in the
        // same change that publishes the field -- `crates/wcore-cli/src/main.rs`
        // on the `McpLifecycleState::Ready` arm -- and every real-connect site
        // sets it false.
        (
            "mcp_ready_skip_annotation_v1".into(),
            ContractCapabilityStatus::Available,
        ),
        (
            "workflow_lifecycle_v1".into(),
            ContractCapabilityStatus::Available,
        ),
    ])
}

fn descriptor(
    fixture_digest: String,
    schema_digest: String,
    source_inputs_digest: String,
    capabilities: BTreeMap<String, ContractCapabilityStatus>,
) -> ContractDescriptor {
    ContractDescriptor {
        name: CONTRACT_NAME.into(),
        major: CONTRACT_MAJOR,
        minor: CONTRACT_MINOR,
        generator: GENERATOR_VERSION.into(),
        fixture_digest,
        schema_digest,
        source_inputs_digest,
        capabilities,
    }
}

fn insert_negotiation_fixtures(
    artifacts: &mut BTreeMap<String, Vec<u8>>,
    descriptor: &ContractDescriptor,
) -> ContractResult<()> {
    let ready = artifacts
        .get("events/ready.json")
        .ok_or_else(|| std::io::Error::other("canonical Ready fixture is missing"))?;
    let mut ready: Value = serde_json::from_slice(ready)?;
    ready["contract"] = serde_json::to_value(descriptor)?;
    artifacts.insert("events/ready.json".into(), canonical_json(&ready)?);

    // The two non-default `ready` postures get the SAME descriptor stamp as
    // the durable one. They live in the corpus because Desktop validates Core
    // frames against it, and each covers a shape `events/ready.json` cannot:
    //
    // * `journaled-without-replay` is the keyring-less PRODUCTION frame — a
    //   named session with no crash replay. A host that tests only the durable
    //   fixture ships a session tracker that has never seen this posture and
    //   will wait for an auto-recovery that never arrives.
    // * `disabled-by-host` is the one `session_id: null` example in the corpus,
    //   and the only one of a value the schema must still ACCEPT but this
    //   producer no longer emits. Stamped, so it genuinely exercises the
    //   current schema branch rather than failing it for an unrelated reason —
    //   the way `ready.minimal.json` does — because the property being proved
    //   is that the enum still admits the legacy value.
    //
    // Both bodies come from `compatibility_event_values`, i.e. real
    // `ProtocolEvent` serializations, never hand-edited JSON.
    for path in [
        "compat/events/ready.journaled-without-replay.json",
        "compat/events/ready.disabled-by-host.legacy.json",
    ] {
        let fixture = artifacts
            .get(path)
            .ok_or_else(|| std::io::Error::other("a non-default Ready fixture is missing"))?;
        let mut fixture: Value = serde_json::from_slice(fixture)?;
        fixture["contract"] = serde_json::to_value(descriptor)?;
        artifacts.insert(path.into(), canonical_json(&fixture)?);
    }

    let mut unsupported_major = descriptor.clone();
    unsupported_major.major += 1;
    let mut unsupported_major_ready = ready.clone();
    unsupported_major_ready["contract"] = serde_json::to_value(unsupported_major)?;
    artifacts.insert(
        "adversarial/events/version-mismatch.jsonl".into(),
        canonical_json(&unsupported_major_ready)?,
    );

    let mut schema_mismatch = descriptor.clone();
    schema_mismatch.schema_digest = format!("sha256:{}", "f".repeat(64));
    let mut schema_mismatch_ready = ready.clone();
    schema_mismatch_ready["contract"] = serde_json::to_value(schema_mismatch)?;
    artifacts.insert(
        "adversarial/events/schema-mismatch.jsonl".into(),
        canonical_json(&schema_mismatch_ready)?,
    );

    let mut fixture_mismatch = descriptor.clone();
    fixture_mismatch.fixture_digest = format!("sha256:{}", "f".repeat(64));
    let mut fixture_mismatch_ready = ready;
    fixture_mismatch_ready["contract"] = serde_json::to_value(fixture_mismatch)?;
    artifacts.insert(
        "adversarial/events/fixture-mismatch.jsonl".into(),
        canonical_json(&fixture_mismatch_ready)?,
    );
    Ok(())
}

/// Regenerate every tracked contract artifact in memory.
/// The corpus may not under-declare the producer wire.
///
/// `EVENT_SPECS` is what the corpus SHIPS; `PRODUCER_EVENT_TYPES` is what Core
/// actually emits. While these were allowed to differ, seven emitted variants —
/// including `workspace_policy`, which arrives on every session right after
/// `ready` — had no entry in `manifest.json` and no payload schema anywhere. A
/// Desktop host built from the shipped corpus classifies such a frame as an
/// unknown event with no `critical` field, and the documented rule for that is
/// to hard error. Requested by Desktop; proved from the consumer side by
/// `tests/desktop_contract_corpus_only_host.rs`.
///
/// This runs inside the generator rather than in a test so the failure mode is
/// "the corpus cannot be built", not "a test that someone may not run is red".
fn assert_producer_event_parity() -> ContractResult<()> {
    let declared = EVENT_SPECS
        .iter()
        .map(|spec| spec.wire_type)
        .collect::<BTreeSet<_>>();
    let produced = PRODUCER_EVENT_TYPES
        .iter()
        .copied()
        .collect::<BTreeSet<_>>();
    if declared == produced {
        return Ok(());
    }
    let undeclared = produced.difference(&declared).copied().collect::<Vec<_>>();
    let phantom = declared.difference(&produced).copied().collect::<Vec<_>>();
    Err(format!(
        "Desktop contract corpus does not match the producer event inventory. \
         Emitted by Core but absent from EVENT_SPECS (a corpus-only host HARD ERRORS \
         on these): {undeclared:?}. Declared in EVENT_SPECS but never emitted: \
         {phantom:?}. Add a WireSpec plus a real fixture in contract/spec.rs, or \
         remove the variant from PRODUCER_EVENT_TYPES."
    )
    .into())
}

/// The command-direction mirror of [`assert_producer_event_parity`].
///
/// An undeclared command does not hard error a host the way an undeclared
/// event does - commands travel host to Core - so this gate was deliberately
/// left off the command direction. #314 is what that cost: three commands the
/// engine has dispatched since 0.13.6 were absent from the published union, so
/// a host that derives its emitter, its codegen or its conformance check from
/// `manifest.json` could not send them, and the resulting silence reads as the
/// FEATURE being broken rather than the contract being incomplete.
fn assert_producer_command_parity() -> ContractResult<()> {
    let declared = COMMAND_SPECS
        .iter()
        .map(|spec| spec.wire_type)
        .collect::<BTreeSet<_>>();
    let produced = PRODUCER_COMMAND_TYPES
        .iter()
        .copied()
        .collect::<BTreeSet<_>>();
    if declared == produced {
        return Ok(());
    }
    let undeclared = produced.difference(&declared).copied().collect::<Vec<_>>();
    let phantom = declared.difference(&produced).copied().collect::<Vec<_>>();
    Err(format!(
        "Desktop contract corpus does not match the producer command inventory. \
         Accepted by Core but absent from COMMAND_SPECS (a host that derives its \
         emitter from the published union cannot send these): {undeclared:?}. \
         Declared in COMMAND_SPECS but never accepted: {phantom:?}. Add a WireSpec \
         plus a real fixture in contract/spec.rs, or remove the variant from \
         PRODUCER_COMMAND_TYPES."
    )
    .into())
}

pub fn generated_artifacts() -> ContractResult<BTreeMap<String, Vec<u8>>> {
    assert_producer_event_parity()?;
    assert_producer_command_parity()?;
    let mut artifacts = BTreeMap::new();

    for (path, value) in command_fixture_values() {
        let _: ProtocolCommand = serde_json::from_value(value.clone())?;
        artifacts.insert(path, canonical_json(&value)?);
    }
    for (path, event) in event_fixture_values()
        .into_iter()
        .chain(compatibility_event_values())
    {
        artifacts.insert(path, canonical_json(&serde_json::to_value(event)?)?);
    }
    for (path, value) in durable_child_fixture_values() {
        artifacts.insert(path, canonical_json(&value)?);
    }

    let mut malformed_child = durable_child_fixture_values()
        .remove("types/durable_child_record.json")
        .expect("durable child record fixture must exist");
    malformed_child["child_id"] = json!(" child-001");
    artifacts.insert(
        "adversarial/types/durable-child-invalid-id.json".into(),
        canonical_json(&malformed_child)?,
    );
    let mut unknown_field_child = durable_child_fixture_values()
        .remove("types/durable_child_record.json")
        .expect("durable child record fixture must exist");
    unknown_field_child["unexpected_authority"] = json!(true);
    artifacts.insert(
        "adversarial/types/durable-child-unknown-field.json".into(),
        canonical_json(&unknown_field_child)?,
    );

    let canonical_events = event_fixture_values();
    let ready = event_value(
        canonical_events
            .get("events/ready.json")
            .expect("ready fixture must exist"),
    )?;
    let policy_changed = event_value(
        canonical_events
            .get("events/execution_policy.json")
            .expect("execution policy fixture must exist"),
    )?;
    let recovery_snapshot = event_value(
        canonical_events
            .get("events/session_recovery_snapshot.json")
            .expect("recovery snapshot fixture must exist"),
    )?;
    let recovery_replay = event_value(
        canonical_events
            .get("events/session_recovery_replay.json")
            .expect("recovery replay fixture must exist"),
    )?;

    artifacts.insert(
        "adversarial/recovery/valid-replay.jsonl".into(),
        json_lines([recovery_snapshot.clone(), recovery_replay.clone()])?,
    );
    let mut recovery_version = recovery_snapshot.clone();
    recovery_version["recovery_version"] = json!(2);
    artifacts.insert(
        "adversarial/recovery/version-mismatch.jsonl".into(),
        json_lines([recovery_version])?,
    );
    let mut cursor_digest_mismatch = recovery_replay.clone();
    cursor_digest_mismatch["from"]["journal_digest"] = json!("f".repeat(64));
    artifacts.insert(
        "adversarial/recovery/cursor-digest-mismatch.jsonl".into(),
        json_lines([recovery_snapshot.clone(), cursor_digest_mismatch])?,
    );
    let mut cursor_gap = recovery_replay.clone();
    let gap_digest = cursor_gap["items"][1]["cursor"]["journal_digest"].clone();
    cursor_gap["items"][0]["cursor"]["journal_sequence"] = json!(42);
    cursor_gap["items"][0]["cursor"]["journal_digest"] = gap_digest;
    artifacts.insert(
        "adversarial/recovery/cursor-gap.jsonl".into(),
        json_lines([recovery_snapshot.clone(), cursor_gap])?,
    );
    let mut state_digest_conflict = recovery_snapshot.clone();
    state_digest_conflict["state_digest"] = json!("f".repeat(64));
    artifacts.insert(
        "adversarial/recovery/state-digest-conflict.jsonl".into(),
        json_lines([recovery_snapshot.clone(), state_digest_conflict])?,
    );

    artifacts.insert(
        "adversarial/policy/valid-revisions.jsonl".into(),
        json_lines([ready.clone(), policy_changed.clone()])?,
    );
    artifacts.insert(
        "adversarial/policy/duplicate-identical.jsonl".into(),
        json_lines([ready.clone(), ready.clone()])?,
    );
    let mut policy_conflict = policy_changed.clone();
    policy_conflict["revision"] = json!(0);
    artifacts.insert(
        "adversarial/policy/duplicate-conflict.jsonl".into(),
        json_lines([ready.clone(), policy_conflict])?,
    );
    let mut policy_gap = policy_changed.clone();
    policy_gap["revision"] = json!(2);
    artifacts.insert(
        "adversarial/policy/revision-gap.jsonl".into(),
        json_lines([ready.clone(), policy_gap])?,
    );
    let mut policy_version = policy_changed.clone();
    policy_version["contract_version"] = json!("2.0");
    artifacts.insert(
        "adversarial/policy/version-mismatch.jsonl".into(),
        json_lines([ready.clone(), policy_version])?,
    );
    let mut policy_noncritical = policy_changed.clone();
    policy_noncritical["critical"] = json!(false);
    artifacts.insert(
        "adversarial/policy/noncritical.jsonl".into(),
        json_lines([ready.clone(), policy_noncritical])?,
    );

    let workflow = workflow_lifecycle_events()
        .iter()
        .map(event_value)
        .collect::<ContractResult<Vec<_>>>()?;
    artifacts.insert(
        "adversarial/workflow/valid-lifecycle.jsonl".into(),
        json_lines(workflow.clone())?,
    );
    artifacts.insert(
        "adversarial/workflow/duplicate-identical.jsonl".into(),
        json_lines([workflow[0].clone(), workflow[0].clone()])?,
    );
    let mut workflow_conflict = workflow[0].clone();
    workflow_conflict["name"] = json!("Conflicting display name");
    artifacts.insert(
        "adversarial/workflow/duplicate-conflict.jsonl".into(),
        json_lines([workflow[0].clone(), workflow_conflict])?,
    );
    artifacts.insert(
        "adversarial/workflow/sequence-gap.jsonl".into(),
        json_lines([workflow[0].clone(), workflow[2].clone()])?,
    );
    let mut early_finish = workflow[6].clone();
    early_finish["event_id"] = json!("workflow-event-terminal");
    early_finish["sequence"] = json!(1);
    let mut empty_workflow_start = workflow[0].clone();
    empty_workflow_start["node_count"] = json!(0);
    artifacts.insert(
        "adversarial/workflow/after-terminal.jsonl".into(),
        json_lines([empty_workflow_start, early_finish, workflow[2].clone()])?,
    );
    let mut first_terminal = workflow[5].clone();
    first_terminal["event_id"] = json!("workflow-event-terminal-node");
    first_terminal["sequence"] = json!(1);
    let mut conflicting_terminal = first_terminal.clone();
    conflicting_terminal["event_id"] = json!("workflow-event-conflicting-terminal");
    conflicting_terminal["sequence"] = json!(2);
    conflicting_terminal["state"] = json!("failed");
    conflicting_terminal["failure"] =
        json!({"code":"stage_failed","message":"conflicting terminal","retryable":false});
    artifacts.insert(
        "adversarial/workflow/conflicting-node-terminal.jsonl".into(),
        json_lines([workflow[0].clone(), first_terminal, conflicting_terminal])?,
    );
    let mut child_gap = workflow[3].clone();
    child_gap["child_sequence"] = json!(1);
    artifacts.insert(
        "adversarial/workflow/child-sequence-gap.jsonl".into(),
        json_lines([workflow[0].clone(), child_gap])?,
    );
    let mut child_conflict = workflow[3].clone();
    child_conflict["inner"]["text"] = json!("conflicting child output");
    artifacts.insert(
        "adversarial/workflow/child-duplicate-conflict.jsonl".into(),
        json_lines([workflow[0].clone(), workflow[3].clone(), child_conflict])?,
    );

    let receipt = event_value(&crate::events::ProtocolEvent::AnvilReceipt {
        receipt: anvil_receipt(),
    })?;
    let invalidation = event_value(&crate::events::ProtocolEvent::AnvilReceiptInvalidated {
        invalidation: anvil_invalidation(),
    })?;
    artifacts.insert(
        "adversarial/anvil/valid-invalidation.jsonl".into(),
        json_lines([receipt.clone(), invalidation.clone()])?,
    );
    let mut altered_invalidation = invalidation.clone();
    altered_invalidation["reason"] = json!("gate_revoked");
    artifacts.insert(
        "adversarial/anvil/altered-invalidation-body.jsonl".into(),
        json_lines([receipt.clone(), altered_invalidation])?,
    );
    artifacts.insert(
        "adversarial/anvil/duplicate-identical.jsonl".into(),
        json_lines([receipt.clone(), receipt.clone()])?,
    );
    let mut receipt_conflict = receipt.clone();
    receipt_conflict["stamp"] = json!("conflicting");
    refresh_anvil_receipt_body_digest(&mut receipt_conflict)?;
    artifacts.insert(
        "adversarial/anvil/duplicate-conflict.jsonl".into(),
        json_lines([receipt.clone(), receipt_conflict])?,
    );
    let mut receipt_gap = receipt.clone();
    receipt_gap["sequence"] = json!(1);
    refresh_anvil_receipt_body_digest(&mut receipt_gap)?;
    artifacts.insert(
        "adversarial/anvil/sequence-gap.jsonl".into(),
        json_lines([receipt_gap])?,
    );
    let mut receipt_version = receipt.clone();
    receipt_version["contract_version"] = json!("2.0");
    refresh_anvil_receipt_body_digest(&mut receipt_version)?;
    artifacts.insert(
        "adversarial/anvil/version-mismatch.jsonl".into(),
        json_lines([receipt_version])?,
    );
    let mut receipt_extension = receipt.clone();
    receipt_extension["required_extensions"] = json!(["future-authority-v2"]);
    refresh_anvil_receipt_body_digest(&mut receipt_extension)?;
    artifacts.insert(
        "adversarial/anvil/unknown-critical-extension.jsonl".into(),
        json_lines([receipt_extension])?,
    );
    artifacts.insert(
        "adversarial/anvil/nested-receipt-inert.jsonl".into(),
        json_lines([json!({
            "type":"sub_agent_event",
            "parent_call_id":"workflow:scan",
            "agent_name":"untrusted-child",
            "inner":receipt.clone()
        })])?,
    );
    artifacts.insert(
        "adversarial/anvil/stale-replay.jsonl".into(),
        json_lines([receipt.clone(), invalidation.clone(), receipt.clone()])?,
    );
    let mut altered_body = receipt.clone();
    altered_body["terminal_state"] = json!("tampered");
    artifacts.insert(
        "adversarial/anvil/altered-body.jsonl".into(),
        json_lines([altered_body])?,
    );
    let mut stale_event = receipt.clone();
    stale_event["receipt_id"] = json!("receipt-desktop-002");
    stale_event["event_id"] = json!("anvil-event-002");
    stale_event["sequence"] = json!(1);
    refresh_anvil_receipt_body_digest(&mut stale_event)?;
    artifacts.insert(
        "adversarial/anvil/out-of-order.jsonl".into(),
        json_lines([receipt.clone(), invalidation.clone(), stale_event])?,
    );
    artifacts.insert(
        "compat/events/anvil_receipt.legacy.json".into(),
        canonical_json(&json!({
            "type":"anvil_receipt",
            "terminal_state":"verified",
            "stamp":"verified",
            "sequence":0
        }))?,
    );

    artifacts.insert(
        "adversarial/commands/continue-with-budget-empty.jsonl".into(),
        b"{\"request_id\":\"budget-empty\",\"type\":\"continue_with_budget\"}\n".to_vec(),
    );
    artifacts.insert(
        "adversarial/commands/continue-with-budget-missing-request-id.jsonl".into(),
        b"{\"additional_tokens\":1,\"type\":\"continue_with_budget\"}\n".to_vec(),
    );
    artifacts.insert(
        "adversarial/commands/continue-with-budget-negative-cost.jsonl".into(),
        b"{\"additional_cost_usd\":-1,\"request_id\":\"budget-negative\",\"type\":\"continue_with_budget\"}\n".to_vec(),
    );
    artifacts.insert(
        "adversarial/commands/continue-with-budget-unknown-field.jsonl".into(),
        b"{\"additional_tokens\":1,\"future_authority\":true,\"request_id\":\"budget-unknown\",\"type\":\"continue_with_budget\"}\n"
            .to_vec(),
    );
    artifacts.insert(
        "adversarial/commands/continue-with-budget-empty-request-id.jsonl".into(),
        b"{\"additional_tokens\":1,\"request_id\":\"\",\"type\":\"continue_with_budget\"}\n"
            .to_vec(),
    );
    artifacts.insert(
        "adversarial/commands/continue-with-budget-whitespace-request-id.jsonl".into(),
        b"{\"additional_tokens\":1,\"request_id\":\"   \t\",\"type\":\"continue_with_budget\"}\n"
            .to_vec(),
    );
    artifacts.insert(
        "adversarial/commands/continue-with-budget-unicode-request-id.jsonl".into(),
        format!(
            "{{\"additional_tokens\":1,\"request_id\":\"{}\",\"type\":\"continue_with_budget\"}}\n",
            "😀".repeat(BUDGET_GRANT_REQUEST_ID_MAX_BYTES)
        )
        .into_bytes(),
    );
    artifacts.insert(
        "adversarial/commands/continue-with-budget-long-request-id.jsonl".into(),
        format!(
            "{{\"additional_tokens\":1,\"request_id\":\"{}\",\"type\":\"continue_with_budget\"}}\n",
            "x".repeat(129)
        )
        .into_bytes(),
    );
    artifacts.insert(
        "adversarial/commands/continue-with-budget-overflow-tokens.jsonl".into(),
        b"{\"additional_tokens\":18446744073709551616,\"request_id\":\"budget-overflow\",\"type\":\"continue_with_budget\"}\n".to_vec(),
    );
    artifacts.insert(
        "adversarial/commands/continue-with-budget-wrong-numeric-type.jsonl".into(),
        b"{\"additional_tokens\":\"1\",\"request_id\":\"budget-wrong-type\",\"type\":\"continue_with_budget\"}\n".to_vec(),
    );
    // wayland#896 quiescence adversarial corpus. Split from
    // `adversarial/commands/` on purpose: those fixtures are frames that must
    // fail to DESERIALIZE, while most of these are perfectly well-formed frames
    // that must be REFUSED. A host that only ever tests the decoder never
    // exercises the refusal path at all, which is how a fail-open guard ships
    // looking exactly like a working one.
    artifacts.insert(
        "adversarial/quiescence/valid-acquire.jsonl".into(),
        json_lines([json!({
            "type":"quiesce_acquire",
            "quiescence_version":1,
            "request_id":"quiesce-acquire-001",
            "lease_id":"lease-desktop-001",
            "session_id":"session-desktop-001",
            "scope":{"include_default":true,"profiles":{"select":"all"}},
            "ttl_ms":120000
        })])?,
    );
    artifacts.insert(
        "adversarial/quiescence/acquire-unsupported-version.jsonl".into(),
        json_lines([json!({
            "type":"quiesce_acquire",
            "quiescence_version":2,
            "request_id":"quiesce-acquire-002",
            "lease_id":"lease-desktop-002",
            "session_id":"session-desktop-001",
            "scope":{"include_default":true,"profiles":{"select":"all"}},
            "ttl_ms":120000
        })])?,
    );
    artifacts.insert(
        "adversarial/quiescence/acquire-empty-profile-selection.jsonl".into(),
        json_lines([json!({
            "type":"quiesce_acquire",
            "quiescence_version":1,
            "request_id":"quiesce-acquire-003",
            "lease_id":"lease-desktop-003",
            "session_id":"session-desktop-001",
            "scope":{"include_default":false,"profiles":{"select":"named","names":[]}},
            "ttl_ms":120000
        })])?,
    );
    artifacts.insert(
        "adversarial/quiescence/acquire-traversal-profile-name.jsonl".into(),
        json_lines([json!({
            "type":"quiesce_acquire",
            "quiescence_version":1,
            "request_id":"quiesce-acquire-004",
            "lease_id":"lease-desktop-004",
            "session_id":"session-desktop-001",
            "scope":{"include_default":false,"profiles":{"select":"named","names":["../../etc"]}},
            "ttl_ms":120000
        })])?,
    );
    artifacts.insert(
        "adversarial/quiescence/acquire-unbounded-ttl.jsonl".into(),
        json_lines([json!({
            "type":"quiesce_acquire",
            "quiescence_version":1,
            "request_id":"quiesce-acquire-005",
            "lease_id":"lease-desktop-005",
            "session_id":"session-desktop-001",
            "scope":{"include_default":true,"profiles":{"select":"all"}},
            "ttl_ms":86400000u64
        })])?,
    );
    artifacts.insert(
        "adversarial/quiescence/acquire-zero-ttl.jsonl".into(),
        json_lines([json!({
            "type":"quiesce_acquire",
            "quiescence_version":1,
            "request_id":"quiesce-acquire-006",
            "lease_id":"lease-desktop-006",
            "session_id":"session-desktop-001",
            "scope":{"include_default":true,"profiles":{"select":"all"}},
            "ttl_ms":0
        })])?,
    );
    artifacts.insert(
        "adversarial/quiescence/acquire-unknown-scope-field.jsonl".into(),
        b"{\"type\":\"quiesce_acquire\",\"quiescence_version\":1,\"request_id\":\"quiesce-acquire-007\",\"lease_id\":\"lease-desktop-007\",\"session_id\":\"session-desktop-001\",\"scope\":{\"include_default\":true,\"profiles\":{\"select\":\"all\"},\"include_secrets\":true},\"ttl_ms\":120000}\n".to_vec(),
    );
    artifacts.insert(
        "adversarial/quiescence/release-missing-epoch.jsonl".into(),
        json_lines([json!({
            "type":"quiesce_release",
            "quiescence_version":1,
            "request_id":"quiesce-release-002",
            "lease_id":"lease-desktop-001",
            "session_id":"session-desktop-001",
            "epoch":""
        })])?,
    );
    artifacts.insert(
        "adversarial/quiescence/release-unsupported-version.jsonl".into(),
        json_lines([json!({
            "type":"quiesce_release",
            "quiescence_version":99,
            "request_id":"quiesce-release-003",
            "lease_id":"lease-desktop-001",
            "session_id":"session-desktop-001",
            "epoch":"sha256:quiesceepoch"
        })])?,
    );
    // Receipt-side adversarial cases: shapes a host must refuse to act on even
    // though they decode cleanly.
    artifacts.insert(
        "adversarial/quiescence/granted-incomplete-coverage.jsonl".into(),
        json_lines([json!({
            "type":"quiesce_lease_granted",
            "quiescence_version":1,
            "request_id":"quiesce-acquire-008",
            "lease_id":"lease-desktop-008",
            "session_id":"session-desktop-001",
            "epoch":"sha256:quiesceepoch",
            "coverage":{"roots":[{"identity":{"kind":"default"},"path":"/home/user/.wayland","root_digest":"sha256:defaultroot","file_count":128,"byte_count":4194304}],"complete":false},
            "acquired_unix_ms":1767225600000u64,
            "expires_unix_ms":1767225720000u64,
            "idempotent_replay":false
        })])?,
    );
    artifacts.insert(
        "adversarial/quiescence/released-mutated.jsonl".into(),
        json_lines([json!({
            "type":"quiesce_lease_released",
            "quiescence_version":1,
            "request_id":"quiesce-release-004",
            "lease_id":"lease-desktop-001",
            "session_id":"session-desktop-001",
            "epoch_at_acquire":"sha256:quiesceepoch",
            "epoch_at_release":"sha256:movedepoch",
            "verdict":"mutated",
            "released_unix_ms":1767225660000u64
        })])?,
    );
    artifacts.insert(
        "adversarial/commands/invalid-json.jsonl".into(),
        b"{not-json}\n".to_vec(),
    );
    artifacts.insert(
        "adversarial/commands/missing-type.jsonl".into(),
        b"{\"msg_id\":\"msg-001\"}\n".to_vec(),
    );
    artifacts.insert(
        "adversarial/commands/non-object.jsonl".into(),
        b"[]\n".to_vec(),
    );
    artifacts.insert(
        "adversarial/commands/non-string-type.jsonl".into(),
        b"{\"type\":1}\n".to_vec(),
    );
    artifacts.insert(
        "adversarial/commands/unknown-type.jsonl".into(),
        b"{\"type\":\"future_command\"}\n".to_vec(),
    );
    artifacts.insert(
        "adversarial/commands/wrong-required-field.jsonl".into(),
        b"{\"content\":\"hello\",\"msg_id\":7,\"type\":\"message\"}\n".to_vec(),
    );
    artifacts.insert(
        "adversarial/events/unknown-critical.jsonl".into(),
        b"{\"critical\":true,\"type\":\"future_authority\"}\n".to_vec(),
    );
    artifacts.insert(
        "adversarial/events/unknown-noncritical.jsonl".into(),
        b"{\"critical\":false,\"payload\":{},\"type\":\"future_observation\"}\n".to_vec(),
    );
    artifacts.insert(
        "adversarial/events/unknown-criticality.jsonl".into(),
        b"{\"payload\":{},\"type\":\"future_unclassified\"}\n".to_vec(),
    );

    let command_schema_fixtures = command_fixture_values();
    let event_schema_fixtures = event_fixture_values()
        .into_iter()
        .map(|(path, event)| Ok((path, event_value(&event)?)))
        .collect::<ContractResult<BTreeMap<_, _>>>()?;
    let compatibility_schema_fixtures = compatibility_event_values()
        .into_iter()
        .map(|(path, event)| Ok((path, event_value(&event)?)))
        .collect::<ContractResult<BTreeMap<_, _>>>()?;
    enforce_approval_gate_contract(&event_schema_fixtures, &compatibility_schema_fixtures)?;
    let legacy_child =
        compatibility_schema_fixtures.get("compat/events/sub_agent_event.legacy.json");
    let command_schema_title =
        format!("Desktop-consumed HostCommand v{CONTRACT_MAJOR}.{CONTRACT_MINOR}");
    let event_schema_title =
        format!("Desktop-consumed CoreEvent v{CONTRACT_MAJOR}.{CONTRACT_MINOR}");
    let (command_schema, command_shapes) = schema_for(
        COMMAND_SPECS,
        &command_schema_fixtures,
        None,
        &command_schema_title,
    );
    let (event_schema, event_shapes) = schema_for(
        EVENT_SPECS,
        &event_schema_fixtures,
        legacy_child,
        &event_schema_title,
    );
    let wire_shapes = wire_shape_digests(command_shapes.into_iter().chain(event_shapes))?;
    artifacts.insert(
        "schema/host-command.schema.json".into(),
        canonical_json(&command_schema)?,
    );
    artifacts.insert(
        "schema/core-event.schema.json".into(),
        canonical_json(&event_schema)?,
    );
    artifacts.insert(
        "schema/producer-complete.schema.json".into(),
        canonical_json(&producer_complete_schema(&command_schema, &event_schema))?,
    );
    artifacts.insert("DEFERRED.md".into(), DEFERRED.as_bytes().to_vec());

    let schema_digest = schemas_digest(&artifacts);
    let source_inputs_digest = source_digest()?;
    let capabilities = contract_capabilities();
    let provisional = descriptor(
        format!("sha256:{}", "0".repeat(64)),
        schema_digest.clone(),
        source_inputs_digest.clone(),
        capabilities.clone(),
    );
    insert_negotiation_fixtures(&mut artifacts, &provisional)?;
    let fixture_digest = fixtures_digest(&artifacts)?;
    let final_descriptor = descriptor(
        fixture_digest.clone(),
        schema_digest.clone(),
        source_inputs_digest.clone(),
        capabilities.clone(),
    );
    insert_negotiation_fixtures(&mut artifacts, &final_descriptor)?;
    debug_assert_eq!(fixture_digest, fixtures_digest(&artifacts)?);
    let fixture_inventory = artifacts
        .keys()
        .filter(|path| {
            ["commands/", "events/", "types/", "compat/", "adversarial/"]
                .iter()
                .any(|prefix| path.starts_with(prefix))
        })
        .cloned()
        .collect::<Vec<_>>();
    let child_type_inventory = artifacts
        .keys()
        .filter(|path| path.starts_with("types/"))
        .cloned()
        .collect::<Vec<_>>();
    let child_type_count = child_type_inventory.len();
    let manifest = json!({
        "capabilities": capabilities,
        "child_types": child_type_inventory,
        "commands": specs_manifest(COMMAND_SPECS),
        "counts": {
            "child_types": child_type_count,
            "commands": COMMAND_SPECS.len(),
            "events": EVENT_SPECS.len(),
            "fixtures": fixture_inventory.len()
        },
        "contract": {
            "major": CONTRACT_MAJOR,
            "minor": CONTRACT_MINOR,
            "name": CONTRACT_NAME
        },
        "deferred_adversarial": [
            "ordinary_turn_tool_replay_reducer",
            "anvil_desktop_replay_reducer",
            "anvil_persistent_mutation_watcher"
        ],
        "events": specs_manifest(EVENT_SPECS),
        "fixture_digest": fixture_digest,
        "fixture_inventory": fixture_inventory,
        "generator": GENERATOR_VERSION,
        "subcontracts": {
            "anvil_receipts": "1.0",
            "durable_child": "1.0",
            "execution_policy": "1.0",
            "operator_tool_effect_resolution": "1.0",
            "quiesced_snapshot_lease": "1.0",
            "runtime_diagnostics": "1.0",
            "semantic_failover_receipts": "1.0",
            "turn_recovery": "1.0",
            "workflow_lifecycle": "1.0"
        },
        "schema_digest": schema_digest,
        "source_inputs": SOURCE_INPUTS,
        "source_inputs_digest": source_inputs_digest,
        "wire_shapes": wire_shapes
    });
    artifacts.insert("manifest.json".into(), canonical_json(&manifest)?);

    Ok(artifacts)
}

/// Reject a regeneration that would move a published wire shape without a
/// contract version decision.
///
/// Deliberately a separate step over the finished artifacts rather than a
/// check inside `generated_artifacts`: the two callers that can actually
/// publish a blessed break - `write_contract` (the `generate` remedy) and
/// `check_contract` (what CI runs) - are gated, while the tests and tooling
/// that only inspect the regenerated bytes keep reporting their own findings
/// instead of all reporting this one.
pub fn enforce_wire_shape_version(
    artifacts: &BTreeMap<String, Vec<u8>>,
    baseline: WireShapeBaseline,
) -> ContractResult<()> {
    let published = published_wire_shapes()?;
    let manifest_bytes = artifacts
        .get("manifest.json")
        .ok_or_else(|| std::io::Error::other("regenerated corpus is missing manifest.json"))?;
    let manifest: Value = serde_json::from_slice(manifest_bytes)?;
    let current = manifest["wire_shapes"]
        .as_object()
        .ok_or_else(|| std::io::Error::other("regenerated manifest is missing wire_shapes"))?
        .iter()
        .filter_map(|(path, digest)| Some((path.clone(), digest.as_str()?.to_string())))
        .collect::<BTreeMap<_, _>>();
    match wire_shape_verdict(
        published.as_ref(),
        &current,
        CONTRACT_MAJOR,
        CONTRACT_MINOR,
        baseline,
    ) {
        Some(refusal) => Err(std::io::Error::other(refusal).into()),
        None => Ok(()),
    }
}

pub fn write_contract(baseline: WireShapeBaseline) -> ContractResult<()> {
    let root = contract_root();
    let artifacts = generated_artifacts()?;
    enforce_wire_shape_version(&artifacts, baseline)?;
    let expected = artifacts.keys().cloned().collect::<BTreeSet<_>>();

    if root.exists() {
        for path in all_relative_files(&root)? {
            if !expected.contains(&path) {
                fs::remove_file(root.join(path))?;
            }
        }
    }
    for (relative, bytes) in artifacts {
        let path = root.join(relative);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(path, bytes)?;
    }
    Ok(())
}

pub fn manifest_digests() -> ContractResult<(String, String, String)> {
    let artifacts = generated_artifacts()?;
    Ok((
        fixtures_digest(&artifacts)?,
        schemas_digest(&artifacts),
        source_digest()?,
    ))
}

pub(crate) fn contract_path() -> PathBuf {
    contract_root()
}

pub(crate) fn all_relative_files(root: &Path) -> ContractResult<BTreeSet<String>> {
    fn visit(root: &Path, current: &Path, files: &mut BTreeSet<String>) -> ContractResult<()> {
        for entry in fs::read_dir(current)? {
            let entry = entry?;
            let path = entry.path();
            if path.is_dir() {
                visit(root, &path, files)?;
            } else if path.is_file() {
                files.insert(
                    path.strip_prefix(root)?
                        .to_string_lossy()
                        .replace('\\', "/"),
                );
            }
        }
        Ok(())
    }

    let mut files = BTreeSet::new();
    if root.exists() {
        visit(root, root, &mut files)?;
    }
    Ok(files)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::contract::{HostContractObserver, HostObservation, HostObservationError};

    fn shape_map(entries: &[(&str, &str)]) -> BTreeMap<String, String> {
        entries
            .iter()
            .map(|(path, digest)| ((*path).to_string(), (*digest).to_string()))
            .collect()
    }

    fn published_at(major: u64, minor: u64, entries: &[(&str, &str)]) -> PublishedWireShapes {
        PublishedWireShapes {
            major,
            minor,
            shapes: shape_map(entries),
        }
    }

    const BASELINE: &[(&str, &str)] = &[
        ("events/ready.json", "sha256:bbb"),
        ("events/tool_request.json", "sha256:aaa"),
    ];

    #[test]
    fn an_unmoved_wire_shape_regenerates_under_the_standing_version() {
        let published = published_at(1, 16, BASELINE);
        assert_eq!(
            wire_shape_refusal(&published, &shape_map(BASELINE), 1, 16),
            None
        );
    }

    #[test]
    fn a_moved_wire_shape_is_refused_while_the_version_stands_still() {
        let published = published_at(1, 16, BASELINE);
        let current = shape_map(&[
            ("events/ready.json", "sha256:bbb"),
            ("events/tool_request.json", "sha256:ccc"),
        ]);
        let refusal = wire_shape_refusal(&published, &current, 1, 16)
            .expect("a standing version must refuse a moved wire shape");
        assert!(
            refusal.contains("events/tool_request.json"),
            "the refusal must name the type that moved: {refusal}"
        );
        assert!(
            !refusal.contains("events/ready.json"),
            "the refusal must not name a type that held still: {refusal}"
        );
        assert!(
            refusal.contains("CONTRACT_MAJOR") && refusal.contains("CONTRACT_MINOR"),
            "the refusal must name the version decision it wants: {refusal}"
        );
    }

    #[test]
    fn a_moved_wire_shape_regenerates_once_the_version_moves_forward() {
        let published = published_at(1, 16, BASELINE);
        let current = shape_map(&[
            ("events/ready.json", "sha256:bbb"),
            ("events/tool_request.json", "sha256:ccc"),
        ]);
        assert_eq!(wire_shape_refusal(&published, &current, 1, 17), None);
        assert_eq!(wire_shape_refusal(&published, &current, 2, 0), None);
        // Backwards is not a decision, it is the same silent bless with a
        // smaller number, so it stays refused.
        assert!(wire_shape_refusal(&published, &current, 1, 15).is_some());
    }

    #[test]
    fn adding_or_dropping_a_wire_type_is_also_a_version_decision() {
        let published = published_at(1, 16, BASELINE);
        let added = shape_map(&[
            ("events/ready.json", "sha256:bbb"),
            ("events/render_artifact.json", "sha256:ddd"),
            ("events/tool_request.json", "sha256:aaa"),
        ]);
        let refusal = wire_shape_refusal(&published, &added, 1, 16)
            .expect("a new wire type must not regenerate under a standing version");
        assert!(
            refusal.contains("events/render_artifact.json"),
            "the refusal must name the added type: {refusal}"
        );
        assert_eq!(wire_shape_refusal(&published, &added, 1, 17), None);

        let dropped = shape_map(&[("events/ready.json", "sha256:bbb")]);
        let refusal = wire_shape_refusal(&published, &dropped, 1, 16)
            .expect("a dropped wire type must not regenerate under a standing version");
        assert!(
            refusal.contains("events/tool_request.json"),
            "the refusal must name the dropped type: {refusal}"
        );
    }

    /// The two justification clauses, named once so the pair of tests below
    /// cannot quietly drift back into agreeing with each other.
    const CORRELATION_CLAUSE: &str = "can no longer correlate or render";
    const DISCOVERY_CLAUSE: &str = "no way to learn the added wire types exist";

    #[test]
    fn an_added_only_refusal_does_not_claim_broken_correlation() {
        let published = published_at(1, 16, BASELINE);
        let mut added = shape_map(BASELINE);
        added.insert("commands/set_effort.json".into(), "sha256:ddd".into());
        let refusal = wire_shape_refusal(&published, &added, 1, 16)
            .expect("a new wire type must not regenerate under a standing version");
        assert!(
            refusal.contains(DISCOVERY_CLAUSE),
            "an added-only refusal must justify itself by discoverability: {refusal}"
        );
        assert!(
            !refusal.contains(CORRELATION_CLAUSE),
            "nothing a pinned host correlates or renders moves when a type is added: {refusal}"
        );
        assert!(
            refusal.contains("CONTRACT_MINOR") && refusal.contains("CONTRACT_MAJOR"),
            "the rule sentence stays on both branches: {refusal}"
        );
    }

    #[test]
    fn an_altered_or_removed_refusal_keeps_the_correlation_justification() {
        let published = published_at(1, 16, BASELINE);
        let moved = shape_map(&[
            ("events/ready.json", "sha256:bbb"),
            ("events/tool_request.json", "sha256:ccc"),
        ]);
        let dropped = shape_map(&[("events/ready.json", "sha256:bbb")]);
        // Mixed: an addition alongside a moved shape still breaks correlation,
        // so the branch turns on altered/removed and never on added.
        let mixed = shape_map(&[
            ("commands/set_effort.json", "sha256:ddd"),
            ("events/ready.json", "sha256:bbb"),
            ("events/tool_request.json", "sha256:ccc"),
        ]);
        for (label, current) in [("altered", moved), ("removed", dropped), ("mixed", mixed)] {
            let refusal = wire_shape_refusal(&published, &current, 1, 16)
                .unwrap_or_else(|| panic!("{label} must refuse under a standing version"));
            assert!(
                refusal.contains(CORRELATION_CLAUSE),
                "{label} must keep the correlation justification: {refusal}"
            );
            assert!(
                !refusal.contains(DISCOVERY_CLAUSE),
                "{label} is not a discoverability problem: {refusal}"
            );
        }
    }

    #[test]
    fn a_corpus_publishing_no_wire_shapes_is_refused_unless_bootstrapping() {
        let current = shape_map(BASELINE);
        let refusal = wire_shape_verdict(None, &current, 1, 16, WireShapeBaseline::Required)
            .expect("a missing baseline must not be a silent skip");
        assert!(
            refusal.contains("--bootstrap-wire-shapes"),
            "the refusal must name the one explicit escape: {refusal}"
        );
        assert_eq!(
            wire_shape_verdict(None, &current, 1, 16, WireShapeBaseline::Bootstrap),
            None
        );
    }

    #[test]
    fn bootstrapping_still_compares_a_baseline_that_does_exist() {
        let published = published_at(1, 16, BASELINE);
        let moved = shape_map(&[
            ("events/ready.json", "sha256:bbb"),
            ("events/tool_request.json", "sha256:ccc"),
        ]);
        assert!(
            wire_shape_verdict(
                Some(&published),
                &moved,
                1,
                16,
                WireShapeBaseline::Bootstrap
            )
            .is_some(),
            "the bootstrap escape must not double as a blanket bypass"
        );
        assert_eq!(
            wire_shape_verdict(
                Some(&published),
                &shape_map(BASELINE),
                1,
                16,
                WireShapeBaseline::Bootstrap
            ),
            None
        );
    }

    #[test]
    fn the_gate_refuses_a_moved_shape_against_the_real_published_baseline() {
        let mut artifacts = generated_artifacts().unwrap();
        enforce_wire_shape_version(&artifacts, WireShapeBaseline::Required)
            .expect("the checked-in corpus must agree with its own generator");

        let mut manifest: Value = serde_json::from_slice(&artifacts["manifest.json"]).unwrap();
        manifest["wire_shapes"]["events/tool_request.json"] =
            json!(format!("sha256:{}", "e".repeat(64)));
        artifacts.insert("manifest.json".into(), canonical_json(&manifest).unwrap());

        let refusal = enforce_wire_shape_version(&artifacts, WireShapeBaseline::Required)
            .expect_err("a moved correlation anchor must not pass under a standing version")
            .to_string();
        // The bootstrap escape permits a MISSING baseline and nothing else, so
        // it must not get this change past the gate either.
        assert!(
            enforce_wire_shape_version(&artifacts, WireShapeBaseline::Bootstrap).is_err(),
            "--bootstrap-wire-shapes must not bless a change to an already published shape"
        );
        assert!(
            refusal.contains("events/tool_request.json"),
            "the refusal must name the moved type: {refusal}"
        );
        assert!(
            refusal.contains("CONTRACT_MAJOR"),
            "the refusal must name the version decision: {refusal}"
        );
    }

    #[test]
    fn every_generated_wire_type_publishes_exactly_one_shape_digest() {
        let artifacts = generated_artifacts().unwrap();
        let manifest: Value =
            serde_json::from_slice(artifacts.get("manifest.json").unwrap()).unwrap();
        let shapes = manifest["wire_shapes"].as_object().unwrap();
        let mut expected = COMMAND_SPECS
            .iter()
            .chain(EVENT_SPECS)
            .map(|spec| spec.path)
            .collect::<BTreeSet<_>>();
        expected.insert("compat/events/sub_agent_event.legacy.json");
        assert_eq!(
            shapes.keys().map(String::as_str).collect::<BTreeSet<_>>(),
            expected
        );
        for (path, digest) in shapes {
            assert!(
                digest
                    .as_str()
                    .is_some_and(|digest| digest.starts_with("sha256:") && digest.len() == 71),
                "{path} must publish a prefixed SHA-256 wire shape digest"
            );
        }
    }

    #[test]
    fn generated_negotiation_fixtures_replay_without_digest_recursion() {
        let artifacts = generated_artifacts().unwrap();
        let ready = artifacts.get("events/ready.json").unwrap();
        let ready_value: Value = serde_json::from_slice(ready).unwrap();
        let expected: ContractDescriptor =
            serde_json::from_value(ready_value["contract"].clone()).unwrap();
        let mut observer = HostContractObserver::new(expected.clone());
        assert_eq!(
            observer.observe_json_line(ready),
            Ok(HostObservation::Negotiated(expected.clone()))
        );

        assert!(matches!(
            observer.observe_json_line(
                artifacts
                    .get("adversarial/events/unknown-noncritical.jsonl")
                    .unwrap()
            ),
            Ok(HostObservation::DroppedUnknownNonCritical { .. })
        ));
        assert!(matches!(
            observer.observe_json_line(
                artifacts
                    .get("adversarial/events/unknown-critical.jsonl")
                    .unwrap()
            ),
            Err(HostObservationError::UnknownCriticalEvent { .. })
        ));

        let manifest: Value =
            serde_json::from_slice(artifacts.get("manifest.json").unwrap()).unwrap();
        assert_eq!(manifest["fixture_digest"], expected.fixture_digest);
        assert_eq!(
            manifest["capabilities"]["contract_negotiation"],
            "available"
        );
        assert_eq!(
            fixtures_digest(&artifacts).unwrap(),
            expected.fixture_digest
        );
    }

    /// wayland#605. `mcp_ready.already_connected` is omitted when false, so a
    /// host cannot feature-detect it by watching the wire: on a producer that
    /// predates it the field is absent on every frame, and on this one it is
    /// absent on every real connect. The named capability is the only thing
    /// that separates those two worlds, so publishing the field without
    /// declaring it ships an annotation no pinned host may trust.
    ///
    /// The coupling is asserted in that direction on purpose: the fixture
    /// premise is checked first, so if the field ever leaves the wire this
    /// test says so instead of demanding a capability for a promise the
    /// producer no longer keeps.
    #[test]
    fn the_mcp_ready_skip_annotation_is_named_in_the_manifest() {
        let artifacts = generated_artifacts().unwrap();
        let fixture: Value =
            serde_json::from_slice(artifacts.get("events/mcp_ready.json").unwrap()).unwrap();
        assert!(
            fixture.get("already_connected").is_some(),
            "the mcp_ready fixture no longer publishes `already_connected`; the capability \
             assertion below would be declaring a promise this producer does not keep: {fixture}"
        );

        let manifest: Value =
            serde_json::from_slice(artifacts.get("manifest.json").unwrap()).unwrap();
        assert_eq!(
            manifest["capabilities"]["mcp_ready_skip_annotation_v1"],
            Value::String("available".into()),
            "`mcp_ready` publishes `already_connected` but the manifest does not name a \
             capability for it, so a pinned host has no way to learn that an ABSENT flag now \
             means a real connect rather than an older Core"
        );
    }

    /// The comment block directly above [`CONTRACT_MINOR`] is this contract's
    /// decision log, and `contract_capabilities`' own doctrine is that the
    /// version moves once and every capability names itself. Nothing enforced
    /// that: wayland#605's first pass moved the constant 19 -> 20 with no entry
    /// and no capability, and the corpus check plus 378 protocol tests all
    /// passed. This is that enforcement.
    #[test]
    fn the_decision_log_explains_the_contract_minor_it_sits_above() {
        let log = decision_log_above_contract_minor(include_str!("generate.rs"));
        assert!(
            log.contains(&format!("-> {CONTRACT_MINOR}")),
            "CONTRACT_MINOR is {CONTRACT_MINOR} but the decision log above it never explains a \
             move to {CONTRACT_MINOR}: a pinned host is asked to re-pin with no recorded reason. \
             Log tail:\n{}",
            log.lines().rev().take(12).collect::<Vec<_>>().join("\n")
        );
        // Negative control in the same run: the log must not already claim a
        // version this producer has not shipped, which also proves the
        // assertion above can fail rather than matching anything.
        let unshipped = CONTRACT_MINOR + 1;
        assert!(
            !log.contains(&format!("-> {unshipped}")),
            "the decision log records a move to {unshipped} while CONTRACT_MINOR is still \
             {CONTRACT_MINOR}"
        );
    }

    /// The trailing run of `//` lines immediately above the `CONTRACT_MINOR`
    /// declaration. Panics rather than returning empty if the declaration is
    /// not found exactly once, so a rename reddens instead of passing on an
    /// empty string.
    fn decision_log_above_contract_minor(src: &str) -> String {
        // Assembled at runtime so this test's own source text cannot be
        // mistaken for the declaration it is looking for.
        let marker = format!("pub const {}: u64 =", "CONTRACT_MINOR");
        assert_eq!(
            src.matches(marker.as_str()).count(),
            1,
            "expected exactly one `{marker}` in generate.rs"
        );
        let head = &src[..src.find(marker.as_str()).unwrap()];
        let mut lines: Vec<&str> = head
            .lines()
            .rev()
            .take_while(|l| l.trim_start().starts_with("//"))
            .collect();
        lines.reverse();
        assert!(
            !lines.is_empty(),
            "there is no comment block above CONTRACT_MINOR at all"
        );
        lines.join("\n")
    }
}
