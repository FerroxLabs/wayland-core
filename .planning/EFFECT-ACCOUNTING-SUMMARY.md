# EFFECT-ACCOUNTING — lane summary

**Branch** `lane/effect-accounting` · **base** `plan/f20-unified-audit-repair` @ `c9ab048b`
**Instrument** `wayland-core 0.12.25`, debug, built on `hetzner-dsm` from this branch, `BUILDRC=0`
**Evidence** `.planning/EFFECT-ACCOUNTING-NOTES.md` + `.planning/evidence/effect-accounting/`

The brief asked me to treat two cross-audit-panel claims about the 2026-07-30 headless keyring
degrade (`d51287b1`) as hypotheses and measure them. **Both are substantially FALSE as written,
and measurement found a different, larger problem that the degrade does not cause.**

---

## Verdict table

| Claim | As stated | Measured |
|---|---|---|
| A — budget spend is unjournaled, so the ceiling is unenforceable across restarts | attributed to the degrade | **FALSE that the journal causes it.** Identical spend in BOTH arms. |
| A — "every restart is a fresh budget", compounding | TRUE, wrong cause | **TRUE.** Cause is that the only operator-reachable ceiling is *per session*. |
| A(Gemini) — "zero proof of what the agent did" | | **FALSE.** A per-session cost ledger is written by default, in the degraded arm too. |
| B — a mid-approval crash loses the human's answer | | **TRUE.** |
| B — the user is re-asked and may re-approve believing it is the first ask | | **FALSE.** The degraded restart refuses to open the session at all. |
| B — human-mediated replay of a destructive approval | | **DOES NOT REPRODUCE.** No path re-applies a prior grant. |

---

## A — what actually happens to budget spend across a restart

Two arms differing only in `WAYLAND_VAULT_PASSPHRASE`; the `off` arm additionally has no session
bus, so `Config::resolve` degrades durable sessions off. Arm control asserted in **both**
directions per launch (degrade notice present 5/5 in `off`, 0/5 in `on`). Spend is metered by the
loopback provider's own log, never by the product's stdout. Cap `[budget] max_tokens_in = 25000`;
each round-trip bills 20000 input tokens.

**Continued session** (`--session-id`, then `--resume`), 5 launches:

| arm | result |
|---|---|
| `on` | L1 billed 20000; **L2–L5 refused**: `budget cap 'per_session_input_tokens' would be exceeded (limit 25000 input tokens, reserved total 32082 input tokens)` |
| `off` | L1 billed 20000; **L2–L5 rc=1** `Error: Session 'aaaaaa-000001' not found` |

The journal **does** enforce the ceiling across a process restart. Degraded, continuation is
impossible and the refusal is loud — it does not silently re-arm anything.

**Fresh session per process** (no `--resume`), 5 launches:

| arm | round-trips | input tokens billed | cap | refusals |
|---|---|---|---|---|
| `on-fresh` | 5 | **100000** | 25000 | **0** |
| `off-fresh` | 5 | **100000** | 25000 | **0** |

Identical. **A new session re-arms the full ceiling whether or not durable sessions are on.**

### The real gap (HIGH, pre-existing, NOT caused by the degrade)

The only operator-configurable provider ceiling is **per session**. The tracker's second cap,
`per_user_daily_usd`, is unreachable from configuration — `wcore-budget/src/tracker.rs:55` says so
in terms: *"has no TOML counterpart today — set it manually"*. There is therefore **no
cross-session, per-day or per-account spend bound at all**, and a crash-looping daemon bills
without limit while every individual run is correctly inside its ceiling. Measured in dollars on
a catalog-priced model: 3 fresh sessions, 60000 input tokens, `cost_usd=0.009180`, **0 refusals**
under a 25000-token cap.

**Not fixed here, deliberately** — see the fix section.

---

## B — what actually happens to a pending approval

Drove the real `wayland-core --json-stream` engine to a genuine `approval_required` on a
destructive `Bash` call (`rm -rf …`), then **SIGKILLed** the process with the human's answer
outstanding, then relaunched with `--resume`. Participant assertion (§6a-i): a run counts only if
the mock logged a round-trip AND the engine emitted `approval_required` — **both arms passed,
`UNRUN_CELLS=0 of 2`**.

| arm | after SIGKILL + `--resume` |
|---|---|
| `on` | `ready`; session resumes, `execution_policy.reason="resume"`, contract advertises `turn_recovery_v1: available` |
| `off` | `error` — `Error: Session 'dddddd-000004' not found` |

The pending question does not survive the degrade. But it is **not silently re-asked**: the
degraded restart refuses to open the session, and the product says why twice — the degrade notice
states *"an interrupted turn cannot be recovered"*, and a separate `warning: previous run did not
shut down cleanly (crash sentinel found at …/.dirty-death.<pid>)` fires in both arms.

And **no mechanism re-applies a prior "yes"**: `recovery.rs:1074-1083` classifies a pending
approval `AwaitApproval` / `ApprovalExpired`; `session_lifecycle.rs:651-669` returns
`RefusedApprovalExpired` for any approval whose tool effect still `requires_reconciliation()`; the
one path carrying a recorded approval forward (`RetryOutcome::Admitted { reapproved }`) demands a
terminal reconciled effect, **forks** rather than overwrites, and is unreachable with no journal.
The panel's "approved twice" needs the human to re-issue the request in a new conversation and
approve it afresh — a first ask for a new request, not a replayed grant.

---

## What I fixed

**`crates/wcore-cli/src/cache_cmd.rs` — `cache list` now emits one `F23_CACHE=total` line.**
Sessions, incomplete sessions (the signature a crash loop leaves), round-trips, input and output
tokens, summed USD and uncached-equivalent USD, and the aggregate cost-truth grade re-derived
through the same four-way rule a single session uses. A non-priced total carries its own warning
line, because summing many sessions makes a floor look *more* authoritative, not less.

Why this and not a cross-session ceiling: cross-audit panel **codex `a`, kimi `a`, gemini `c`** —
majority for the aggregate, and gemini's own answer conceded that enforcement read from a
cache-diagnostics store resets when an operator clears the cache. Both `a` legs independently
made the failure-shape argument: fail-open silently restores the hole, fail-closed bricks every
launch on a pruned or partially-written ledger. This is **observability, not enforcement**, and
the summary says so in the code.

Live proof, both directions:

```
sessions=5 round_trips=5 input_tokens=100000 output_tokens=500 cost_truth=unpriced   # degraded arm
sessions=5 round_trips=5 input_tokens=100000 output_tokens=500 cost_truth=unpriced   # journalled arm (identical)
sessions=0 round_trips=0 cost_usd=0.000000  cost_truth=unpriced                      # empty store is NOT a trustworthy zero
sessions=3 round_trips=3 input_tokens=60000 cost_usd=0.009180 cost_truth=priced      # catalog-priced model
```

`60000 × $0.15/1M + 300 × $0.60/1M = $0.00918` reproduces the printed USD exactly, and the
loopback's independent meter logged exactly 3 round-trips.

---

## Gates

| Gate | Result |
|---|---|
| `cargo fmt --all -- --check` (Mac) | **0** |
| `cargo metadata --locked` | **0** |
| `cargo clippy -p wcore-cli --all-targets -- -D warnings` | **0** |
| `cargo check --workspace --all-targets` | **0** |
| `cargo test -p wcore-cli --lib` | **0** — `1923 passed; 0 failed; 1 ignored; 0 measured; 0 filtered out` |
| new tests executed | 4/4 named and `ok` (`cache_cmd::store_total_tests::*`) |
| `cargo test -p wcore-cli` (all targets) | **101** — one pre-existing failure, see below |

`cargo clippy` first came back **101** on my own 9-argument test helper. Restructured into a typed
builder rather than `#[allow]`ed; the builder also derives `round_trips` from the pricing
breakdown so a test cannot construct a ledger the recorder could never emit.

### The one red, and why it is not mine

`wcore-cli --test f14_sigkill_recovery`: `10 passed; 1 failed; 1 ignored`. The failure is
`isolated_profile_without_secure_store_fails_before_turn_or_provider_intent` — it asserts Core
opens **no** session without a secure store, which the 2026-07-30 degrade deliberately changed.
`crates/wcore-cli/tests/f14_sigkill_recovery.rs` is **FENCED to `lane/durable-posture`**, so I did
not touch it.

**Ablation control, not an assertion:** I restored `cache_cmd.rs` to base with
`git checkout c9ab048b -- crates/wcore-cli/src/cache_cmd.rs` (permitted — one named path, moves no
ref), re-ran that test binary, and it failed identically: `10 passed; 1 failed; 1 ignored`,
`ABLATION_RC=101`. Then restored my version. My change is confined to that one file, so this is an
exact ablation. **Handoff to `lane/durable-posture`: this fenced test needs updating to the new
degrade behaviour.**

Also in the log, and NOT a failure: `test always_fails … FAILED / panicked … "deliberate"` from a
`failing_fixture` crate. That is a **nested cargo run's output leaking into the parent stream** —
its parent, `plugin::scaffold::tests::plugin_test_propagates_a_failing_suite`, passed, and
`cargo test -p wcore-cli --lib` exited **0** with the real lib result of 1923 passed on the same
stream. Read the whole log before grading this one.

---

## Instrument defects found in my own harness, and repaired (§6b-ii)

1. **`grep -c … || echo 0` emits `0\n0`.** `grep -c` exits 1 on zero matches, so the fallback fires
   *as well as* grep's own `0`. The arithmetic then aborted the launch loop after L1 and the first
   run read as a product failure. Repaired to `grep | wc -l`, with a meter self-test carrying
   three assertions: known-positive, known-negative, and **arithmetic-usable** — the third is the
   only one the broken version would have failed.
2. **Cumulative counted as per-arm.** The second arm's spend was reported as 200000 when it was
   100000, by multiplying the whole-script round-trip count. Repaired to a per-arm delta and the
   whole experiment re-run; every figure above is from the repaired run.

## Product defects found in passing

- **Setting `max_cost_usd` at all on an unpriced model refuses every call** — *"pricing is
  unavailable for openai/mock-model, so the explicit or managed USD cap cannot be enforced…
  remove the explicit max_cost_usd to use token-only governance"* — and the process still exits
  **rc=0** having completed 0 turns.
- **The emitted `approval_required` carries `resume_token: ""`.**
  `docs/json-stream-protocol.md` §1.N+4 documents `resume_token` as the field a host MUST echo
  back to route the decision; the live value lives in `correlation_id` instead (the Wave-SC
  correlation-id model). Documented shape and emitted shape disagree. → BACKLOG.
- **A degraded run given `--session-id` leaves an orphan `<id>.journal` and
  `<id>.journal.writer.lock`** in `sessions/` with no index entry. I hypothesised this poisons a
  later vault-unlocked run and **REFUTED it**: the `poison` arm ran degraded → vault-unlocked →
  `--resume` on the same home and id, and all three behaved, with the resume correctly refused at
  `reserved total 32084 > limit 25000`.

## Unrun cells — counted, not skipped

1. **The channel / gateway restart path was NOT driven live.** `channel_dispatch.rs:223-247`
   resumes by a stable hashed conversation id via `load_for_run_if_exists`, and *silently creates
   a fresh session* when the store is empty rather than erroring as the CLI does. Degraded, the
   store IS always empty (measured: `off-fresh session_dir_entries=0`). So this is the one place
   the ceiling may re-arm **silently** for the same logical conversation. Driving it needs a real
   channel platform. **Reported as a hypothesis with its call sites, not as a finding.**
2. **Windows and macOS not exercised.** Linux only. The change is arithmetic and `println!` with
   no platform branch, and `cargo check --workspace --all-targets` is Linux-only here.
3. **`uncached_equivalent_usd` in the total was only proven equal to `cost_usd`** — the loopback
   never exercises a cache read or write, so a non-zero cache saving in the aggregate is covered
   by unit test only, not live.

## Not done

No PR, no merge, no tag, no issue closed, no `wcore-contract generate`, no rebase, no
`git reset --hard`, no `git stash`, no `git clean`. No fenced file touched
(`wcore-config/src/config.rs`, `wcore-config/src/credentials.rs`, `wcore-agent/src/engine.rs`,
`wcore-cli/tests/f14_sigkill_recovery.rs`). No shared-file edit — I did not touch
`wcore-cli/src/lib.rs` or `main.rs`. No credential used or transmitted; the only key-shaped values
in this lane are the synthetic literals `effect-accounting-lane-not-a-secret` and
`effacc-throwaway-not-a-secret`, which authenticate nothing.
