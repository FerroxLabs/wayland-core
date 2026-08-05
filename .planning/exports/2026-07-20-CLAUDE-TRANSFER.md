# Wayland Core GSD Plan — Claude Transfer

This export is the live GSD plan as of 2026-07-20. It is deliberately labeled WIP: two MEDIUM planning findings remain, and no new product plan beyond 20-01 and 20-02 was completed overnight.

## Paste this into Claude Code

```text
You are taking over the Wayland Core Frontier GSD execution.

Repository planning snapshot: /Users/seandonahoe/dev/waylandcore-gsd-planning/wt-f20-unified-audit-repair
Branch: plan/f20-unified-audit-repair
Primary checkout /Users/seandonahoe/dev/waylandcore is heavily dirty. Never edit, clean, reset, rebase, or run execution there.

Read completely, in this order:
1. AGENTS.md
2. .planning/HANDOFF.json
3. .planning/phases/20-transactional-delegated-mutation/.continue-here.md
4. .planning/PROJECT.md
5. .planning/ROADMAP.md
6. .planning/REQUIREMENTS.md
7. Every Phase 20 PLAN and existing SUMMARY
8. .planning/scripts/test-phase20-proof-scripts.sh and every helper it invokes

Current product position: plans 20-01 and 20-02 are complete; 20-03 through 20-18 are not executed. The one reconciled product source is 94f014d039b8babf3f5926385a3bbc5cb5cf3c41, tree 49635e1678bd96e42353ab0f7f943ba87497e9d0.

Before product execution, fix exactly two remaining planning findings:
1. Bind independent-review source and reviewer IDs to actual stock-GSD execution history and the exact source/review plan, with hostile tests.
2. Re-bake installed Claude/GSD agents from .planning/config.json and prove model configuration is current. Preserve and re-test the installed execute-plan Resume/Start Fresh authority behavior.

Also correct .planning/STATE.md from its stale 14-plan counter to the actual 18-plan graph.

Run:
- git diff --check
- bash -n .planning/scripts/test-phase20-proof-scripts.sh
- .planning/scripts/test-phase20-proof-scripts.sh
- the standard GSD plan checker for Phase 20
- a fresh independent all-severity audit

Acceptance requires zero BLOCKER, CRITICAL, HIGH, MEDIUM, and LOW findings. Commit the accepted planning candidate. Then create a clean standalone clone with a real .git directory and execute 20-03 through 20-18 using standard GSD. Do not execute in this linked planning worktree.

Never run Cargo on this Mac. Authoritative Cargo proof runs on Hetzner through /Users/seandonahoe/.ratchet/harness/remote-cargo.sh against exact committed HEAD. Do not push, merge to main, release, deploy, close issues, publish a native ref, or dispatch native proof without Sean's explicit authorization. Plan 20-18 must stop for exact pending-digest authorization.

Do not restart planning or re-audit the entire historical program. Preserve F00-F19 and the current Phase 20 graph. Close the two findings and start 20-03.
```

## Exact state

| Item | State |
|---|---|
| Roadmap | Phases 20–30 |
| Current phase | Phase 20 — Transactional Delegated Mutation |
| Phase 20 plans | 18 |
| Complete | 20-01, 20-02 |
| Incomplete | 20-03 through 20-18 |
| Reconciled product source | `94f014d039b8babf3f5926385a3bbc5cb5cf3c41` |
| Reconciled product tree | `49635e1678bd96e42353ab0f7f943ba87497e9d0` |
| Planning helper hostile suite | PASS |
| Scoped helper audit | Zero findings |
| Whole-plan audit | BLOCKED by two MEDIUM findings |
| Native Windows/macOS UAT | Not run and not claimed |
| Push/main merge/release/deploy | Not performed |

## Phase 20 execution order

1. 20-01 — journal-authoritative transaction persistence — complete.
2. 20-02 — Windows AppContainer ACL lifecycle — complete.
3. 20-03 — cross-platform isolated mutation substrate.
4. 20-15 — independent review of 20-03.
5. 20-04 — production spawner propagation.
6. 20-06 — opaque live candidate seal.
7. 20-09 — independent review of 20-06.
8. 20-10 — hard-containment authority.
9. 20-11 — independent review of 20-10.
10. 20-12 — gate execution and receipt authority.
11. 20-13 — black-box gate and containment proof.
12. 20-14 — independent integrated audit.
13. 20-05 — orchestrator propagation, including Anvil/Council/Crucible/workflow CLI.
14. 20-07 — recoverable parent compare-and-swap landing.
15. 20-08 — complete delegated-mutation lifecycle and native-UAT machinery.
16. 20-16 — independent final construction review.
17. 20-17 — deterministic pending native-proof request.
18. 20-18 — Sean-authorized native plus aggregate acceptance and requirement closure.

The frontmatter dependencies in the individual PLAN files remain authoritative if this prose list and a plan ever disagree.

## Later roadmap

`20 → D1 → 21 → 22 → 23A → 23B/D2 → {24,25,26,27 bounded parallel} → 28 → 29 → 30`

- Phase 21: Child Authority and Budget Inheritance.
- Phase 22: Supervision, Durable Goals, Fleet, and Loops.
- Phase 23: Governed Continuous Personal Agency.
- Phase 24: Gateway, Automation, Channels, and Typed API.
- Phase 25: Remote Reach, Nodes, and Plugin Lifecycle.
- Phase 26: Migration, Export, Backup, and Restore.
- Phase 27: Multimodal, Browser, Generation, and Voice Contracts.
- Phase 28: Native Cross-Platform Certification.
- Phase 29: Supply Chain and Release Integrity.
- Phase 30: Continuous Scorecard and Frontier Review.

## Known reusable source

- `worktree-agent-f20-swarm-dispatch` at `2ab5ca32...`: clean salvage branch.
- `repair/f20-03-windows` at `d1e623cf...`: clean Windows repair branch.
- Treat both as reviewable salvage inputs. Do not merge them blindly or advance them as alternate candidates.

## Ground-truth rule

Repository state and executable tests outrank this export. If this handoff disagrees with the checked-out files, stop and reconcile the discrepancy rather than silently choosing one.
