# 24-C1-abandoned — running notes (append-only, committed per §6b-i)

Lane `lane/24-c1-abandoned`. Base asserted: `f8b8ec25372fb4ed4280a5aa365873ae8465abfc`
(== `git ls-remote gh plan/f20-unified-audit-repair`).

## T0 — the brief's premise is PARTLY STALE. Measured before building.

The work order restates four claims from `24-C1-IDEMPOTENCY-SUMMARY.md` §3. Between that
summary and my base, a **later lane (`lane/24-abandon-surface`) already landed a fix** —
commit `c74dd4bd feat(24-C1): make an abandoned delivery nameable by an operator`, written
up in `24-C1-ABANDON-SURFACE.md`. So the claims must be re-graded against current code, and
three of the four have moved.

Re-verified against `crates/wcore-gateway/src/ledger.rs` at base:

| §3 claim | Still true at base? | Evidence |
|---|---|---|
| `pending()` filters to `Accepted\|Attempted`, excluding `Abandoned` | **TRUE — and now deliberate/documented** | `ledger.rs:392-398`; doc at `:388-391` says it "stays excluded" because re-adding it would re-dispatch a delivery the product chose not to send. Correct as policy; it is no longer the *only* read path. |
| `pending_count()` same filter, doc "the number drain publishes" | **TRUE, unchanged** | `ledger.rs:400-406` |
| `compact()` classes `Abandoned` as terminal history under the `retain_settled` bound → compactable out | **FALSE NOW** | `ledger.rs:432-465`: three separate budgets. `abandoned` has its own cap `ABANDON_RETENTION = 10_000` (`:187`), settled traffic can no longer evict an abandonment, and overflow is counted into `dropped_abandonments()` + `tracing::warn!`. |
| `DeliveryState::Abandoned` has no consumer outside `ledger.rs` | **FALSE NOW** | `wcore-cli/src/gateway.rs:642` (`ledger.abandoned()`), `wcore-cli/src/gateway/support.rs:214` (`abandoned_count()`), plus `automation.rs` / `drain.rs` writers. Instrument known-positive: 36 `Abandoned` hits outside `ledger.rs` — non-zero, so the grep is alive. |

## T0 — so what is ACTUALLY still open (the delta I own)

Work order asks for three things. Status at base:

1. **List abandoned deliveries on the operator surface — ALREADY DONE.**
   `wayland-core gateway abandoned [--json]`, `gateway.rs:126-143` (clap) + `:637-706` (impl).
   Reads the journal from disk, not a live gateway. Prior lane live-proved it against a real
   `DrainBudgetExpired` abandonment. **I must still DRIVE it myself** (work order: "must be
   driven, not merely implemented") and I have not yet.
2. **Exempt from compaction until acknowledged — PARTIALLY DONE, the "until acknowledged"
   half is ABSENT.** Separate budget exists; an *acknowledge* concept does not.
   Concept search (multi-vocabulary, unproxied `/usr/bin/grep`, over `wcore-gateway/src` +
   `wcore-cli/src/gateway*`): `resend|re_send|re-send|requeue|re_queue|acknowledge|acknowledged|ack`
   → **4 hits, all prose in doc comments, zero implementation**
   (`ledger.rs:105`, `automation.rs:176`, `automation.rs:379`, `gateway.rs:129`).
   Known-positive for the same instrument in the same tree: `grep -c bandoned gateway.rs` = 14.
3. **Re-send path — ABSENT.** Same search as above; there is no verb that re-dispatches an
   abandoned delivery.

Matrix (§2 of work order) and Discord (§3) were **both already fixed by
`lane/24-abandon-surface` Task 2**, and the Matrix silent-drop hypothesis was **measured real
against a live Synapse and graded HIGH** (`24-C1-ABANDON-SURFACE.md` §"Task 3"). Discord's
**dedup window was NOT measured** — that lane explicitly left it as residual risk. So my
Matrix/Discord delta is: re-verify the fixes are present at base, and close the Discord
measurement.

## Next steps (in order)
- [ ] Verify Matrix/Discord fixes present at base; confirm what the prior lane proved.
- [ ] Measure Discord's dedup window (the one thing nobody has measured).
- [ ] Build acknowledge + re-send; exempt unacknowledged abandonments from compaction.
- [ ] Drive the real `gateway abandoned` verb and capture its real output.
- [ ] Gates on hetzner, workspace-wide check, mutation proofs for every new test.
