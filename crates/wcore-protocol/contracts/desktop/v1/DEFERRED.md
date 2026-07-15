# Deferred Desktop contract adversarial cases

This v1.0 corpus foundation records the current 11-command and 33-event wire.
It does not invent authority that the current protocol does not carry.

- `unknown_critical_fail_closed`: deferred until events carry top-level
  `critical` and `contract_version` and Desktop has a contract-aware decoder.
- `version_mismatch_handshake`: deferred until `ready` advertises a versioned
  contract descriptor and schema digest.
- `ordering_duplicate_terminal_reducer`: deferred because ordinary current
  events have no producer event ID or monotonic sequence.
- `effective_execution_policy_revisions`: deferred; the current event is a
  launch snapshot without revision/change semantics.
- `workflow_node_child_lifecycle`: deferred; current workflow IDs collide on
  repeated runs and there is no node event, run ID, event ID, or sequence.
- `anvil_origin_replay_mutation_staleness`: deferred and unavailable. The
  legacy receipt is not promoted by this corpus.

Malformed command fixtures and the current unknown-type behavior are proved by
`desktop_contract_adversarial.rs`. Browser, CUA, and plugin event fixtures are
shape-only because no production emitter is proven at this source baseline.
