# 23B Criterion 4 — cache & compaction truth — WORKING NOTES

Lane `23b-c4-cache`. Branch `lane/23b-c4-cache`, base `lane/grade-23b` @ `5bbb0fbc`.
Started 2026-07-29. **Append after every measurement; never batch to the end.**

Criterion text (ROADMAP:104, quoted by 23B-PHASE-VERDICT:26):
> Cache and compaction expose quality, invalidation, token-pressure, cost truth.

Verdict grade: **NOT MET**, cause = plan 23B-02 Task 2 (F23-04) never started.

---

## M0 — inherited claims I intend to re-derive, not trust

The verdict says (`23B-PHASE-VERDICT.md:159-164`):

| Clause | verdict's finding |
|---|---|
| cost truth | partly, pre-existing (`/cost` TUI) |
| cache quality / invalidation | no — warning-only telemetry |
| token-pressure | no — `TokenPressure` 0 refs in agent/cli |
| cost-regression thresholds | no |

Plus the lane brief's warning: **the existing cost observable is reported broken —
invariant across harnesses (same number regardless of what happened).** Must verify
before building on it.

---

## M1 — cache surface inventory (measured 2026-07-29, `/usr/bin/grep`, unproxied)

Files found by `/usr/bin/find crates -name '*cache*.rs'` (10):

```
crates/wcore-providers/src/cache_observation.rs      278 lines
crates/wcore-agent/src/cache_diagnostics.rs          654 lines
crates/wcore-observability/src/cache.rs
crates/wcore-tools/src/file_cache.rs                 (file-read cache, not prompt cache)
crates/wcore-config/src/file_cache.rs
crates/wcore-types/src/cache_tier.rs
+ 4 test files
```

### M1a — `PromptCacheObservation` / `InvalidationCause` are DEAD TYPES

```
/usr/bin/grep -rn "PromptCacheObservation" crates --include="*.rs" | grep -v cache_observation.rs
  -> crates/wcore-providers/src/lib.rs:55   (the pub use re-export, ONLY)
/usr/bin/grep -rn "InvalidationCause" crates --include="*.rs" | grep -v cache_observation.rs
  -> crates/wcore-providers/src/lib.rs:55   (the pub use re-export, ONLY)
```

Known-positive control in the same invocation flags: `grep -rln "LlmProvider" crates
--include="*.rs" | wc -l` -> **119 files**. Instrument alive.

**Finding C4-F1 (HIGH): `PromptCacheObservation` has ZERO production construction sites
and ZERO consumers.** It is `pub use`d from `wcore-providers` — i.e. *advertised on the
crate's public API* — and never built, never emitted, never read. `InvalidationCause`,
the type that carries the criterion's whole "invalidation" clause, is reachable only by
an external crate that constructs it itself. **This is the advertised-but-dead surface
class the brief names (11 prior recorded instances).**

### M1b — the LIVE cache path is `wcore-agent::cache_diagnostics`

`CacheBreakDetector` IS wired: `engine.rs:29` (import), `:2495` (field), `:3111/:3349`
(construct), `:3668` (reset), `:11041` (`check_response`), `:11078` (`check_cache_health`).
Its own enum `CacheBreakCause {SystemPromptChanged, ToolsChanged, TtlExpiry, FirstRequest}`
is a SECOND, parallel vocabulary to `InvalidationCause`'s seven variants. The two never
meet. (Note for the absence rule: "invalidation is absent" would have been FALSE — the
concept exists under `CacheBreakCause`. Exactly the §3b-i vocabulary trap.)

### M1c — how the live path is exposed today

Three exposure routes, all weak:

1. `emit_info` of `Cache full miss (cause: ...)` / `Cache: N% hit rate` — but gated on
   `self.compact_config.cache_diagnostics`, which **defaults to `false`**
   (`wcore-config/src/compact.rs:175`) and is settable only via TOML
   (`cache_diagnostics = true`). No CLI flag, no slash command found yet.
2. `tracing::warn!(target: "cache_health", ...)` at `engine.rs:11091` — log-only, and the
   code's own comment says *"Warning-only structured telemetry: greppable in the engine
   log, never alters the request."* Greppable-in-a-log is not operator-reachable.
3. Nothing aggregates across a session. No hit/miss/saving totals anywhere.

**So: cache quality is computed per-turn and thrown away.** No accumulator, no query
surface, no exit report.

---

## M2 — questions from M0, now answered

- [x] **Is the cost observable invariant?** No — but it was **unlabelled**, which is worse
      in a different way. `resolve_turn_cost` reports `priced = true` for a
      `ProviderCompat` family-rate fallback exactly as it does for a catalog row.
      Measured live: `ollama:smollm2:135m`, a LOCAL model, billed **$0.0756 at Anthropic's
      rate**, with the engine's own log saying
      `W7: wcore-pricing model is unresolvable; falling back to ProviderCompat cost heuristic`.
      → finding **C4-F1**, fixed by recording `CostSource` and grading `CostTruth::Estimated`.
      Variance itself is fine: two live sessions gave `$0.0756` and `$0.06165`, and the
      engine test asserts a 100× workload costs 100.0 ± 0.01×.
- [x] **Token pressure concept.** `TokenPressure` as a symbol genuinely has 0 refs
      (control: `CompactConfig` → 19 files in the same invocation), but the CONCEPT lives
      in `compact/auto.rs` (`should_autocompact`), `compact/emergency.rs`
      (`is_at_emergency_limit`) and `CompactState::last_real_input_tokens`. Both thresholds
      were computed inline inside their predicates; now extracted as
      `auto::autocompact_threshold` / `emergency::emergency_limit` so the number reported
      cannot drift from the number enforced.
- [x] **`wcore-observability/src/cache.rs`** — it is the breakpoint-marking helper
      (`mark_cache_boundaries`), request-side. Not an observability surface; nothing to
      expose there.
- [x] **`wcore-pricing`** already has cache-aware pricing
      (`estimate_cost_with_cache_status_resolved`) and a `priced` flag. The gap was that
      nothing aggregated or surfaced it, and that the flag conflates two provenances.

## M2b — a second gap found live

Round-trip 1 arrived with **no invalidation cause**. `CacheBreakDetector::compute_diagnostic`
returns `Healthy { hit_rate: 0.0 }` for the first request (no previous turn to compare), so
`CacheBreakCause::FirstRequest` is unreachable from the engine — the one round-trip
guaranteed to be a miss was the one nothing explained. → finding **C4-F2**, fixed narrowly
(`NoMarker` only when the opening round-trip neither read nor wrote cache).

## M3 — what was built

A session-scoped `CacheLedger` in `wcore-agent`, flushed after every record to
`<wayland home>/cache-ledger/<session>.json`, plus `wayland-core cache
{report|list|show|verify}` reading it without an engine. Driven end to end from the shipped
binary on hetzner: see `23B-C4-LIVE-EVIDENCE.md`. `cache verify` exited **7** on the live
ledger and **8** on an empty store — both observed, unpiped.

Deviation from the M3 sketch: **no `/cache` slash command.** The slash `SlashHandler` trait
is `&self` and stateless, so reaching a live engine's ledger through it needs shared
interior state; the CLI verb reads the same file with none of that, and is the path I could
prove end to end. Recorded as a deliberate omission, not an oversight.
