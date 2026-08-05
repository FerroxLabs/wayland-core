# Phase 28 Candidate Ledger

**GENERATED — do not edit.** Produced from `evidence/28-01/candidate.json` by
`.planning/scripts/f28-resolve-candidate.py --render`. The JSON is authoritative;
this file is its human rendering, so the two cannot disagree.

## 1. Candidate identity

| Field | Value |
|---|---|
| commit | `32e2f57d09fe4b287e513081862217dc9daa5901` |
| tree | `63ec0e6c36ff8e63789aab2f9760870304b671df` |
| provisional | YES |
| provisional reason | Pre-merge integration-branch tip, not a released candidate: lanes for phases 24, 26 and 23B were still executing when this was resolved. Plan 28-02 MUST re-resolve against the actual certification candidate. |
| KR-05 wedge repair (`455dd836`) present | PRESENT (455dd836 is an ancestor of the candidate; verified by git merge-base --is-ancestor) |

A commit alone is not a candidate. Commit and tree are bound together so a moved
branch cannot masquerade as the same candidate.

## 2. Surface-probe binary

| Field | Value |
|---|---|
| sha256 | `da69ae6f7fac9e61c7e9b8bc08407bbc957894136465730785ccd4e29cc65163` |
| path | `/root/wayland-p28/target/debug/wayland-core` |
| host | `Ubuntu-2404-noble-amd64-base` |
| build profile | `debug` |

This is the binary whose own command tree produced section 4. It is recorded
separately from the per-target release artifacts in section 3 and is NOT a
substitute for them.

## 3. Per-target binaries

Every target carries a digest or an explicit unbindable entry. An omission is
impossible by schema, so an OS family cannot appear certified on evidence that
never existed.

| Target | OS family | Artifact | Status | Digest / reason |
|---|---|---|---|---|
| `x86_64-unknown-linux-gnu` | linux | `wayland-core-x86_64-unknown-linux-gnu` | **unbindable** | CI run 30269095004 for this exact commit is status=queued (measured 2026-07-27); the release-binary matrix has produced no artifact yet. Re-run the resolver once that run completes. |
| `aarch64-unknown-linux-gnu` | linux | `wayland-core-aarch64-unknown-linux-gnu` | **unbindable** | CI run 30269095004 for this exact commit is status=queued (measured 2026-07-27); the release-binary matrix has produced no artifact yet. Re-run the resolver once that run completes. |
| `x86_64-apple-darwin` | macos | `wayland-core-x86_64-apple-darwin` | **unbindable** | CI run 30269095004 for this exact commit is status=queued (measured 2026-07-27); the release-binary matrix has produced no artifact yet. Re-run the resolver once that run completes. |
| `aarch64-apple-darwin` | macos | `wayland-core-aarch64-apple-darwin` | **unbindable** | CI run 30269095004 for this exact commit is status=queued (measured 2026-07-27); the release-binary matrix has produced no artifact yet. This target is obtainable from CI since d9c7683b and must be bound before the macOS leg is certified. |
| `x86_64-pc-windows-msvc` | windows | `wayland-core-x86_64-pc-windows-msvc` | **unbindable** | CI run 30269095004 for this exact commit is status=queued (measured 2026-07-27); the release-binary matrix has produced no artifact yet. Re-run the resolver once that run completes. |
| `aarch64-pc-windows-msvc` | windows | `wayland-core-aarch64-pc-windows-msvc` | **unbindable** | CI run 30269095004 for this exact commit is status=queued (measured 2026-07-27); the release-binary matrix has produced no artifact yet. Re-run the resolver once that run completes. |

## 4. Surface inventory — read off the binary

**116 surfaces** discovered by interrogating the binary's own
command tree. No feature name was read out of a planning document into this list.

| Surface | Entrypoint | Attributed to |
|---|---|---|
| `cmd:acp` | `wayland-core acp` | — |
| `cmd:acp/request` | `wayland-core acp request` | — |
| `cmd:acp/serve` | `wayland-core acp serve` | — |
| `cmd:agent` | `wayland-core agent` | — |
| `cmd:agent/create` | `wayland-core agent create` | — |
| `cmd:agent/delete` | `wayland-core agent delete` | — |
| `cmd:agent/edit` | `wayland-core agent edit` | — |
| `cmd:agent/list` | `wayland-core agent list` | — |
| `cmd:agent/show` | `wayland-core agent show` | — |
| `cmd:auth` | `wayland-core auth` | 26 |
| `cmd:auth/add` | `wayland-core auth add` | 26 |
| `cmd:auth/list` | `wayland-core auth list` | 26 |
| `cmd:auth/login` | `wayland-core auth login` | 26 |
| `cmd:auth/logout` | `wayland-core auth logout` | 26 |
| `cmd:auth/remove` | `wayland-core auth remove` | 26 |
| `cmd:auth/status` | `wayland-core auth status` | 26 |
| `cmd:backend` | `wayland-core backend` | 25 |
| `cmd:backend/cancel` | `wayland-core backend cancel` | 25 |
| `cmd:backend/diff` | `wayland-core backend diff` | 25 |
| `cmd:backend/list` | `wayland-core backend list` | 25 |
| `cmd:backend/orphans` | `wayland-core backend orphans` | 25 |
| `cmd:backend/probe` | `wayland-core backend probe` | 25 |
| `cmd:backend/receipt` | `wayland-core backend receipt` | 25 |
| `cmd:backend/run` | `wayland-core backend run` | 25 |
| `cmd:backend/scan` | `wayland-core backend scan` | 25 |
| `cmd:backup` | `wayland-core backup` | 26 |
| `cmd:backup/create` | `wayland-core backup create` | 26 |
| `cmd:backup/digest` | `wayland-core backup digest` | 26 |
| `cmd:backup/recover` | `wayland-core backup recover` | 26 |
| `cmd:backup/restore` | `wayland-core backup restore` | 26 |
| `cmd:backup/verify` | `wayland-core backup verify` | 26 |
| `cmd:cron` | `wayland-core cron` | 24 |
| `cmd:cron/add` | `wayland-core cron add` | 24 |
| `cmd:cron/daemon` | `wayland-core cron daemon` | 24 |
| `cmd:cron/disable` | `wayland-core cron disable` | 24 |
| `cmd:cron/enable` | `wayland-core cron enable` | 24 |
| `cmd:cron/history` | `wayland-core cron history` | 24 |
| `cmd:cron/list` | `wayland-core cron list` | 24 |
| `cmd:cron/logs` | `wayland-core cron logs` | 24 |
| `cmd:cron/remove` | `wayland-core cron remove` | 24 |
| `cmd:cron/status` | `wayland-core cron status` | 24 |
| `cmd:crucible` | `wayland-core crucible` | — |
| `cmd:fetch` | `wayland-core fetch` | — |
| `cmd:forge` | `wayland-core forge` | — |
| `cmd:gateway` | `wayland-core gateway` | 24 |
| `cmd:gateway/drain` | `wayland-core gateway drain` | 24 |
| `cmd:gateway/install` | `wayland-core gateway install` | 24 |
| `cmd:gateway/restart` | `wayland-core gateway restart` | 24 |
| `cmd:gateway/run` | `wayland-core gateway run` | 24 |
| `cmd:gateway/start` | `wayland-core gateway start` | 24 |
| `cmd:gateway/status` | `wayland-core gateway status` | 24 |
| `cmd:gateway/stop` | `wayland-core gateway stop` | 24 |
| `cmd:gateway/uninstall` | `wayland-core gateway uninstall` | 24 |
| `cmd:image` | `wayland-core image` | — |
| `cmd:init` | `wayland-core init` | — |
| `cmd:mcp-serve` | `wayland-core mcp-serve` | — |
| `cmd:migrate` | `wayland-core migrate` | — |
| `cmd:migrate/hermes` | `wayland-core migrate hermes` | — |
| `cmd:migrate/openclaw` | `wayland-core migrate openclaw` | — |
| `cmd:models` | `wayland-core models` | — |
| `cmd:models/list` | `wayland-core models list` | — |
| `cmd:node` | `wayland-core node` | 25 |
| `cmd:node/advertise` | `wayland-core node advertise` | 25 |
| `cmd:node/attribution` | `wayland-core node attribution` | 25 |
| `cmd:node/identity` | `wayland-core node identity` | 25 |
| `cmd:node/list` | `wayland-core node list` | 25 |
| `cmd:node/pair` | `wayland-core node pair` | 25 |
| `cmd:node/probe` | `wayland-core node probe` | 25 |
| `cmd:node/revoke` | `wayland-core node revoke` | 25 |
| `cmd:node/show` | `wayland-core node show` | 25 |
| `cmd:node/submit` | `wayland-core node submit` | 25 |
| `cmd:plugin` | `wayland-core plugin` | 25 |
| `cmd:plugin/approve` | `wayland-core plugin approve` | 25 |
| `cmd:plugin/available` | `wayland-core plugin available` | 25 |
| `cmd:plugin/inspect` | `wayland-core plugin inspect` | 25 |
| `cmd:plugin/install` | `wayland-core plugin install` | 25 |
| `cmd:plugin/list` | `wayland-core plugin list` | 25 |
| `cmd:plugin/marketplace` | `wayland-core plugin marketplace` | 25 |
| `cmd:plugin/new` | `wayland-core plugin new` | 25 |
| `cmd:plugin/publish` | `wayland-core plugin publish` | 25 |
| `cmd:plugin/recover` | `wayland-core plugin recover` | 25 |
| `cmd:plugin/remove` | `wayland-core plugin remove` | 25 |
| `cmd:plugin/rollback` | `wayland-core plugin rollback` | 25 |
| `cmd:plugin/sign` | `wayland-core plugin sign` | 25 |
| `cmd:plugin/test` | `wayland-core plugin test` | 25 |
| `cmd:plugin/update` | `wayland-core plugin update` | 25 |
| `cmd:plugin/verify` | `wayland-core plugin verify` | 25 |
| `cmd:profile` | `wayland-core profile` | — |
| `cmd:profile/create` | `wayland-core profile create` | — |
| `cmd:profile/delete` | `wayland-core profile delete` | — |
| `cmd:profile/export` | `wayland-core profile export` | — |
| `cmd:profile/import` | `wayland-core profile import` | — |
| `cmd:profile/list` | `wayland-core profile list` | — |
| `cmd:profile/rename` | `wayland-core profile rename` | — |
| `cmd:profile/show` | `wayland-core profile show` | — |
| `cmd:profile/use` | `wayland-core profile use` | — |
| `cmd:project-context` | `wayland-core project-context` | — |
| `cmd:self-update` | `wayland-core self-update` | — |
| `cmd:session` | `wayland-core session` | — |
| `cmd:session/cancel` | `wayland-core session cancel` | — |
| `cmd:session/checkpoint` | `wayland-core session checkpoint` | — |
| `cmd:session/export` | `wayland-core session export` | — |
| `cmd:session/fork` | `wayland-core session fork` | — |
| `cmd:session/list` | `wayland-core session list` | — |
| `cmd:session/reconcile` | `wayland-core session reconcile` | — |
| `cmd:session/retain` | `wayland-core session retain` | — |
| `cmd:session/retry` | `wayland-core session retry` | — |
| `cmd:session/rewind` | `wayland-core session rewind` | — |
| `cmd:session/search` | `wayland-core session search` | — |
| `cmd:session/show` | `wayland-core session show` | — |
| `cmd:setup` | `wayland-core setup` | — |
| `cmd:swarm` | `wayland-core swarm` | — |
| `cmd:workflow` | `wayland-core workflow` | — |
| `cmd:workflow/list` | `wayland-core workflow list` | — |
| `cmd:workflow/run` | `wayland-core workflow run` | — |
| `cmd:workflow/validate` | `wayland-core workflow validate` | — |

## 5. Phase attribution

Explicit claims (`wayland-core <verb>`) can assert claimed-but-absent. Bare-only
claims (`<verb> <sub>`) attribute but never accuse — the form is noisier, and a
noisy instrument must not be what accuses a phase.

| Phase | Artifacts | Claimed (explicit) | Claimed (bare only) |
|---|---|---|---|
| 24 | 5 | `cron` | `gateway`, `pid`, `wcore-contract` |
| 25 | 4 | `backend`, `node`, `plugin` | `unrecognized` |
| 26 | 2 | `auth` | `backup` |
| 27 | 5 | — | — |

## 6. Findings

Class `attribution-weak` records a limit of the resolver's claim extractor, NOT
a property of the product: the surface IS certified, only its attribution is
unproven.

| id | class | Phase 28 severity | Subject |
|---|---|---|---|
| `F-28-01-R001` | present-but-unclaimed | MEDIUM | surface `wayland-core acp` is exposed by the candidate binary and appears NOWHERE in any phase 24-27 artifact |
| `F-28-01-R002` | present-but-unclaimed | MEDIUM | surface `wayland-core agent` is exposed by the candidate binary and appears NOWHERE in any phase 24-27 artifact |
| `F-28-01-R003` | present-but-unclaimed | MEDIUM | surface `wayland-core crucible` is exposed by the candidate binary and appears NOWHERE in any phase 24-27 artifact |
| `F-28-01-R004` | attribution-weak | LOW | surface `wayland-core fetch` is exposed by the candidate binary and is discussed by phase artifact(s) 24,26, but not in a form this resolver recognises as a claim |
| `F-28-01-R005` | present-but-unclaimed | MEDIUM | surface `wayland-core forge` is exposed by the candidate binary and appears NOWHERE in any phase 24-27 artifact |
| `F-28-01-R006` | attribution-weak | LOW | surface `wayland-core image` is exposed by the candidate binary and is discussed by phase artifact(s) 27, but not in a form this resolver recognises as a claim |
| `F-28-01-R007` | present-but-unclaimed | MEDIUM | surface `wayland-core init` is exposed by the candidate binary and appears NOWHERE in any phase 24-27 artifact |
| `F-28-01-R008` | present-but-unclaimed | MEDIUM | surface `wayland-core mcp-serve` is exposed by the candidate binary and appears NOWHERE in any phase 24-27 artifact |
| `F-28-01-R009` | attribution-weak | LOW | surface `wayland-core migrate` is exposed by the candidate binary and is discussed by phase artifact(s) 26, but not in a form this resolver recognises as a claim |
| `F-28-01-R010` | present-but-unclaimed | MEDIUM | surface `wayland-core models` is exposed by the candidate binary and appears NOWHERE in any phase 24-27 artifact |
| `F-28-01-R011` | attribution-weak | LOW | surface `wayland-core profile` is exposed by the candidate binary and is discussed by phase artifact(s) 24,26, but not in a form this resolver recognises as a claim |
| `F-28-01-R012` | present-but-unclaimed | MEDIUM | surface `wayland-core project-context` is exposed by the candidate binary and appears NOWHERE in any phase 24-27 artifact |
| `F-28-01-R013` | present-but-unclaimed | MEDIUM | surface `wayland-core self-update` is exposed by the candidate binary and appears NOWHERE in any phase 24-27 artifact |
| `F-28-01-R014` | attribution-weak | LOW | surface `wayland-core session` is exposed by the candidate binary and is discussed by phase artifact(s) 24,27, but not in a form this resolver recognises as a claim |
| `F-28-01-R015` | attribution-weak | LOW | surface `wayland-core setup` is exposed by the candidate binary and is discussed by phase artifact(s) 24,26, but not in a form this resolver recognises as a claim |
| `F-28-01-R016` | present-but-unclaimed | MEDIUM | surface `wayland-core swarm` is exposed by the candidate binary and appears NOWHERE in any phase 24-27 artifact |
| `F-28-01-R017` | attribution-weak | LOW | surface `wayland-core workflow` is exposed by the candidate binary and is discussed by phase artifact(s) 24,26, but not in a form this resolver recognises as a claim |

## 7. Recorded inputs

`--verify-reproducible` re-resolves from exactly these and compares bytes.

| Role | Path | sha256 |
|---|---|---|
| surface_capture | `.planning/phases/28-native-cross-platform-certification/evidence/28-01/surface-capture.txt` | `37f96ab8eaef4cd3…` |
| phase_artifact_24 | `.planning/phases/24-gateway-automation-channels-typed-api/24-01-SUMMARY.md` | `2cf10f89f173fc25…` |
| phase_artifact_24 | `.planning/phases/24-gateway-automation-channels-typed-api/24-02-SUMMARY.md` | `8c44f3be813cc8fe…` |
| phase_artifact_24 | `.planning/phases/24-gateway-automation-channels-typed-api/24-B-SUMMARY.md` | `dd4cfe254b5b5c36…` |
| phase_artifact_24 | `.planning/phases/24-gateway-automation-channels-typed-api/24-C-SUMMARY.md` | `a3b6c9f5efd01a8d…` |
| phase_artifact_24 | `.planning/phases/24-gateway-automation-channels-typed-api/24-PHASE-REPORT.md` | `bda192d6d19aedce…` |
| phase_artifact_25 | `.planning/phases/25-remote-reach-nodes-plugin-lifecycle/25-01-SUMMARY.md` | `c40626b083382f92…` |
| phase_artifact_25 | `.planning/phases/25-remote-reach-nodes-plugin-lifecycle/25-02-SUMMARY.md` | `accb22ce6ad3504d…` |
| phase_artifact_25 | `.planning/phases/25-remote-reach-nodes-plugin-lifecycle/25-03-SUMMARY.md` | `7e99eeba681b7043…` |
| phase_artifact_25 | `.planning/phases/25-remote-reach-nodes-plugin-lifecycle/25-04-SUMMARY.md` | `9b402eb0bce41ecc…` |
| phase_artifact_26 | `.planning/phases/26-migration-export-backup-restore/26-01-SUMMARY.md` | `7baf42afec7fc5fa…` |
| phase_artifact_26 | `.planning/phases/26-migration-export-backup-restore/26-03-SUMMARY.md` | `d34c46eb4940e32c…` |
| phase_artifact_27 | `.planning/phases/27-multimodal-browser-generation-voice/27-01-SUMMARY.md` | `600d06bea6c9f240…` |
| phase_artifact_27 | `.planning/phases/27-multimodal-browser-generation-voice/27-02-SUMMARY.md` | `bca114866dc9bf39…` |
| phase_artifact_27 | `.planning/phases/27-multimodal-browser-generation-voice/27-03-SUMMARY.md` | `d6deaf6193bb2401…` |
| phase_artifact_27 | `.planning/phases/27-multimodal-browser-generation-voice/27-04-SUMMARY.md` | `c695ca8dd5f990c7…` |
| phase_artifact_27 | `.planning/phases/27-multimodal-browser-generation-voice/27-PHASE-VERDICT.md` | `da1591d0c79a8b85…` |
