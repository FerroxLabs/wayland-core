# 23B Criterion 4 — closing the three live gaps — WORKING NOTES

Lane `c4-live-cache`. Branch `lane/c4-live-cache`, base `plan/f20-unified-audit-repair`
@ `19c10666`. Started 2026-07-29. **Append after every measurement; never batch to the end.**

## Mandate

Lane `23b-c4-cache` graded C4 **MET on all four sub-clauses** and left three stated
live gaps (`23B-C4-LIVE-EVIDENCE.md` §6). Sean has now supplied a working
`ANTHROPIC_API_KEY` on `hetzner-dsm`. I am NOT re-grading C4. I am converting three
gaps into observations:

1. a live cache **HIT** (`hit_ratio`/`cache_read` non-zero) via `wayland-core cache report`
2. a live **COMPACTION** under token pressure (`compactions` non-zero)
3. a live **`history_rewritten`** invalidation cause

Each must come from the operator-reachable path (`cache report` / `cache verify`),
not an internal probe. Provider must be read back from the product's own output
(LANE-BRIEF §3b-ii) — `/root/.wayland/.env` injects `ANTHROPIC_API_KEY` into every
process regardless of what I unset, so a hit claim that silently ran on Ollama is
worthless, and so is one that ran on Anthropic when I believed it was Ollama.

## M0 — code read before any run (measured, unproxied `/usr/bin/grep`)

- `autocompact_threshold(config) = context_window - output_reserve - autocompact_buffer`
  (`compact/auto.rs:78`). All three are `CompactConfig` TOML fields with serde defaults
  (`wcore-config/src/compact.rs:10-21`). **So the 167000 threshold is configurable
  downward** — a small `context_window` makes compaction reachable for a few cents
  rather than by burning 167k tokens of real Anthropic input.
- `should_autocompact(last_input_tokens, config)` compares the **API-reported** last
  input tokens against that threshold, and returns false when `!config.enabled`.
- Compaction recording site: `engine.rs:13363` — the `Auto` success arm calls
  `record_cache_ledger_compaction(CompactionKind::Auto, …)`. Its own comment names
  the join: after the prefix is replaced, the next round-trip's miss must be
  attributed to `history_rewritten`, not to the hash comparison's guess.
- `history_rewritten` grep across `crates` (unproxied): 6 hits, all in
  `cache_observation.rs` (the enum + its parser), `cache_ledger.rs:1052` (a unit test),
  `engine.rs:13363` (the comment at the compaction site), and
  `cache_ledger_cli.rs:274,287` (CLI test). Instrument-liveness control in the same
  invocation: `CacheLedger` → 4 files. So the concept exists and is wired; what has
  never happened is a LIVE attribution.

## M1 — plan

One Anthropic session can in principle produce all three:
turn 1 large stable prefix (cache **write**) → turn 2 same prefix (cache **read** = HIT)
→ input crosses a deliberately-low autocompact threshold (**compaction**) → turn 3
(**history_rewritten**). Cheapest caching-capable model, small `max_tokens`.

Money: real billable key. Minimum spend that proves the point; no repeat runs once
observed. Never print/commit the value; sweep artifacts at the end WITH a liveness
control (a sweep returning 0 on an empty needle has already fired on this programme).

## M1b — the three observations are NOT equally reachable (code read, before any run)

Read the actual attribution code before spending money. Three constraints found:

**(a) Anthropic's prompt-cache minimum is per-model and the cheapest model has the
highest one.** `claude-haiku-4-5` is $1/$5 per MTok but needs a **4096-token** prefix
before Anthropic will create a cache entry at all (Opus 4.8 / Sonnet 5 = 1024; Opus 5
= 512). Below the minimum Anthropic silently declines to cache — `cache_creation=0`,
no error. So a cache-HIT proof on haiku needs a >4096-token stable prefix. The
product ALSO has its own floor: `DEFAULT_CACHE_MIN_PREFIX_TOKENS = 1024`
(`wcore-config/src/config.rs:830`), estimated at **bytes/2** (deliberately
over-counting, `anthropic.rs`), below which it strips every `cache_control` marker.
Two independent floors, and the product's is the looser one — so the binding
constraint is Anthropic's 4096.

**(b) Compaction thresholds are TOML-configurable, so compaction is cheap to reach.**
`threshold = context_window - output_reserve - autocompact_buffer`. Setting
`autocompact_buffer` large drives the threshold far below the emergency limit
(`context_window - emergency_buffer`), which must stay above it or the session
hard-stops instead of compacting. Target: threshold 8000, emergency 29000.

**(c) `history_rewritten` may be UNREACHABLE on Anthropic — predicted before running.**
`engine.rs:13065-13070` only overrides the cause when **all three** hold:
`cache_read_tokens == 0` AND `cause.is_some()` AND `compacted_since_last_round_trip()`.
But compaction rewrites the **messages** zone only — the system and tools zones are
untouched, so on the post-compaction round-trip Anthropic can still serve the
system/tools cache, `cache_read_tokens > 0`, and the override is skipped.
`cause.is_some()` is itself conditional: `compute_diagnostic`
(`cache_diagnostics.rs:178`) returns `Healthy` (no cause) unless it reaches
`FullMiss` (needs prev-had-cache AND current read == 0) or `PartialMiss` (>5% drop).

So the design that makes it reachable is to put the cached bulk in the **messages**
zone and keep system+tools **below Anthropic's 4096 minimum**, so the post-compaction
turn reads nothing at all. That is what I will drive. **If it still does not fire, the
honest result is that `history_rewritten` is unreachable live on this provider, and I
will report that with the exact guard that blocks it rather than manufacture it.**

## M2 — status

- [x] read prior lane summary + live evidence in full
- [x] code read above
- [x] hetzner worktree + build
- [x] observation 1 (cache HIT)
- [x] observation 2 (compaction fires — and FAILS 400; C4L-F1)
- [~] observation 3 (`history_rewritten` seen, but on a FAILED compaction = false positive)
- [ ] secret sweep + liveness control

---

# SESSION 2 — resumed 2026-07-29 after the first agent was killed

The first agent's process died after `bc65e989`. Its five commits survive and are the
starting point. `23B-C4-LIVE-EVIDENCE.md` **ends mid-write** at a
`<!-- ferrox:write-continue -->` sentinel: §4 (the findings C4L-F1 / C4L-F2 that §2 and
§3 both forward-reference) and §5 (the secret sweep) were never written. They are
therefore NOT established, whatever the body text implies.

## M3 — re-verifying the C4L-F1 diagnosis before building on it (code read, unproxied)

The dead agent's commit message says *"autocompact is checked mid-tool-loop"*. That
wording is **imprecise, and the precise mechanism matters** — a fix that works for the
wrong reason is a coincidence. What the code actually says:

- `run_compaction` is called from **three** sites (`/usr/bin/grep`, quoted glob —
  an unquoted `--include=*.rs` was eaten by zsh on the first attempt, exactly the
  LANE-BRIEF §3b-i trap): `engine.rs:9149` (turn-loop top, *"before each API call"*),
  `engine.rs:10486` (`ContextOverflow` compact-and-retry) and `engine.rs:10824`
  (length-wedge forced compaction). Liveness control in the same invocation:
  `CompactionKind` → 14 hits.
- `engine.rs:9112` states the loop shape outright: *"this is the turn-loop top (the
  tool loop lives below `provider.stream` in the same iteration)"*, and 9140 says
  *"Run multi-level compaction before each API call."* **So compaction is evaluated
  before every API call, including the continuation calls of a tool loop** — not once
  per user instruction.
- The actual defect is at `engine.rs:13243`:
  ```rust
  let live_user_turn: Option<Message> = match self.messages.last() {
      Some(m) if matches!(m.role, Role::User) => self.messages.pop(),
      _ => None,
  };
  ```
  It pops **any** trailing `User` message. On a tool continuation the trailing `User`
  message is not a live instruction at all — it is the **`tool_result` carrier**.
  Popping it strands the preceding `Assistant` turn's `tool_use` blocks with no
  answer, and `autocompact` then appended a plain-text summary prompt where the
  provider demands a `tool_result`. Hence the 400.

That refines rather than contradicts the dead agent's diagnosis, and it predicts the
live data: three failures at `messages.2`, `messages.4`, `messages.6` — **stepping by
exactly 2**, one `Assistant`+`User` pair per tool round-trip — carrying 1, 2 and 2
tool-use ids (parallel calls). Extracted unproxied from `/root/c4-live/B-show.txt`;
liveness control in the same invocation: 8 `F23_CACHE` lines, 3 `kind=auto_failed`.

The engine already knows the trailing `User` turn can carry `tool_result`s — `#285` at
`engine.rs:13280` demotes orphaned results to text **at the post-compaction fold**. The
uncovered gap is the *request handed to the compaction LLM*, which is exactly where
`c0b0e18e` put `drop_unanswered_tool_calls`. Right layer.

**Still to establish (nothing below is proven yet):**
- the fix works LIVE against real Anthropic (`failed=0`, `tokens_reclaimed>0`);
- reverting it brings the 400 back (known-negative on the fix itself);
- `history_rewritten` on a SUCCESSFUL compaction;
- provider read back from the product's own output on every run (§3b-ii);
- secret sweep with a liveness control.

**Residual gap already visible in the code, not yet tested:** the `PromptTooLong`
retry path (`compact/auto.rs:246`) truncates the oldest 20% *after* sanitation, which
can strand a `tool_result` whose `tool_use` was truncated away — the mirror-image
violation. Recorded now so it is not lost.
