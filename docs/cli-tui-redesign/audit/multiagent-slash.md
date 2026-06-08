# Functional audit — Multi-Agent / Slash Commands / REPL / Skills-MCP-Memory-Repomap-Replay

Scope: surfaces 04 (Agent Manager), 05 (Command Palette), 02 (Workspace
composer), 06 (Plan Review safety panel), plus the rail panels.

Verdict legend: **ACCURATE** = matches engine. **RENAME** = capability
exists under a different name/shape. **ASPIRATIONAL-OK** = net-new UI but
a real lower-level primitive exists to build on (primitive named).
**WRONG** = no backing capability; would mislead.

## Findings table

| # | Mockup element | Engine reality (file:line) | Verdict | Fix needed |
|---|---|---|---|---|
| 1 | "Agent Manager" — multi-agent mission control surface | No such surface. The engine has no orchestrator UI, no agent-manager screen, no persistent agent process list. `crates/wcore-cli/src/main.rs:494` `repl_loop` is bare readline. | ASPIRATIONAL-OK | Primitives: `SpawnTool` (`crates/wcore-agent/src/spawn_tool.rs:19`), `AgentSpawner.spawn_parallel` (`spawner.rs:102`), `SubAgentRelay`/`ChannelSink` event relay (`spawn_tool.rs:222`). The *card grid* is net-new UI; the *data* it would render is real but only mid-`Spawn`-call. See §Agent Manager. |
| 2 | "orchestrator · 4 agents spawned · 2 running · 1 blocked · 1 done" status counts | `MAX_SUB_AGENTS = 5` (`spawn_tool.rs:17`). Sub-agents run as parallel `tokio::spawn` futures inside ONE `Spawn` tool call (`spawner.rs:118-125`); they are joined and returned together. No "running/blocked/done" lifecycle state is tracked or queryable. | WRONG | "blocked / waiting on another agent" has NO backing — `spawn_parallel` has no inter-agent dependency graph. Drop the lifecycle counts or rebuild on new infra. |
| 3 | Per-agent progress bar + "64% / 41% / 12%" | No progress signal exists. `SubAgentResult` carries `text, usage, turns, is_error` only (`spawner.rs:79-85`). `orchestration/monitor.rs` tracks budget + repeated-errors, NOT completion percent. A sub-agent cannot report "% done" — it doesn't know its own end state. | WRONG | A ratatui Gauge has nothing to bind to. Remove % bars, or replace with turn-count / token-count (those ARE in `SubAgentResult`). |
| 4 | Per-agent live feed ("read vertex.rs", "grep …") | REAL but only with relay wired: `spawn_with_relay` (`spawn_tool.rs:222`) drains `SubAgentRelay` events via `emit_sub_agent_event`. Requires `with_registry` + `with_parent_output` both set (`spawn_tool.rs:43-55`); otherwise sub-agents use `NullSink` and emit nothing (`spawner.rs:163`). | ASPIRATIONAL-OK | Primitive: `ChannelSink` + `emit_sub_agent_event`. The live feed is buildable, but only during an active Spawn call and only on the relay path. |
| 5 | Agent roles "code-explorer / builder / code-reviewer / doc-writer" | `AgentRegistry` loads named `AgentManifest`s from `~/.wayland-core/agents/*.yaml` and project dirs (`agents/registry.rs:1-5`). A `Spawn` task may name one (`spawn_tool.rs:92`). | RENAME | Roles are user-authored YAML manifests, not built-in fixed roles. Mockup should say "agents from your registry" not imply 4 canned roles. Also: one named agent per Spawn call max (`spawn_tool.rs:241` "first named task wins"). |
| 6 | "+ spawn agent" button | `Spawn` is an LLM-invoked tool, not a user action. There is no user-facing "spawn an agent" command anywhere. | WRONG | No backing. Either cut, or it becomes new infra that lets the user (not the model) trigger a Spawn. |
| 7 | "auto-merge when all agents settle" | No merge concept on the `Spawn` path — results are concatenated into one `ToolResult` string (`spawn_tool.rs:152-168`). Worktree-isolated merge exists ONLY in `wcore-swarm` (separate subprocess-per-worker dispatch, `crates/wcore-swarm/src/lib.rs`), which is NOT wired to the REPL or `SpawnTool`. | WRONG | "auto-merge" is not real for the Spawn surface. `wcore-swarm` could back a real version but is a different, unwired subsystem. |
| 8 | Command Palette — `/` opens fuzzy picker over "31 commands", "ordered by frecency" | The REPL handles exactly TWO inputs starting with `/`: `/quit` and `/exit` (`main.rs:508`). There is no palette, no fuzzy match, no frecency, no command registry. Everything else is a `clap` CLI flag. | WRONG | The entire palette is net-new. "31 commands" is invented. See §Slash commands table for what each shown command can map to. |
| 9 | `/rewind` — "time-travel — restore history, files, or both", "esc esc" | Partial primitive. `FileHistory` snapshots pre-edit file bytes (`file_history.rs:126`), `RollbackTool` restores a file N edit-steps back (`rollback_tool.rs:1-19`). BUT: it is an LLM-invoked tool, per-file, max 10 snapshots (`file_history.rs:53`), session-ephemeral. "Restore history" (conversation rewind) does NOT exist. `wcore-replay` is trace *inspection* only — `lib.rs:11-15` explicitly states in-process rehydration is out of scope. | ASPIRATIONAL-OK | Primitive for FILE restore: `RollbackTool` + `FileHistory`. There is NO conversation-history restore. `/rewind` may claim file restore only; "restore history, or both" is WRONG. |
| 10 | `/review` — "review the working tree for bugs & convention drift" | No engine command. `GitTool` (`crates/wcore-tools/git.rs`) gives diff/status; review is just a prompt to the agent. | WRONG | No backing capability. It would be a canned prompt, not a command — represent it as such or cut. |
| 11 | `/resume` — "reopen a previous session" | REAL as a CLI flag: `--resume <id>` and `--list-sessions` (`main.rs:147-156`); `SessionManager` in `crates/wcore-agent/src/session.rs`. Not a REPL slash command today. | ASPIRATIONAL-OK | Primitive: `--resume` / `SessionManager`. Promoting it to an in-REPL `/resume` picker is net-new UI over a real capability. |
| 12 | `/repomap` — "rebuild the codebase symbol index" | REAL crate: `wcore-repomap` builds a `RepoMap` via `RepoMap::build` (`crates/wcore-repomap/src/lib.rs:27`), and `RepoMapTool` exists (`crates/wcore-tools/repomap.rs`). No `/repomap` REPL command and no `--repomap` flag. | ASPIRATIONAL-OK | Primitive: `wcore-repomap` + `RepoMapTool`. Surfacing it as a slash command is net-new but trivially backed. |
| 13 | `/model` — "change model and reasoning effort", shortcut ⌥M | No interactive model switch. Model is set via `--model` flag / config / profile (`main.rs:119`). The json-stream protocol has a `SetConfig` command, but the REPL has no mid-session model change. | ASPIRATIONAL-OK | Primitive: `Config.model` + reasoning-effort plumbing (`engine.set_initial_reasoning_effort`, `spawner.rs:217`) and protocol `SetConfig`. In-REPL `/model` is net-new wiring. |
| 14 | Composer ghost text "seeded from git history", Tab accepts | Nothing like this exists. No autosuggest, no git-log mining, no readline ghost-completion. `repl_loop` is `read_line` into a `String` (`main.rs:504-506`). | WRONG | Pure invention. No primitive — `GitTool` can read log, but there is zero suggestion engine. Cut or label as a stretch feature. |
| 15 | Composer hints "/ commands", "@ file", "! shell" | `/` → only `/quit`/`/exit` (see #8). `@ file` reference → no `@`-expansion in the REPL. `! shell` → skills support a `!shell:` *directive inside SKILL.md* (`crates/wcore-skills/src/shell.rs`), not a composer `!` prefix. | WRONG | None of the three composer affordances exist as shown. `!shell:` is a skill-authoring syntax, not a REPL prefix. |
| 16 | "Path map" rail — live file tree, touched/read/idle dots | No live touched-files tree. `wcore-repomap` produces a `RepoMap` (flat `Vec<FileSummary>`, `repomap/src/lib.rs:38`) renderable as a tree, but it is a static index, not a live per-session touch tracker. File-write events do exist (`file_write_notifier.rs`, `file_watcher_notifier.rs`). | ASPIRATIONAL-OK | Primitives: `RepoMap` for the tree shape + `FileWriteNotifier` for touched-status. Combining them into a live rail is net-new. |
| 17 | Activity feed: "repomap indexed 777 files" | `RepoMap::build` walks the tree and counts files (`repomap/src/lib.rs:40-45`). The engine repo has **929 git-tracked files / 640 `.rs` files**. "777" is plausible as a file count (repomap walks more than just `.rs`, respecting `.gitignore`) but is a fabricated specific number. | RENAME | The *capability* is real; the literal "777" is invented. Use a generic placeholder or wire the real count. "indexed" is accurate. |
| 18 | Activity feed: "AGENTS.md loaded as context", "session a3f8c2 started" | Real: AGENTS.md ingestion exists (`crates/wcore-agent/src/agents_md.rs`); sessions have IDs (`session.rs`). No structured "Activity feed" surface emits these as a timeline today, but the events are real. | ASPIRATIONAL-OK | Primitive: protocol events + `agents_md.rs`. Feed UI is net-new aggregation of real events. |
| 19 | "workshop" fullscreen mode, Ctrl+W | No TUI, no ratatui, no crossterm, no fullscreen mode anywhere. `grep` for `ratatui/crossterm/tui/Workshop` in the CLI crate returns nothing. Today's CLI is line-oriented stdout + `--json-stream` protocol mode (`main.rs:163`). | WRONG | The whole TUI ("maps 1:1 to ratatui") is a redesign target, not current state. "Ctrl+W workshop" has zero backing. Acceptable as a redesign *proposal* but must not be shown as existing behavior. |
| 20 | Plan Review `compat_parity.rs` test + atomic "/rewind safe" commits | `compat_parity.rs` is illustrative content of an example plan, not an engine fixture — fine as mock data. Plan mode itself is real (`crates/wcore-agent/src/plan/`). "atomic commits" — `GitTool` can commit (`git.rs`), `git_commit_message.rs` exists; but commits are not auto-atomic-per-step and `/rewind` does NOT restore commits (see #9). | RENAME | Plan mode = ACCURATE. "/rewind safe atomic commits" overstates: `/rewind`/`RollbackTool` restores files from a snapshot store, not git commits. Reword the safety claim. |

## Agent Manager — what is real vs net-new

**The real primitive is the `Spawn` tool.** Read `spawn_tool.rs` and
`spawner.rs` end to end:

- `Spawn` is a single **LLM-invoked tool call** (`SpawnTool` impl `Tool`,
  `spawn_tool.rs:59`). The *model* decides to spawn; the user never does.
- It is **fire-and-collect**, not live orchestration. `spawn_parallel`
  builds N `tokio::spawn` futures, then a `for future in futures` loop
  `await`s every one before returning (`spawner.rs:118-140`). The parent
  tool call blocks until all sub-agents finish; results are concatenated
  into one `ToolResult` string (`spawn_tool.rs:152-168`).
- **Max 5** sub-agents per call (`MAX_SUB_AGENTS`, `spawn_tool.rs:17`).
- Sub-agents are **stateless and independent** by design — the tool's own
  description says "Do NOT use for tasks that need shared state or
  sequential coordination" (`spawn_tool.rs:71`). So "auditor blocked,
  waiting on mason" — an inter-agent dependency — is the **opposite** of
  what `Spawn` supports.
- There is **no progress signal**. `SubAgentResult` = `{name, text,
  usage, turns, is_error}` (`wcore_types::spawner`, re-exported
  `spawner.rs:22`). No percent, no phase, no "running/blocked/done".
- The **one genuinely buildable piece** is the live feed: when both
  `with_registry` and `with_parent_output` are wired, `spawn_with_relay`
  streams each sub-agent's events to the parent via `ChannelSink` +
  `emit_sub_agent_event` (`spawn_tool.rs:222-300`). That is a real
  event stream you could render as a per-agent feed — but only while a
  Spawn call is in flight, and only on the relay path (default path uses
  `NullSink`, `spawner.rs:163`, and emits nothing).

**Separately**, `wcore-swarm` (`crates/wcore-swarm/src/lib.rs`) is a
*different* subsystem: worktree-isolated, subprocess-per-worker dispatch
with `dispatch`/`collect`/`cleanup`, heartbeats, and consensus/debate
scoring. It is closer in spirit to "mission control" but is **not wired
to the REPL or `SpawnTool`** and runs external processes, not in-engine
agents. The `orchestration/` module (`graph.rs`, `templates.rs`,
`monitor.rs`) has a directed-graph executor with templates
(Sequential / Parallel / Hierarchical / Consensus) — but `mod.rs:14`
notes the graph executor was "additive — not wired into the per-turn
loop" until Wave OR wired the Direct template only.

**Verdict on the Agent Manager surface:** the screen is **mostly
aspirational**. Buildable on real primitives: the agent *list* (names
from `AgentRegistry`), the *live feed* (`ChannelSink` relay), and
*tokens/turns/files* stats (`SubAgentResult.usage`/`.turns`). Net-new
infrastructure required for: per-agent **progress %** (no signal
exists), the **running/blocked/done lifecycle** (Spawn is fire-and-join,
no lifecycle states), **inter-agent dependencies** ("blocked on mason" —
explicitly unsupported), **"+ spawn agent"** as a user action (Spawn is
model-invoked), and **"auto-merge"** (results are string-concatenated).
If the redesign wants a true Agent Manager, it should be built on
`wcore-swarm` (which has real worker lifecycle + worktree merge), not on
`SpawnTool` — and that is a substantial integration, not a UI skin.

## Slash commands — real backing per command

The REPL today recognizes **only `/quit` and `/exit`** (`main.rs:508`).
Every other "command" is either a `clap` flag or nonexistent. For a
palette to ship, each entry needs backing:

| Palette command | Real engine capability | Status |
|---|---|---|
| `/quit`, `/exit` | `repl_loop` break (`main.rs:508`) | EXISTS as REPL input |
| `/resume` | `--resume` flag + `SessionManager` (`main.rs:147`, `session.rs`) | Real flag → needs REPL wiring |
| `/repomap` | `wcore-repomap` `RepoMap::build` + `RepoMapTool` (`repomap/src/lib.rs:27`) | Real crate → needs command wiring |
| `/model` | `Config.model`, reasoning-effort plumbing, protocol `SetConfig` | Partial → needs in-REPL switch infra |
| `/rewind` (files) | `RollbackTool` + `FileHistory` (`rollback_tool.rs`, `file_history.rs`) | Real, per-file, 10-snapshot cap, model-invoked → needs user command |
| `/rewind` (conversation history) | — | **No backing. Conversation rewind does not exist.** |
| `/review` | — (just a prompt; `GitTool` gives diff only) | **No backing — would be a canned prompt** |
| `/compact` | `wcore-compact` + engine compaction (`--compaction` flag) | Real flag → needs command wiring |
| `/doctor` | `--doctor` (`main.rs:184`, `doctor/` module) | Real flag → needs command wiring |
| `/login`, `/logout` | `--login` / `--logout` OAuth (`main.rs:241-247`) | Real flags → need command wiring |
| `/skills` (audit/promote/archive) | `--skills-audit` / `--skills-promote` / `--skills-archive` (`main.rs:189-211`) | Real flags |
| `/memory` | `--memory-show`, `wcore-memory` (`main.rs:219`) | Real flag |
| `/replay` | `--replay` trace inspection (`main.rs:228`) — inspection only, NOT live rewind | Real flag (inspection only) |
| `/plugin` | `plugin` subcommand (`main.rs:273`) | Real subcommand |
| user skills as `/<name>` | Skills carry `user_invocable` + `hide-from-slash-command-tool` frontmatter (`wcore-skills/src/types.rs:28-29`), and `processPromptSlashCommand` parity exists in `executor.rs:22`. Skills CAN be modeled as slash commands. | **Real primitive** — best basis for a genuine palette |

**Bottom line on the palette:** "31 commands ordered by frecency" is
invented — there is no command registry and no frecency store. But a
*real* palette is buildable: ~10 of the existing `clap` flags/subcommands
plus user-invocable skills (`wcore-skills` already has the
`user-invocable` / `hide-from-slash-command-tool` frontmatter and a
slash-command parser). The honest framing: the palette is net-new UI
that should be backed by (a) promoting existing CLI flags to in-REPL
commands and (b) the skills system's user-invocable surface — NOT by a
fictional 31-command registry.

## Summary

- **Plan mode** (06) and **session resume**, **repomap**, **memory**,
  **replay-as-inspection**, **skills**, **MCP**, **rollback (file-level)**
  are all real engine capabilities — the redesign can surface them.
- **Agent Manager** (04) is the highest-risk surface: real primitive is
  the fire-and-collect `Spawn` tool (max 5, no progress, no lifecycle,
  no dependencies). Progress bars, running/blocked/done, inter-agent
  blocking, "+ spawn agent", and "auto-merge" are all net-new. A true
  mission control belongs on `wcore-swarm`, not `SpawnTool`.
- **Command Palette** (05): no palette, no frecency, "31 commands"
  invented. Buildable from existing flags + user-invocable skills.
- **WRONG / pure invention**: ghost text from git history, `@ file` and
  `!` composer prefixes, `/review`, conversation-history `/rewind`,
  "workshop" Ctrl+W fullscreen TUI (no ratatui/crossterm in the repo).
- The mockup's own header ("TUI prototype · maps 1:1 to ratatui") is
  honest that this is a *redesign*. The audit's job is to ensure the
  redesign is built on real primitives — flagged above per element.
