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
