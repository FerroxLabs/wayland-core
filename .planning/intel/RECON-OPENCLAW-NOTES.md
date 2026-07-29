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
