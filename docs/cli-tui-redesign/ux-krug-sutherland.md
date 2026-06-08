# UX Audit & Surface Design — Krug / Sutherland

Applies *Don't Make Me Think* (Krug) and psycho-logic / choice architecture
(Sutherland) to `wayland-tui.html`. Task 1 audits the 6 existing surfaces.
Tasks 2 & 3 design the missing Configuration surface and the command surface.

Constraints applied throughout: **no OAuth** — auth is API-keys only; the
third approval mode is **Force** (was Yolo). Modes are `Default / Auto-edit /
Force`.

---

## Task 1 — Krug/Sutherland audit of the 6 surfaces

| # | Surface · quoted element | Issue — why it makes the user think | Principle | Fix |
|---|---|---|---|---|
| 1 | Onboarding · `"Sign in with Anthropic — OAuth device flow"` | Dead option. OAuth is being removed; the engine never had per-provider OAuth anyway. A non-working choice is the worst kind of friction — it fails the billboard test ("what can I do here") with a lie. | Krug: don't make me think; error tolerance | Delete the row. Connect step is API-key-only — see redesign below. |
| 2 | Onboarding · `"Paste an API key, or sign in with Anthropic below."` | Label describes a mechanism that no longer exists, and the input is pre-filled with a half-masked key (`sk-ant-api03-xJ7n••••`) — implying a key is already entered when the field is empty. Hidden state. | Krug: hidden state; Sutherland: framing | Relabel: `"Paste your provider API key — we'll detect which provider."` Empty field shows a real placeholder, not a fake key. |
| 3 | Onboarding · `"Skip — run `wayland-core --init-config` later"` | Tells the user to memorize a flag for a future shell session. The consequence of skipping is unstated — what *happens* if I skip? | Krug: happy talk inversion / unframed choice; Sutherland: frame by consequence | Reframe by consequence: `"Skip — start in read-only mode (browse code, no API calls)"`. Re-entry is `/setup`, not a flag. |
| 4 | Onboarding · footer `Connect · Configure · Ready` (3 steps) | Three steps implies three screens of work before the user can do anything. "Configure" is vague — configure *what*? Onboarding-cliff anxiety. | Krug: billboard test; Sutherland: perceived effort | Collapse to 2 steps: `Connect · Ready`. Everything else has a sane default; deep config lives behind `/config` *after* first use. |
| 5 | Onboarding · `"provider detected from key prefix · saved to config.toml"` | "saved to config.toml" leaks a mechanism. The user doesn't care about TOML; they care that they won't have to do this again. | Sutherland: hide the seams | `"✓ Anthropic — you won't need to enter this again."` |
| 6 | Workspace · status bar `Default` badge, hint `"Default mode · every tool action prompts"` | "Default" names itself, not its behavior. A new user cannot tell from the word whether Default is safe or risky. The badge is the single most consequential piece of state on screen and it's the least legible. | Krug: don't make me think; Sutherland: framing > feature | Badge keeps the name; add a one-glyph safety cue: `🛡 Default` (shield), `✎ Auto-edit` (pencil), `⚡ Force` (bolt). Hint frames by consequence: `"Default — Wayland asks before it writes or runs anything."` |
| 7 | Workspace · empty state `"Type to begin · / for commands · ⇧Tab cycle mode"` | Three hints, equal weight, no "where do I start." `⇧Tab cycle mode` is meaningless before the user knows what a mode is. | Krug: billboard test ("where do I start") | One primary CTA, demoted secondaries: large `"Ask anything about this codebase →"`; below, dim `/ commands · ? help`. Drop the mode hint from the empty state — it belongs next to the badge. |
| 8 | Workspace · `Tools 13` / `+7 more · 2 MCP servers` | "13" and "+7 more" force mental math and tell the user nothing actionable. A count is not information. | Krug: satisficing — give the answer, not the raw data | Show the 6 named built-ins, then `"+7 more — press t to list"`. Make the panel a real affordance, not a tally. |
| 9 | Agent-live · approval `"Apply, and don't ask again for Edit this session"` | Good third option, but "for Edit this session" buries the scope. The user can't tell if "don't ask again" means *this file*, *all edits*, or *everything*. Scope ambiguity on a security decision is the worst place for it. | Sutherland: third option must be *legible*; Krug: don't make me think | Make scope explicit and visually distinct: `"Always allow Edit — for this session"` with the scope word in accent color. The risky breadth ("all edits") is stated, not implied. |
| 10 | Agent-live · `"No — explain what to change instead"` | The reject option is a paragraph. Under time pressure (the agent is mid-run) the user scans, sees "No", and may miss that this is the *productive* path, not a dead stop. | Krug: scannability; Sutherland: framing the decline as collaboration | Tighten: `"Reject — tell Wayland what to fix"`. The verb leads. |
| 11 | Agent-live · composer `"working — drafting edit for openai.rs next"` + `"8.3s elapsed"` | Good — this directly answers the #1 CLI complaint (silent work). But "elapsed" with no token/cost counter misses the Sutherland move: make the wait *legible* so it stops being dead time. | Sutherland: small psycho-logical change; cli-complaints "silent work" | Add a live counter: `"working · 8.3s · 12.4k tokens · $0.04"`. The visible progress converts wait into evidence of effort. |
| 12 | Sub-agents · `"results return to the main agent when all 4 settle"` | "settle" is jargon (borrowed from futures/promises). A user reading it cold doesn't know if "settle" means "succeed" or just "stop." | Krug: plain language, not jargon | `"results return to the main agent when all 4 finish"`. |
| 13 | Sub-agents · `"max 5 parallel · no shared state"` | "no shared state" is an implementation fact stated as a user-facing reassurance. It reads as a limitation, not a feature. | Sutherland: framing determines behavior | Frame as a benefit: `"each works independently — one failing won't break the others"`. |
| 14 | Command-palette · `"frecency-ordered · 4 of 14 + skills"` | "frecency" is invented jargon visible to the user. "4 of 14 + skills" is mental math. The footer is talking about itself instead of helping. | Krug: happy talk must die; jargon | Drop "frecency" from the UI (keep the algorithm). Footer: `"↑↓ move · ⏎ run · esc close"` and nothing else. Ordering is felt, not announced. |
| 15 | Command-palette · results list (`/rewind`, `/resume`, `/repomap`, `/replay`) | All four r-commands look alike — same glyph weight, descriptions in muted gray. Nothing tells the user which is destructive (`/rewind` restores files — it *discards* current work). | Krug: don't make me think; Sutherland: status-quo bias on destructive ops | Group by intent (see Task 3a). Tag destructive commands with an accent dot or `↩` glyph. Never let a file-discarding command look identical to a read-only one. |
| 16 | Plan-review · `"Approve & run — exit plan mode, execute all 4 steps"` | The default-selected option commits to executing 4 steps across 5 files. It's the *right* default for an approved plan — but the framing leads with the irreversible verb ("run") before the safety ("you reviewed it"). | Sutherland: frame the default as the safe action | Keep it default-selected (status-quo bias works *for* us here — the user did review the plan). Reframe: `"Run this plan — 4 steps, 5 files, atomic commits you can undo"`. The undo promise neutralizes the commitment anxiety. |
| 17 | Plan-review · `"Keep planning — tell Wayland what to adjust"` keybind `R` | Good middle option. But `R` is non-obvious next to `A` and `esc`. Krug: conventions. | Krug: conventions only work because they work | Keep `R` but show it as `r` consistently with the other single-key hints; the palette/approval surfaces mix `A`/`S`/`esc` casing — normalize all to lowercase. |
| 18 | All surfaces · `ctx 100%` / `78%` meter | A percentage with no anchor. 78% of *what* — and is high good or bad? A full context window is *bad* (compaction imminent); a user reading "100%" sees a full bar and feels reassured. The meter is semantically inverted. | Krug: don't make me think; cli-complaints: hidden state | Label it `context used` and color it: green→amber→red as it fills. At ≥80%: `"context 82% — Wayland will compact soon"`. Make the number mean something. |
| 19 | All surfaces · no `?` affordance, no notification cue | The mockup has per-surface hints but no universal "what can I do here." And no surface shows where a completion/approval *bell* fires. | Krug: self-explanatory beats documented; cli-complaints: OSC 9 notify | Add a persistent `? help` to every composer hint row. Spec OSC 9 notification on approval-needed and long-task-done when terminal is unfocused. |

---

## Task 2 — Configuration / Settings surface

`/config` opens a single full-screen surface. Krug: scannable, one job per
section. Sutherland: **every setting framed by consequence, not mechanism** —
the user reads what *happens*, never the TOML key. The real config has ~10
top-level blocks and ProviderCompat alone has 24 fields; the answer is
**progressive disclosure** — show 8 decisions that matter, fold the other 200.

Three depth tiers:
- **Tier 1 (always visible)** — the 8 settings a normal user touches.
- **Tier 2 (one keypress: `⏎` on a section)** — per-section detail.
- **Tier 3 (`x` — "expert")** — raw ProviderCompat / hooks / budgets, with
  a one-line plain-language gloss above each raw key.

```
┌─ Wayland · Settings ──────────────────────────── /config ──┐
│  ~/dev/wayland/engine          global + project · merged   │
├────────────────────────────────────────────────────────────┤
│                                                            │
│  CONNECTION                                                │
│   Provider      Anthropic          ▸ change   ✓ key set    │
│   Model         claude-sonnet-4-6  ▸ pick                  │
│   Profiles      default · fast · review     ▸ manage       │
│                                                            │
│  HOW WAYLAND ACTS                                          │
│   Approval      ● Default   ○ Auto-edit   ○ Force          │
│                 Asks before it writes or runs anything.    │
│   Plan first    ○ off   ● on for big changes               │
│   Stop after    25 turns          ▸  (runaway guard)       │
│                                                            │
│  MEMORY & CONTEXT                                          │
│   Compaction    ○ Off   ● Safe   ○ Full                    │
│                 Safe — folds old turns, keeps decisions.   │
│   Long-term     ○ off   ● on    remembers across sessions  │
│                                                            │
│  CAPABILITIES                                              │
│   Tools         13 built-in · Bash gated      ▸ permissions│
│   MCP servers   2 connected · 1 off           ▸ manage     │
│   Skills        6 active                      ▸ manage     │
│   Plugins       3 installed                   ▸ /plugins   │
│   Hooks         2 enabled                     ▸ manage     │
│                                                            │
│  ─────────────────────────────────────────────────────    │
│   x  expert settings (provider tuning, budgets, traces)    │
│                                                            │
├────────────────────────────────────────────────────────────┤
│  ↑↓ move   ⏎ open   space toggle   x expert   esc save&close│
│  Changes save to .wayland-core.toml · esc to undo all       │
└────────────────────────────────────────────────────────────┘
```

Design rules behind this layout:

- **Sections framed as questions the user actually has** — "How Wayland
  acts," "Memory & context" — not "DefaultConfig / CompactConfig." Krug:
  organize by user intent, not by data structure.
- **Sane defaults visible.** Every radio shows the current selection filled.
  The user sees the safe answer is already chosen — Sutherland's status-quo
  bias works for them, not against them. Satisficing: the modal user reads
  one screen, sees nothing alarming, presses `esc`.
- **Consequence, not mechanism.** `max_turns` → "Stop after 25 turns
  (runaway guard)." `compact.level` → the three radios *plus* a one-line
  gloss of what Safe does. The user never sees `[compact] level = "safe"`.
- **The 24 ProviderCompat fields are not on this screen.** They live under
  `x → Provider tuning`, and even there they're grouped: "Message format
  (6 toggles)," "Pricing (4 fields)," "Capabilities (thinking, effort)."
  Each raw toggle gets a plain gloss: `merge_assistant_messages` →
  *"Combine back-to-back AI messages — required by OpenAI."*
- **`▸` means "there's more here."** Consistent disclosure glyph. Krug
  convention: one symbol, one meaning, everywhere.
- **Save is invisible; undo is loud.** Footer states both: changes persist
  on navigate-away, `esc` reverts the whole session. Krug: error tolerance —
  cheap reversibility beats a confirm dialog.
- **Expert mode is one keypress, clearly bounded.** The `x` row sits below a
  rule, labeled with exactly what's behind it. No surprise depth.

Expert sub-surface (`x`) sketch — same disclosure discipline:

```
┌─ Settings · Expert ─────────────────────────────── x ──────┐
│  Provider tuning   Anthropic compat · 24 fields            │
│   Message format   6 toggles — alternation, merge, orphans │
│   Pricing          $15 / $75 per Mtok · ▸ override         │
│   Capabilities     thinking ✓   effort ✗                   │
│  Budgets           wall-time ∞ · tokens ∞ · cost ∞  ▸ set  │
│  Resilience        provider fallback chain      ○ off      │
│  Traces            structured traces · OTLP     ○ off      │
│  Credentials       stored: plaintext  ▸ switch to keyring  │
│  Config files      global ▸  ·  project ▸  (open in editor)│
└────────────────────────────────────────────────────────────┘
```

`Credentials → keyring` is surfaced here because it's a real
consequence-bearing choice (plaintext vs OS keychain), framed as such.

---

## Task 3 — Command surface

### 3a. Slash commands — grouped by intent

The mockup's palette is a flat alphabetical-ish list. Krug: group by intent
so the user scans one block, not 14 rows. Sutherland: order *within* a group
by frecency (recent + frequent) but **never announce "frecency"** — ordering
is felt. Top of each group is pre-highlighted.

```
┌─ /  command ──────────────────────────────────────────────┐
│  /re|                                                      │
├────────────────────────────────────────────────────────────┤
│  SESSION                                                   │
│   /resume      reopen a past session                       │
│   /rewind   ↩  restore files to an earlier snapshot        │
│   /new         start fresh, keep this provider             │
│   /compact     fold context now, keep decisions            │
│   /quit                                                    │
│                                                            │
│  MODEL & PROVIDER                                          │
│   /model       switch model                                │
│   /provider    switch provider                             │
│   /profile     load a saved profile (fast · review)        │
│                                                            │
│  CONTEXT & MEMORY                                          │
│   /repomap     rebuild the codebase symbol index           │
│   /memory      what Wayland remembers about you            │
│   /cost        tokens & spend this session                 │
│                                                            │
│  TOOLS & EXTENSIONS                                        │
│   /tools       list tools, toggle permissions              │
│   /mcp         manage MCP servers                          │
│   /skills      browse & run skills                         │
│   /plugins     install / remove plugins                    │
│   /hooks       manage hooks                                │
│                                                            │
│  WORKFLOW                                                  │
│   /plan        plan before acting (read-only)              │
│   /mode        Default · Auto-edit · Force                 │
│   /config      all settings                                │
│                                                            │
│  DIAGNOSTICS                                               │
│   /doctor      check provider, keys, MCP health            │
│   /replay      inspect a recorded session trace            │
│   /help    ?   keys & commands for this screen             │
├────────────────────────────────────────────────────────────┤
│  ↑↓ move   ⏎ run   esc close                                │
└────────────────────────────────────────────────────────────┘
```

Choice-architecture rules:
- **Six intent groups, each ≤5 items.** Krug's billboard test per group.
- **Destructive commands carry a `↩` glyph** (`/rewind` discards current
  work). A read-only command and a file-discarding command must never look
  identical — finding #15.
- **Frecency within group, silent.** First time the palette opens it's the
  order above; after use, the user's top-3 float up *inside their group*.
  The group structure is the stable mental map; frecency is the polish.
- **Descriptions are consequences, ≤6 words, present tense, lowercase.**
  No "wcore-repomap," no crate names — finding #14.
- **Skills extend the TOOLS group**, not a separate namespace. A user-run
  skill appears under `/skills` as a sub-list — discovered, not memorized.

### 3b. `@` commands — in-message context references

`@` is the inline counterpart to `/`. `/` *does* something; `@` *attaches*
something to the next message. Autocomplete fires on the `@` keystroke —
Krug: the affordance announces itself, no documentation needed.

```
›  explain the bug in @cra|

   @  attach context ──────────────────────────────────
   @crates/wcore-config/src/compat.rs    file · 28 KB
   @crates/wcore-config/                 directory
   @ProviderCompat                       symbol · compat.rs:10
   @resolve_field                        symbol · compat.rs
   ────────────────────────────────────────────────────
   ↑↓ move   ⏎ insert   esc cancel
```

The set:

| Token | Attaches | Surfaced as |
|---|---|---|
| `@file` | one file's contents | fuzzy path picker, repomap-ranked |
| `@dir/` | a directory tree (names, not full contents) | path picker |
| `@<symbol>` | a function/type definition + call sites | repomap symbol index |
| `@diff` | the working-tree diff (or `@diff main` vs a ref) | static, no picker |
| `@url` | fetched + readable-extracted web page | inline, validated on `⏎` |
| `@session` | a past session as reference context | session picker |
| `@output` | the last shell command's stdout/stderr | static |

Guardrails (Sutherland: hide the seams; cli-complaints: silent work):
- **Size preview before send.** Each chip shows its cost: `@compat.rs 28 KB
  ≈ 7k tokens`. `@dir/` warns when a tree would blow the context budget and
  offers `@dir/ (names only)`.
- **`@url` is the one with a network seam** — show a 1-line "fetching…" and
  a clear failure path; never hang silently.
- **Resolved chips are editable.** An inserted `@file` becomes a removable
  chip in the composer, not raw text — the user can undo an attach.
- **`@` respects the gitignore + a denylist** (`.env`, key files) — never
  silently attach a secret.

### 3c. Plugin management

The engine already has `plugin {install,list,available,remove}` and a real
registry. Plugins are *occasional, deliberate* actions — not per-session.
So: **a `/plugins` command opening a dedicated panel**, linked from
`/config → Plugins`. Not buried in config (too deep for a marketplace), not
a top-level surface (too rare to earn a tab).

```
┌─ /plugins ────────────────────────────────────────────────┐
│  search  ›  |                          source: registry ▾  │
├────────────────────────────────────────────────────────────┤
│  INSTALLED  3                                              │
│   ✓ wayland-ollama     0.6.1   local inference provider    │
│   ✓ wayland-browser    0.6.1   browser automation tools    │
│   ✓ wayland-ijfw       0.4.0   IJFW workflow + skills      │
│                                                            │
│  AVAILABLE  from registry                                  │
│   + wayland-cua        0.6.1   computer use (screen/mouse) │
│   + wayland-honcho     0.3.2   process orchestration       │
│   ⚠ unsigned plugins are not shown — see /doctor           │
├────────────────────────────────────────────────────────────┤
│  ⏎ install/remove   i details   esc close                  │
│  installs to ~/.../wayland-core/plugins · removable anytime │
└────────────────────────────────────────────────────────────┘
```

- **One verb per row, framed by state.** Installed rows show `✓` and
  remove-on-`⏎`; available rows show `+` and install-on-`⏎`. Krug: the
  affordance *is* the state.
- **Source switcher** (`registry` ▾ → `github://<org>`) is a quiet dropdown,
  not a flag the user must recall — grounds the real `--source` arg.
- **`i` for details** before install — Sutherland: earn the right to ask;
  show what a plugin *does* and *touches* before the user commits.
- **Footer states reversibility** — "removable anytime" kills install
  anxiety (Krug: error tolerance).

### 3d. What the mockup is still missing

1. **`/help` and a universal `?`** — every surface needs an on-screen
   "what can I do here." Krug principle 4; finding #19. `?` opens a
   context-scoped key list, not the manual.
2. **`/doctor` surface** — the engine has a real doctor probe. It's the
   honest home for "is my key valid, is Ollama up, are MCP servers healthy."
   The onboarding audit cut live key-validation from *onboarding* — `/doctor`
   is where it belongs instead.
3. **`/cost`** — a dedicated spend view (per-session, per-day, by model).
   Sutherland: making cost visible builds trust; hiding it reads as evasion.
4. **A notification spec** — OSC 9 / bell on approval-needed and
   long-task-done when the terminal is unfocused (cli-complaints Tier 3).
5. **`/feedback` or `/memory` correction path** — the engine has long-term
   memory; the user needs a way to *see and correct* what it learned. An
   agent that remembers but can't be corrected is a Sutherland trust leak.
6. **A resize/`NO_COLOR`/`TERM=dumb` story** — not a surface, but the
   redesign must commit to the costly-signal craft: footer never wraps,
   color is semantic and disable-able. This is the cheapest credibility the
   CLI can buy.
