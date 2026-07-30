# lane/fix-clippy-gate — NOTES

Base: `e7bc6d883027102ff1e5bbaa2dd19f9265268cab` (integration head at spawn).
hetzner worktree: `/root/wayland-fix-clippy-gate`, branch `hz/fix-clippy-gate`, SHA asserted
equal to the above.

## 1. Premise check — the brief was WRONG, and wrong in a specific, instructive way

The brief said: `cargo clippy --workspace --all-targets -- -D warnings` fails with
**"exactly one failing file, two errors"** — `crates/wcore-cron/tests/single_owner.rs`.
It also said two earlier lanes had reported
`crates/wcore-agent/tests/cache_ledger_engine_test.rs:82` (`needless_update`) and that it
"did NOT appear in my run", speculating it had been fixed by a merge.

Measured at the identical SHA. **The brief's run was a fail-fast run.**

- `cargo clippy --workspace --all-targets -- -D warnings` → `WLRC=101`, log 688 lines,
  ends with `warning: build failed, waiting for other jobs to finish...`.
  cargo **stops scheduling new units** at the first failing crate, so the crates after
  `wcore-cron` in the schedule were never linted at all.
- Re-run at the same SHA with `--keep-going` → `WLRC=101`, log 219 lines,
  **10 lint errors across 5 test targets in 4 crates.**

So the earlier lanes were right and the brief was wrong: `cache_ledger_engine_test.rs:82`
**is still present at `e7bc6d88`**. It was invisible to the brief's run because cargo
aborted before `wcore-agent`'s test targets were scheduled — not because a merge fixed it.

**Generalisable instrument defect, worth recording:** *a fail-fast clippy run is a LOWER
BOUND, not an inventory.* Anyone enumerating clippy debt must pass `--keep-going`, or they
will under-report and then wrongly conclude that another lane's finding was stale. This is
the same family as the `--no-fail-fast` nextest drift already documented in `ci.yml`
(one failure reported where there were three).

## 2. The complete, true list at `e7bc6d88` (from the `--keep-going` capture)

| # | Crate | Target | Lint | Site |
|---|-------|--------|------|------|
| 1 | wcore-cron | test `single_owner` | `clippy::zombie_processes` | `tests/single_owner.rs:505` |
| 2 | wcore-cron | test `single_owner` | `clippy::collapsible_if` | `tests/single_owner.rs:514` |
| 3 | wcore-memory | lib test | `non_snake_case` | `src/activation.rs:198` |
| 4 | wcore-browser | test `process_count_reaper_baseline_test` | `clippy::collapsible_if` | `tests/process_count_reaper_baseline_test.rs:99` |
| 5 | wcore-agent | test `cache_ledger_engine_test` | `clippy::needless_update` | `tests/cache_ledger_engine_test.rs:82` |
| 6-9 | wcore-agent | test `user_model_identity_wire` | `clippy::needless_borrows_for_generic_args` ×4 | `tests/user_model_identity_wire.rs:229,337,396,472` |

Nine lint errors, five `could not compile` summary lines, `WLRC=101`.

Raw captures: `evidence/base-clippy.log` (fail-fast), `evidence/kg-clippy.log` (`--keep-going`).

## 3. Is #1 a real defect or a lint nit? — REAL

`spawn_child` (`single_owner.rs:504`) spawns the test binary as a child, polls up to
300×50 ms for a `STARTED` marker, and:

- on success `return child` — every caller (`a`, `b`, `c`) later `.wait()`s it. Fine.
- on timeout **`panic!`s with `child` still live and never reaped**.

The leaked child is not inert: it holds the `ScheduleLease` on the schedule directory and
spins for up to 30 s waiting for a `.release` file that the panicking parent will never
write. Meanwhile the parent's `tempfile::tempdir()` is dropped and the tree removed, so the
child's own `std::fs::write(...).unwrap()` at `single_owner.rs:500` then panics into a
deleted directory. Net effect on a timeout: an orphaned process holding a lease on a path
that no longer exists.

That is exactly what `zombie_processes` exists to catch. **Do not `#[allow]` it.** Fix:
kill + reap before panicking.

## 4. Gate-gap finding — the brief's hypothesis is ALSO wrong

The brief suspected "CI runs a NARROWER scope than `--workspace --all-targets`".
It does not. `.github/workflows/ci.yml:602` runs the exact command:

    cargo clippy --workspace --all-targets -- -D warnings

and `ci.yml` fires on `push` to `plan/f20-unified-audit-repair` and `lane/**`
(`ci.yml:26,41`). So CI scope is CORRECT and CI *is* running on integration.
The gap is elsewhere — see §5, to be established.

## 5. Open / TODO

- [ ] Fix all 9 lints.
- [ ] Establish WHY 18 merges landed with CI clippy red — CI runs and is red; who reads it?
- [ ] Add clippy to the merge cadence, and prove the gate BOTH directions.
- [ ] Re-run full clippy + fmt + `metadata --locked` + `check --workspace --all-targets`.
- [ ] Full test suite for every crate touched: wcore-cron, wcore-memory, wcore-browser, wcore-agent.
