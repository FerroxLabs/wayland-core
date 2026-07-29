# RECON-OPENCLAW — working notes (append-only, committed continuously)

Lane `recon-openclaw`. Read-only peer tree at `/Users/seandonahoe/dev/resources/openclaw`.
Nothing in that tree is mutated, installed or executed. Every measurement below names the
command that produced it so it can be re-run.

## Instrument discipline

Per LANE-BRIEF §3b/§3b-i: all load-bearing counts and every absence claim use `/usr/bin/grep`
and `/usr/bin/find` with a known-positive in the same invocation. `rtk` rewrites grep output
(measured: reported "9 matches in 7 files" for a one-file search whose true answer was zero),
so an unproxied instrument is mandatory before I write any zero into the report.

## Pins (measured, not recalled)

| Fact | Value | How measured |
|---|---|---|
| Peer HEAD | `3659c85e534fdb8b8ce6b7505a83d92cc2e4df8e` | `/usr/bin/git -C .../openclaw rev-parse HEAD` |
| HEAD subject | `fix(codex): distinguish available usage after limit errors (#110381)` | `git log --oneline -1` |
| `package.json` version | **2026.7.2** | `cat package.json` |
| Ledger's pinned baseline | `11a0ad10`, 2026.6.2 | COMPETITIVE-LEDGER.md (not re-read from tree) |
| Our lane base | `fab33493` on `lane/recon-openclaw` | `git rev-parse` in worktree |

Local HEAD has moved off the ledger's pin. Everything I report is measured at `3659c85e53`
(2026.7.2) and I say so; where 2026.6.2 differs materially I check and note it.

## Shape, first pass

- `src/` has **114 entries** (`ls src/ | wc -l`), of which ~70 are directories.
- `packages/` has **24 workspace packages**.
- `src/commands/` has **238 non-test entries** — this is the CLI command surface, and it is
  where reachability gets decided.
- `src/cli/` carries the program/argv layer: `command-catalog.ts` (13.8K), `argv.ts` (16.3K),
  `program/`. `command-catalog.ts` is the reachability oracle — a capability that is not in
  the catalog is not reachable from the CLI.

### Media (our weakest family — `MEDIA-*` at `SOURCE`)

Directories confirmed present with real file volume, not empty shells:

- `src/media/` — ~40 non-test modules. Notable by size: `web-media.ts` 37.7K,
  `store.ts` 22.1K, `parse.ts` 21.7K, `fetch.ts` 20.8K, `input-files.ts` 15.8K,
  plus `ffmpeg-exec.ts`, `pdf-extract.ts`, `image-ops.ts`, `audio-transcode.ts`,
  `qr-image.ts`, `video-dimensions.ts`, `local-roots.ts`, `read-capability.ts`.
  This is a **media intake/normalisation layer**, not a generation layer.
- `src/media-understanding/` — the largest single media area. `runner.ts` 33.7K,
  `runner.entries.ts` 37.8K, `apply.ts` 23.9K, `image.ts` 16.7K, `local-audio.ts` 14.8K,
  `attachments.cache.ts` 17.3K, plus `provider-registry.ts`, `provider-capability-registry.ts`,
  `openai-compatible-audio.ts`, `openai-compatible-video.ts`, `audio-preflight.ts`.
  Test files are enormous (`image.test.ts` 56.4K, `apply.test.ts` 65.0K,
  `runner.vision-skip.test.ts` 32.9K) — that is a strong shipped-not-vestigial signal.
- `src/image-generation/` — `runtime.ts` 8.2K, `openai-compatible-image-provider.ts` 11.2K,
  `provider-registry.ts`, `image-assets.ts`, `normalization.ts`, `capabilities.ts`.
- `src/media-generation/` — `runtime-shared.ts` 22.6K + `provider-capabilities.contract.test.ts`.
- Also present: `src/video-generation/`, `src/music-generation/`, `src/tts/`, `src/talk/`,
  `src/realtime-transcription/`, `src/link-understanding/`, `src/web-fetch/`, `src/web-search/`.
- Packages: `media-core`, `media-generation-core`, `media-understanding-common`, `speech-core`,
  `web-content-core`.

**Working hypothesis (to be proven by reading code, not sizes):** they have a
three-layer media stack — intake/normalisation (`src/media`), understanding
(`src/media-understanding`, provider-backed), and generation (`image-`/`video-`/`music-generation`
over a shared `media-generation-core`) — with a provider-capability registry mediating all of it.

### Durable tasks (we are recorded behind, especially here)

`src/tasks/` is far larger than the brief implied — not 4 files, ~90.

- `task-registry.ts` **87.8K** with `task-registry.test.ts` **170.5K**.
- **SQLite-backed persistence**: `task-registry.store.sqlite.ts` 15.5K,
  `task-flow-registry.store.sqlite.ts` 8.9K. Durability is a real store, not a JSON blob.
- `task-flow-registry.ts` 23.8K + `.audit.ts` + `.maintenance.ts` + `.store.ts` — a second
  registry for *flows* layered over the task registry.
- Lifecycle machinery: `detached-task-runtime.ts`, `detached-task-runtime-state.ts`,
  `detached-task-runtime-contract.ts`, `task-completion-contract.ts`,
  `task-cancellation-state.ts`, `task-restart-blocker.ts`, `task-retention.ts`,
  `task-registry.process-state.ts`, `task-registry.reconcile.ts`,
  `task-registry.maintenance.ts` (36.6K), `task-executor.ts` + `task-executor-policy.ts`.
- Access control per task: `task-owner-access.ts`, `task-status-access.ts`,
  `task-flow-owner-access.ts`.
- `src/commitments/` is separate and smaller (~15 files): `runtime.ts` 12.8K, `store.ts` 14.6K,
  `extraction.ts` 11.5K, `store-record.ts`, plus
  `commitments-full-chain.integration.test.ts` and `commitments-heartbeat-policy.e2e.test.ts`.
  There is a `src/commands/commitments.ts` (5.1K) — so it is CLI-reachable, pending catalog check.

**Not yet established:** what actually survives a restart, what the reconcile pass does on
boot, and whether "detached" means a real OS process or an in-daemon record.

## Still to establish

1. Reachability: read `src/cli/command-catalog.ts` and map which of these areas have CLI verbs.
2. Media: read the provider abstraction (`provider-registry` / `provider-capability-registry`)
   and the actual user surface for image in, image out, TTS, speech.
3. Durability: read `detached-task-runtime.ts` + `task-registry.reconcile.ts` + the sqlite store
   to state the restart-survival model precisely.
4. Migration from Hermes: locate `src/commands/migrate/` contents and what concepts it maps.
5. Native app targets: `apps/{macos,ios,android}` + `appcast.xml` scope and update mechanism.
6. Our nearest equivalent per area, from `crates/` in this repo — and honest gap sizing.
7. Dead-surface sweep: which advertised areas are NOT reachable / NOT exported / NOT packaged.

---

## MEASUREMENT 2 — the CLI surface is now definitive (reachability oracle found)

Reachability is decided by two descriptor catalogs, both `as const satisfies` typed, both
consumed by `buildCommandGroupEntries` in `src/cli/program/command-group-descriptors.ts`,
which **throws `Unknown command descriptor: <name>`** if a registrar names a command that has
no descriptor. So the catalogs are load-bearing, not documentation.

- `src/cli/program/core-command-descriptors.ts` — 22 root commands.
- `src/cli/program/subcli-descriptors.ts` — 41 sub-CLIs.
- `src/cli/command-catalog.ts` — 102 `commandPath:` entries (startup/fast-path policy only).

Core roots: setup, crestodian(hidden), onboard, configure, config, backup, **migrate**, doctor,
dashboard, reset, uninstall, message, mcp, transcripts, agent, agents, status, health, audit,
sessions, **commitments**, **tasks**.

Sub-CLIs include: acp, gateway, daemon, logs, system, models, promos, **infer**, **capability**,
approvals, exec-approvals, exec-policy, **nodes**, devices, node, worker, sandbox, fleet,
worktrees, attach, tui, terminal, chat, cron, dns, docs, qa(env-gated), proxy, hooks, webhooks,
qr, clawbot, pairing, plugins, channels, directory, security, secrets, skills, update, completion.

### Correction to my own first-pass hypothesis

I initially saw no `media`/`image`/`tts` root command and was about to record media as
"agent-internal only". **That would have been wrong.** The surface is named `infer`
(alias `capability`) — the vocabulary trap §3b-i.3 warns about. Registered in
`src/cli/capability-cli.ts:registerCapabilityCli`, which composes eight registrars:
model, image, audio, tts, video, web, embedding, plus list/inspect.

### `openclaw infer` — 31 self-describing capability ids

From `src/cli/capability-cli/metadata.ts` (`CAPABILITY_METADATA`), each entry carrying
`id`, `description`, `transports` (`local` | `gateway`), `flags[]`, `resultShape`:

- `model.run|list|inspect|providers|auth.login|auth.logout|auth.status`
- `image.generate|edit|describe|describe-many|providers`
- `audio.transcribe|providers`
- `tts.convert|voices|providers|personas|status|enable|disable|set-provider|set-persona`
- `video.generate|describe|providers`
- `web.search|fetch|providers`
- `embedding.create|providers`

`infer capability list` / `infer capability inspect --name <id>` render this table at runtime —
the CLI **introspects its own capability matrix**, and `CapabilityEnvelope` is a uniform result
shape (`ok`, `capability`, `transport`, `provider`, `model`, `attempts[]`, `inputs[]`,
`outputs[]`, `ignoredOverrides[]`, `error`) across every one of them.

`image generate` alone exposes 13 flags including `--aspect-ratio`, `--resolution` (1K/2K/4K),
`--output-format`, `--background`, `--quality`, `--openai-moderation`, `--timeout-ms`.
This is not a demo surface.

**`transports: ["local","gateway"]`** on `model.run` and the whole `tts.*` family is significant:
the same capability id runs either in-process or against the gateway daemon.

---

## MEASUREMENT 3 — media provider architecture: contracts in core, providers in extensions

### The architecture

`src/image-generation/provider-registry.ts:12` declares
`const BUILTIN_IMAGE_GENERATION_PROVIDERS: readonly ImageGenerationProviderPlugin[] = [];`
— **empty by design**. Every media provider is contributed through
`resolvePluginCapabilityProviders({ key: "imageGenerationProviders", cfg })`
(`src/plugins/capability-provider-runtime.ts`, 22.0K). Core owns the *contract*; extensions
own the *implementation*. Same pattern for media-understanding, speech, video, music.

The contract in `src/image-generation/types.ts` is unusually rich — providers declare a
**capability matrix**, not just a function:
`ImageGenerationProviderCapabilities = { generate, edit, geometry, output }` with
`maxInputImages`, `maxInputImagesByModel`, `maxInputImagesByModelPrefix`, `sizesByModel`,
`aspectRatiosByModel`, `resolutionsByModel`, `qualities`, `formats`, `backgrounds`.
Unsupported user flags are not errors — they surface as
`ImageGenerationIgnoredOverride { key, value }`, which is the `ignoredOverrides[]` field on the
`CapabilityEnvelope`. **The CLI tells the user which of their flags the chosen provider dropped.**

`src/media-understanding/provider-registry.ts:hydrateModelBackedMediaProvider` is the other
half: a provider that declares `capabilities: ["image"]` but supplies no `describeImage` hook is
auto-backed by the generic model runtime (`describeImageWithModel`). Config providers with
image-capable models are auto-registered (`#51392`). So **vision works on any model provider
whose catalog says the model is image-capable, with no media-specific plugin code at all.**

### Near-miss on my own instrument (recording per §3b-ii)

My first contract scan used `--include="package.json"` and returned **0 for all nine capability
kinds**. That zero was *arithmetically correct and completely misleading* — manifests are
`openclaw.plugin.json`, not `package.json`. I caught it only because I ran the known-positive
(`"name"` in `extensions/**/package.json` → **146**) in the same pass, proving the instrument
alive and therefore forcing the conclusion that my *query* was wrong rather than the tree empty.
Had I skipped the known-positive I would have filed "OpenClaw ships zero media providers", which
is the exact inverse of the truth. **Repairing the instrument (correct filename) rather than just
noting it**: rescan below.

### Provider counts (measured: `grep -rl '"<key>"' extensions --include="openclaw.plugin.json"`)

148 plugin manifests total. Providers per media capability:

| Capability contract | Count | Extensions |
|---|---|---|
| `mediaUnderstandingProviders` | **18** | anthropic, codex, deepgram, deepinfra, elevenlabs, google, groq, minimax, mistral, moonshot, openai, opencode, opencode-go, openrouter, qwen, senseaudio, xai, zai |
| `speechProviders` (TTS) | **15** | azure-speech, deepinfra, elevenlabs, google, gradium, inworld, microsoft, minimax, openai, openrouter, tts-local-cli, volcengine, vydra, xai, xiaomi |
| `videoGenerationProviders` | **15** | alibaba, byteplus, comfy, deepinfra, fal, google, minimax, openai, openrouter, pixverse, qwen, runway, together, vydra, xai |
| `webSearchProviders` | **15** | brave, codex, duckduckgo, exa, firecrawl, google, minimax, moonshot, ollama, parallel, perplexity, qa-lab, searxng, tavily, xai |
| `imageGenerationProviders` | **11** | comfy, deepinfra, fal, google, litellm, microsoft-foundry, minimax, openai, openrouter, vydra, xai |
| `realtimeTranscriptionProviders` | **5** | deepgram, elevenlabs, mistral, openai, xai |
| `musicGenerationProviders` | **5** | comfy, fal, google, minimax, openrouter |
| `webFetchProviders` | **1** | firecrawl |
| `embeddingProviders` | **1** | llama-cpp |
| `channels` | **26** | discord, telegram, slack, signal, whatsapp, imessage, matrix, msteams, googlechat, mattermost, irc, line, sms, twitch, nostr, feishu, qqbot, zalo, zalouser, tlon, reef, raft, clickclack, nextcloud-talk, synology-chat, qa-channel |

These are not stubs. `extensions/fal/image-generation-provider.ts` is **27.9K** with a **49.8K**
test; `extensions/fal/video-generation-provider.ts` **23.3K** / 23.7K test;
`extensions/elevenlabs/speech-provider.ts` **20.8K** / 9.8K test, plus a 10.6K
`realtime-transcription-provider.ts`. There are also cross-extension live suites at
`extensions/video-generation-providers.live.test.ts` (21.5K) and
`extensions/music-generation-providers.live.test.ts` (11.4K).

**`extensions/migrate-hermes/` exists** — that is priority 3, read next.

---

## MEASUREMENT 4 — durability model: one migrated SQLite state DB, 85 tables

`src/state/openclaw-state-db.generated.d.ts` declares `export interface DB` with **85 tables**
(measured: `sed -n '1305,1390p' ... | grep -cE '^  [a-z_]+: '`). Kysely-typed, generated from a
real migration chain (`schema_meta` table is in it), opened through
`src/state/openclaw-state-db.ts` with `runOpenClawStateWriteTransaction`,
`assertSqliteTableIntegrity`, `runSqliteDeferredTransactionSync`, and a
`src/state/openclaw-state-lease.ts` lease. There is a second per-agent DB
(`openclaw-agent-db.generated.d.ts`).

Tables that matter for this comparison:

- Tasks: `task_runs`, `task_delivery_state`, `subagent_runs`, `flow_runs`, `cron_jobs`
- Restart: `gateway_restart_handoff`, `gateway_restart_intent`, `gateway_restart_sentinel`,
  `gateway_boot_lifecycle`, `state_leases`
- Delivery: `delivery_queue_entries` (durable outbound queue)
- Migration: **`migration_runs`, `migration_sources`** — migrations are themselves durable records
- Media: `media_blobs`, `managed_outgoing_image_records`, `model_capability_cache`
- Sessions: `session_state_events`, `session_state_heads` (event-sourced with heads),
  `session_watch_cursors`, `session_upstream_links`, `session_groups`
- Commitments: `commitments`
- Approvals/security: `operator_approvals`, `exec_approvals_config`, `plugin_binding_approvals`,
  `audit_events`, `audit_identity_keys`, `device_auth_tokens`, `device_identities`
- Skills: `skill_lifecycle`, `skill_usage`, `skill_curator_state`, `skill_uploads`
- Voice: `voicewake_triggers`, `voicewake_routing_config`, `voicewake_routing_routes`
- Push: `apns_registrations`, `apns_registration_tombstones`, `web_push_subscriptions`,
  `web_push_vapid_keys`

### What actually survives a restart

`src/tasks/task-registry.maintenance.ts` (36.6K) runs a periodic **sweep** with four outcomes
per task — `reconciled`, `recovered`, `cleanupStamped`, `pruned` — and the decision logic is
notably careful about *not* declaring a task dead just because this process cannot see it:

- `hasLostGraceExpired(task, now)` → `retained / lost_grace_pending` (grace window before any
  mark-lost).
- `hasBackingSession(task, context)` → `would_reconcile / backing_session_missing`.
- `isRuntimeAuthoritative()` → a `cron` or `acp` task is **retained**, not marked lost, when this
  process is not the authoritative runtime for it (`cron_runtime_not_authoritative`,
  `acp_runtime_not_authoritative`). This is precisely the "absence of evidence is not evidence of
  absence" discipline our own lane brief keeps re-learning, implemented in product code.
- `hasActiveCliRun(task)` → `retained / active_cli_run`.
- `isSubagentRecoveryWedgedEntry(entry)` → `would_reconcile / subagent_recovery_wedged`.
- `tryRecoverTaskBeforeMarkLost` (`detached-task-runtime.ts`) — an async, **best-effort,
  time-warned (5s) plugin recovery hook** that gets a chance to reclaim the task before it is
  marked lost; a hook that throws, hangs or returns garbage is logged and bypassed rather than
  blocking cleanup.
- `findDetachedTaskRun` returns `{ lookup: "available" | "unavailable" }` — it deliberately
  **distinguishes "not found" from "cannot tell"**, with the comment: *"an empty fallback cannot
  prove that the runtime-owned task is absent."*

Operator surface on top of it: `previewTaskRegistryMaintenance()` (dry-run of the sweep) and
`getTaskRegistryMaintenanceDiagnostics()` returning per-task
`{ taskId, runtime, status, decision, reason, ageMs, detail }` for every stale-running task.
Reachable as `openclaw tasks list` / `openclaw tasks audit` (both are `CliRoutedCommandId`s).

### The completion contract — a genuinely novel guard

`src/tasks/task-completion-contract.ts` refuses to accept a detached task as *succeeded* when the
agent's final text is progress narration rather than a deliverable. Three regexes
(`PROGRESS_ONLY_PATTERN`, `BARE_PROGRESS_ONLY_PATTERN`, `FOLLOW_UP_PLANNING_PREFIX_PATTERN`)
catch "I'll now check…", "Looking into…", "Next, I'll verify…", with a
`hasNonProgressFollowupSentence` escape so "I'll check X. Here is the answer: …" still passes.
Failing text sets `terminalOutcome: "blocked"` +
`"Required completion ended with progress-only text, not a final deliverable."`
Empty text is likewise blocked. This is an anti-"claimed done" gate **in the product**, and it is
the same failure class our own §5 honesty rules exist to police in agents.

---

## MEASUREMENT 5 — migration (`openclaw migrate`)

Registered in `src/cli/program/register.migrate.ts` → `src/commands/migrate/` (16 modules).

Surface: `migrate list`, `migrate plan <provider>`, `migrate apply <provider>`, and a default
`migrate <provider>` that **previews then prompts**. Flags: `--from <path>`, `--dry-run`, `--yes`,
`--include-secrets`, `--no-auth-credentials`, `--overwrite` (with *item-level backups*),
`--skill <name>` (repeatable), `--plugin <name>` (repeatable), `--backup-output <path>`,
`--no-backup`, `--force`, `--json`, `--verify-plugin-apps`.

**Safety posture: a verified full backup is taken before apply by default.** `--no-backup` is
refused unless `--force` is also passed. Help text is explicit: *"Apply Hermes migration
non-interactively after writing a verified backup."*

Providers are plugin-contributed under a `migrationProviders` contract:
- `extensions/migrate-hermes/` — manifest `contracts.migrationProviders: ["hermes"]`,
  `"Imports Hermes configuration, memories, skills, and supported credentials into OpenClaw."`
  Modules: `source.ts` 12.5K, `memory.ts` 8.6K, `skills.ts` 6.1K, `config.ts` 6.3K, `plan.ts`,
  `apply.ts`, `helpers.ts`, `provider.test.ts` 19.9K. `activation.onStartup: false` (lazy).
- `extensions/migrate-claude/` — much larger (auth.ts 16.3K, secrets.ts 13.4K +48.4K test,
  config-providers.ts 13.2K, config-provider-contract.ts 13.8K, config-mcp.ts 12.3K,
  model.ts 12.7K, source.ts 8.1K, apply.ts 8.6K, plan.ts 7.3K, files-and-skills.test.ts 28.8K).

Still to read: what `migrate-hermes/source.ts` + `memory.ts` + `skills.ts` actually map.

---

## MEASUREMENT 6 — what Hermes migration actually maps, and how unmappable state is handled

`extensions/migrate-hermes/plan.ts:buildHermesPlan` produces a `MigrationPlan`
(`{ providerId, source, target, summary, items[], warnings[], nextSteps[], metadata }`)
where each `MigrationItem` is `{ id, kind, action, source, target, status, reason, message,
details }`. `kind` ∈ file | workspace | memory | skill | auth | secret | manual | archive;
`action` ∈ copy | append | archive; `status` ∈ planned | conflict.

Discovery (`source.ts`) probes for: `config.yaml`, `.env`, `auth.json`, `active_profile`,
`SOUL.md`, `AGENTS.md`, `skills/`, `memories/`, plus an OpenCode `auth.json` at
`~/.local/share/opencode/auth.json` (XDG-aware, with root-parent and home fallbacks).

Concept mapping:
- `config.yaml` → OpenClaw config items + a **model ref** item (`resolveHermesModelRef`)
- `.env` → provider secrets (`buildSecretItems`)
- `auth.json` / global / opencode auth → auth profile items (`buildAuthItems`)
- `SOUL.md`, `AGENTS.md` → `workspace/` files
- `MEMORY.md`, `USER.md` → memory items, **`action: "append"`** into the workspace; in
  memory-only mode instead copied under `workspace/memory/imports/hermes/` with
  `collectionId: "hermes"` so imported memory stays an identifiable collection
- `skills/` → skill items, with `EXCLUDED_SKILL_DIRS` (`.git`, `node_modules`, `.venv`,
  `__pycache__`, `.pytest_cache`, …) and `SKILL_SUPPORT_DIRS`
  (`references`, `templates`, `assets`, `scripts`) preserved as part of a skill

**Unmappable state is archived, not dropped.** `HERMES_ARCHIVE_DIRS` (plugins, sessions, logs,
cron, mcp-tokens, plans, workspace, skins, kanban, pairing, platforms) and
`HERMES_ARCHIVE_FILES` (`state.db`, `hermes_state.db`, `projects.db`, `response_store.db`,
`memory_store.db`, `verification_evidence.db`, `kanban.db`, `retaindb_queue.db`,
`gateway_state.json`, `channel_directory.json`, `channel_aliases.json`, `processes.json`,
`feishu_comment_pairing.json`) become `kind: "archive"` items with the message
*"Archived in the migration report for manual review; not imported into live config."*

Warnings are specific and security-aware, e.g.: *"Hermes and OpenClaw must not keep using the
same imported OpenAI OAuth refresh grant after migration; reauthenticate one side before running
both."* And retired providers become `kind: "manual"` items with a concrete remedy
(`usesRetiredHermesQwenProvider` → "Authenticate qwen with an API key after migration:
`openclaw onboard --auth-choice qwen-api-key`").

### Our side, measured (not assumed)

`crates/wcore-cli/src/migrate/` — `mod.rs` 41.5K, `quarantine.rs` 34.3K, `hermes.rs` 19.7K,
**`openclaw.rs` 18.8K**, `select.rs` 12.8K, `provenance.rs` 10.5K.

`hermes.rs` doc comments state the import set directly: `profiles/<name>/config.yaml` (a `model:`
block), `profiles/<name>/.env` (provider-named keys), and `profiles/<name>/{skills/, SOUL.md,
memories/}` **"counted for the deferred"** — i.e. enumerated, reported, and *not imported*
(`deferred.skills += count_subdirs(...)`, `deferred.memory_files += count_memory_notes(...)`).
Root-level `config.yaml` + `.env` were added after a measured gap (F26-01 gap 4).

So the brief's "we import 4 files" is accurate in kind: **we import config + env; we defer skills,
SOUL and memories.** OpenClaw imports all of those plus auth, and archives the rest.
We do have `openclaw.rs` — **we migrate FROM OpenClaw; they do not migrate from us.**

---

## MEASUREMENT 7 — native app targets

`apps/` = `macos/`, `ios/`, `android/`, `linux/`, `macos-mlx-tts/`, `swabble/`, `shared/`, `.i18n/`.

- **macOS** — SwiftPM (`apps/macos/Package.swift`). Products: `.executable OpenClaw`,
  `.executable openclaw-mac` (CLI), `.library OpenClawIPC`, `.library OpenClawDiscovery`.
  Dependencies include **Sparkle** (updates), `OpenClawKit` + `OpenClawChatUI`,
  `OpenClawMLXTTSProtocol`, `SwabbleKit`, `MenuBarExtraAccess`, `KeyboardShortcuts`,
  `PeekabooBridge`/`PeekabooAutomationKit` (screen automation), `swift-subprocess`.
- **Update mechanism** — `appcast.xml` at repo root is a **Sparkle 2 RSS appcast**
  (`xmlns:sparkle`), served from `raw.githubusercontent.com/openclaw/openclaw/main/appcast.xml`.
  Latest item at this HEAD: **2026.7.1**, `sparkle:version 2607000190`,
  `sparkle:minimumSystemVersion 15.0`. 2131 further lines = full release-note history in-band.
- **iOS** — `Sources/` covers Calendar, Camera, Contacts, EventKit, Health, Location, Motion,
  Media, Push, LiveActivity, Gateway, Chat, Onboarding, Permissions, Design, Device, Model.
  Plus `WatchApp/`, `ShareExtension/`, `ActivityWidget/`, `UITests/`, `fastlane/`,
  `APP-REVIEW-NOTES.md`, signing xcconfigs. This is a shipped App Store product.
- **Android** — Gradle (`build.gradle.kts`), `app/`, `wear/`, `wear-shared/`, `benchmark/`,
  `fastlane/`, `VERSIONING.md`, `THIRD_PARTY_LICENSES/`.
- **swabble** — Swift 6.2 on-device wake-word daemon (Speech.framework, macOS 26),
  default wake word `clawd`, "zero network usage"; `SwabbleKit` shared into iOS/macOS.
- **linux** — `src-tauri/` + `ui/` (Tauri).

---

## MEASUREMENT 8 — dead-surface sweep

I looked for advertised-but-unreachable surfaces (we have found ten in our own tree). Findings:

- **Music generation is NOT dead**, though it has no `infer` verb. It is reachable as an *agent
  tool*: `src/agents/tools/music-generate-tool.ts` + `.actions.ts` + `music-generate-background.ts`,
  with 5 provider manifests. So media has **two** surfaces — the `infer` CLI and the agent tool
  family (`image-generate-tool.ts` 41.7K with a **109.0K** test, `media-tool-shared.ts` 20.5K,
  `media-generate-background-shared.ts` 25.2K for async generation). I nearly filed music as
  dead on the strength of its CLI absence; checking the second surface corrected it.
- **`fleet`** is self-labelled *"Provision and manage isolated tenant cells (experimental)"* —
  their own honest not-finished marker.
- **`qa`** is env-gated out of help unless `isPrivateQaCliEnabled()` — deliberately private,
  not dead.
- **`clawbot`** is described as "Legacy clawbot command aliases" — retained compatibility shim.
- `crestodian` is a hidden deprecated alias for `setup`.
- I did **not** find an advertised media/task/migration surface with no implementation behind it.
  Every capability id in `CAPABILITY_METADATA` traces to a registrar in
  `src/cli/capability-cli/`, and every media contract kind in a manifest traces to an
  implementation file in that extension.

### Their gaps versus us (measured)

- **Plugin isolation: none.** `grep -rl "WebAssembly|\.wasm|wasmtime" src/plugins` → **0**
  (instrument proven alive: `grep -rl "plugin" src/plugins` → **724**). Plugins are in-process
  TypeScript.
- **Sandboxing is Docker.** Their `sandbox` sub-CLI is *"Manage sandbox containers (Docker-based
  agent isolation)"*. Only 10 files in `src` mention `bwrap|sandbox-exec|AppContainer`, and the
  one I read (`src/infra/dispatch-wrapper-resolution.ts:458`) uses `sandbox-exec` to
  **unwrap/recognise** an already-wrapped invocation, not to apply its own confinement.
