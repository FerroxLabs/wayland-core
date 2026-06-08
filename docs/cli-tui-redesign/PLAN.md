# wayland-core CLI/TUI Redesign — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development to implement each task. Steps use checkbox (`- [ ]`) syntax for tracking. This plan is structured for **parallel multi-agent execution** — see "Multi-Agent Execution Protocol" below.

**Goal:** Replace wayland-core's bare readline REPL with a full ratatui TUI — 7 surfaces, slash + `@` command systems, plugin and config management — built in-process on the existing engine, shipped as one PR.

**Architecture:** A new `tui/` module inside the `wcore-cli` crate, rendering with ratatui + crossterm. `AgentEngine` emits `ProtocolEvent`s over an in-process `tokio::mpsc` channel; the TUI is a pure consumer of that stream — the same event contract as the existing `--json-stream` host protocol, with no subprocess boundary. Work is split into 4 dependency-ordered waves; Wave 0 freezes all shared interfaces so Wave 1 surfaces build in parallel without collision.

**Tech Stack:** Rust 2021 · ratatui · crossterm · tui-input (text fields) · nucleo (fuzzy match) · syntect (diff syntax highlight) · tui-tree-widget (path map) · the existing `wcore-*` crates.

**Design inputs (read before executing) — all committed in `docs/cli-tui-redesign/`:**
- Visual spec: `docs/cli-tui-redesign/mockup.html` — the 7 surfaces, authoritative for layout and the Hearth Palette.
- Functional audit: `docs/cli-tui-redesign/AUDIT.md` (+ `audit/` slice files) — what the engine really provides vs net-new.
- UX design: `docs/cli-tui-redesign/ux-krug-sutherland.md` — the config surface, command surfaces, and 19 Krug/Sutherland fixes.
- Build analysis: `docs/cli-tui-redesign/build-estimate.md` — ratatui feasibility, architecture rationale.

---

## Branch & PR Strategy

- **Design branch (current):** `design/cli-redesign` off `origin/main` — holds all design artifacts + this plan. Committed as the PR's first commits.
- **Implementation branch:** `feat/cli-tui`, branched from `design/cli-redesign`. All waves land here.
- **Per-task branches:** each parallel agent works in its own git worktree on `feat/cli-tui-<taskid>` (e.g. `feat/cli-tui-t1.1`), branched from `feat/cli-tui` at the start of its wave. Merged back into `feat/cli-tui` at wave close.
- **Final PR:** `feat/cli-tui` → `main`, once Wave 3 is verified. Fully isolated from `feat/v0.6.1-hardening` throughout.
- The TUI ships behind no flag — it becomes the default for `wayland-core` on a TTY with no prompt. `--json-stream` and `-p` headless modes are untouched, so merge risk is contained to the new `tui/` module + a small `main.rs` dispatch branch.

---

## Multi-Agent Execution Protocol

The waves are a dependency graph, not a calendar:

| Wave | Tasks | Parallelism | Gate to next wave |
|---|---|---|---|
| **0 — Foundation** | T0.1–T0.6 | 1–2 agents, tightly coordinated | All shared contracts compile + are frozen |
| **1 — Surfaces** | T1.1–T1.10 | **~10 agents fully parallel** | Each surface compiles + passes its acceptance test against a stub event stream |
| **2 — Integration** | T2.1–T2.3 | 1–2 agents, mostly serial | A real end-to-end conversation works in the TUI |
| **3 — Polish** | T3.1–T3.3 | 3 agents parallel, then 1 serial taste pass | Best-in-class checklist passes |

**Rules for parallel agents:**
1. **Wave 0 freezes the contracts** (the `Surface` trait, `Theme`, `App`, the widget API, the protocol bridge). Wave 1 agents build *against* these and **must not change them**. If a Wave 1 agent believes a contract is wrong, it stops and flags it — the change is made once, centrally, and re-propagated.
2. **One agent owns one file set.** The File Structure below partitions ownership. No two Wave 1 agents write the same file. Shared files (`surfaces/mod.rs` router, `app.rs`) are touched only by the integrator in Wave 2.
3. **Every task is TDD'd** via `subagent-driven-development` — write the failing test, implement, verify, commit. Atomic commits per logical step.
4. **The mockup is the visual spec.** When a surface task needs a layout decision, the answer is in `mockup.html` — match it.
5. Wave 1 agents test against a **stub `ProtocolEvent` stream** (T0.5 ships canned fixtures), so no surface depends on the live engine until Wave 2.

---

## File Structure

```
crates/wcore-cli/src/
  main.rs                 -- MODIFY: dispatch to tui::run() on TTY + no prompt
  tui/
    mod.rs                -- TUI entry: run(), crossterm lifecycle, panic restore   [T0.2]
    app.rs                -- App state model (the central view state)               [T0.3]
    event.rs              -- input event loop, key routing                          [T0.2]
    theme.rs              -- Hearth Palette tokens, Theme struct                     [T0.4]
    protocol_bridge.rs    -- in-process ProtocolEvent channel -> App                 [T0.5]
    keybind.rs            -- declarative context-aware keymap + ? help               [T0.6]
    frecency.rs           -- persisted frecency store                               [T0.6]
    widgets/
      mod.rs              -- widget API surface                                     [T0.4]
      statusbar.rs        -- status bar + context meter                             [T0.4]
      panel.rs            -- bordered panel / rail helpers                          [T0.4]
      spinner.rs          -- braille spinner                                        [T0.4]
      toolcard.rs         -- tool-call card                                         [T0.4]
      diff.rs             -- syntax-highlighted diff view                           [T0.4]
      tree.rs             -- path-map tree                                          [T0.4]
    surfaces/
      mod.rs              -- Surface trait + SurfaceId + router                     [T0.3]
      onboarding.rs       -- surface 01                                             [T1.2]
      workspace.rs        -- surface 02 + 03 (idle + agent-live)                    [T1.1]
      subagents.rs        -- surface 04                                            [T1.3]
      palette.rs          -- surface 05 (command palette overlay)                  [T1.4]
      plan_review.rs      -- surface 06                                            [T1.5]
      config.rs           -- surface 07 (settings)                                 [T1.6]
      plugins.rs          -- /plugins surface                                      [T1.7]
      diagnostics.rs      -- /doctor, /cost, /memory surfaces                       [T1.10]
    commands/
      mod.rs              -- slash dispatcher + registry + "did you mean"           [T1.8]
      at_refs.rs          -- @-reference expansion + completion                     [T1.9]

crates/wcore-protocol/src/commands.rs   -- MODIFY: SessionMode::Yolo -> Force        [T0.1]
crates/wcore-protocol/src/events.rs     -- MODIFY: ensure edit-preview content       [T0.5]
```

---

## Shared Contracts (frozen by Wave 0 — Wave 1 builds against these)

These signatures are the integration boundary. Wave 0 delivers them; Wave 1 agents import and conform.

```rust
// surfaces/mod.rs
pub enum SurfaceId { Onboarding, Workspace, SubAgents, Palette, PlanReview, Config, Plugins, Diagnostics }

pub enum SurfaceAction {
    None,
    Switch(SurfaceId),
    OpenOverlay(SurfaceId),
    CloseOverlay,
    SendMessage(String),
    Command(String),          // a slash command line
    Approve { call_id: String, scope: ApprovalScope },
    Deny { call_id: String, reason: String },
    SetMode(SessionMode),
    Quit,
}

pub trait Surface {
    fn id(&self) -> SurfaceId;
    fn render(&mut self, frame: &mut Frame, area: Rect, app: &App, theme: &Theme);
    fn handle_key(&mut self, key: KeyEvent, app: &mut App) -> SurfaceAction;
    fn on_enter(&mut self, _app: &mut App) {}
}

// app.rs — central view state, mutated only by protocol_bridge + the router
pub struct App {
    pub surface: SurfaceId,
    pub overlay: Option<SurfaceId>,
    pub session: SessionView,     // turns, streaming buffer, tool calls, sub-agents
    pub config: ConfigView,       // snapshot of resolved Config
    pub mode: SessionMode,        // Default | AutoEdit | Force
    pub context: ContextView,     // used tokens, window size, pct
    pub quit: bool,
}

// theme.rs — Hearth Palette (exact tokens from the brand spec §09)
pub struct Theme { /* orange, surface, border, text*, success, warning, error, ... as ratatui::Color */ }
impl Theme { pub fn hearth() -> Self; }

// widgets/mod.rs — every widget is a free fn taking (frame, area, &model, &Theme)
pub fn status_bar(f: &mut Frame, area: Rect, app: &App, t: &Theme);
pub fn panel<'a>(title: &str, t: &Theme) -> Block<'a>;
pub fn spinner_frame(tick: u64) -> &'static str;
pub fn tool_card(f: &mut Frame, area: Rect, card: &ToolCardModel, t: &Theme);
pub fn diff_view(f: &mut Frame, area: Rect, diff: &DiffModel, t: &Theme);
pub fn path_tree(f: &mut Frame, area: Rect, tree: &TreeModel, t: &Theme);

// protocol_bridge.rs
pub fn spawn_bridge(engine_rx: mpsc::Receiver<ProtocolEvent>, app: Arc<Mutex<App>>);
```

---

## WAVE 0 — Foundation & Contracts

*Serial. 1–2 agents. Must compile and freeze all interfaces above before Wave 1 starts.*

### Task 0.1: Rename SessionMode::Yolo → Force

**Files:** Modify `crates/wcore-protocol/src/commands.rs` (the `SessionMode` enum + `current_mode()` string mapping in `src/lib.rs`); update all call sites (grep `Yolo` / `yolo` workspace-wide, ~6 sites).

- [ ] Write a test asserting `SessionMode::Force` serializes to `"force"` and round-trips.
- [ ] Rename the variant; update `current_mode()` to emit `"force"`; fix all call sites until `cargo build` is clean.
- [ ] Run `cargo test -p wcore-protocol`; `cargo build --workspace`. Commit.

**Acceptance:** workspace compiles; no `Yolo`/`yolo` identifiers remain; protocol round-trip test passes.

### Task 0.2: TUI shell — lifecycle, render loop, event loop

**Files:** Create `tui/mod.rs`, `tui/event.rs`. Modify `main.rs` (add the dispatch branch — guarded so it is inert until T2.3 wires it).

- [ ] crossterm: enter raw mode + alt-screen on start; restore on exit AND on panic (install a panic hook that restores the terminal before printing).
- [ ] ratatui `Terminal` init; a 30fps tick; the draw/poll loop; clean `Quit` exit.
- [ ] `tui::run()` renders an empty themed frame and exits on `q` / `Ctrl+C`.

**Acceptance:** `tui::run()` opens a full-screen frame, survives a forced panic with the terminal restored (no corrupted shell), exits cleanly.

### Task 0.3: App state model + Surface trait + router

**Files:** Create `tui/app.rs`, `tui/surfaces/mod.rs`.

- [ ] Define `App`, `SurfaceId`, `SurfaceAction`, `Surface` trait, and the supporting view structs (`SessionView`, `ConfigView`, `ContextView`, `ToolCardModel`, `DiffModel`, `TreeModel`) exactly as in Shared Contracts.
- [ ] Router: holds the active + overlay surface, dispatches `render`/`handle_key`, applies `SurfaceAction`. `Tabs` chrome for the 7 surfaces.
- [ ] A `StubSurface` proves the trait + router with a placeholder render.

**Acceptance:** router switches between stub surfaces via tab keys; `SurfaceAction::Switch/Quit` work; all contract types are public and documented.

### Task 0.4: Theme + shared widget library

**Files:** Create `tui/theme.rs`, `tui/widgets/{mod,statusbar,panel,spinner,toolcard,diff,tree}.rs`.

- [ ] `Theme::hearth()` — the exact Hearth Palette tokens (orange `#ff6b35`, surface `#141414`, border `#333`, etc.) as `ratatui::Color`. Respect `NO_COLOR` (a `Theme::no_color()`).
- [ ] Build each widget as a free function per the contract. `diff.rs` uses `syntect` with a per-file-version highlight cache. `spinner.rs` cycles the braille frames. Flat color only — no shadow/gradient emulation (brand §07).
- [ ] Each widget gets a render-snapshot test (ratatui `TestBackend`).

**Acceptance:** every widget renders correctly in `TestBackend` snapshots; the diff view syntax-highlights Rust; `NO_COLOR` produces an uncolored theme.

### Task 0.5: Protocol bridge + edit-preview event

**Files:** Create `tui/protocol_bridge.rs`. Modify `crates/wcore-protocol/src/events.rs`.

- [ ] Verify the tool-request/tool-result events carry enough for the host to render an Edit/Write diff (the `old_string`/`new_string` args). If not, add a minimal `ToolEditPreview { call_id, path, old, new }` event variant — additive, non-breaking.
- [ ] `spawn_bridge`: a task draining an `mpsc::Receiver<ProtocolEvent>`, decoding all ~30 variants into `App` mutations behind the `Arc<Mutex<App>>`.
- [ ] Ship a `fixtures/` set of canned `ProtocolEvent` sequences (a full conversation, a tool call + approval, a sub-agent spawn) — Wave 1 surfaces test against these.

**Acceptance:** feeding a fixture stream mutates `App` to the expected state; the edit-preview path yields a renderable `DiffModel`; `cargo test -p wcore-protocol` passes.

### Task 0.6: Keybinding layer + frecency store

**Files:** Create `tui/keybind.rs`, `tui/frecency.rs`.

- [ ] `keybind.rs`: a declarative context-aware keymap (global vs per-surface), plus a `?` help overlay that renders the active context's bindings.
- [ ] `frecency.rs`: a persisted recency+frequency store (commands, files); `record(key)` / `rank(items)`. Persists under the config dir.
- [ ] Tests: keymap resolves context-correctly; frecency ranks a frequently+recently used item above a stale one.

**Acceptance:** `?` overlay lists correct bindings per surface; frecency ranking test passes; store survives a reload.

**WAVE 0 GATE:** `cargo build --workspace` clean; the TUI launches to an empty themed shell with tab chrome; all Shared Contract types are frozen and published.

---

## WAVE 1 — Surfaces & Command Systems

*Fully parallel — one agent per task, each in its own worktree. Build against Wave 0 contracts + the T0.5 fixture streams. The mockup `mockup.html` is the visual spec for every surface.*

### Task 1.1: Workspace surface (idle + agent-live)

**Files:** Create `tui/surfaces/workspace.rs`. **Depends on:** T0.3, T0.4, T0.5.

- [ ] Implement `Surface` for the 3-pane workspace: transcript (user/assistant turns, streaming text, thinking, tool cards via `tool_card`, the diff-as-approval card via `diff_view`); the right rail (path-map via `path_tree`, tools panel, activity feed); the composer.
- [ ] Idle state (empty transcript + ghost-free composer) and agent-live state (streaming + the 3-option approval) both per the mockup.
- [ ] Test against the conversation + tool-call fixtures from T0.5.

**Acceptance:** renders idle + mid-stream + approval states from fixtures, snapshot-matching the mockup layout; the approval emits `SurfaceAction::Approve/Deny`.

### Task 1.2: Onboarding surface (API-key only)

**Files:** Create `tui/surfaces/onboarding.rs`. **Depends on:** T0.3, T0.4.

- [ ] The Connect → Configure → Ready flow. **API-key auth only** — no OAuth. Options: paste API key (provider picked explicitly or detected by prefix as a convenience), use Ollama (probed at `localhost:11434`), skip → read-only. Grounds on `config::init_config()` + key entry via `tui-input`.
- [ ] Apply ux-krug-sutherland findings #1–#5 (dead OAuth row removed, real placeholder not a fake key, consequence-framed skip, 2-step not 3, no "config.toml" leak).

**Acceptance:** completes a key-entry flow, writes config via `init_config()`, emits `SurfaceAction::Switch(Workspace)`; no OAuth path exists.

### Task 1.3: Sub-agents monitor surface

**Files:** Create `tui/surfaces/subagents.rs`. **Depends on:** T0.3, T0.4, T0.5.

- [ ] The honest live monitor of an in-flight `Spawn` call — ≤5 named sub-agents, running/done states, turn + token counts, the live feed (from `SubAgentEvent` fixtures). **No** progress %, blocked state, +spawn button, or auto-merge (audit C9–C12).

**Acceptance:** renders 3-running/1-done from the sub-agent fixture; the expanded card shows the live feed.

### Task 1.4: Command palette surface

**Files:** Create `tui/surfaces/palette.rs`. **Depends on:** T0.3, T0.4, T0.6, T1.8 (command registry — coordinate the registry type early).

- [ ] A centered overlay: `nucleo` fuzzy filter over the command registry, grouped by intent, frecency-ordered (T0.6). One-line consequence descriptions. Destructive commands tagged. No "frecency" word in the UI (ux finding #14).

**Acceptance:** fuzzy-filters the registry; `⏎` emits `SurfaceAction::Command`; grouping + destructive-tag render per the UX doc.

### Task 1.5: Plan review surface

**Files:** Create `tui/surfaces/plan_review.rs`. **Depends on:** T0.3, T0.4, T0.5.

- [ ] The read-only plan-mode banner + the file-scoped plan + the safety panel + the 3 options (Run / Keep planning / Discard). Entry is via `EnterPlanMode` from the engine — not a keybind.

**Acceptance:** renders a plan from a fixture; options emit the correct `SurfaceAction`; the plan-mode banner is present.

### Task 1.6: Config / Settings surface

**Files:** Create `tui/surfaces/config.rs`. **Depends on:** T0.3, T0.4.

- [ ] The 3-tier progressive-disclosure config surface from `ux-krug-sutherland.md` §Task 2: 8 Tier-1 settings visible; per-section detail on `⏎`; an expert tier (`x`) for the 24 `ProviderCompat` fields, each glossed in plain language. Settings framed by consequence. Edits write `.wayland-core.toml`; `esc` reverts.
- [ ] Uses `tui-input` for text fields + a focus state machine (no native form widget).

**Acceptance:** renders all three tiers; a toggle writes config and reloads; `esc` reverts an unsaved change.

### Task 1.7: Plugins surface

**Files:** Create `tui/surfaces/plugins.rs`. **Depends on:** T0.3, T0.4; the existing `wcore-cli/src/plugin/` marketplace backend.

- [ ] The `/plugins` panel from `ux-krug-sutherland.md` §3c: installed vs available, one verb per row, install/remove on the real `plugin::{install,list,available,remove}` backend, `i` for details, reversibility stated.

**Acceptance:** lists installed + available plugins from the real backend; install/remove round-trips.

### Task 1.8: Slash-command dispatcher + registry

**Files:** Create `tui/commands/mod.rs`. **Depends on:** T0.3.

- [ ] A `CommandRegistry` (single source of truth — name, group, description, destructive flag, handler) feeding the palette (T1.4), `/help`, and "did you mean" (Damerau-Levenshtein ≤2). The ~14 grounded commands from the UX doc §3a, grouped into 6 intents. User-invocable skills extend the registry.

**Acceptance:** parses a slash line, dispatches or returns "did you mean"; `/help` renders the grouped list; the registry is consumed by the palette.

### Task 1.9: @-reference expansion

**Files:** Create `tui/commands/at_refs.rs`. **Depends on:** T0.3, T0.6.

- [ ] The `@` system from UX doc §3b: `@file @dir @symbol @diff @url @session @output`, autocomplete popup on `@`, size-budget preview, gitignore + secret-denylist guardrails. Resolves into the `Message.files`/content payload.

**Acceptance:** `@file` resolves + previews token size; `@dir` warns on oversized trees; the secret denylist blocks `.env`.

### Task 1.10: Diagnostics surfaces — /doctor, /cost, /memory

**Files:** Create `tui/surfaces/diagnostics.rs`. **Depends on:** T0.3, T0.4; the existing `doctor/` module.

- [ ] Three compact surfaces: `/doctor` (provider/key/MCP health — the honest home for key validation, which onboarding cut), `/cost` (session token + spend), `/memory` (what long-term memory holds, with a correction path). OSC-9 notification helper (bell on approval-needed / long-task-done when unfocused).

**Acceptance:** `/doctor` renders the real `doctor` report; `/cost` shows session usage from `SessionCost` events; `/memory` lists + can delete an entry.

**WAVE 1 GATE:** every surface compiles, passes its acceptance test against fixtures, and snapshot-matches the mockup. Merge all task branches into `feat/cli-tui`.

---

## WAVE 2 — Integration & Wiring

*Mostly serial. 1–2 agents. Owns the shared files (`app.rs`, `surfaces/mod.rs`, `main.rs`).*

### Task 2.1: Live engine wiring

**Files:** Modify `tui/mod.rs`, `tui/protocol_bridge.rs`; integrate `AgentEngine`.

- [ ] Wire the real `AgentEngine` → in-process `mpsc<ProtocolEvent>` → `spawn_bridge` → `App`. Real message send, streaming, `Esc` cancellation, the approval round-trip (`ToolApprove`/`ToolDeny`).

**Acceptance:** a real prompt in the TUI streams a real response with real tool calls and a working approval.

### Task 2.2: Surface routing + command/mode integration

**Files:** Modify `tui/surfaces/mod.rs`, `tui/app.rs`, `tui/event.rs`.

- [ ] The onboarding→workspace handoff; tab/overlay transitions; `Shift+Tab` cycling `Default→Auto-edit→Force` via `SetMode`; the slash dispatcher + `@` system wired into the composer; frecency recording on command/file use.

**Acceptance:** all 8 surfaces reachable; mode cycle works; `/` and `@` function in the composer.

### Task 2.3: Default-mode dispatch + terminal fallbacks

**Files:** Modify `main.rs`.

- [ ] `wayland-core` on a TTY with no prompt → `tui::run()`. Preserve `--json-stream`, `-p`/headless, `--no-tui`. Handle `TERM=dumb` / non-TTY → fall back to the existing readline path.

**Acceptance:** `wayland-core` opens the TUI; `wayland-core -p "x"` and `--json-stream` are unchanged; a dumb terminal falls back cleanly.

**WAVE 2 GATE:** a full session — launch → onboard → converse → tool calls → approve → plan → resume — works end to end.

---

## WAVE 3 — Best-in-Class Polish

*3 agents parallel on T3.1/T3.2 split, then 1 serial taste pass.*

### Task 3.1: Per-surface state polish

- [ ] Every surface gets explicit empty / loading / error states; theme refinement; transition feel. Split per-surface across agents.

**Acceptance:** no surface shows a blank or raw-error state; every state is designed.

### Task 3.2: Performance + hardening

- [ ] 60fps redraw budget; diff-highlight cache verified; large tool-output handling; resize mid-stream; large-paste handling; terminal-quirk pass (tmux, iTerm2, Windows Terminal, kitty, Ghostty).

**Acceptance:** no dropped frames on a long session; resize never corrupts; documented terminal matrix passes.

### Task 3.3: Taste + accessibility pass

- [ ] One agent (or the user) does the final coherence pass: keybinding discoverability, fuzzy-match tuning, micro-copy, `NO_COLOR` + narrow-terminal reflow, screen-reader tolerance.

**Acceptance:** the best-in-class checklist (latency, empty states, discoverability, terminal hardening, accessibility) passes.

**WAVE 3 GATE:** open the PR `feat/cli-tui` → `main`.

---

## Self-Review

**Spec coverage:** All 7 mockup surfaces map to tasks (T1.1, T1.2, T1.3, T1.4, T1.5, T1.6, T1.7; diagnostics T1.10). Command systems: slash (T1.8), `@` (T1.9). The 19 UX findings: onboarding fixes in T1.2; the rest are surface-local and carried by "the mockup + UX doc are the spec" instruction. Engine changes: Yolo→Force (T0.1), edit-preview event (T0.5). Architecture (in-process, protocol-as-view-model) is T0.5 + T2.1. Plugin + config management: T1.7, T1.6. ✓

**Placeholder scan:** No "TBD"/"handle edge cases". Each task names exact files, dependencies, acceptance. Internal TDD micro-stepping is delegated to `subagent-driven-development` per the header — intentional, not a placeholder.

**Type consistency:** `Surface`, `SurfaceAction`, `App`, `Theme`, `SurfaceId`, the widget fns — defined once in Shared Contracts, referenced by ID thereafter. `SessionMode::Force` consistent post-T0.1.

**Scope:** One cohesive subsystem (the `wcore-cli` TUI). One plan, one PR. Wave 0 is the only serialization bottleneck; Wave 1's 10 tasks are genuinely independent.
