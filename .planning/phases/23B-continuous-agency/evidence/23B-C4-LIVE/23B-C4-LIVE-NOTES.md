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

## M2 — status

- [x] read prior lane summary + live evidence in full
- [x] code read above
- [ ] hetzner worktree + build
- [ ] observation 1 (cache HIT)
- [ ] observation 2 (compaction)
- [ ] observation 3 (history_rewritten)
- [ ] secret sweep + liveness control
