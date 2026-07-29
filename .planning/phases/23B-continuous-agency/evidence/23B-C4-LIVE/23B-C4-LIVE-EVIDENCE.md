# 23B Criterion 4 — closing the three live gaps

Lane `c4-live-cache`. Host `hetzner-dsm`, worktree `/root/wayland-c4-live-cache`,
binary `target/debug/wayland-core` built at `57e6a9a5` (`BUILDRC=0`).
Model `claude-haiku-4-5` on the real Anthropic API. Captures under `/root/c4-live/`.

This file closes the three gaps `23B-C4-LIVE-EVIDENCE.md` §6 stated. It does **not**
re-grade Criterion 4 — the four sub-clauses were already graded MET.

---

## Credential handling

The key was **not** supplied by me and never left the box. `/root/.wayland/.env`
(mode 600, 108 chars) already carried it; the runner sourced that file inside the
remote shell so the value entered only the child process's environment — never
`argv`, never a new file, never a capture, never this report. The only thing
printed about it is `KEY_PRESENT=yes LEN=108`. Sweep + liveness control in §5.

An isolated `WAYLAND_HOME` does **not** pick the key up on its own — the first
attempt failed with `Error: No API key found` (rc=1), which is the prior lane's
"an isolated profile does not import auth.json" finding reproduced.

---

## 1. OBSERVATION 1 — a live cache HIT

### Provider read back from the product's own output (LANE-BRIEF §3b-ii)

Not inferred from what I exported. `wayland-core cache show`, verbatim:

```
F23_CACHE=turn round_trip=3 turn=2 provider=anthropic model=claude-haiku-4-5 ...
```

`provider=anthropic model=claude-haiku-4-5` on every round-trip. This is the arm the
claim depends on, and it is asserted from the ledger the engine wrote, not from the
environment. **A second, independent check comes free: `cache_read`/`cache_write` are
non-zero, and Ollama — the arm the prior lane ran on — implements no prompt cache and
cannot produce either.** The two agree.

### The engine's own live line

```
[turns: 3 | tokens: 9 in (10474 cached) / 179 out | cache: 21375 created, 10474 read]
SESSION_A_RC=0
```

### The operator-reachable path — `wayland-core cache report`, verbatim

```
F23_CACHE=session id=22e5baee-237f-4f09-98dd-39f2efe8d6c8 round_trips=3 complete=true
F23_CACHE=quality hit_ratio=0.3288 warm_hit_ratio=0.9744 hit_round_trips=1
          miss_round_trips=2 warm_round_trips=1 cache_read=10474 cache_write=21375
          uncached_input=9 output=179 total_input=31858
F23_CACHE=invalidation distinct_causes=1 causes=expired:1
F23_CACHE=invalidation_cause name=expired count=1
F23_CACHE=pressure peak_watermark=10749 autocompact_threshold=167000
          emergency_limit=197000 peak_pressure=0.0644 compactions=0 micro=0 auto=0
          failed=0 tokens_reclaimed=0
F23_CACHE=cost usd=0.028670 uncached_equivalent_usd=0.032753 saving_usd=0.004083
          saving_ratio=0.1247 cost_truth=priced catalog_priced_round_trips=3
          estimated_round_trips=0 unpriced_round_trips=0
```

**The gap is closed.** The prior lane reported `hit_ratio=0.0000 … cache_read=0
cache_write=0` — every field present, nothing to report. Here the same fields carry a
real hit: `cache_read=10474`, `hit_round_trips=1`, `warm_hit_ratio=0.9744`.

Per round-trip (`cache show`), which is where the shape is legible:

| RT | uncached_in | cache_read | cache_write | hit | hit_ratio | invalidation |
|---|---|---|---|---|---|---|
| 1 | 3 | 0 | 10484 | false | 0.0000 | – |
| 2 | 3 | 0 | 10619 | false | 0.0000 | `expired` |
| 3 | 3 | **10474** | 272 | **true** | **0.9744** | – |

Round-trip 1 is a cold write; round-trip 3 reads that prefix back. The `warm_hit_ratio`
of 0.9744 is the number the clause is actually about — of the round-trips that *could*
hit, 97% of the input was served from cache.

### Three things this run establishes that were NOT previously live

1. **`cost_truth=priced`, not `estimated`.** The prior lane only ever observed
   `estimated`, because `ollama:smollm2:135m` misses the pricing catalog and falls back
   to a family rate. `claude-haiku-4-5` resolves to a real catalog row —
   `catalog_priced_round_trips=3, estimated_round_trips=0`. So the C4-F1 machinery is
   now observed on **both** sides: it flagged the untrustworthy number then, and it
   passes the trustworthy one now.
2. **`cache verify` exits 0.** `RC_VERIFY=0`, captured unpiped. The prior lane observed
   exit **7** (untrustworthy cost) and exit **8** (empty store) on the same binary. All
   three exit states of that gate have now been observed live, which is what makes
   `verify` a gate rather than a decoration — **it can fail, and it did, and this time
   it correctly did not.**
3. **A positive live saving.** `saving_usd=0.004083 saving_ratio=0.1247` — the cache
   actually saved money against its own uncached counterfactual, measured by the
   product. Note round-trips 1 and 2 each carry a **negative** saving
   (`saving_usd=-0.002621`, `-0.002655`): a cache *write* costs 1.25× input, so a
   write-only turn genuinely costs more than not caching. RT3 repays it at
   `saving_usd=+0.009359`. The ledger reports both signs rather than flooring at zero.

### An unplanned live invalidation cause

Round-trip 2 recorded `invalidation=expired` (`InvalidationCause::Expired`, from
`CacheBreakCause::TtlExpiry`). That is a **second** live invalidation cause on top of
the prior lane's `no_marker`, and it arrived without being asked for. It is a real
attribution of a real miss: RT2 wrote 10619 and read 0 because the message zone had
grown by the assistant's `tool_use` block, so the breakpoint moved off the cached
prefix — the hashes still matched, so `attribute_cause` fell through to `TtlExpiry`.
That label is arguably wrong for this shape (nothing expired; the breakpoint moved),
and it is recorded here as a finding rather than absorbed — see §4 C4L-F1.

Exit codes, captured **unpiped** (no `tee`, no `grep` — a pipe steals exit status):

```
RC_REPORT=0  RC_SHOW=0  RC_LIST=0  RC_VERIFY=0
```

<!-- ferrox:write-continue -->
