# RECON-OPENCLAW — capability map

**Peer tree:** `/Users/seandonahoe/dev/resources/openclaw`, read-only, never executed.
**Measured at HEAD `3659c85e534fdb8b8ce6b7505a83d92cc2e4df8e`, version `2026.7.2`.**
The ledger's pin is `11a0ad10` / `2026.6.2`; local HEAD has moved. Everything below is measured at
`3659c85e53` and I say so. I did not re-pin the ledger.

Working notes with every command and near-miss: `.planning/intel/RECON-OPENCLAW-NOTES.md`.
All counts came from `/usr/bin/grep` / `/usr/bin/find`, each with a known-positive in the same
invocation (§3b-i). Two of my own instruments produced false zeros before I caught them; both are
recorded in the notes rather than quietly fixed.

**Scale, for calibration.** 114 entries in `src/`, 24 workspace packages, **148 plugin manifests**
in `extensions/`, **85 tables** in one migrated SQLite state DB, 22 root CLI commands + 41 sub-CLIs.
Ours: 56 crates, 29 top-level CLI commands.

---

## 1. Capability map

Gap sizes are about **user-visible capability**, not code volume. "Lane-sessions" assumes one lane
at the intensity this programme runs — a session being roughly one plan executed and proven.

| Area | What it does for a user | How it's built | Our nearest equivalent | Gap | Lane-sessions to match |
|---|---|---|---|---|---|
| **Media CLI surface** | `openclaw infer <verb>` drives image gen/edit/describe, audio transcribe, TTS convert/voices/personas, video gen/describe, web search/fetch, embeddings — **31 self-describing capability ids**, each with declared `transports`, `flags[]`, `resultShape`. `infer capability list/inspect` prints the matrix at runtime. | `src/cli/capability-cli.ts` composes 8 registrars. `CAPABILITY_METADATA` in `capability-cli/metadata.ts` is the single source. Uniform `CapabilityEnvelope { ok, capability, transport, provider, model, attempts[], inputs[], outputs[], ignoredOverrides[], error }`. | `wayland-core image` (`crates/wcore-cli/src/image.rs`, 16.3K) + agent tools `image_generate`, `text_to_speech`, `transcribe_audio`, `vision_analyze` (`crates/wcore-tools/src/registry.rs`). No unified verb, no capability introspection, no uniform envelope. | **wide** | 4–6 |
| **Media provider breadth** | Any of 18 vision, 15 TTS, 15 video, 11 image, 5 realtime-transcription, 5 music, 15 web-search providers, selected by config. | Core owns contracts only — `BUILTIN_IMAGE_GENERATION_PROVIDERS` is deliberately `[]`. Every provider arrives via `resolvePluginCapabilityProviders({key, cfg})` from an extension manifest. | Backends compiled in: `image_gen.rs` 58.3K, `tts.rs` 41.0K, `video_analyze.rs` 41.6K, `piper.rs` 35.0K, `voice_mode.rs` 37.8K, `openai_compat_whisper.rs`, 3 vision backends, 7 web-search backends. Real code, far fewer providers, no plugin contribution path for media. | **wide** | 6–10 |
| **Media capability negotiation** | User passes `--aspect-ratio 21:9` to a provider that can't do it; the run succeeds and the CLI **reports which flags were dropped**. | `ImageGenerationProviderCapabilities { generate, edit, geometry, output }` with per-model maps (`sizesByModel`, `maxInputImagesByModelPrefix`, …). Mismatches become `ignoredOverrides[]` on the envelope, not errors. | Nothing equivalent measured. | **absent** | 2–3 |
| **Vision without provider code** | Vision works on any configured model provider whose catalog marks the model image-capable — zero media-specific plugin code. | `hydrateModelBackedMediaProvider` back-fills `describeImage`/`describeImages` with the generic model runtime; config providers auto-register (`#51392`). | Per-provider vision backends (`anthropic_vision.rs`, `gemini_vision.rs`, `openai_vision.rs`) — new provider means new file. | **narrow** | 1–2 |
| **Media intake / normalisation** | Attachments from any channel are fetched, sniffed, size-bounded, transcoded, PDF/doc-extracted, and cached before reaching the model. | `src/media/` ~40 modules: `web-media.ts` 37.7K, `store.ts` 22.1K, `parse.ts` 21.7K, `fetch.ts` 20.8K, `input-files.ts` 15.8K, `ffmpeg-exec.ts`, `pdf-extract.ts`, `image-ops.ts`, `local-roots.ts`, `read-capability.ts`, SSRF policy, `configured-max-bytes.ts`. | `crates/wcore-cli/src/attachments.rs` 6.4K + channel-side handling. Much thinner. | **wide** | 3–5 |
| **Durable tasks** | `openclaw tasks list` / `tasks audit` show background tasks that survive restarts, with per-task retention reasoning. | `src/tasks/` ~90 modules. `task-registry.ts` 87.8K (170.5K test), SQLite store, `task-flow-registry.ts` 23.8K for flows, per-task owner/status access control, retention + cleanup stamping. | `crates/wcore-swarm`, `wcore-dispatch`, `wcore-cron`; `Swarm`/`Cron` CLI. Durable pieces exist but no single durable task registry with an operator inspect surface measured. | **wide** | 5–8 |
| **Restart survival** | A gateway restart doesn't orphan or falsely kill in-flight work. | 85-table state DB incl. `gateway_restart_handoff`, `gateway_restart_intent`, `gateway_restart_sentinel`, `gateway_boot_lifecycle`, `state_leases`, `delivery_queue_entries`. Sweep classifies each task `reconciled / recovered / cleanupStamped / pruned`. | `crash_sentinel.rs` 25.8K, `wcore-replay`, session state. Partial. | **wide** | 4–6 |
| **Not-dead-just-unseen discipline** | A task isn't marked lost because *this* process can't see it. | `hasLostGraceExpired`, `hasBackingSession`, `isRuntimeAuthoritative()` (cron/acp tasks retained when this process isn't authoritative), `hasActiveCliRun`, `isSubagentRecoveryWedgedEntry`. `findDetachedTaskRun` returns `{lookup: "available" \| "unavailable"}` — *"an empty fallback cannot prove that the runtime-owned task is absent."* | Not measured on our side. | **absent** | 2–3 |
| **Maintenance dry-run + diagnostics** | Operator can preview exactly what the sweep would do, and read *why* each stale task is being kept. | `previewTaskRegistryMaintenance()`; `getTaskRegistryMaintenanceDiagnostics()` → per-task `{taskId, runtime, status, decision, reason, ageMs, detail}`. | Nothing equivalent measured. | **absent** | 1–2 |
| **Completion contract** | An agent that ends a background task with "I'll now check the logs" is marked **blocked**, not succeeded. | `task-completion-contract.ts` — 3 regexes + `hasNonProgressFollowupSentence` escape hatch; sets `terminalOutcome:"blocked"`, `"Required completion ended with progress-only text, not a final deliverable."` | Nothing equivalent. | **absent** | 1 |
| **Commitments** | Follow-ups inferred from conversation become tracked commitments with heartbeat policy. `openclaw commitments`. | `src/commitments/` — `runtime.ts` 12.8K, `store.ts` 14.6K, `extraction.ts` 11.5K, `commitments` table, full-chain integration + heartbeat e2e tests. | None measured. | **absent** | 3–4 |
| **Migration in** | `migrate list / plan / apply`, preview-then-prompt, **verified backup before apply by default** (`--no-backup` needs `--force`), item-level backups on `--overwrite`, per-item `--skill` / `--plugin` selection, `--json`. | `src/commands/migrate/` (16 modules) + plugin `migrationProviders` contract. Hermes and Claude/Codex providers. `migration_runs` + `migration_sources` tables. | `wayland-core migrate` — `mod.rs` 41.5K, `hermes.rs` 19.7K, `quarantine.rs` 34.3K, `provenance.rs` 10.5K, `select.rs` 12.8K. Structurally comparable; **narrower in what it imports** (see below). | **narrow** | 2–3 |
| **Migration fidelity** | Hermes `config.yaml`, `.env`, `auth.json`(+OpenCode), `SOUL.md`, `AGENTS.md`, `MEMORY.md`/`USER.md` (appended), `skills/` all imported; 11 dirs + 13 db/json files **archived into the report** rather than dropped; retired-provider items become `manual` with a concrete remedy command. | `MigrationItem{id,kind,action,status,reason,message,details}`, `kind` ∈ file/workspace/memory/skill/auth/secret/manual/archive. | `hermes.rs` imports `config.yaml` + `.env`; `skills/`, `SOUL.md`, `memories/` are **counted as deferred**, not imported. | **wide** | 2–3 |
| **Native apps** | Real macOS menu-bar app, App Store iOS app with Watch app, share extension and live activities, Android app with Wear, Linux Tauri app. | `apps/macos` SwiftPM (Sparkle, Peekaboo automation), `apps/ios` (Calendar/Camera/Contacts/EventKit/Health/Location/Motion/Push/Media), `apps/android` Gradle + `wear/`, `apps/linux` Tauri, `apps/swabble` on-device wake word. | **None.** We ship a CLI/TUI; Wayland Desktop is a separate Electron product outside this repo. | **absent** | 25+ (out of scope for a build wave) |
| **Desktop update channel** | macOS app self-updates. | Root `appcast.xml` — Sparkle 2 RSS, latest item `2026.7.1`, `sparkle:version 2607000190`, `minimumSystemVersion 15.0`, served from raw.githubusercontent. | `self_update.rs` 30.3K + `update_trust.rs` 43.2K for the CLI binary. Different target; ours is arguably stronger on trust. | **narrow** (different scope) | n/a |
| **Channels** | 26 chat channels. | Plugin manifests. | ~10 channel crates. | **wide** | 6–8 |
| **Plugin isolation** | — | In-process TypeScript. `grep -rl "WebAssembly\|\.wasm\|wasmtime" src/plugins` → **0** (known-positive `"plugin"` → 724). | `wcore-plugin-wasm`, `wcore-plugin-subprocess`, `wcore-plugin-api` boundary lint. | **we lead** | — |
| **Sandboxing** | Docker containers (`openclaw sandbox` = *"Docker-based agent isolation"*). Only 10 files mention `bwrap\|sandbox-exec\|AppContainer`, and the one read uses `sandbox-exec` to *unwrap* an existing invocation. | — | `wcore-sandbox`: bwrap (Linux), sandbox-exec (macOS), AppContainer + Job Object (Windows), probing real spawn. No Docker required. | **we lead** | — |

---

## 2. The five things they do that we cannot do at all

Ranked by what a user would feel first.

1. **Use the product on a phone, watch or as a Mac app.** `apps/ios` ships Chat, Push, Share
   Extension, Live Activities, a Watch app with full voice turns, and offline transcript caches;
   `apps/android` ships with Wear; `apps/macos` is a menu-bar app self-updating through a Sparkle
   appcast. We have no native client in this repo at all. This is the single widest gap and the
   one a user meets before anything else.
2. **Reach 26 chat channels and 11–18 providers per media capability without us writing code.**
   148 plugin manifests, every media provider contributed through a declared contract
   (`imageGenerationProviders`, `speechProviders`, …). Our media backends are compiled in, so
   every new provider is our work. Their `BUILTIN_IMAGE_GENERATION_PROVIDERS = []` is the whole
   architecture in one line.
3. **Discover and drive every media capability from one CLI verb.** `openclaw infer capability
   list` enumerates 31 ids with their transports, flags and result shapes; `infer capability
   inspect --name image.edit` explains one. Every result is the same `CapabilityEnvelope`, and
   `ignoredOverrides[]` tells the user which of their flags the chosen provider silently dropped.
   We have `wayland-core image` and four agent tools with no shared shape and no introspection.
4. **Survive a gateway restart with in-flight background work intact, and prove it to an
   operator.** One migrated 85-table SQLite state DB, explicit restart handoff/intent/sentinel
   tables, a durable delivery queue, and a sweep whose four outcomes are individually explainable
   per task via `getTaskRegistryMaintenanceDiagnostics()`. A user's long-running task is still
   there — and an operator can ask *why* it was kept or reclaimed.
5. **Move a whole Hermes install across in one previewable, backed-up transaction.** Config, env,
   auth (including OpenCode's `auth.json`), SOUL, AGENTS, memories (appended, or filed as an
   identifiable `hermes` collection), and skills — with the 24 unmappable dirs/DBs archived into
   the migration report instead of dropped, security warnings about shared OAuth refresh grants,
   and a verified backup taken before apply unless the user passes `--force`. We import config and
   env, and *count* the skills, SOUL and memories we do not import.

---

## 3. The things we do that they cannot

Three, and I checked each rather than reaching for a round number.

1. **OS-level sandboxing with no Docker dependency.** `wcore-sandbox` implements bwrap (Linux),
   sandbox-exec (macOS), and AppContainer + Job Object (Windows), and probes a real spawn rather
   than trusting an API check. OpenClaw's `sandbox` sub-CLI is explicitly *"Docker-based agent
   isolation"*; their only `sandbox-exec` usage I found is recognising a wrapper someone else
   applied. A user without Docker gets isolation from us and not from them.
2. **Untrusted plugin isolation.** `wcore-plugin-wasm` and `wcore-plugin-subprocess` behind a
   lint-enforced `wcore-plugin-api` boundary. Their plugins are in-process TypeScript — measured
   zero WASM/wasmtime references anywhere in `src/plugins` against a live instrument. With 148
   plugins in-process, this is a real security asymmetry in our favour.
3. **We migrate from them; they do not migrate from us.** `crates/wcore-cli/src/migrate/
   openclaw.rs` (18.8K) exists alongside `hermes.rs`, plus `quarantine.rs` (34.3K) and
   `provenance.rs` (10.5K) — machinery they have no equivalent of by name. Their
   `migrationProviders` contract lists hermes and claude only.

**Honest caveat on all three:** these are *architectural* advantages I verified by reading both
trees. I could not run either binary (no cargo on the Mac), so I am claiming the capability
exists and is structurally sound, not that it is field-proven here.

---

## 4. Things that look impressive but are dead — and a correction

**Dead surfaces found: essentially none.** I went looking, because we have found ten in our own
tree. Every id in `CAPABILITY_METADATA` traces to a registrar in `src/cli/capability-cli/`; every
media contract kind in a manifest traces to an implementation file in that extension; and the
implementations are large with proportionally large tests (`extensions/fal/image-generation-
provider.ts` 27.9K / 49.8K test; `src/agents/tools/image-tool.test.ts` **109.0K**).

What I did find, and it is honest labelling rather than rot:

- **`fleet`** — self-described *"Provision and manage isolated tenant cells (experimental)"*.
  Their own not-finished marker.
- **`qa`** — filtered out of help unless `isPrivateQaCliEnabled()`. Private, not dead.
- **`clawbot`** — *"Legacy clawbot command aliases"*, a compatibility shim.
- **`crestodian`** — hidden deprecated alias for `setup`.

**Correction I have to record, because it nearly became a finding.** I was about to file *music
generation* as advertised-but-dead: 5 provider manifests declare `musicGenerationProviders`, and
there is **no music verb in `infer`**. It is not dead — it is reachable as an *agent tool*
(`src/agents/tools/music-generate-tool.ts`, `.actions.ts`, `music-generate-background.ts`). Media
has **two** surfaces, the `infer` CLI and the agent tool family, and checking only one produces a
false absence. Same shape as the `infer`-not-`media` vocabulary trap that nearly made me call the
entire media CLI non-existent an hour earlier.

---

## 5. Where I would point a build wave

Not asked for, but it falls out of the table. Ranked by capability-per-lane-session:

1. **Media capability CLI + uniform envelope (4–6 sessions).** We already own the expensive part —
   `image_gen.rs`, `tts.rs`, `video_analyze.rs`, `piper.rs`, whisper and three vision backends are
   real code. What is missing is the *front*: one verb, a capability metadata table, introspection,
   and a shared result envelope. This converts existing `SOURCE`-grade artifacts into a reachable,
   demonstrable surface faster than anything else on the list.
2. **Completion contract + maintenance diagnostics (2–3 sessions).** Small, self-contained, and
   directly aimed at the failure mode this programme keeps measuring in its own agents.
3. **Migration fidelity: import the skills, SOUL and memories we currently defer (2–3 sessions).**
   The mapping is already understood; `hermes.rs` counts these files today. Adopting their
   archive-and-report pattern for what we cannot map is the honest completion.

**A caution on the `MEDIA-*` = `SOURCE` ledger grade.** On artifact presence that grade looks
understated — I measured five large media backends and four registered tool names on our side.
On *reachability and uniformity* it looks correct. I could not test effectiveness (no cargo on the
Mac), so I am not proposing a regrade; I am flagging that the row deserves a re-read before a wave
is sized against it.

---

_Measured 2026-07-29 by lane `recon-openclaw` against OpenClaw `3659c85e53` (2026.7.2)._
_Peer tree read-only throughout; nothing installed, nothing executed._
