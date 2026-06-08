# wayland-core TUI Mockup — Functional Audit

**Date:** 2026-05-22 · **Audited:** `mockups/wayland-tui.html` against `<engine-root>` @ `feat/v0.6.1-hardening`
**Method:** 5 parallel auditor agents, claim-by-claim against engine source. Slice detail in `audit/{tools,config-auth,providers-modes,multiagent-slash,swarm}.md`.

## Headline

The engine has **no TUI at all** today — no `ratatui`/`crossterm` anywhere. The current CLI is a bare readline REPL (`/quit`, `/exit` only) plus a `--json-stream` protocol. So the mockup is **100% a redesign target**, not a depiction of current state. The audit's job: make sure every element is (a) named correctly, (b) grounded on a real engine primitive, (c) not contradicting how the engine actually works.

Verdict spread across ~55 audited claims: **~40% accurate**, **~25% net-new but grounded**, **~35% wrong** (mis-named or contradicts the engine).

---

## A. ACCURATE — keep verbatim

| Element | Engine truth |
|---|---|
| Tool names Read / Edit / Bash / Grep / Glob / Spawn | All verbatim-correct (`wcore-tools`, `spawn_tool.rs:61`) |
| Tool-card layout (glyph · name · arg · chip) | Maps to `ToolInfo{name,category,args}` + `ToolCategory` (`events.rs:482`) |
| The diff CODE — `self.compat.max_tokens_field.as_deref().unwrap_or("max_tokens")` | Exact real code. `max_tokens_field: Option<String>` on `ProviderCompat` (`compat.rs:13`), consumed verbatim (`openai.rs:266`) |
| Approval modal — 3 options | Maps 1:1: `ToolApprove{Once}` / `ToolApprove{Always}` (category `edit`, session-lived) / `ToolDeny{reason}` (`commands.rs:71`). Most accurate surface in the mockup |
| Session id `a3f8c2` | Real format — `format!("{:06x}", …)`, 6 hex chars (`session.rs:224`) |
| Compaction | `CompactionLevel` Off/Safe/Full, default Safe, threshold-driven (`wcore-compact/level.rs`) |
| Plan mode CORE claim — "proposes, read-only until you approve" | Real — `EnterPlanMode`/`ExitPlanMode` tools, read-only while active (`agent/plan/tools.rs`) |
| Status bar cwd + branch | Real |

## B. WRONG — naming. Must fix.

| # | Mockup says | Reality | Fix |
|---|---|---|---|
| B1 | model `sonnet 4.6` (×6) | Canonical id is `claude-sonnet-4-6`; `4.6` is not a valid id (`model_aliases.rs:44`) | → `sonnet 4-6` everywhere |
| B2 | Tools panel `12` · `+7 more` | 13 default builtins (Read, Write, Edit, Bash, Grep, Glob, Git, RepoMap, Skill, Spawn, EnterPlanMode, ExitPlanMode, ToolSearch) | → `13`; `+8 more`; surface `Write` in the visible list |
| B3 | Mode badges `Ready` / `Plan` / `Plan only` | Real `SessionMode` = `Default` / `AutoEdit` / `Yolo` (`commands.rs:79`). "Ready" is a connection state; "Plan" is not a SessionMode | → badge shows `Default`/`Auto-edit`/`Yolo`; plan mode is a separate banner |
| B4 | `⇧Tab` cycles "approval mode" | Capability exists via `SetMode` command; the cycle is net-new sugar | → cycle is `Default → Auto-edit → Yolo` |

## C. WRONG — capability. Contradicts the engine.

| # | Mockup depicts | Reality | Fix |
|---|---|---|---|
| C1 | Onboarding: key prefix auto-detect (`sk-ant-`→Anthropic) | No prefix detection anywhere. Provider is explicit (`--provider`/config) | Drop, or mark explicitly as a net-new feature; honest flow asks which provider |
| C2 | Onboarding: live-validate key vs `/v1/models` in ~2s | Engine never calls `/v1/models` for validation | Drop, or mark net-new |
| C3 | Onboarding: "Trust folder" step + "read-only mode" | No folder-trust concept exists. (Plan mode is read-only *tools*, not folder trust) | Drop both — no primitive |
| C4 | `/setup` slash command | REPL has only `/quit`/`/exit` | Net-new — needs a slash dispatcher |
| C5 | Plan mode entered by `Shift+Tab` | Entered by the model calling `EnterPlanMode` (or a net-new `/plan`) | Fix entry; `Shift+Tab` is for SessionMode only |
| C6 | Plan "Edit in $EDITOR" option | No `$EDITOR` plan-editing hook | Drop that option |
| C7 | `Edit` card `+12 −3` + diff block | `EditTool` returns only `"Edited {file}: replaced N occurrence(s)"` — no diff | Keep diff-as-approval, but host renders it from the edit's `old_string`/`new_string` args (or a net-new structured edit event) |
| C8 | Plan treats `compat.rs` as a NEW file | `crates/wcore-config/src/compat.rs` already exists (28 KB) | Reframe that plan step as a refactor, not new-file |
| C9 | Agent Manager: progress-% bars | No completion signal in `Spawn` (`SubAgentResult`) or `wcore-swarm` (terminal states only) | Drop % bars — show turns/tokens instead (those are real) |
| C10 | Agent Manager: "blocked — waiting on another agent" | `Spawn` explicitly forbids coordination; `wcore-swarm` is all-parallel, no dep graph, no `Blocked` state | Drop the dependency/blocked concept entirely |
| C11 | Agent Manager: "+ spawn agent" button | `Spawn` is LLM-invoked; `wcore-swarm` is unwired from the CLI | Drop the user-spawn button |
| C12 | Agent Manager: "auto-merge when agents settle" | `Spawn` concatenates result strings; `wcore-swarm` *deletes* worktrees, no merge | Reframe: results concatenated/returned |
| C13 | Command palette over "31 commands, frecency-ordered" | REPL has 2 commands; no registry/fuzzy/frecency | Net-new — show the real ~10 grounded commands + skills |
| C14 | `repomap indexed 777 files` | Repo has 929 tracked / 640 `.rs`; number is fabricated | Use a real count or "indexed N files" |

## D. CUT — pure invention, no backing primitive.

- **Ghost text seeded from git history** — nothing like it exists; `repl_loop` is plain `read_line`.
- **`@` file-reference / `!` shell composer prefixes** — don't exist as REPL prefixes.
- **`/review`** — no engine command; would be a canned prompt.
- **Conversation-history `/rewind`** — only *file* rollback exists (`RollbackTool` + `FileHistory`, 10-snapshot cap). History restore is unbacked.

## E. Net-new but GROUNDED — keep, label as redesign built on a real primitive

| Surface | Real primitive it builds on |
|---|---|
| The TUI itself (panels, ratatui) | Net-new — no `ratatui`/`crossterm` in the repo |
| Onboarding wizard | `init_config()` + `OAuthManager::login()` (Anthropic OAuth device flow, **real**) + `--doctor` Ollama probe |
| Command palette | New slash dispatcher + `wcore-skills` (already has `user-invocable` frontmatter + a slash parser) |
| `/resume` | Real `--resume` flag + `SessionManager` |
| `/repomap` | Real `wcore-repomap::RepoMap::build` |
| `/model` mid-session | `SetConfig{model}` protocol command |
| `/rewind` (files only) | `RollbackTool` + `FileHistory` |
| Path-map rail panel | `RepoMap` (static index) + `FileWriteNotifier` |
| Agent live feed | `ChannelSink` + `emit_sub_agent_event` — **real**, on the `Spawn` relay path |

## Agent Manager — the honest options

The Agent Manager was grounded (wrongly) on the `Spawn` tool. Two real substrates exist, both limited:

- **`Spawn` tool** — fire-and-collect parallel sub-agents, max 5, **LLM-invoked**. Real: per-agent roles (AgentRegistry YAML), a **live action feed** (`ChannelSink`), turn + token counts. Not real: progress %, blocked/dependencies, user-spawn, merge.
- **`wcore-swarm`** — worktree-isolated process workers, but: identical briefs (no roles), only terminal states (no running/blocked), heartbeat = timestamp + free-text step only, no merge, and **completely unwired from the CLI**.

Neither supports the drawn mission-control. The honest near-term Agent Manager is a **live monitor of an in-flight `Spawn` call** — ≤5 named sub-agents, live feeds, turns/tokens, running/done. The ambitious version (persistent orchestrator, lifecycle, dependencies, merge) is a real roadmap item requiring `wcore-swarm` to be wired into the CLI and substantially extended.
