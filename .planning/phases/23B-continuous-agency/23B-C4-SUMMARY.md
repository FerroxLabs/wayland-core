# Phase 23B — Criterion 4 (cache & compaction truth) — LANE SUMMARY

**Lane:** `23b-c4-cache` · **Branch:** `lane/23b-c4-cache`
**Base:** merged `gh/plan/f20-unified-audit-repair` @ `4a872413` (merge-base
confirmed by `git merge-base`, not by branch name)
**Date:** 2026-07-29
**Evidence:** `evidence/23B-C4/23B-C4-LIVE-EVIDENCE.md`, `evidence/23B-C4/23B-C4-NOTES.md`,
raw captures in `evidence/23B-C4/live/`

Criterion 4 (ROADMAP:104): *"Cache and compaction expose quality, invalidation,
token-pressure, cost truth."* Graded **NOT MET** by `23B-PHASE-VERDICT.md:140`,
cause: never started.

---

## VERDICT: **MET on all four sub-clauses, with two stated live gaps**

Not "MET" unqualified. Every clause is now computed, persisted and reachable
from the shipped binary, and I drove that path end to end. But two of the four
were live-proven only on shapes a cache-less provider can produce, because **no
permitted host has a working prompt-caching credential** (§4). I am not going to
call that a full green.

| Sub-clause | Status | Operator-reachable path PROVED |
|---|---|---|
| **quality** | MET | `wayland-core cache report` → `F23_CACHE=quality hit_ratio=… warm_hit_ratio=… cache_read=… cache_write=…`. Live: yes. Live with a non-zero hit: **no** (§4). |
| **invalidation** | MET | `… cache report` → `F23_CACHE=invalidation` + one `F23_CACHE=invalidation_cause name=… count=…` per cause. Live: yes — `no_marker:1` on the real run. |
| **token-pressure** | MET | `… cache report` → `F23_CACHE=pressure peak_watermark=4095 autocompact_threshold=167000 emergency_limit=197000 peak_pressure=0.0245 …`. Live: yes. Live with a compaction: **no** (§4). |
| **cost truth** | MET | `… cache report` → `F23_CACHE=cost` + `F23_CACHE=cost_warning`; `… cache verify` → **exit 7**. Live: yes, and it failed on the live data, which is the point. |

---

## 1. What landed

New module `crates/wcore-agent/src/cache_ledger.rs` (~700 lines + 20 unit
tests): a session-scoped ledger accumulating one row per LLM round-trip and one
per compaction attempt, flushed atomically to
`<wayland home>/cache-ledger/<session-id>.json` **after every record**, so a
killed session still leaves everything up to its last round-trip.

Engine wiring in `crates/wcore-agent/src/engine.rs`: one recording call beside
the existing `CacheBreakDetector` block, one at each of the four compaction
outcomes (micro, auto, auto-failed-circuit-broken, auto-failed-other), one
retention note on the request side, `finish()` at session end, `rotate()` on
conversation reset. Threshold accessors `auto::autocompact_threshold` and
`emergency::emergency_limit` were **extracted from** the predicates that already
computed them, so the number shown to an operator cannot drift from the number
the engine acts on.

New operator surface `crates/wcore-cli/src/cache_cmd.rs`:
`wayland-core cache {report|list|show|verify} [--session] [--dir] [--json]`.
Exit map: `0` ok, `1` failed, `7` cost not trustworthy, `8` no ledger.

**Recording is ON by default.** The nearest existing surface,
`compact.cache_diagnostics`, defaults to `false` — which is exactly why the
verdict graded the diagnostics that already existed as unexposed. Kill switch is
`WAYLAND_CACHE_LEDGER=0`, and a test asserts it leaves no file.

### Two dead types revived rather than duplicated

`wcore_providers::cache_observation::{PromptCacheObservation, InvalidationCause}`
were `pub use`d from that crate — advertised on its public API — with **zero
construction sites** anywhere in the workspace (measured: the only non-defining
reference was the `pub use` itself; known-positive control `LlmProvider` → 119
files in the same invocation). Meanwhile the engine used a second, parallel
vocabulary, `CacheBreakCause`. I bridged them (`invalidation_cause_of`) instead
of inventing a third. `TurnSample::as_observation` is `PromptCacheObservation`'s
first production construction site.

Note for the absence rule (§3b-i): "invalidation is absent" would have been
**false** — the concept existed under `CacheBreakCause`. The vocabulary trap the
brief names, hit and avoided.

---

## 2. Findings — three, all measured, all from a red

### C4-F1 (HIGH, FIXED) — a family-rate estimate was rendering as spend

`resolve_turn_cost` has two price paths and reports `priced = true` for **both**:
an exact `wcore-pricing` catalog row, and — on a catalog miss — the
`ProviderCompat` family default. Found by a failing test:
`an_uncatalogued_model_is_recorded_unpriced_rather_than_free` came back
`left: Priced, right: Unpriced` on model `test-model`.

Confirmed live: `ollama:smollm2:135m` on a LOCAL model that cost nothing was
billed **$0.0756 at Anthropic's rate**, with the engine's own log saying
`W7: wcore-pricing model is unresolvable; falling back to ProviderCompat cost
heuristic`.

Fixed inside the ledger, not by changing `resolve_turn_cost` (which the budget
path depends on): `TurnSample` carries
`CostSource {Catalog, ProviderDefaults, Unpriced}` instead of a bool, and
`CostTruth` gained an `Estimated` grade. `verify` exits 7 on it; `report` prints
`cost_warning text=usd_is_a_family_rate_estimate_not_spend`.

**This is the orchestrator's "cost observable is broken" warning, met with a
specific mechanism.** The number was not invariant — it was *unlabelled*.

### C4-F2 (MEDIUM, FIXED) — the one guaranteed miss was the one nothing explained

Live: round-trip 1 reached the ledger with no invalidation cause.
`CacheBreakDetector::compute_diagnostic` returns `Healthy { hit_rate: 0.0 }` for
the first request because it has no previous turn to compare, which makes
`CacheBreakCause::FirstRequest` **unreachable from the engine** (its only
construction site is inside `attribute_cause`, which the first-request path
returns before reaching).

Fixed narrowly: attribute `NoMarker` when round-trip 1 neither read nor wrote
cache — the exact case that variant documents. A cold open that *wrote* cache is
normal and is deliberately left unattributed; a test pins that so the fix cannot
drift into mislabelling healthy behaviour.

### C4-F3 (MEDIUM, NOT FIXED — pre-existing) — `provider` is the compat profile, not the route

The live ledger records `provider=anthropic` on a round-trip that ran on
`ollama:smollm2:135m`. The field comes from `self.compat.provider_type()`, which
is the configured compatibility profile, not the plugin route that served the
turn. `TurnTrace.provider` and the budget charge read the same value, so this is
**pre-existing and workspace-wide**, not introduced here.

I did not fix it: doing it correctly means plumbing the plugin route's identity
through the turn, which is a different subsystem and another lane's surface.
A wrong provider label on a cost surface is a real defect and should be filed.
Recorded here rather than absorbed.

---

## 3. Gates — every number read back, `0 ignored` stated

Captured over ssh so no local `rtk` proxy could strip the `ignored` /
`filtered out` fields the anti-vacuity rule depends on
(`evidence/23B-C4/gate-counts.txt`):

```
cargo test -p wcore-agent --lib cache_ledger              20 passed; 0 failed; 0 ignored; 2184 filtered out
cargo test -p wcore-agent --test cache_ledger_engine_test  6 passed; 0 failed; 0 ignored; 0 filtered out
cargo test -p wcore-cli   --test cache_ledger_cli         13 passed; 0 failed; 0 ignored; 0 filtered out
-- no regression in what this lane touched --
cargo test -p wcore-agent --lib cache_diagnostics         14 passed; 0 ignored
cargo test -p wcore-agent --lib compact::                118 passed; 0 ignored
cargo test -p wcore-agent --test engine_compact_test      15 passed; 0 filtered out
```

`cargo clippy -p wcore-agent -p wcore-cli --all-targets` → zero error/warning
lines. `cargo fmt --all -- --check` clean.
`python3 scripts/check-no-vacuous-cargo-test.py` → `GATE: PASSED`; its
`--self-test` → `PASSED (6 assertions)`. This lane adds no shell/CI `cargo test`
invocation, so it neither trips nor needs the `vacuity-checked:` marker.

### The gates can fail — seven observed reds

Not asserted, observed, on this code during this lane:
`cache verify` **exit 7** on the live ledger; `cache verify` **exit 8** on an
empty store; `ledger_path_cannot_escape_its_directory` (assertion was wrong, not
the sanitizer — rewritten to test path *components* with a raw-join
known-positive); `a_real_run_writes_a_ledger…` at hit_ratio 0.497 vs a wrong
`> 0.6`; `an_uncatalogued_model…` (→ finding C4-F1);
`recorded_cost_varies…` twice (ContextTooLong, then 48× on a 100× workload);
`json_output_carries…` `left: Null`. Detail in the live-evidence file §5.

**Three of those reds corrected the TEST, not the code, and I have said which in
each case** — cache writes really do belong in the hit-ratio denominator, and a
write-heavy session really does cost more than its uncached counterfactual
(Anthropic charges 1.25× input for a cache write). Both directions are now
asserted, each on a shape that exhibits it.

### The broad `wcore-agent --lib` run is red — and it is red at the BASE

`cargo test -p wcore-agent --lib` on this lane's HEAD: `2184 passed; 17 failed`.
I did not report that as a pass and I did not report it as my regression — I
built the merge-base in a separate worktree and ran the same command:

| Run | Commit | Result |
|---|---|---|
| lane HEAD, run 1 | `f8b437fb` | 2184 passed; **17 failed**; 3 ignored |
| lane HEAD, run 2 | `f8b437fb` | 2189 passed; **12 failed**; 3 ignored |
| **merge-base** | **`4a872413`** | 2164 passed; **17 failed**; 3 ignored |

The base — containing none of this lane's code — fails 17 in the same families
(`engine::audit_2026_05_22_tests`, `session::tests`, `session_journal::fault_tests`,
`session_lifecycle::tests`, `orchestration::f13_durability_tests`,
`engine::retry_wedge_protection_tests`). The failing set also **moves between two
runs of the identical binary**, which a code regression does not do, and
`--test-threads=1 session:: session_journal::` on the lane HEAD passes
**96/96**. Pre-existing flake in the integration branch under parallel load;
**something the orchestrator should know about, and not this lane's.**

`cargo test -p wcore-cli --lib` also shows `test always_fails ... FAILED` in a
one-test binary on both trees — the `31-vacuous-greens` canary, which is
supposed to fail. The real suite in the same invocation: `1854 passed; 0 failed`.

### Known-negatives carried

Every suite carries at least one assertion that must fail if the instrument is
dead: an unknown subcommand must exit non-zero (this is what makes the CLI suite
evidence of *exposure* rather than of arithmetic); a clean session must report
zero invalidation causes; a catalogued model must grade `Priced` while
`test-model` grades `Estimated`; a 1k-token session must report a different
pressure ratio than a 130k one; the kill switch must leave **no file at all**
(this is what proves the other engine tests observe a file the engine really
wrote); a raw `join` must show a `ParentDir` component.

---

## 4. What is NOT proved — the honest limits

1. **No live cache HIT.** `hit_ratio=0.0000` on the live run is real: Ollama has
   no prompt cache. Hit / warm-ratio / positive-saving paths are proved against
   the ENGINE (`cache_ledger_engine_test.rs` feeds real `TokenUsage` with
   `cache_read_tokens` set and reads the JSON back off disk) and against the
   CLI — not against a live caching provider.
2. **No live compaction**, therefore no live `history_rewritten` attribution.
   Those recording paths are covered by tests only.
3. **The live cost variance is on the output side.** Two live sessions produced
   `$0.0756` and `$0.06165` — the number varies. But both report
   `uncached_input=4095`, because `smollm2:135m`'s window truncates, so the live
   pair does not prove variance *with input*. That is proved in the engine test,
   which asserts a 100× workload costs **100.0 ± 0.01×**.

### The blocker behind 1–3, measured rather than assumed

`hetzner-dsm` has **no working prompt-caching credential**:
`/root/.wayland/.env`'s `ANTHROPIC_API_KEY` returns
`401 … "API key is invalid."` from the product itself, and
`/root/.wayland/auth.json`'s single anthropic pool entry declares
`source = "env:ANTHROPIC_API_KEY"` — the same key. No other provider variable
exists in the environment or in `/root/.bashrc` / `/root/.profile`.
(Inspected by field name and length only; **no value was printed, and no
credential was embedded or supplied** — LANE-BRIEF §0.)

**Sean-reserved:** supplying a working provider credential. With one, the three
gaps above close in a single run of the existing `cache report` path.

Two environment facts recorded for the next lane: with the DEFAULT home every
session aborts pre-API-call on `storage.credentials.backend … "plaintext"`, and
a project-level `.wayland/config.toml` with `[session] enabled = false` did NOT
take effect while an isolated `WAYLAND_HOME` with the same two lines did; and an
isolated profile does not import `auth.json`.

---

## 5. Shared-file fence

Diffed against the **merge-base SHA** `4a872413`, never the branch name:

```
git diff 4a872413 -- crates/wcore-cli/src/lib.rs crates/wcore-cli/src/main.rs
```

Three additive blocks, zero deletions, zero reordering, zero reformatting:
`pub mod cache_cmd;` in `lib.rs`; one `Cache(...)` variant in `TopCmd`; one
`TopCmd::Cache(args) => …` dispatch arm beside `TopCmd::Index`.

**Orchestrator correction honoured:** I was based on `lane/grade-23b` and merged
`gh/plan/f20-unified-audit-repair @ 4a872413` before finishing. Re-checked
Criterion 4 against the merged tree — `git diff <old-base> 4a872413` over
`cache_diagnostics.rs`, `cache_observation.rs`, `compact/`, `wcore-pricing`,
`wcore-observability/src/cost.rs`, `tui/surfaces/diagnostics.rs` returns
**zero files**, so nothing in the merge train collided with or pre-empted this
work. `WCORE_EVAL_TURN_TRACE` (lane `ci-green`) is eval-scenario turn *timing*
and does not touch the cost path; nothing here is wired through it.

---

## 6. Files

Created: `crates/wcore-agent/src/cache_ledger.rs`,
`crates/wcore-agent/tests/cache_ledger_engine_test.rs`,
`crates/wcore-cli/src/cache_cmd.rs`,
`crates/wcore-cli/tests/cache_ledger_cli.rs`,
`.planning/phases/23B-continuous-agency/evidence/23B-C4/*`.

Modified: `crates/wcore-agent/src/engine.rs` (recording sites + two accessors +
`set_cache_ledger_dir`), `crates/wcore-agent/src/lib.rs` (module),
`crates/wcore-agent/src/compact/auto.rs` + `compact/emergency.rs` (threshold
accessors extracted from the existing predicates),
`crates/wcore-cli/src/lib.rs` + `src/main.rs` (fenced, additive).

Not done, deliberately: no TUI `/cache` screen (the criterion asks for exposure,
and the CLI verb is the reachable path this lane could prove end to end); no
change to `resolve_turn_cost` or `TurnTrace` (budget-path blast radius);
no fix for C4-F3; no `wcore-contract generate`; no merge, PR, tag or release.
