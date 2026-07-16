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
pub const CONTRACT_MINOR: u64 = 8;
pub const GENERATOR_VERSION: &str = "wcore-desktop-contract-gen/11";
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
            let items = match item_schemas.as_slice() {
                [] => json!({}),
                [schema] => schema.clone(),
                _ => json!({"oneOf": item_schemas}),
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
            json!({"enum": ["continue", "reconcile", "cancel"], "type": "string"})
        }
        ("resolve_interrupted_approval", "decision") => {
            json!({"enum": ["approve", "deny"], "type": "string"})
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

fn schema_for(
    specs: &[WireSpec],
    fixtures: &BTreeMap<String, Value>,
    legacy_child: Option<&Value>,
    title: &str,
) -> Value {
    let mut one_of = Vec::with_capacity(specs.len() + 1);
    for spec in specs {
        let fixture = fixtures
            .get(spec.path)
            .unwrap_or_else(|| panic!("missing canonical fixture {}", spec.path));
        if spec.wire_type == "sub_agent_event" {
            one_of.push(schema_branch(spec, fixture));
            let legacy = legacy_child.expect("legacy sub-agent fixture must be present");
            let mut legacy_branch = schema_branch(
                &WireSpec {
                    wire_type: "sub_agent_event",
                    path: "compat/events/sub_agent_event.legacy.json",
                    required: &["type", "parent_call_id", "agent_name", "inner"],
                    criticality: spec.criticality,
                    correlation: spec.correlation,
                    capability: spec.capability,
                },
                legacy,
            );
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
            one_of.push(legacy_branch);
        } else {
            one_of.push(schema_branch(spec, fixture));
        }
    }
    json!({
        "$schema": "https://json-schema.org/draft/2020-12/schema",
        "oneOf": one_of,
        "title": title
    })
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
        (
            "host_delegated_delivery".into(),
            ContractCapabilityStatus::Available,
        ),
        ("plugin_events".into(), ContractCapabilityStatus::ShapeOnly),
        (
            "semantic_failover_receipts".into(),
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
pub fn generated_artifacts() -> ContractResult<BTreeMap<String, Vec<u8>>> {
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
    let legacy_child =
        compatibility_schema_fixtures.get("compat/events/sub_agent_event.legacy.json");
    let command_schema_title =
        format!("Desktop-consumed HostCommand v{CONTRACT_MAJOR}.{CONTRACT_MINOR}");
    let event_schema_title =
        format!("Desktop-consumed CoreEvent v{CONTRACT_MAJOR}.{CONTRACT_MINOR}");
    let command_schema = schema_for(
        COMMAND_SPECS,
        &command_schema_fixtures,
        None,
        &command_schema_title,
    );
    let event_schema = schema_for(
        EVENT_SPECS,
        &event_schema_fixtures,
        legacy_child,
        &event_schema_title,
    );
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
            "runtime_diagnostics": "1.0",
            "semantic_failover_receipts": "1.0",
            "turn_recovery": "1.0",
            "workflow_lifecycle": "1.0"
        },
        "schema_digest": schema_digest,
        "source_inputs": SOURCE_INPUTS,
        "source_inputs_digest": source_inputs_digest
    });
    artifacts.insert("manifest.json".into(), canonical_json(&manifest)?);

    Ok(artifacts)
}

pub fn write_contract() -> ContractResult<()> {
    let root = contract_root();
    let artifacts = generated_artifacts()?;
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
}
