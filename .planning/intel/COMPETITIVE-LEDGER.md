# Competitive Capability Ledger v1

This control ledger exists before broad execution. It prevents F30 from first discovering product gaps.

## Maturity states

`ABSENT → SOURCE → CONFIGURED → CONSTRUCTED → REACHED → EFFECTIVE → OPERATOR_COMPLETE → PACKAGED_PROVEN`

Every capability row must record: stable coverage ID, owner (`core`, `protocol`, `desktop`, or shared), current maturity, security authority owner, exact evidence IDs, pinned Hermes/OpenClaw comparison baseline, delta, limitation, and last refresh phase. Source presence alone never earns effectiveness or parity.

## Admission rule

- Bootstrap and retroactively map accepted F03/F05 evidence before Phase 21 begins.
- Pin exact Hermes and OpenClaw versions before Phase 21; `UNPINNED` is an explicit open state, not a baseline.
- Refresh changed rows at every admitted phase.
- Contradictory live/customer evidence reopens the row and enters `FIELD-REGRESSIONS.md`.
- F30 independently reviews the accumulated ledger; it does not author the first comparison.
- CTRL-01 remains open until every active row uses the declared maturity enum and has a pinned peer baseline, security owner, exact evidence IDs, delta, limitation, and refresh phase.

## Initial coverage families

| Coverage IDs | Family | Owner | Security authority owner | Maturity | Evidence IDs | Hermes/OpenClaw baseline | Delta | Limitation | Last refresh | Next proof |
|---|---|---|---|---|---|---|---|---|---|---|
| AUTH-* | posture, approval, policy, sandbox, secrets, egress | core | core | SOURCE | F03/F05-REMAP-PENDING | UNPINNED | PENDING | Historical evidence is not yet mapped into this schema | Phase 20 recon | Phase 20 admission amendment |
| TXN-* | delegated workspace, journal, gates, parent CAS | core | core | SOURCE | F20-PENDING | UNPINNED | PENDING | Successor is not yet admitted or natively proven | Phase 20 recon | Phase 20 |
| GOAL-* | Goal, Task, Wait, Fleet, loop ownership | shared | core | SOURCE | F22A-F22D-REMAP-PENDING | UNPINNED | PENDING | Existing contracts require current activation and operator proof | Phase 20 recon | Phases 21-22 |
| CONT-* | governed skills, session recovery, memory, index, cache economics | shared | core | SOURCE | F23-PENDING | UNPINNED | PENDING | Continuous Agency outcomes are not yet admitted | Phase 20 recon | Phase 23A/23B |
| GATEWAY-* | service, automation, channels, typed API | shared | core | ABSENT | F24-PENDING | UNPINNED | PENDING | Operator-complete runtime is not yet built | Phase 20 recon | Phase 24 |
| REACH-* | backends, nodes, plugins | shared | core | SOURCE | F25-PENDING | UNPINNED | PENDING | Reference backend and plugin lifecycle are incomplete | Phase 20 recon | Phase 25 |
| PORT-* | import, export, backup, restore | shared | core | SOURCE | F26-PENDING | UNPINNED | PENDING | Reciprocal migration and recovery proof are incomplete | Phase 20 recon | Phase 26 |
| MEDIA-* | attachment, browser/CUA/web, generation, voice | shared | core | SOURCE | F27-PENDING | UNPINNED | PENDING | Readiness, credential, and packaged-native proof are incomplete | Phase 20 recon | Phase 27 |
| NATIVE-* | macOS/Linux/Windows packaged certification | shared | shared | SOURCE | F28-PENDING | UNPINNED | PENDING | No exact candidate has passed the full native matrix | Phase 20 recon | Phase 28 |
| SUPPLY-* | provenance, SBOM, signing, update, rollback | shared | shared | SOURCE | F29-PENDING | UNPINNED | PENDING | Clean-room release and rollback chain are incomplete | Phase 20 recon | Phase 29 |
