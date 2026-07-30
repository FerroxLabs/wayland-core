# 24-C3 LIVE — lane/discord-live NOTES (append-only, committed continuously)

Base SHA asserted: `43c69ca71bc788dcd925fc070204d6918c2d7e0f` (matches brief's `43c69ca7`).
Worktree: `/Users/seandonahoe/dev/waylandcore-frontier-worktrees/lane-discord-live`.

## Mission

Close the gap six lanes declined: **no message was ever sent or received against a live
Discord.** Prove 5 capabilities through the PRODUCT, corroborated by an independent observer.

1. send  2. edit  3. delete  4. receive (inbound, non-empty content)  5. **outbound
idempotency across a real restart** — the high-value one.

## Premise verification (LANE-BRIEF: "your brief's MEASUREMENTS are probably stale")

| Brief claim | Verified? | Note |
|---|---|---|
| `docs/delivery-semantics.md` puts Discord in exactly-once | **TRUE** | §2 row + machine-readable block line `discord = exactly-once` |
| that row rests on "a mockito test with an unbounded dedup window" | **TRUE** | row's last column reads verbatim `**No — mock only.**`, window open as `BL-24C1-DISCORD-WINDOW` |
| exactly-once scoped to delivery id `cron:{job}:{scheduled_millis}` | **TRUE** | `docs/delivery-semantics.md` §4 cites `wcore-cron/src/runner.rs:324-338` |
| `24-C3` declined by prior lanes | **TRUE** | ledger:868 "`24-C3` is still NOT MET and the repairing lane declines to claim it" |

So the doc is **already honest** that Discord is mock-only. My job is not to catch a lie; it is
to replace a labelled unknown with a measurement, and to change the Guarantee cell if the
measurement dissents.

## Architecture traced (before touching the network)

- `LedgeredHandler::dispatch_fire`, `crates/wcore-gateway/src/automation.rs:143-237`.
- Restart path that matters: state `Attempted` + outcome UNKNOWN + `destination_dedupes==true`
  → falls THROUGH the abandon arm (`:201`, guarded on `!destination_dedupes`) → `begin_attempt`
  (`:218`) → `self.inner.dispatch_fire` re-sends **with the same delivery id**.
- Discord's key on the wire: `rest::nonce_for_key`, used `wcore-channel-discord/src/lib.rs:170-172`.
- **The claim under test is therefore precisely:** a second send carrying an identical `nonce`,
  separated by a real process restart, yields ONE message at Discord. If Discord's dedup window
  is shorter than a restart, this is at-least-once in practice and the table must change.

## Instrument discipline for this lane

Per LANE-BRIEF §3b: every number redirected to a file and read with the Read tool, never
through Bash. Every absence gets a known-positive in the same capture. Observer must be proven
able to see a FAILURE (read a message id that does not exist → expect 404) before any of its
200s are trusted.

## Log

- [t0] Worktree created, SHA asserted, brief + delivery-semantics + ledger 24-C3 rows read.
- [t0] NOTES committed before any network work.
