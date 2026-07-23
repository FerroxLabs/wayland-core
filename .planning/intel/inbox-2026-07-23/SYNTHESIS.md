# Synthesis Summary — inbox-2026-07-23

Single entry point for downstream consumers (ferrox-roadmapper). Merge mode.
Reconciled against `.planning/{STATE,ROADMAP,REQUIREMENTS,PROJECT}.md`.

## Docs synthesized: 3
- DOC (2): MASTER-HANDOFF-frontier-v2-PROGRAM.md (precedence 1), MASTER-HANDOFF-phase20-native-parity.md (precedence 2)
- SPEC (1): native-uat-repair-BRIEF.md (precedence 0, manifest-authoritative)
- All high-confidence. Cross-ref graph acyclic (frontier → {phase20, brief}; phase20 → brief). No cycles, no UNKNOWNs.

## What these docs ARE
A CURRENT-STATE consolidation/status handoff for an already-planned program
(Frontier Candidate v2, F20→F30), plus a repair SPEC for the Phase-20 native-UAT
blocker. NOT new program scope. Phases 21–30 and their requirements already exist
in ROADMAP.md/REQUIREMENTS.md and were treated as overlap (INFO), not additions.

## Decisions locked: 0
No ADRs, no `locked` decisions. One proposed (non-locked) directional stance
recorded: repair native as a "repaired 20-08 successor" (decisions.md).

## Requirements extracted: 12 (all NEW, additive)
REQ-native-r1…r12 — the Phase-20 native-repair requirements from the SPEC §6.
They refine Phase-20 Success Criterion #3; they do NOT duplicate existing
F20/F21…F30 requirements. See requirements.md.

## Constraints: 8
Security-boundary / cross-platform-discipline / no-drift / non-goals /
defect-inventory (4.A–4.G) / native acceptance criteria / infra facts /
Sean-only gates. Types: nfr + protocol. See constraints.md.

## Context topics: 9
Program objective + parity thesis, current state, stall root cause, phase map,
admission controls, remaining spine, Phase-20 build record, open research
questions, document map. See context.md.

## Conflicts: 0 blockers, 4 warnings, 6 info
See INGEST-CONFLICTS.md. The four WARNINGs are all inbox-vs-existing CURRENT-STATE
contradictions requiring human reconciliation before routing:
1. Plan count: inbox/STATE.md say 16/18; ROADMAP.md progress table still says 2/18 (stale).
2. Candidate SHA `6937ef6`/`be84bd2` not recorded in STATE.md (lineage ambiguity).
3. Native status + runner availability: STATE.md (20-17 blocked, runners offline, native never run) vs inbox (20-18 ran RED, runners online, macOS 8/8 exists).
4. Next-plan/sequencing: STATE.md next = 20-17; inbox inserts native-repair + fresh 20-16 first.

## Intel files
- /Users/seandonahoe/dev/waylandcore-ferrox/.planning/intel/inbox-2026-07-23/decisions.md
- /Users/seandonahoe/dev/waylandcore-ferrox/.planning/intel/inbox-2026-07-23/requirements.md
- /Users/seandonahoe/dev/waylandcore-ferrox/.planning/intel/inbox-2026-07-23/constraints.md
- /Users/seandonahoe/dev/waylandcore-ferrox/.planning/intel/inbox-2026-07-23/context.md
- /Users/seandonahoe/dev/waylandcore-ferrox/.planning/intel/inbox-2026-07-23/INGEST-CONFLICTS.md

## Status
AWAITING USER — 4 competing current-state variants need human reconciliation
before the roadmapper routes. No blockers; intel is safe to read.
