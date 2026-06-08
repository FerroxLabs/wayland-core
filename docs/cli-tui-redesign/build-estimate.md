# wayland-core TUI Redesign — Feasibility & Build Estimate

**Date:** 2026-05-22 · **Scope:** `mockups/wayland-tui.html` (7 surfaces) on `wayland/engine @ feat/v0.6.1-hardening`
**Question answered:** *Can we build this, how long until it's functional, and can it be best-in-class?*

**Bottom line up front:** Yes, buildable with no architectural blockers. **Functional v1 in 4–6 weeks, feature-complete in 9–13 weeks, best-in-class in 15–21 weeks** — one focused senior Rust engineer. Two engineers compress the calendar to ~10–14 weeks but not below, because the polish tail and protocol work serialize. The redesign is **~80% presentation layer on a finished backend**; the risk is entirely in the new TUI, not the engine.

---

## 1. ratatui Feasibility — Widget by Widget

ratatui + crossterm render **every surface in the mockup**. Nothing here is at or beyond the framework's edge — lazygit/gitui/atuin/helix/television all do strictly harder things in the same toolkit (helix is a full modal editor; television does live fuzzy-find over millions of paths; atuin ships exactly this aesthetic).

| Surface element | ratatui technique | Comparator proof |
|---|---|---|
| Panels / blocks (3-pane workspace) | `Block` with `Borders` + `Layout` constraint splits (`Length`/`Min`/`Percentage`), nested | lazygit's 5-panel layout |
| Syntax-colored diff view | `Paragraph` of pre-styled `Line`/`Span`; syntax via `syntect` → `ratatui::Style`; +/− gutter as colored spans | gitui diff panel (syntect-backed) |
| Sub-agent monitor cards | `List` of bordered mini-`Block`s, or a constraint-split grid of `Block`s; per-card `Gauge`/sparkline | gitui's stashes/branches lists |
| Command-palette overlay | Centered floating `Rect` + `Clear` widget + `List` + a top `Paragraph` input line | television *is* this; atuin search |
| Config / settings surface | `List` of rows; each row a label + a value widget (toggle = styled span, select = inline cycler, text = inline `Input`). No native form widget — compose from `List` + a focused-field state machine | gitui's options menu; helix `:config` |
| Status bar + context meter | Bottom `Layout` strip; meter = `Gauge` or `LineGauge`; segmented spans for cwd/branch/model/mode | every comparator |
| Path-map tree | `List` with manual indent + expand/collapse state, or `tui-tree-widget` (mature crate) | television file panel |
| Braille spinner | Manual frame cycle on a tick (`⠋⠙⠹⠸…`) rendered as a `Span`; or `throbber-widgets-tui` | universal pattern |
| Tabs (surface switcher) | `Tabs` widget, built-in | helix bufferline |
| Onboarding card | Centered `Clear`+`Block`; stepper = styled spans; key entry = masked `Input` | — |

**Genuine blockers: none.** Three things to *budget*, not fear:

1. **No native form/text-input widget.** ratatui is immediate-mode; you own editor state. Use `tui-input` (mature) for single-line fields; build a small focus state machine for the config surface. This is real work, sized in §3 — not a blocker.
2. **Diff rendering cost.** syntect highlighting must be cached per file version, not recomputed per frame. Standard; gitui solved it.
3. **Resize / reflow.** Immediate-mode redraw makes this free *if* layout is constraint-driven from the start. A discipline cost, not a capability gap.

ratatui is the correct, proven choice. No reason to evaluate alternatives.

---

## 2. What the Engine ALREADY Provides — Do Not Re-Estimate

The backend is substantial and finished. Measured LOC (`.rs`, excl. tests dirs vary): `wcore-agent` **22.2k**, `wcore-skills` **14.2k**, `wcore-memory` **9.4k**, `wcore-config` **6.2k**, `wcore-tools` **5.8k**, `wcore-providers** **5.3k**, `wcore-mcp` **2.1k**, `wcore-protocol` **1.6k**, `wcore-repomap` **1.3k**, `wcore-swarm` **1.3k**. **~70k LOC of working backend** the TUI consumes for free:

- **Agent runtime + turn loop** — `AgentEngine::run()`, streaming, thinking, multi-turn. The TUI calls this; it does not build it.
- **Tool execution** — 13 builtins (Read/Write/Edit/Bash/Grep/Glob/Git/RepoMap/Skill/Spawn/EnterPlanMode/ExitPlanMode/ToolSearch), approval gating, panic recovery, streaming chunks.
- **Providers** — Anthropic/OpenAI/Bedrock/Vertex behind `LlmProvider`, `ProviderCompat`, circuit-breaker fallback. Mid-session `/model` is already a protocol command.
- **Session management** — `SessionManager`, 6-hex IDs, `--resume`. `/resume` is wiring, not new code.
- **The json-stream protocol — a near-complete UI contract.** `wcore-protocol` already defines **30 event variants** (`StreamStart`, `TextDelta`, `Thinking`, `ToolRequest`, `ToolRunning`, `ToolResult`, `ToolChunk`, `ApprovalRequired`, `SubAgentEvent`, `TraceEvent`, `SessionCost`, `ProviderCircuitEvent`, `BudgetExceeded`, …) and the command side (`Message`, `Stop`, `ToolApprove`, `ToolDeny`, `SetMode`, `SetConfig`, `AddMcpServer`, `ApprovalResume`). It is a 1271-line documented spec. **This is ~60% of the UI's data contract, already designed, versioned, and tested.**
- **MCP** (`wcore-mcp`), **skills** (`wcore-skills`, with `user-invocable` frontmatter + a slash parser already present), **memory** (`wcore-memory`), **repomap** (`RepoMap::build`).
- **Plugin marketplace** — `wcore-cli/src/plugin/` (resolver/registry/installer, ~520 LOC) exists; needs a UI, not a backend.
- **Approval manager** + **compaction** (`CompactionLevel` Off/Safe/Full) — both done.
- **Sub-agent live feed** — `ChannelSink` + `emit_sub_agent_event` + `SubAgentEvent` are **real and wired on the Spawn relay path**. The data exists; only the *rendering* is missing.

**Net:** the engine carries the agent loop, all I/O, all provider quirks, the approval state machine, and most of the wire format. The redesign is a presentation layer. The one engine-side code change required is trivial: rename `SessionMode::Yolo → Force` (one enum, ~6 call sites, ~0.25 day).

---

## 3. Net-New Work — Sized (S ≤ 2d · M 3–6d · L 7–14d)

Engineer-days = focused implementation incl. tests, excl. review latency.

| # | Component | Size | Days | Notes |
|---|---|---|---|---|
| 1 | **TUI shell**: render loop, event loop, crossterm raw-mode/alt-screen, tick, resize, panic-restore | **L** | 8–11 | Foundation. Get it right once. |
| 2 | App state model + surface router (`Tabs`, focus stack, modal layering) | **M** | 4–6 | The architecture spine. |
| 3 | Protocol client: decode 30 events → view state, encode commands | **M** | 4–6 | If client-of-protocol (§4). Lower if in-process. |
| 4 | **Workspace surface** (3-pane: convo + tools + path-map) | **L** | 7–10 | The primary screen; most-used. |
| 5 | **Agent-live surface** (streaming text, thinking, tool cards, status) | **M** | 5–7 | Shares widgets with #4. |
| 6 | **Sub-agents surface** (monitor cards, live feed) | **M** | 4–6 | Data exists (`ChannelSink`); honest scope = live monitor of one in-flight `Spawn`, not orchestrator. |
| 7 | **Command-palette overlay** (fuzzy, frecency-ordered) | **M** | 4–6 | `nucleo` for fuzzy. |
| 8 | **Plan-review surface** (read-only plan + approve/reject) | **S** | 2–3 | `EnterPlanMode`/`ExitPlanMode` exist. |
| 9 | **Onboarding wizard** (API-key only, no OAuth) | **M** | 3–5 | Simplified by no-OAuth: provider pick → masked key entry → write config. Drop prefix auto-detect & `/v1/models` validation (audit C1/C2). |
| 10 | **Config / settings surface** (forms, toggles, selects) | **L** | 7–10 | No native form widget; build the focus state machine + field widgets. The 7th surface. |
| 11 | **Slash-command dispatcher** (registry, parse, "did you mean", help) | **M** | 4–6 | ~10 grounded commands + skills. |
| 12 | **@-reference expansion** (`@file` → inline content, completion popup) | **M** | 3–5 | `Message.files` field already exists in the protocol. |
| 13 | **Plugin-management UI** (browse/install/remove on the marketplace) | **M** | 3–5 | Backend done; UI only. |
| 14 | **Structured edit/diff protocol event** — diff-as-approval | **M** | 3–5 | New `ToolEditPreview { old, new, path, hunks }` event so the host renders a real diff instead of inferring from args (audit C7). Small additive variant + engine emit site. |
| 15 | **Path-map panel** (tree, expand/collapse, file-write highlight) | **M** | 3–5 | `tui-tree-widget` + `RepoMap` + `FileWriteNotifier`. |
| 16 | **Frecency tracking** (palette + file ordering, persisted) | **S** | 2–3 | Small SQLite/JSON store. |
| 17 | **Theming** (Hearth Palette, `Theme` struct, all widgets themed) | **M** | 3–5 | Cheap if every widget reads `Theme` from day one; expensive as a retrofit. Do it early. |
| 18 | **Keybinding layer** (declarative map, context-aware, help overlay) | **M** | 3–5 | helix-style. |
| 19 | Engine: `SessionMode::Yolo → Force` rename | **S** | 0.25 | Trivial. |
| 20 | Glue: streaming integration, cancellation, error surfaces, end-to-end wiring | **L** | 8–12 | Always under-estimated; the "make it actually work together" tax. |

**Subtotal: ~84–124 engineer-days of net-new code.** Plus a **polish tail** (§5) of **~25–40 days** that buys best-in-class. **Grand total ≈ 110–165 engineer-days.**

---

## 4. Architecture Call — TUI in-process, talking to the **protocol contract** as its types, not its transport

**Recommendation: build the TUI inside `wcore-cli` as a new module, calling `AgentEngine` in-process — but model all UI state on the `wcore-protocol` event/command types.** Do **not** spawn a child process and pipe JSON Lines.

Reasoning:

- **The protocol is the right *contract*, not the right *transport* for a colocated TUI.** Piping JSONL through a subprocess adds serialization cost, a process boundary, IPC failure modes, and a backpressure problem — for two halves of the *same binary*. AionUI (Electron, separate process) genuinely needs the wire format; an in-process TUI does not.
- **But reuse the protocol's *types* as the view model.** The 30 event variants are a battle-tested, exhaustive description of everything the engine can tell a UI. Have `AgentEngine` emit `ProtocolEvent` values into an in-process channel (`tokio::mpsc`) the TUI consumes. You get the proven contract, the existing `ProtocolSink` plumbing, and zero IPC. `ChannelSink` already proves this exact pattern internally.
- **Risk/estimate impact:** in-process saves the **~4–6 days** of item #3's transport layer, removes a whole class of integration bugs, and keeps cancellation/streaming synchronous and debuggable. The subprocess design would *add* risk for no benefit here.
- **Bonus:** because the TUI consumes `ProtocolEvent`, the JSONL `--json-stream` mode and the TUI stay contract-compatible by construction — AionUI and the TUI are two renderers of one event stream. That's the best of both: shared contract, no shared process.

**Verdict:** TUI module in `wcore-cli`; engine emits `ProtocolEvent` over an in-process channel; protocol crate is the shared vocabulary. This is lower-risk *and* cheaper than the subprocess alternative.

---

## 5. Phased Build Plan & Timeline

**Assumption: one focused senior Rust engineer, ratatui-competent.** Calendar weeks ≈ engineer-days ÷ 4 (meetings, review, context-switching, the real world). Ranges are honest, not padded.

### Phase 1 — Functional v1 (genuinely usable) · **4–6 weeks**
Items 1, 2, 3(channel), 4, 5, 8, 11(core), 17(scaffold), 18(core), 19, plus core of 20.
Deliverable: launch TUI → onboard with an existing config → workspace surface → send a message → watch streaming + tool cards → approve/deny tools → plan review works. **One engineer can dogfood this as their daily driver.** It will not be pretty yet. ~40–55 days of the estimate.

### Phase 2 — Feature-complete (all 7 surfaces, full command surface) · **5–7 weeks**
Items 6, 7, 9, 10, 12, 13, 14, 15, 16, full 11, rest of 20.
Deliverable: onboarding wizard, sub-agent monitor, command palette with frecency, config surface, @-references, plugin UI, diff-as-approval, path-map. Every surface in the mockup exists and works. ~44–65 days. **Cumulative: 9–13 weeks.**

### Phase 3 — Best-in-class polish · **6–8 weeks**
The 20% that separates "works" from lazygit/atuin/helix-tier. Honestly itemized because this is where teams quit early:
- Theme refinement, every state styled, dark/light, color-blind safety.
- Animation/transition feel — spinner cadence, surface transitions, no jank on resize.
- Empty states, error states, loading states for *every* surface (the unglamorous half).
- Keybinding discoverability, contextual help, "did you mean", fuzzy-match quality tuning.
- Latency: 60fps redraw budget, diff-cache, large-file/large-output handling.
- Accessibility: screen-reader-tolerable output, no-color fallback, narrow-terminal reflow.
- Cross-platform terminal quirks (Windows Terminal, iTerm, tmux, kitty) — CI can't fully cover this; manual.
- Real-world hardening: kill a long Bash, resize mid-stream, paste 10k chars, network drop.

~25–40 days. **This tail is not optional for "best-in-class" and it does not parallelize well** — it's one person with taste, iterating. **Cumulative: 15–21 weeks.**

**Two engineers:** Phase 1 ≈ 3–4 wk, Phase 2 ≈ 3–5 wk (surfaces parallelize cleanly once the shell exists), Phase 3 ≈ 4–6 wk (polish serializes on one taste-owner). **Total ≈ 10–15 weeks.** The shell (items 1–3) is a hard serial dependency — a second engineer is near-idle until it lands, so don't staff two before week 2.

**Honest calendar:** functional daily-driver in **~1.5 months**, feature-complete in **~3 months**, best-in-class in **~4.5–5 months** solo.

---

## 6. Best-in-Class Verdict

**Yes — wayland-core's CLI can become one of the best agent CLIs, and the conditions for it are unusually favorable.**

Why it's achievable: the hard part of an agent CLI is the agent — multi-provider, streaming, tools, approvals, sub-agents, MCP, memory, compaction. **That is done and tested.** Most agent CLIs ship a great backend behind a mediocre line-printer UI. wayland-core would be doing the *opposite* of the usual failure: a finished engine getting a deliberately designed TUI, with a real mockup, a fidelity audit already done, and a 30-variant protocol that pre-specifies the data contract. Best-in-class agent TUIs are rare precisely because few teams have both halves; wayland-core has the expensive half already.

**Critical path:** TUI shell (items 1–3) → workspace + agent-live surfaces (4, 5) → the polish tail (Phase 3). Everything else is parallelizable or additive once the shell exists. The shell quality determines the ceiling of every surface above it — over-invest there.

**Single biggest risk: the Phase 3 polish tail gets cut.** Phases 1–2 are well-specified, low-uncertainty, mockup-backed — they will land close to estimate. The danger is declaring victory at "feature-complete" (week 13). Feature-complete is *not* best-in-class; lazygit, atuin, and helix are loved because of the last 20% — feel, latency, empty states, keybinding discoverability, terminal-quirk hardening. That work is unglamorous, hard to demo, and the first thing cut under deadline pressure. **Protect Phase 3 explicitly, or you ship a good agent CLI instead of a best-in-class one.**

Secondary risk: scope creep on the Agent Manager. The mockup's mission-control (progress bars, dependencies, user-spawn, auto-merge) is **not backed by the engine** (audit C9–C12). Build the honest version — a live monitor of one in-flight `Spawn` (≤5 named agents, real turns/tokens, live feed). The orchestrator is a separate roadmap item requiring `wcore-swarm` to be wired into the CLI and substantially extended; do not let it bleed into this estimate.

**Recommendation: green-light it.** Single focused engineer, in-process architecture, ratatui. Functional in 4–6 weeks, best-in-class in 15–21 weeks — and budget the polish tail as a named, defended phase, not a hopeful afterthought.
