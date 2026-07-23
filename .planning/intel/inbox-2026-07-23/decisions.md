# Decisions (ADRs)

No ADR-typed documents were present in this ingest set. The three ingested docs
classified as DOC (2) and SPEC (1); none is an Architecture Decision Record and
none carries `locked: true`. Directional decisions asserted inside the handoffs
are recorded as context (see `context.md`) and as constraints (see
`constraints.md`), not as authoritative ADR decisions.

Notable directional stance carried from the SPEC (precedence 0), recorded for
traceability only:

## Repair native path as a "repaired 20-08 successor" (recommended, not locked)
- source: .planning/inbox/native-uat-repair-BRIEF.md (§6.6 open questions; MASTER-HANDOFF-phase20-native-parity.md §4)
- status: proposed
- decision: Repair the native path in place under a repaired 20-08 successor through the existing gate loop, rather than spinning native-parity into its own dedicated phase/milestone.
- scope: Phase 20 native-UAT closure sequencing
