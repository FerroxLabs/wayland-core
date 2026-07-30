# EFFECT-ACCOUNTING — running notes

Lane `lane/effect-accounting`. Worktree
`/Users/seandonahoe/dev/waylandcore-frontier-worktrees/lane-effect-accounting`.
Base integration `c9ab048b952c5bc74c75ea8f76df06788408de59` (asserted with `/usr/bin/git rev-parse`).

Brief asks two questions about state lost when durable sessions degrade OFF (the
2026-07-30 keyring change, `d51287b1` + `551d9001`):

- **A — budget spend is unjournaled**, so a restart re-arms the ceiling and every
  restart is a fresh budget. Compounding: a crash-looping daemon bills forever.
- **B — approval state evaporates**, so a mid-approval crash loses the human's
  answer and the user may be re-asked and re-approve a destructive op.

Both are PANEL CLAIMS, not measurements. Measure first.

## Log

### t0 — orientation (source reading only, nothing measured yet)

Commits that created the degrade path:

- `d51287b1 fix(config): degrade durable sessions at startup when no secure store exists`
  — touches `wcore-agent/src/engine.rs`, `wcore-config/src/config.rs`,
  `wcore-config/src/credentials.rs`, `wcore-agent/tests/headless_durable_session_test.rs`.
- `551d9001 feat(config): report WHY durable sessions are off` — `wcore-config/src/config.rs`,
  adds a degrade-reason seam with no consumer yet.

**All four of those files except the test are FENCED to `lane/durable-posture`.** I must not
edit `crates/wcore-config/src/config.rs`, `crates/wcore-config/src/credentials.rs`,
`crates/wcore-agent/src/engine.rs`, `crates/wcore-cli/tests/f14_sigkill_recovery.rs`.

Structural reading relevant to A (`crates/wcore-agent/src/budget_authority.rs`):

- `BudgetAuthorityCoordinator::bind` (line ~159): when `config.journal` is `None` it returns
  early with `provider_tracker: BudgetTracker::new(config.provider_caps)`,
  `execution_root: config.execution_policy.start_root()`, `authority_epoch: 0`,
  `journal: None`. i.e. **full caps, zero spend, no durable epoch.**
- `is_durably_bound()` = `journal.is_some() && authority_epoch > 0`.
- `engine.rs:4014` `durable_budget_authority()` refuses only when
  `self.session_manager.is_some() && !is_durably_bound()`. With sessions degraded off there
  is no session manager, so **the refusal does not fire** and the unjournaled authority is
  used normally.

That is consistent with claim A but is NOT yet a measurement — it does not tell me whether
some *other* surface persists spend (candidates seen in the tree:
`wcore-agent/src/cache_ledger.rs`, `wcore-gateway/src/ledger.rs`, `goal/ledger.rs`), nor what
the product actually reports to the operator.

Still to establish:
1. Whether any cost/spend record survives a restart by another path (control: search for the
   CONCEPT, not the word `budget`).
2. What the product prints/enforces across a real kill+restart with a low ceiling.
3. B: where a pending approval lives, and what a mid-approval kill does to it.

### t1 — A MEASURED LIVE. Claim A is substantially FALSE as written.

Binary: `wayland-core 0.12.25`, debug, built on `hetzner-dsm` from `8548e834`, `BUILDRC=0`,
`/root/wayland-effect-accounting/target/debug/wayland-core`.
Harness: `.planning/evidence/effect-accounting/{mock_provider.py,run-budget.sh,run-fresh.sh}`.
Meter: the loopback provider's OWN log (one `BILLED` line per round-trip, carrying the usage it
will report) — never the product's stdout. Meter self-test asserts three things, including that
the returned value is arithmetic-usable (see harness defect below).

Config under test: `[budget] max_tokens_in = 25000`; mock bills 20000 input tokens per
round-trip. Two arms differ ONLY in whether `WAYLAND_VAULT_PASSPHRASE` is supplied; the `off`
arm additionally has no session bus, so `Config::resolve` degrades durable sessions off. Arm
control asserted BOTH directions per launch: the degrade notice must appear in `off`
(5/5 launches) and must NOT appear in `on` (0/5).

**Continued session (`--session-id` then `--resume`), 5 launches each:**

| arm | L1 | L2..L5 | round-trips | verdict |
|---|---|---|---|---|
| `on` (journal) | ok, 20000 billed | **budget refusal**, 0 billed | 1 | ceiling ENFORCED across restart |
| `off` (degraded) | ok, 20000 billed | `Error: Session 'aaaaaa-000001' not found`, rc=1 | 1 | continuation IMPOSSIBLE, loud |

`on`-L2 verbatim: `error: Provider call not started: budget cap 'per_session_input_tokens'
would be exceeded (limit 25000 input tokens, reserved total 32082 input tokens).`

So the `off` arm does **not** silently re-arm the ceiling on this path. It refuses with rc=1.

**Fresh session per process (no `--resume`), 5 launches each:**

| arm | round-trips | input tokens billed | cap | refusals |
|---|---|---|---|---|
| `on-fresh` (journal) | 5 | **100000** | 25000 | **0** |
| `off-fresh` (degraded) | 5 | **100000** | 25000 | **0** |

**Identical.** A new session gets a full fresh ceiling *whether or not durable sessions are on.*
So "every restart is a fresh budget" is TRUE, but the journal is not what causes it — the
configured ceiling is `per_session_input_tokens`, i.e. per session by construction. The only
other ceiling in the tracker is `per_user_daily_usd`, and `wcore-budget/src/tracker.rs:55` says
in terms: *"has no TOML counterpart today — set it manually"*. **There is no operator-reachable
cross-session, per-day or per-account ceiling at all.** That, not the journal, is the
compounding surface.

**Gemini's half of the claim ("zero proof of what the agent did") is FALSE.** The cache/cost
ledger is on by DEFAULT (`cache_ledger.rs:779 recording_enabled()`, env opt-OUT only), flushes
after every round-trip, and is written in the degraded arm too: `ledger_files=5` in BOTH
`on-fresh` and `off-fresh`. Sample from the degraded arm carries
`"uncached_input_tokens": 20000, "output_tokens": 100, "cost_usd": 0.0,
"cost_source": "unpriced"` (0.0 because `mock-model` is unpriced, which the product states
explicitly rather than passing off as free).

**Two side findings, both measured:**
- Setting `max_cost_usd` at all on an unpriced model refuses EVERY call:
  *"pricing is unavailable for openai/mock-model, so the explicit or managed USD cap cannot be
  enforced… remove the explicit max_cost_usd to use token-only governance."* rc was **0** with
  0 turns. (First harness run, `on-L1.stderr`.)
- A degraded run given `--session-id` leaves an orphan `<id>.journal` and
  `<id>.journal.writer.lock` in `sessions/` with no index entry. **Hypothesis that this poisons
  a later vault-unlocked run: REFUTED.** The `poison` arm ran degraded, then vault-unlocked on
  the same home+id, then `--resume`: all three behaved, and the resume was correctly refused at
  `reserved total 32084 > limit 25000`.

### t2 — B MEASURED LIVE. The panel's mechanism does NOT reproduce.

Harness: `.planning/evidence/effect-accounting/{mock_tool_provider.py,run-approval.py}`.
Drives the real `wayland-core --json-stream` engine. The loopback provider always answers with
a destructive `Bash` tool call (`rm -rf /tmp/effacc-destructive-target`), so the default
`approvals = "prompt"` posture must park the turn. The instant `approval_required` arrives the
process is **SIGKILLed** — the human has not answered and now cannot. A second process then
launches with `--resume <same id>`.

Participant assertion (§6a-i): a run counts only if the mock logged a round-trip AND the engine
actually emitted `approval_required`. **Both arms: reached_provider=True, parked_on_approval=True,
`UNRUN_CELLS=0 of 2`.** The frame that was interrupted, verbatim:

```
{"type":"approval_required","call_id":"call_effacc_0001","resume_token":"",
 "correlation_id":"call_effacc_0001","reason":"exec",
 "context":"Execute: rm -rf /tmp/effacc-destructive-target"}
```

| arm | degrade notice run1/run2 | events after SIGKILL + `--resume` |
|---|---|---|
| `on` (journal) | no / no | `ready` — session resumes, `execution_policy.reason="resume"`, contract advertises `turn_recovery_v1: available` |
| `off` (degraded) | yes / yes | `error` — `Error: Session 'dddddd-000004' not found` |

**The pending question does not survive the degrade — that half of B is TRUE.** Everything
built on top of it is not:

- It is **not silently re-asked.** The degraded restart refuses outright, and says why twice:
  the degrade notice states in terms *"an interrupted turn cannot be recovered"*, and a separate
  `warning: previous run did not shut down cleanly (crash sentinel found at …/.dirty-death.<pid>)`
  fires in both arms. A user cannot be re-asked by a session that will not open.
- **No mechanism re-applies a prior "yes".** Journal-backed recovery classifies a pending
  approval as `RecoveryDisposition::AwaitApproval` with
  `RecoveryReconcileReason::ApprovalExpired` (`recovery.rs:1074-1083`, `1366-1377`), and
  `session_lifecycle::retry` returns `RefusedApprovalExpired` for any approval whose tool effect
  still `requires_reconciliation()` (`session_lifecycle.rs:651-669`). The single path that
  carries a recorded approval forward, `RetryOutcome::Admitted { reapproved }`, requires a
  terminal reconciled effect and **forks** rather than overwriting — and is unreachable with no
  journal, because there is no journal to read.

So B's danger — *"a destructive operation approved twice"* — needs the human to re-issue the
request in a NEW conversation and approve it afresh. That is a first ask for a new request, not
a replayed grant. **Reported as not reproducing.**

Side finding, out of lane scope → BACKLOG: the emitted `approval_required` carries
`resume_token: ""` while `docs/json-stream-protocol.md` §1.N+4 documents it as the field the host
MUST echo back to route the decision. The value lives in `correlation_id` instead (the Wave-SC
correlation-id model). Documented shape and emitted shape disagree.

### t3 — the visibility gap, measured on the product's own surfaces

`cache report` reports **one** session (the most recently updated). `cache list` prints one row
per session plus `F23_CACHE=list sessions=5 dir=…` — **and no totals of any kind**: no summed
USD, no summed tokens, no aggregate cost-truth grade. Measured against the degraded arm's home,
which holds 5 ledgers for 5 restart-fragments of the same work.

So the durable spend record exists and the *sum* does not. That is the precise shape of "every
individual run believes it is within budget".

**Harness defect found and repaired in-lane (§6b-ii).** `grep -c` exits 1 on zero matches, so
`grep -c … || echo 0` emitted `0\n0`; the arithmetic aborted the launch loop after L1 and the
first run looked like a product failure. Repaired to `grep | wc -l`, plus a meter self-test with
three assertions (known-positive, known-negative, and *arithmetic-usable* — the third is the only
one the broken version would have failed). A second defect in the same script reported arm two's
spend as 200000 by multiplying the *cumulative* round-trip count; repaired to a per-arm delta and
re-run. Both figures above are from the repaired run.

### t4 — the fix, cross-audited and live-proven in both directions

Panel on *"what should this lane build — (a) a cross-session aggregate view, (b) a cross-session
ceiling read from the ledger, (c) both, (d) nothing"*: **codex `a`, kimi `a`, gemini `c`.**
Majority `a`, and the minority conceded the coupling risk in its own answer ("introduces a
structural impurity… if an operator clears their cache, the daily budget resets"). Both `a` legs
made the same argument independently: enforcement on a best-effort diagnostics store has two
failure shapes and both are bad — fail-open silently restores the hole, fail-closed bricks every
launch on a pruned or partially-written file. Internal adversarial pass argued *"an aggregate
nobody runs is documentation, not a fix"*; answered by not claiming A fixed — the enforcement gap
is reported below as an open HIGH with a named home, and the sum is the thing any future
enforcement must first be able to compute correctly.

Built: `crates/wcore-cli/src/cache_cmd.rs` — `cache list` now emits `F23_CACHE=total`.

Live, on the binary built from this branch:

```
# PASS direction — the degraded arm's five restart-fragments
F23_CACHE=total sessions=5 incomplete_sessions=0 round_trips=5 input_tokens=100000
  output_tokens=500 cost_usd=0.000000 cost_truth=unpriced … unpriced_round_trips=5
F23_CACHE=total_cost_warning text=total_usd_is_a_floor_not_spend cost_truth=unpriced

# CONTROL — the journalled arm, which spent the same: byte-identical figures.

# FAIL direction — an empty store must not read as a trustworthy zero
F23_CACHE=total sessions=0 … cost_usd=0.000000 cost_truth=unpriced

# USD axis on a CATALOG-PRICED model (gpt-4o-mini against the same loopback)
F23_CACHE=total sessions=3 round_trips=3 input_tokens=60000 output_tokens=300
  cost_usd=0.009180 cost_truth=priced catalog_priced_round_trips=3
```

Two independent cross-checks on that last line: the loopback's own meter logged exactly 3
round-trips, and `60000 × $0.15/1M + 300 × $0.60/1M = $0.00918` reproduces the printed figure
exactly. Three fresh sessions, 60000 input tokens, **0 refusals against a 25000-token cap** — the
compounding shape, in dollars, on a real price.
