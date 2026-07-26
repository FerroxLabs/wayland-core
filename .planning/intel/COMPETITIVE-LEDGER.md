# Competitive Capability Ledger v1 — CTRL-01

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

---

## Pinned peer baselines

Both peer baselines are **PINNED** as of 2026-07-26. Every pin below was read directly from a
read-only local checkout on 2026-07-26; nothing here is recalled, inferred, or fetched from the
network. Each pin names the exact file and field or the exact git command that produced it.

**Baseline token `BASE-2026-07-13`** — the frozen comparison baseline. This is the snapshot pair
that the accepted frontier evaluation program and gap audit were measured against
(`docs/design/2026-07-13-wayland-core-frontier-evaluation-program.md:7`,
`docs/design/2026-07-13-wayland-core-frontier-gap-audit-and-execution-plan.md:6`). Every delta
recorded in this ledger is bound to it.

| Peer | Repository | Baseline commit | Exact version | Version pin source | Commit date |
|---|---|---|---|---|---|
| Hermes Agent | `https://github.com/NousResearch/hermes-agent.git` | `dbe734beff0caf5e8ee2acbe4277db7f6cf84a21` | **0.17.0** | `git show dbe734be:pyproject.toml` → line 10 `version = "0.17.0"` | 2026-06-27 |
| OpenClaw | `https://github.com/openclaw/openclaw.git` | `11a0ad10e91a50d5a0e636494eea4d7ad3eaf9fc` | **2026.6.2** | `git show 11a0ad10:package.json` → line 3 `"version": "2026.6.2"` | 2026-06-16 |

Both baseline commits were verified to **resolve and be ancestors of local HEAD**
(`git cat-file -t` → `commit`; `git merge-base --is-ancestor <base> HEAD` → true) in the
checkouts at `/Users/seandonahoe/dev/resources/hermes-agent` and
`/Users/seandonahoe/dev/resources/openclaw`, both with clean working trees
(`git status --short` → empty). The recovered Hermes version `0.17.0` independently corroborates
the string already recorded at
`docs/design/2026-07-13-wayland-core-frontier-evaluation-program.md:379`. The OpenClaw version
`2026.6.2` was **not** recorded in any program document and is newly recovered here; before this
refresh OpenClaw had a commit pin but no version label anywhere in the repo.

**Declared refresh candidate `HEAD-2026-07-26`** — the newer snapshot on disk, recorded so the
next refresh has an exact forward target. It is **not** the baseline for any delta in this ledger.

| Peer | HEAD commit | Version | Version pin source | `git describe --tags` | HEAD date |
|---|---|---|---|---|---|
| Hermes Agent | `d59b79fadd1e9edd7afc5c679cc3b143838e7c01` | 0.18.2 | `pyproject.toml:10` | `v2026.7.7.2-1200-gd59b79fad` | 2026-07-17 |
| OpenClaw | `3659c85e534fdb8b8ce6b7505a83d92cc2e4df8e` | 2026.7.2 | `package.json:3` | `release-publish/ced50f88e928-20260717-311-g3659c85e53` | 2026-07-18 |

Baseline-to-HEAD drift at the time of pinning: Hermes `0.17.0 → 0.18.2`; OpenClaw
`2026.6.2 → 2026.7.2`. Deltas below are **not** re-measured against `HEAD-2026-07-26`.

**Peers not in the CTRL-01 contract.** `gemini-cli` and `grok-build` are also checked out at
`/Users/seandonahoe/dev/resources/`. CTRL-01, `REQUIREMENTS.md` CTRL-01/F30-01/F30-03, and
`ROADMAP.md:83` all declare the peer set as Hermes and OpenClaw only. No family row references
either tool, so neither was added — widening the declared peer set is a change to the control's
contract, not a refresh of it.

## Evidence ID index

| Evidence ID | Artifact |
|---|---|
| `F03-RECEIPT@1c644ccd` | `docs/design/2026-07-13-wayland-core-f03-evidence-receipt.md`; implementation source `1c644ccdee8180bd2eded312d391f486be99902d` on `frontier/m0` |
| `F05-RECEIPT@0825c92d` | `docs/design/2026-07-13-wayland-core-f05-capability-activation-receipt.md`; implementation source `0825c92d42fe1777822e2c3463f9eb581ba5cd5d` on `frontier/m0` |
| `F05-TRUTH-{n}` | Row `{n}` of the F05 startup truth table, `…-f05-capability-activation-receipt.md` §2 |
| `F20-SEAL@01a5b0ae` | Phase 20 close, SHA `01a5b0ae459c9d5088cfd7e41271a5d4ece1b9bb` (tree `4a5247ca`); `cargo nextest` 11519/11519 passed, 48 skipped. Logs: `phases/20-transactional-delegated-mutation/20-56-evidence/{build,test}-01a5b0ae-GREEN.log.gz` |
| `F20A-SEAL@9821ef76` | Phase 20A close, SHA `9821ef7603ac1e687b600cda591af1657c883484` (tree `0a1267a9`, tag `f20a-candidate-9821ef76`) |
| `RUN-30184651330` | `nightly-windows-soak` `workflow_dispatch` run `30184651330`, 2026-07-26. Windows job `89747993276` 6/6 PASS; macOS job `89747992986` 8/8 PASS; both at nonce `96c91107…`. Detail: `phases/20A-native-windows-macos-uat/20A-04-SUMMARY.md` §13 |
| `GAP-AUDIT-2026-07-13 §3` | Comparative scorecard, `docs/design/2026-07-13-wayland-core-frontier-gap-audit-and-execution-plan.md` §3, table at lines 64-90. Derived from direct inspection of the `BASE-2026-07-13` snapshots. **Static source comparison, not a runtime benchmark.** |
| `PEER-PROBE-2026-07-26` | Structural probes (`git ls-tree`, `git grep`) executed against both peer trees **at the `BASE-2026-07-13` commits** on 2026-07-26. Records presence/absence of a counterpart, never a performance claim. |

---

## Initial coverage families

Delta column convention: the verdict is from `GAP-AUDIT-2026-07-13 §3` where one exists, followed
by the `PEER-PROBE-2026-07-26` structural counterpart observed at the pinned baseline. Both are
static-source statements. No cell below asserts a measured runtime, cost, or success-rate number —
F30-03 owns that and has not run.

| Coverage IDs | Family | Owner | Security authority owner | Maturity | Evidence IDs | Hermes/OpenClaw baseline | Delta | Limitation | Last refresh | Next proof |
|---|---|---|---|---|---|---|---|---|---|---|
| AUTH-* | posture, approval, policy, sandbox, secrets, egress | core | core | CONSTRUCTED | `F03-RECEIPT@1c644ccd`; `F05-RECEIPT@0825c92d`; `F05-TRUTH-6` (Delegate isolation) | `BASE-2026-07-13` — Hermes 0.17.0 @ `dbe734be`; OpenClaw 2026.6.2 @ `11a0ad10` | Sandbox/egress: **Core architectural lead, operationally unproven**; permission UX: **mixed/behind in product semantics** (`GAP-AUDIT §3`). Probe: OpenClaw ships `packages/net-policy`, `src/security`, `src/secrets`, a documented sandbox surface (`docs/gateway/sandboxing.md`, `docs/gateway/sandbox-vs-tool-policy-vs-elevated.md`) and a sandbox CI smoke (`.github/workflows/sandbox-common-smoke.yml`); Hermes has **no** dedicated sandbox module at baseline — only `tests/agent/test_file_safety_sandbox_mirror.py` and `tests/tools/test_modal_sandbox_fixes.py`, i.e. isolation is delegated to its execution backends | `F05-TRUTH-6` records **Delegate isolation as "Unavailable: isolation not enforced"** at `0825c92d` — an honest negative, not a pass. `F03` records provider attempts/retries/tokens/cache, egress/filesystem deltas and resource peaks as `Unavailable`, so those AUTH measurements have no observed value. Local receipts are non-authoritative by construction; no CI trust root is bound | Phase 20 + 20A close, 2026-07-26 | Phase 21 (F21-01…04 authority intersection) |
| TXN-* | delegated workspace, journal, gates, parent CAS | core | core | EFFECTIVE | `F20-SEAL@01a5b0ae`; `F20A-SEAL@9821ef76`; `RUN-30184651330` | `BASE-2026-07-13` — Hermes 0.17.0 @ `dbe734be`; OpenClaw 2026.6.2 @ `11a0ad10` | Autonomous coding: **Core architectural lead, runtime certification required** (`GAP-AUDIT §3`). Probe: **no counterpart in either peer baseline.** No delegated-workspace/gated-merge/parent-CAS lifecycle exists in Hermes @ `dbe734be` (its `git worktree` references are LSP workspace management — `agent/lsp/workspace.py`, `agent/lsp/manager.py` — plus CI lint) or OpenClaw @ `11a0ad10` (references are `src/infra/update-runner.test.ts` and two plugin-install tests). This is Core's clearest unique capability | Aggregate 11519/11519 proof is **Linux-only** (`TEST-AUDIT.md`); native evidence is the 20A lifecycle proof, not a packaged E5 certification. `REQ-native-r12` and `r13` remain **OPEN** — no fresh 20-16 review at `9821ef76` and no schema-validated per-reviewer review artifact for this candidate. Not OPERATOR_COMPLETE: no operator-facing supervision surface | Phase 20 + 20A close, 2026-07-26 | Phase 28 (F28 native certification), Phase 29 (F29 supply chain) |
| GOAL-* | Goal, Task, Wait, Fleet, loop ownership | shared | core | SOURCE | `F05-TRUTH-2` (Mid-flight monitor); `F05-TRUTH-4` (Learned policy) | `BASE-2026-07-13` — Hermes 0.17.0 @ `dbe734be`; OpenClaw 2026.6.2 @ `11a0ad10` | Formal orchestration: **Core lead in primitives**; durable async agents: **Core behind, especially OpenClaw**; loop and spend governance: **partially landed, not complete** (`GAP-AUDIT §3`). Probe: OpenClaw ships a durable task plane — `src/tasks/` with `task-completion-contract.ts`, `detached-task-runtime.ts`, `detached-task-runtime-state.ts`, `cron-task-cancel.ts` — plus `src/commitments/` (heartbeat policy, extraction, store, runtime) and `src/cron`, `src/flows`. Hermes ships `cron/` and `agent/` spawners. Core has the primitives but no durable Goal/Task/Wait kernel | Two of the eight F05-audited capabilities in this family are proven **UNAVAILABLE with the runtime path unwired** at `0825c92d`. Existing contracts require current activation and operator proof; no Phase 21/22 execution has occurred | Phase 20 + 20A close, 2026-07-26 | Phases 21-22 (F21-01…04, F22-01…07) |
| CONT-* | governed skills, session recovery, memory, index, cache economics | shared | core | REACHED | `F05-RECEIPT@0825c92d`; `F05-TRUTH-5` (Smart handoff); `F05-TRUTH-7` (Procedure skill drafting); `F05-TRUTH-8` (Legacy auto-skill drafting); `F05-TRUTH-1` (Pricing refresher); `F05-TRUTH-3` (Cooldown tracker) | `BASE-2026-07-13` — Hermes 0.17.0 @ `dbe734be`; OpenClaw 2026.6.2 @ `11a0ad10` | Memory: **Core architectural lead, outcome proof needed**; self-improving skills: **Core stronger research machinery, weaker governance/completion**; crash recovery: **Core behind** — WAL is not a complete turn/event journal (`GAP-AUDIT §3`). Probe: OpenClaw ships `src/memory`, `src/sessions`, `src/transcripts`, `src/trajectory`, `src/skills`, `packages/memory-host-sdk`; Hermes ships `skills/`, `optional-skills/`, `agent/` memory paths and `agent/curator_backup.py` | **REACHED, not EFFECTIVE.** Three capabilities emit runtime outcome proof only after a real side effect (episode persistence, quarantine staging, draft write) — that is reach, not proven operator outcome. Two cache-economics capabilities (pricing refresher, cooldown tracker) are **UNAVAILABLE: no production constructor**. Governed promotion/revoke/rollback (F23-01) is unbuilt; F06 made generated skills inert as containment, which is not governance | Phase 20 + 20A close, 2026-07-26 | Phase 23A/23B (F23-01…06) |
| GATEWAY-* | service, automation, channels, typed API | shared | core | ABSENT | `PEER-PROBE-2026-07-26`; `GAP-AUDIT-2026-07-13 §3` | `BASE-2026-07-13` — Hermes 0.17.0 @ `dbe734be`; OpenClaw 2026.6.2 @ `11a0ad10` | Persistent gateway/service: **Core/Wayland behind**; channels: **Wayland behind** (`GAP-AUDIT §3`). Probe — the widest measured gap: Hermes ships a full `gateway/` package (`pairing.py`, `delivery.py`, `drain_control.py`, `platform_registry.py`, `channel_directory.py`, `authz_mixin.py`, `relay/`, `platforms/`, `builtin_hooks/`, `memory_monitor.py`, `code_skew.py`) plus `tui_gateway/`, `cron/`, `apps/`, `web/`, `website/`. OpenClaw ships `src/gateway`, `src/daemon`, `src/channels`, `src/pairing`, `src/node-host`, `src/hooks`, `src/commitments`, `packages/gateway-protocol`, `packages/gateway-client`, `packages/sdk`. Core has fragmented headless surfaces and **no persistent gateway runtime** | ABSENT is a true statement of the family, not a placeholder: operator-complete runtime is not built. Core's channel and protocol primitives exist but do not constitute a gateway lifecycle (install/start/stop/restart/status/doctor/logs/drain) | Phase 20 + 20A close, 2026-07-26 | Phase 24 (F24-01…05) |
| REACH-* | backends, nodes, plugins | shared | core | SOURCE | `PEER-PROBE-2026-07-26`; `GAP-AUDIT-2026-07-13 §3` | `BASE-2026-07-13` — Hermes 0.17.0 @ `dbe734be`; OpenClaw 2026.6.2 @ `11a0ad10` | Remote execution: **Core behind Hermes**; extension distribution: **architecture competitive, ecosystem behind** (`GAP-AUDIT §3`). Probe: Hermes ships **seven** execution backends behind one `base.py` contract — `tools/environments/{local,docker,ssh,singularity,modal,managed_modal,daytona}.py` plus `file_sync.py`. OpenClaw ships `src/node-host`, `src/plugins`, `src/plugin-sdk`, `src/plugin-state`, `packages/plugin-sdk`, `packages/plugin-package-contract`, `extensions/`. Core has `wcore-plugin-api` and sandbox/worktree assurance but **no user-facing execution-backend matrix** | Reference backend and plugin lifecycle are incomplete. Core's advantage (local sandbox/worktree assurance) does not substitute for backend reach; the F25-01 provider-neutral contract does not exist | Phase 20 + 20A close, 2026-07-26 | Phase 25 (F25-01…05) |
| PORT-* | import, export, backup, restore | shared | core | SOURCE | `PEER-PROBE-2026-07-26`; `GAP-AUDIT-2026-07-13 §3` | `BASE-2026-07-13` — Hermes 0.17.0 @ `dbe734be`; OpenClaw 2026.6.2 @ `11a0ad10` | Migration: **Core behind** — no complete OpenClaw import and incomplete profile migration (`GAP-AUDIT §3`). Probe — **both peers migrate from each other, and Core is the only party with no reciprocal path**: Hermes ships `hermes_cli/migrate.py`, `hermes_cli/backup.py`, `hermes_cli/subcommands/backup.py`, `hermes_cli/codex_runtime_plugin_migration.py`, `agent/curator_backup.py` and an explicit `optional-skills/migration/openclaw-migration/scripts/openclaw_to_hermes.py`. OpenClaw ships `docs/install/migrating-hermes.md`, `docs/install/migrating-claude.md`, `docs/cli/{backup,migrate}.md`, `docs/plugins/reference/migrate-hermes.md`, `extensions/anthropic/cli-migration.ts`, `extensions/codex/src/migration/{apply,auth}.ts`, `apps/macos/…/UserDefaultsMigration.swift` | Reciprocal migration and recovery proof are incomplete. Core's importer work is partial (`GAP-AUDIT §5.2`). Migration is also a security boundary — imported executable content must be inert until reviewed; that quarantine contract (F26-02) is unbuilt | Phase 20 + 20A close, 2026-07-26 | Phase 26 (F26-01…05) |
| MEDIA-* | attachment, browser/CUA/web, generation, voice | shared | core | SOURCE | `PEER-PROBE-2026-07-26`; `GAP-AUDIT-2026-07-13 §3` | `BASE-2026-07-13` — Hermes 0.17.0 @ `dbe734be`; OpenClaw 2026.6.2 @ `11a0ad10` | Browser and computer use: **competitive engine; behind device product**; voice/mobile/devices: **whole-Wayland behind** (`GAP-AUDIT §3`). Probe: OpenClaw ships `packages/{media-core,media-generation-core,media-understanding-common,speech-core,web-content-core}` and `src/{image-generation,media-generation,media-understanding,music-generation,video-generation,media,tts,talk,web-search,web-fetch,link-understanding}`. Hermes ships `plugins/browser/{browser_use,browserbase,firecrawl}` and `tools/computer_use`. Core has `wcore-browser` and `wcore-cua` crates with policy boundaries — engine-competitive, product-behind | Readiness, credential, and packaged-native proof are incomplete. `wcore-browser`/`wcore-cua` publish no live activation/readiness truth (F27-02); no deterministic media corpus has run on native macOS/Windows | Phase 20 + 20A close, 2026-07-26 | Phase 27 (F27-01…05) |
| NATIVE-* | macOS/Linux/Windows packaged certification | shared | shared | SOURCE | `F20A-SEAL@9821ef76`; `RUN-30184651330`; `PEER-PROBE-2026-07-26` | `BASE-2026-07-13` — Hermes 0.17.0 @ `dbe734be`; OpenClaw 2026.6.2 @ `11a0ad10` | Cross-platform contract: **unproven until packaged E5 matrix** (`GAP-AUDIT §3`). Probe: OpenClaw ships real native app targets — `apps/{macos,ios,android,macos-mlx-tts,swabble}` — plus `appcast.xml` and `deploy/`. Hermes ships `packaging/homebrew`, `docker/`, `nix/` and `docker-compose.windows.yml`. Core has explicit cross-platform architecture and CI intent but **no packaged native product target** | **SOURCE despite real native evidence.** `RUN-30184651330` proved the Phase-20 delegated-mutation lifecycle natively (Windows 6/6, macOS 8/8) at one sealed SHA — the first genuine native datapoint — but that is lifecycle execution, not certification. Zero of F28-01…04 have run: no E5 matrix, no 1,000-session soak, no signed platform-binding receipt. The 11519-test aggregate is Linux-only | Phase 20 + 20A close, 2026-07-26 | Phase 28 (F28-01…04) |
| SUPPLY-* | provenance, SBOM, signing, update, rollback | shared | shared | SOURCE | `F03-RECEIPT@1c644ccd`; `PEER-PROBE-2026-07-26` | `BASE-2026-07-13` — Hermes 0.17.0 @ `dbe734be`; OpenClaw 2026.6.2 @ `11a0ad10` | No `GAP-AUDIT §3` dimension covers supply chain; delta is probe-derived. Probe: OpenClaw has npm trusted publishing with provenance (`scripts/openclaw-npm-publish.sh:44` passes `--provenance`; `.github/workflows/openclaw-npm-release.yml:622` has a `Verify prepared tarball provenance` step) plus an update/rollback surface (`src/cli/update-cli.ts`, `src/infra/update-runner.ts`, `appcast.xml`, `scripts/make_appcast.sh`). **Hermes has no SBOM, cosign, provenance or SLSA match anywhere in `.github/` or `scripts/` at `dbe734be`** — a real negative finding, so OpenClaw alone sets the peer bar. **Neither peer ships an SBOM at baseline**, so Core's F29-01 SBOM requirement has no counterpart to match — it would be a lead if proven | Clean-room release and rollback chain are incomplete. `F03` delivered the cryptographic primitives — SHA-256 content addressing, detached domain-separated Ed25519 signatures, and a trust policy binding receipts to key/source commit/binary digest/repo/ref/workflow — but `F03-RECEIPT` states plainly that the CLI **exposes no signing-key flag**, emits only non-authoritative local receipts, and that a trusted CI signer must exist **out of band**. No such trust root is bound. Signing primitives ≠ a release supply chain | Phase 20 + 20A close, 2026-07-26 | Phase 29 (F29-01…04) |

---

## F03/F05 retroactive evidence map

This section discharges the admission rule's first clause. Both receipts were read in full; every
mapping below cites the specific claim it rests on.

**`F03-RECEIPT@1c644ccd`** — the evidence substrate for the whole ledger, not a capability row of
its own. It maps to two families:

- **AUTH-***: render-all-then-scan publication (a provider/canary secret in any projection rejects
  the whole bundle before persistence); redacted JSON/JSONL/JUnit/console/Markdown projections with
  no raw prompt, model output, tool payload, stderr, call ID, secret or worktree path; typed egress
  and filesystem evidence fields. **Counter-evidence in the same receipt:** those egress/filesystem
  and provider attempt/retry/token/cache fields are recorded `Unavailable`, and the receipt states
  the design intent — absent measurements "cannot be represented by plausible zero values" and
  therefore fail the milestone gate rather than becoming fake success.
- **SUPPLY-***: SHA-256 content addressing over the canonical body; detached, domain-separated
  Ed25519 signatures; an external verification policy binding receipt → trusted key, source commit,
  binary digest, repository, ref, workflow, authority; separated integrity / authority / release-gate
  decisions. **Bounded by:** local receipts are always non-authoritative and no trusted CI signer is
  bound.

**`F05-RECEIPT@0825c92d`** — the origin of this ledger's maturity vocabulary. F05's typed stages
(`declared`, `configured`, `constructed`, `ready`, `reached`, `outcome_changed`, `observed`,
`unavailable`) are the direct ancestor of the `ABSENT → … → PACKAGED_PROVEN` enum above, which is
why F05's per-capability truth table can be mapped into row maturity without reinterpretation.

All eight audited capability identities are mapped; none is dropped:

| # | F05 capability | F05 effective startup truth | Runtime outcome proof | Ledger row |
|---|---|---|---|---|
| 1 | Pricing refresher | Unavailable: no production constructor | None | CONT-* (cache economics) |
| 2 | Mid-flight monitor | Unavailable: runtime path unwired | None | GOAL-* (loop ownership) |
| 3 | Cooldown tracker | Unavailable: no production constructor | None | CONT-* (cache economics) |
| 4 | Learned policy | Unavailable: runtime path unwired | None | GOAL-* (loop ownership) |
| 5 | Smart handoff | Ready from concrete memory construction | Successful episode persistence | CONT-* (memory) |
| 6 | Delegate isolation | **Unavailable: isolation not enforced** | None | AUTH-* (sandbox/isolation) |
| 7 | Procedure skill drafting | Ready from concrete memory construction | Successful quarantine staging | CONT-* (governed skills) |
| 8 | Legacy auto-skill drafting | Ready | Successful draft write | CONT-* (governed skills) |

Three capabilities reach runtime outcome proof after a real side effect — that is what lifts CONT-*
from `SOURCE` to `REACHED`. Five are honest negatives; per the receipt, "an unavailable row is an
honesty result, not capability completion."

**One unresolved mapping, recorded rather than resolved.** Row 6 (Delegate isolation) is proven
`Unavailable: isolation not enforced` at `0825c92d`. Phase 20 later delivered transactional
delegated mutation at `01a5b0ae`/`9821ef76`, which plausibly supersedes that finding — but no
Phase 20 artifact re-runs the F05 capability gate against the `delegate_isolation` identity, so
there is **no evidence that the F05 negative was cleared**. AUTH-* therefore keeps the negative and
stays at `CONSTRUCTED`. Asserting otherwise would be exactly the source-presence-earns-parity error
the admission rule forbids. See the disposition below for the single input that closes it.

---

## CTRL-01 disposition

**Status: CLOSED for admission purposes — with two carried limitations, neither of which is a
missing external input.**

Every clause of the close condition is now satisfied. Quoting it: *"CTRL-01 remains open until
every active row uses the declared maturity enum and has a pinned peer baseline, security owner,
exact evidence IDs, delta, limitation, and refresh phase."*

| Close-condition clause | State |
|---|---|
| Every active row uses the declared maturity enum | **MET** — all 10 rows use `ABSENT`/`SOURCE`/`CONSTRUCTED`/`REACHED`/`EFFECTIVE` from the declared enum. Zero `PENDING`. |
| Pinned peer baseline | **MET** — Hermes 0.17.0 @ `dbe734be`, OpenClaw 2026.6.2 @ `11a0ad10`, each with a named version-pin source and a verified ancestor relationship to a clean local checkout. Zero `UNPINNED`. |
| Security owner | **MET** — 8 rows `core`, 2 rows (`NATIVE-*`, `SUPPLY-*`) `shared`. |
| Exact evidence IDs | **MET** — every row cites resolvable IDs from the evidence index. Zero `*-PENDING`. |
| Delta | **MET** — every row carries a `GAP-AUDIT §3` verdict and/or a `PEER-PROBE-2026-07-26` structural finding at the pinned baseline. |
| Limitation | **MET** — every row states its own honest boundary, including the negative findings. |
| Refresh phase | **MET** — all rows refreshed to Phase 20 + 20A close, 2026-07-26, at seals `01a5b0ae` and `9821ef76`. |
| Bootstrap and map accepted F03/F05 evidence | **MET** — both receipts read in full; all 8 F05 capability identities mapped; F03 mapped to AUTH-* and SUPPLY-*. |

### Per-row disposition

| Row | Maturity | Disposition |
|---|---|---|
| AUTH-* | CONSTRUCTED | **Closed with a carried negative** — `F05-TRUTH-6` Delegate isolation `Unavailable` is deliberately retained (see below). |
| TXN-* | EFFECTIVE | **Closed.** Strongest row: Linux aggregate + native Windows/macOS proof, and no counterpart in either peer baseline. |
| GOAL-* | SOURCE | **Closed.** Two F05 negatives mapped; OpenClaw counterpart pinned. |
| CONT-* | REACHED | **Closed.** Three F05 runtime outcome proofs and two F05 negatives mapped. |
| GATEWAY-* | ABSENT | **Closed.** `ABSENT` is the accurate value; peer counterpart is the widest measured gap. |
| REACH-* | SOURCE | **Closed.** Hermes' seven-backend matrix pinned file-by-file. |
| PORT-* | SOURCE | **Closed.** Both peers' reciprocal migration paths pinned. |
| MEDIA-* | SOURCE | **Closed.** Both peers' media/voice surfaces pinned. |
| NATIVE-* | SOURCE | **Closed.** 20A native evidence recorded without inflating maturity to certification. |
| SUPPLY-* | SOURCE | **Closed.** Includes a negative peer finding (Hermes has no supply-chain tooling at baseline). |

No row is left open for want of an external input. Nothing in this ledger is blocked on Sean.

### Two carried limitations (tracked, not blocking)

These are recorded so F30 inherits them explicitly. Neither is an admission blocker; each is
internal work already owned by a scheduled phase.

1. **The `delegate_isolation` F05 identity has not been re-gated after Phase 20.** AUTH-* carries a
   negative that Phase 20 may have already cleared. *Input required:* re-run the F05 capability
   activation gate against the `delegate_isolation` identity at `9821ef76` and record the result. No
   external input; Core-side work. Owner: Phase 21 (nearest admitted phase touching authority).
2. **Every delta in this ledger is static-source, not runtime-measured.** `GAP-AUDIT §3` and
   `PEER-PROBE-2026-07-26` both compare source structure. No correctness, recovery, cost, or
   cognitive-tax number is claimed anywhere above, and none may be quoted from this ledger as a
   benchmark. *Input required:* `F30-03` — common trials across Wayland/Hermes/OpenClaw with
   repeated runs and confidence bounds. Owner: Phase 30, as designed.

### Refresh obligations for the next admitted phase

- Deltas are frozen at `BASE-2026-07-13`. `HEAD-2026-07-26` (Hermes 0.18.2 @ `d59b79fa`,
  OpenClaw 2026.7.2 @ `3659c85e`) is the declared forward target; moving the baseline requires
  re-deriving every delta cell, not editing the version strings in place.
- `REQ-native-r2`, `r8`, `r12`, `r13` remain open against `9821ef76` and bound TXN-*'s maturity
  below `PACKAGED_PROVEN`. Closing them is Phase 20A carry-over, tracked in `REQUIREMENTS.md`.
- Any contradictory live evidence reopens the affected row and enters `FIELD-REGRESSIONS.md`.

*CTRL-01 refreshed 2026-07-26 against Phase 20 seal `01a5b0ae` and Phase 20A seal `9821ef76`. Peer
baselines pinned from read-only local checkouts the same day.*
