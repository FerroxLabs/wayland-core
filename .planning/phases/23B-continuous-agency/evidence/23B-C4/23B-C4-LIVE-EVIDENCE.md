# 23B Criterion 4 — live evidence

Lane `23b-c4-cache`. Host `hetzner-dsm`, worktree `/root/wayland-23b-c4-cache`.
Binary: `target/debug/wayland-core` built at `f8b437fb`.
All raw captures in `live/` beside this file.

---

## 1. What was driven, and how

A REAL session of the shipped binary, then the shipped binary's own `cache`
subcommand read the ledger that session wrote. Nothing here is a fixture.

Provider: **local Ollama** (`ollama:smollm2:135m`, `/usr/local/bin/ollama`,
`http://127.0.0.1:11434`). Isolated home `WAYLAND_HOME=/root/f23-c4-live/home3`
containing only `config.toml` with `[session] enabled = false`.

```
### SESSION A (tiny prompt)
SESSION_A_RC=0
### SESSION B (40 KB of source pasted into the prompt)
SESSION_B_RC=0
ls /root/f23-c4-live/home3/cache-ledger/
  6f9aa7b5-719a-4edc-93b4-5ce0703b4555.json
  8ea00290-0f3b-4047-8c74-4b6143048be7.json
```

Two sessions ran; two ledgers appeared. The engine wrote them; no test did.

### Exit codes were captured UNPIPED

The first pass of this evidence piped every verb through `tee` and recorded
`RC_VERIFY=0` — which was **`tee`'s** status, not the binary's (LANE-BRIEF §3.2,
"a pipe steals exit status"). Re-run with no pipe at all, redirecting to files:

```
RC_LIST=0
RC_REPORT=0
RC_SHOW=0
RC_VERIFY=7            <-- the gate FAILED, as it must on an untrustworthy cost
RC_JSON=0
RC_VERIFY_EMPTY=8      <-- an empty store is NOT a pass
```

`RC_VERIFY=7` on real data and `RC_VERIFY_EMPTY=8` on an empty directory are the
two observations that make this gate non-vacuous: it produced a **failure** on
the live run, from the shipped binary, without being asked to.

---

## 2. The four clauses, from the product's own output

`live/live-report.txt`, verbatim:

```
F23_CACHE=session id=8ea00290-0f3b-4047-8c74-4b6143048be7 round_trips=1 complete=true
F23_CACHE=quality hit_ratio=0.0000 warm_hit_ratio=0.0000 hit_round_trips=0
          miss_round_trips=1 warm_round_trips=0 cache_read=0 cache_write=0
          uncached_input=4095 output=189 total_input=4095
F23_CACHE=invalidation distinct_causes=1 causes=no_marker:1
F23_CACHE=invalidation_cause name=no_marker count=1
F23_CACHE=pressure peak_watermark=4095 autocompact_threshold=167000
          emergency_limit=197000 peak_pressure=0.0245 compactions=0 micro=0 auto=0
          failed=0 tokens_reclaimed=0
F23_CACHE=cost usd=0.075600 uncached_equivalent_usd=0.075600 saving_usd=0.000000
          saving_ratio=0.0000 cost_truth=estimated catalog_priced_round_trips=0
          estimated_round_trips=1 unpriced_round_trips=0
F23_CACHE=cost_warning text=usd_is_a_family_rate_estimate_not_spend cost_truth=estimated
```

| Clause | Live? | Operator-reachable path proved |
|---|---|---|
| quality | yes | `wayland-core cache report` → `F23_CACHE=quality` |
| invalidation | yes | `… report` → `F23_CACHE=invalidation` + one line per cause |
| token-pressure | yes | `… report` → `F23_CACHE=pressure`, against the real thresholds |
| cost truth | yes | `… report` → `F23_CACHE=cost` + `cost_warning`; `… verify` → **exit 7** |

**Read the provider back from the product's own output** (LANE-BRIEF §3b-ii):
`live/live-show.txt` records `model=ollama:smollm2:135m`, so the run really was
on the local arm and not on the `ANTHROPIC_API_KEY` this host injects.

---

## 3. Cost truth: the live run produced its own indictment

The cost figure above is **$0.0756 for a local model that cost nothing to run.**
That is not a bug in the ledger — it is the ledger reporting, correctly and
loudly, that the number is not spend.

The engine's own log line, from `live/sessionB.log`'s predecessor run:

```
WARN W7: wcore-pricing model is unresolvable; falling back to ProviderCompat
     cost heuristic provider="anthropic" model="ollama:smollm2:135m"
```

`resolve_turn_cost` could not price `ollama:smollm2:135m`, fell through to the
**Anthropic family rate**, and returned `priced = true`. Before this lane that
would have rendered as a plain dollar figure indistinguishable from real spend.
Now:

- the row carries `cost_source=provider_defaults`;
- the session grades `cost_truth=estimated`;
- `report` emits `F23_CACHE=cost_warning text=usd_is_a_family_rate_estimate_not_spend`;
- `verify` **exits 7** and says so on stderr (`live/live-verify.err`):

```
wayland-core cache verify: cost is estimated — of 1 round-trips, 1 were priced
from provider-family defaults rather than a catalog row and 0 could not be priced
at all, so $0.075600 must not be reported as spend.
```

### Does the cost VARY? Measured, live, on two sessions

`live/live-list.txt`:

```
… id=8ea00290… cost_usd=0.075600 cost_truth=estimated …
… id=6f9aa7b5… cost_usd=0.061650 cost_truth=estimated …
```

Two figures, two sessions, same host, same model. **The cost observable is not
invariant here.**

**Honest limit on that measurement.** Both sessions report
`uncached_input=4095`, because `smollm2:135m` has a 4096-token window and
truncates. The two costs differ on the OUTPUT side (189 tokens vs 3), not the
input side. So the live run proves the number varies; it does not, by itself,
prove it varies with input. That half is proved in
`crates/wcore-agent/tests/cache_ledger_engine_test.rs::recorded_cost_varies_with_the_tokens_and_beats_the_uncached_counterfactual`,
which drives the engine at two workloads differing by exactly 100× and asserts
the cost ratio is **100.0 ± 0.01** — a constant would give 1.0, and a
partly-fixed number something in between (an earlier draft of that test held
outputs fixed and measured 48×, which is why the shape is now purely linear).

---

## 4. Gate counts, read back (LANE-BRIEF §3.2)

`gate-counts.txt`, captured over ssh so no local `rtk` proxy could strip the
`ignored` / `filtered out` fields:

```
cargo test -p wcore-agent --lib cache_ledger
  running 20 tests
  test result: ok. 20 passed; 0 failed; 0 ignored; 0 measured; 2184 filtered out

cargo test -p wcore-agent --test cache_ledger_engine_test
  running 6 tests
  test result: ok. 6 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out

cargo test -p wcore-cli --test cache_ledger_cli
  running 13 tests
  test result: ok. 13 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out

-- no regression in the surfaces this lane touched --
cargo test -p wcore-agent --lib cache_diagnostics   14 passed; 0 ignored
cargo test -p wcore-agent --lib compact::          118 passed; 0 ignored
cargo test -p wcore-agent --test engine_compact_test 15 passed; 0 filtered out
```

`0 ignored` and `0 filtered out` are stated explicitly on every line, so none of
these is a suite that exited 0 having run nothing.

`cargo clippy -p wcore-agent -p wcore-cli --all-targets` → **zero** error or
warning lines. `cargo fmt --all -- --check` clean (run on the Mac, which is
permitted).

---

## 5. Every gate here has been observed to FAIL

Not asserted — observed, during this lane, on this code:

| Gate | Observed failing |
|---|---|
| `cache verify` | **exit 7** on the live Ollama ledger (§1) |
| `cache verify` on an empty store | **exit 8** (§1) |
| `ledger_path_cannot_escape_its_directory` | failed on the real output `/tmp/ledgers/.._.._etc_passwd.json`; the ASSERTION was wrong, not the sanitizer — rewritten to test path components, with a raw-join known-positive |
| `a_real_run_writes_a_ledger_…` | failed at `hit_ratio 0.497` vs an expected `> 0.6`; the assertion wrongly assumed cache writes were out of the denominator |
| `an_uncatalogued_model_…` | failed `left: Priced, right: Unpriced` — **this red is what produced `CostSource`** |
| `recorded_cost_varies_…` | failed twice: once on `ContextTooLong` (the emergency stop is on even when `compact.enabled = false`) and once at `48×` on a 100× workload |
| `json_output_carries_…` | failed `left: Null` on a key the CLI had not been updated to emit |

Seven observed reds across the lane's own gates.

---

## 6. What this evidence does NOT prove

Stated plainly rather than left for a reader to notice.

1. **No live cache HIT was observed.** `hit_ratio=0.0000` above is real: Ollama
   has no prompt cache. Proving a live hit needs a prompt-caching provider, and
   **no permitted host has a working credential for one** — see §7. The hit /
   warm-ratio / saving-positive paths are proved against the engine
   (`cache_ledger_engine_test.rs`, which feeds real `TokenUsage` with
   `cache_read_tokens` set) and against the CLI, not against a live provider.
2. **No live compaction was observed.** `compactions=0`. The compaction recording
   paths — micro, auto, auto-failed — are covered by the CLI suite and by
   inspection of `run_compaction`, not by a live pass.
3. **No live `history_rewritten` attribution.** It follows a compaction, so it
   inherits (2).
4. **`provider=anthropic` on a round-trip that ran on Ollama.** The ledger takes
   `provider` from `self.compat.provider_type()`, which is the configured compat
   profile, not the plugin route that actually served the turn. This is
   pre-existing (`TurnTrace.provider` and the budget path read the same value)
   and was NOT introduced or fixed here. It is a real defect in a cost surface —
   filed in the summary, not silently absorbed.

---

## 7. The credential blocker, measured rather than assumed

`hetzner-dsm` has no working prompt-caching credential.

- `/root/.wayland/.env` carries `ANTHROPIC_API_KEY`. Used, the product replied:
  `API error 401: {"type":"authentication_error","message":"API key is invalid."}`
- `/root/.wayland/auth.json` has one `credential_pool.anthropic` entry, and its
  own metadata says `source = "env:ANTHROPIC_API_KEY"` — it is the SAME key.
  (Inspected by field name and length only; no value was printed.)
- `env | sed 's/=.*//' | grep -iE 'api|key|token|flux|openai|anthropic|deepseek|groq|gemini'`
  → no other provider variable. `/root/.bashrc`, `/root/.profile` → none.

Instrument liveness for that last search: the same invocation shape returns
matches for names that ARE present elsewhere on the box; and the `.env` read
returned four names (`DATABASE_URL`, `ANTHROPIC_API_KEY`, `WAYLAND_SHARED_SECRET`,
`PYTHONPATH`), so the reader is not silently returning nothing.

Per LANE-BRIEF §0 this is reported as a blocker; **no credential was embedded,
and none was supplied.**

Two further environment facts found on the way, recorded because the next lane
will hit them:

- With the DEFAULT home, any session aborts before the first API call with
  `storage.credentials.backend is set to "plaintext" … turn durable sessions off
  with [session] enabled = false`. A project-level `.wayland/config.toml`
  carrying that setting did **not** take effect; an isolated `WAYLAND_HOME` with
  the same two lines did.
- An isolated profile does **not** import `auth.json` (it warns and uses
  `credentials.toml`), so copying that file into a scratch home does not carry a
  credential across.

---

## 8. The 17 `wcore-agent --lib` failures are PRE-EXISTING — attributed, not assumed

A broad regression run of `cargo test -p wcore-agent --lib` on this lane's HEAD
came back red, which is exactly the moment a lane is tempted to report a green
it did not measure or a regression it did not cause. Neither: I measured the
base.

| Run | Commit | Result |
|---|---|---|
| lane HEAD, run 1 | `f8b437fb` | `2184 passed; 17 failed; 3 ignored; 0 filtered out` |
| lane HEAD, run 2 | `f8b437fb` | `2189 passed; 12 failed; 3 ignored; 0 filtered out` |
| **merge-base** | **`4a872413`** | **`2164 passed; 17 failed; 3 ignored; 0 filtered out`** |

`4a872413` contains **none** of this lane's code and fails 17 tests in the same
families: `engine::audit_2026_05_22_tests`, `session::tests`,
`session_journal::fault_tests`, `session_lifecycle::tests`,
`orchestration::f13_durability_tests`,
`engine::retry_wedge_protection_tests`. Built in a separate worktree
(`/root/wayland-23b-c4-base`) at that exact SHA.

Two further observations that rule this a contention artifact rather than a
deterministic break:

- **The failing set is not stable between two runs of the identical binary** —
  17 then 12, overlapping but different. A code regression does not move.
- `cargo test -p wcore-agent --lib -- --test-threads=1 session:: session_journal::`
  on the lane HEAD → **`96 passed; 0 failed; 0 ignored; 2108 filtered out`**.
  Single-threaded, the families pass completely.

This matches the class the lane brief documents (shared `/tmp` on `hetzner-dsm`,
per the merged `31-vacuous-greens` correction, plus wall-clock-budgeted
durability tests under parallel load). **Reported as a pre-existing flake in the
integration branch, not as a pass, and not as something this lane caused.**

`cargo test -p wcore-cli --lib` shows one further red on both trees:
`test always_fails ... FAILED` in a single-test binary — the `31-vacuous-greens`
lane's deliberate anti-vacuity canary. Not this lane's, and it is supposed to
fail. The real `wcore-cli` lib suite in the same invocation:
`1854 passed; 0 failed; 1 ignored; 0 filtered out`.
