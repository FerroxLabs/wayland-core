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

**Harness defect found and repaired in-lane (§6b-ii).** `grep -c` exits 1 on zero matches, so
`grep -c … || echo 0` emitted `0\n0`; the arithmetic aborted the launch loop after L1 and the
first run looked like a product failure. Repaired to `grep | wc -l`, plus a meter self-test with
three assertions (known-positive, known-negative, and *arithmetic-usable* — the third is the only
one the broken version would have failed). A second defect in the same script reported arm two's
spend as 200000 by multiplying the *cumulative* round-trip count; repaired to a per-arm delta and
re-run. Both figures above are from the repaired run.
