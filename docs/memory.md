# Memory model

Wayland-Core's memory layer is a **5-partition × 3-tier** SQLite-backed
store that captures what an agent saw, did, learned, and concluded across
sessions. The two axes are orthogonal: every memory write is addressed by
both a *partition* (what kind of memory) and a *tier* (how durable). This
document covers the model, how it persists, when it prunes, and how to
inspect it.

The authoritative type surface lives in
[`crates/wcore-memory/src/v2_types.rs`](../crates/wcore-memory/src/v2_types.rs)
— this doc tracks that file, not the other way around.

## 1. Overview

Wayland-Core agents are long-lived: one user, many projects, many
sessions. A blank-slate agent is useless after session 2 — it re-asks
the same questions, re-discovers the same project quirks, and re-makes
the same mistakes. The memory layer fixes that by writing structured
records (episodes, facts, skill outcomes, user preferences) during the
session and reading them back the next time the agent boots.

Three rules govern the layer:

1. **Opt-in.** Memory is off by default. Until you set
   `memory.enabled = true`, every write is a no-op against
   `NullMemory` and every read returns empty.
2. **Append-only at the row level.** Decay and the dream cycle can flip
   a row's status to `archived`, but rows are never deleted by the
   normal pipeline. Audit and reproducibility need the tail.
3. **Deny-by-default access.** Every read and write presents an
   `AccessToken` (`System` / `MainAgent` / `SubAgent { agent_name }`)
   which the access gate validates against a partition+tier ACL before
   any I/O. See `crates/wcore-memory/src/gate.rs`.

## 2. The model — 5 partitions × 3 tiers

### Partitions (what kind of memory)

The `wcore_memory::v2_types::Partition` enum has exactly five variants:

| Partition    | Variant               | Stores                                                                  |
|--------------|-----------------------|-------------------------------------------------------------------------|
| Working      | `Partition::Working`    | The live turn buffer — recent turns, tool calls, bookmarks.             |
| Episodic     | `Partition::Episodic`   | Timestamped event summaries (`Episode` records).                        |
| Semantic     | `Partition::Semantic`   | Distilled facts as subject/predicate/object triples (`Fact` records).   |
| Procedural   | `Partition::Procedural` | Reusable skill artifacts + Thompson stats (`Procedure` records).        |
| Core         | `Partition::Core`       | The user model — preferences, profile entries (`UserModelEntry` rows).  |

### Tiers (how durable)

The `wcore_memory::v2_types::Tier` enum has exactly three variants:

| Tier      | Variant         | Scope                                                          |
|-----------|-----------------|----------------------------------------------------------------|
| Session   | `Tier::Session` | Lives only while the session is open. Discarded on shutdown.   |
| Project   | `Tier::Project` | Per project root, in `.wayland-core/memory/memory.db`.         |
| Global    | `Tier::Global`  | Cross-project, in `<config_dir>/memory/memory.db`.             |

### The 9 valid (partition, tier) combinations

Not every cross of the two axes is meaningful. Working memory has no
durable form. Core (the user model) has no project-scoped form. Semantic
and Procedural never live at session tier — they exist precisely to
outlive the session. The dispatcher enforces these constraints via
`wcore_memory::v2_types::valid_combinations()` and
`wcore_memory::v2_types::is_valid(p, t)`:

| Partition  | Session | Project | Global |
|------------|:-------:|:-------:|:------:|
| Working    | yes     | —       | —      |
| Episodic   | yes     | yes     | yes    |
| Semantic   | —       | yes     | yes    |
| Procedural | —       | yes     | yes    |
| Core       | —       | —       | yes    |

That's 1 + 3 + 2 + 2 + 1 = **9 valid combinations**. Writes to any other
cell are rejected at the dispatcher boundary. The unit tests
`partition_all_has_five_unique`, `tier_all_has_three_unique`, and
`valid_combinations_count` in `v2_types.rs` lock these invariants in.

Each partition has a *default* tier the dispatcher uses when the caller
doesn't pin one (`Partition::default_tier`): Working→Session,
Episodic→Project, Semantic→Project, Procedural→Project, Core→Global.

## 3. Persistence semantics

### Configuration

Memory is off by default. To enable it, set the following in your config
(`.wayland-core.toml` for project scope, or the global config — see
[getting-started.md](getting-started.md) for cascading precedence):

```toml
[memory]
enabled = true
dream_cycle_throttle_secs = 1800   # minimum gap between dream cycles (30 min)
decay_interval_secs       = 3600   # decay sweep cadence (1 hour)
```

The fields map one-to-one to `wcore_config::config::MemoryConfig`. With
`enabled = false` (the default), the agent binds `NullMemory` —
every mutator is a silent no-op and every reader returns empty. The CLI
inspection flag still works against the null store; it just reports
zero rows.

### Where files live

| Path                                                       | Contents                                  |
|------------------------------------------------------------|-------------------------------------------|
| `<project_root>/.wayland-core/memory/memory.db`            | Project-tier SQLite                       |
| `<config_dir>/memory/memory.db`                            | Global-tier SQLite                        |
| `<config_dir>/memory/sessions/<session_id>.db`             | Session-tier SQLite (one per session)     |
| `<config_dir>/memory/audit.db`                             | Access-gate audit log                     |
| `<config_dir>/memory/changelog/<tier>.changelog.jsonl`     | Append-only CDC tail per tier             |

`<config_dir>` follows OS convention (`~/.config/wayland-core` on Linux,
`~/Library/Application Support/wayland-core` on macOS, `%APPDATA%\wayland-core`
on Windows).

Override the base directory by exporting `WCORE_MEMORY_DIR` before
launch. The legacy `AIONRS_MEMORY_DIR` is honored as a fallback when
`WCORE_MEMORY_DIR` is unset or empty. The resolution order lives in
[`crates/wcore-memory/src/paths.rs`](../crates/wcore-memory/src/paths.rs).

### What's actually stored

Each record type has a stable serializable shape:

- **`Episode`** — `id` (UUIDv7), `tier`, `ts`, `episode_type`, `summary`,
  `atomic_facts`, `source`, `source_product`, `session_id`,
  `project_root`, `decay_score`, `status` (`Active`/`Archived`).
- **`Fact`** — `id`, `tier`, `ts`, `subject`, `predicate`, `object`,
  `confidence`, `source_episode`, `superseded_by` (facts are never
  rewritten in place — supersession is a new row pointing back).
- **`Procedure`** — `id`, `tier`, `ts`, `name`, `description`,
  `artifact` (YAML/markdown skill body), `status`
  (`Staged`/`Active`/`Archived`/`Pinned`), `created_by`, `thompson_alpha`,
  `thompson_beta`, `use_count`, `success_count`.
- **`UserModelEntry`** — `key`, `value` (arbitrary JSON), `ts`.
  Aggregated by `UserModel { entries: Vec<UserModelEntry> }`.

Procedure status transitions are also locked at the type layer:
`Staged → Active | Archived`, `Active → Archived | Pinned`,
`Pinned → Active | Archived`. `Archived` is terminal. The
`ProcedureStatus::can_transition_to` method is the single source of
truth.

### Sources and provenance

Every write carries a `Source` discriminator
(`main-agent` / `sub-agent:<name>` / `consolidate` / `compact` / `legacy`
 / `user` / `system`) and a `source_product` string identifying the crate
that produced the record (`wcore-agent`, `wcore-consolidate`,
`wcore-compact`, `legacy`). This lets the inspector and audit gate
distinguish, e.g., facts the user asserted from facts the dream cycle
derived.

## 4. Decay and the dream cycle

Two automated pipelines prevent unbounded growth. Both run inside
`wcore_memory::consolidate::ConsolidationEngine`.

### Decay (per-row score, continuous)

Every active `Episode` carries a `decay_score: f64`. A background
scheduler runs the decay phase every `decay_interval_secs` and applies
an Ebbinghaus-style curve:

```
new_score = exp(-age_days / 7.0)
```

That is: scores halve roughly every 5 days, and an episode older than
30 days flips to `EpisodeStatus::Archived`. Archived rows stay on disk
and stay queryable for audit — they are simply excluded from active
recall. There is **no DELETE** path in the decay pipeline. The exact
implementation is `ConsolidationEngine::decay()` in
`crates/wcore-memory/src/consolidate.rs`.

### Dream cycle (consolidation, at session end)

The dream cycle runs once per session-end, gated by a `DreamThrottle`
(default minimum 30 minutes between cycles, configurable via
`dream_cycle_throttle_secs`). Within a cycle, four phases run in order:

1. **Compress** (`compress()`) — drains the Working-partition queue,
   summarises every batch of 20 entries, and writes one Episode per
   batch into `Partition::Episodic` at the project tier.
2. **Consolidate** (`consolidate()`) — scans the most recent Episodic
   summaries for fact patterns and asserts them as Semantic rows
   (subject/predicate/object/confidence).
3. **Crystallize** (`crystallize()`) — finds Episode types that recur
   often enough (≥3 occurrences) and crystallizes a `Staged`
   `Procedure` for each repeating pattern. Curation later promotes
   Staged to Active or archives losers.
4. **Decay** (`decay()`) — same per-row decay sweep described above,
   run inline at the tail of every cycle.

Each phase is best-effort: a failure in one phase doesn't block the
next phase. The cycle returns a `DreamReport` with counts:
`episodes_compressed`, `facts_consolidated`, `procedures_crystallized`,
`episodes_decayed`, `elapsed_ms`. The agent-side wiring fires the dream
cycle in `wcore_agent::engine::fire_on_session_end` only when
`cfg.memory.enabled` is true.

The throttle prevents a session-thrash workload from triggering the
cycle on every short session; long inter-active sessions still get one
cycle per `dream_cycle_throttle_secs` window. The throttle helper
(`DreamThrottle`) lives alongside `ConsolidationEngine` and is exercised
by the unit tests in `crates/wcore-memory/tests/dream_throttle_test.rs`.

## 5. Inspection

### CLI: `wayland-core --memory-show <session>`

The fastest way to see what the agent remembers about a session is the
inspection flag (shipped in M3.4):

```bash
$ wayland-core --memory-show 2026-05-16-abc123 --project-dir ~/project
{
  "session_id": "2026-05-16-abc123",
  "project_root": "/home/me/project",
  "episodes": 12,
  "facts": 3,
  "procedures": 7,
  "user_model_entries": 4,
  "recent_episodes": [ ... ],
  "top_procedures": [ ... ]
}
```

The output is JSON for easy piping into `jq`. It works whether memory
is enabled or not — when disabled, all counts return zero. The handler
implementation is in `crates/wcore-cli/src/memory_show.rs`.

### Direct SQL

Each tier's SQLite database is openable with any client. To inspect
project-tier memory directly:

```bash
sqlite3 .wayland-core/memory/memory.db ".schema episodes"
sqlite3 .wayland-core/memory/memory.db \
  "SELECT id, episode_type, ts, decay_score, status FROM episodes ORDER BY ts DESC LIMIT 20;"
```

The schema lives in
[`crates/wcore-memory/src/schema/v1.sql`](../crates/wcore-memory/src/schema/v1.sql).

### Traces

When `memory.enabled = true`, every memory operation emits a
`MemoryOpTrace` (op name, partition, tier, latency in ms, success/error).
Traces flow through the same observability pipeline as the rest of the
agent: stdout JSON sink, OTLP exporter, or the JSON-stream protocol —
see [json-stream-protocol.md](json-stream-protocol.md). This makes
"what does the agent touch on a cold boot?" an observable property,
not a guess.

### Audit log

Every gated access (read or write) writes a row to
`<config_dir>/memory/audit.db`: who asked (`AccessToken` kind +
optional agent name), what partition+tier, allowed or denied, and a
timestamp. The audit log is the system-of-record for sub-agent
permission disputes.

## 6. Configuration reference

```toml
[memory]
# Master switch. Default: false (NullMemory bound; all ops no-op).
enabled = false

# Minimum seconds between dream cycles. Default: 1800 (30 min).
dream_cycle_throttle_secs = 1800

# Background decay sweep cadence in seconds. Default: 3600 (1 hour).
decay_interval_secs = 3600
```

The struct backing this section is `wcore_config::config::MemoryConfig`.
Defaults live in `default_dream_throttle_secs()` and
`default_decay_interval_secs()` in
`crates/wcore-config/src/config.rs`. The Default impl is
`enabled: false` plus those two defaults — every change to the default
posture has to pass the regression tests in
`crates/wcore-config/tests/memory_config_test.rs`.

To opt in for a single project, add the `[memory]` block to that
project's `.wayland-core.toml`. To opt in globally, add it to the
global config (`<config_dir>/wayland-core/config.toml`). Project values
override global values per the cascading rule documented in
[getting-started.md](getting-started.md).

## 7. Privacy notes

What's stored:

- Compressed *summaries* of turns and tool calls — not the raw turn
  text. The Working partition holds raw text in memory during a session
  but only summaries reach the durable tiers.
- Distilled facts derived from those summaries (subject/predicate/object).
- Procedure outcomes — skill name, Thompson alpha/beta, use/success
  counters. No skill arguments, no skill output.
- User-model entries — only what the agent or user explicitly writes to
  the Core partition (e.g. "user prefers imperative commit messages").
  Sub-agents cannot write to Core; only `AccessToken::System` can.

What's not stored:

- Raw conversation transcripts. Compact/compress is summarisation, not
  archival.
- Credentials, secrets, env values. The compression path strips known
  secret patterns; the access gate rejects writes that match the
  sanitization filter (see `crates/wcore-memory/src/compact.rs`).
- Anything from a session that ended before `memory.enabled` was set.
  Memory is forward-only; previous-session content is not retroactively
  ingested.

To wipe project-tier memory:

```bash
rm .wayland-core/memory/memory.db
```

To wipe global-tier memory (including the user model):

```bash
rm -rf "$(wayland-core --print-config-dir)/memory"
```

Both operations are non-recoverable. The CDC changelog tail under
`<config_dir>/memory/changelog/<tier>.changelog.jsonl` is a separate
file and should be removed alongside the database if you want a clean
slate.

## 8. Further reading

- [architecture.md §3 Substrate boundaries](architecture.md#3-substrate-boundaries) —
  where `wcore-memory` sits between IJFW (storage substrate) and the
  agent/skills/evolve substrates.
- [advanced.md](advanced.md) — sub-agent spawning, hook system, plan
  mode, and the older v1 memory surface that the v2 model replaced.
- [skills.md](skills.md) — how the procedural partition feeds the
  skill prioritizer and what gets written when a skill runs.
- [wcore-evolve.md](wcore-evolve.md) — evolutionary prompt variants
  (GEPA). Memory and evolution share the same SQLite handle
  (`wcore_memory::db::Db`) but live in distinct tables.
- [json-stream-protocol.md](json-stream-protocol.md) — how
  `MemoryOpTrace` events surface to host integrations.
- Source of truth: [`crates/wcore-memory/src/v2_types.rs`](../crates/wcore-memory/src/v2_types.rs)
  for the type surface, [`crates/wcore-memory/src/consolidate.rs`](../crates/wcore-memory/src/consolidate.rs)
  for the dream-cycle pipeline,
  [`crates/wcore-memory/src/gate.rs`](../crates/wcore-memory/src/gate.rs)
  for the access ACL.
