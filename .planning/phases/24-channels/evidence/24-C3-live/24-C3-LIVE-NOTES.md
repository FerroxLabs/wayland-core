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

## Observer control — PASSED IN BOTH DIRECTIONS before any product run

`/tmp/lane-discord-live/observer-control.txt`:

```
me_code=200            <- known-positive: GET /users/@me
nonexistent_code=404   <- known-NEGATIVE: GET .../messages/000000000000000001
chan_code=200
bot_id 1532224324075913297 user WaylandCoreBot
chan_id 1532226655102173318 name general guild 1532226655102173315
```

The 404 is the load-bearing one: it proves the observer can report a message's ABSENCE, so a
later 404-after-delete is a real reading rather than a dead instrument. Bot id, channel id and
guild id all match the brief exactly.

hetzner egress verified before assuming any failure is the product's:
`curl https://discord.com/api/v10/gateway` → `discord_https=200`, 986G free, 96 cores.

## PREMISE REFUTED #1 — the ledger's "native actions" claim is STALE

Ledger `.planning/CRITERIA-GAP-LEDGER.md:824-825` and `:868`:
*"**media and native actions remain untouched for every adapter**"* and *"media and native
actions remain at zero"*.

**False for Discord at HEAD.** `wcore-channel-discord/src/lib.rs:465-472` declares
`.edit(Implemented).delete(Implemented).react(Implemented).typing(Implemented)`, and
`async fn edit_message` (`:475`) / `async fn delete_message` (`:502`) are real overrides
calling `rest::edit_message` / `rest::delete_message`.

## FINDING F24-C3-D1 — implemented, declared, and UNREACHABLE from the product

Searched with `/usr/bin/grep`, captures in `/tmp/lane-discord-live/action-surface.txt` and
`edit-on-callers.txt`:

| Search | Result |
|---|---|
| `.edit_message(` / `.delete_message(` in wcore-cli, wcore-gateway, wcore-agent, wcore-tools, wcore-protocol | **0** (rc=1) |
| known-positive, same tool, same dirs: `.send_message(` | **6 hits** — instrument alive |
| manager wrappers `edit_on` / `delete_on` callers, whole `crates/` | **only tests** — `framework_matrix.rs:416,421,441,443`, `native_action_matrix.rs:265,270` |

`wayland-core channel --help` offers `list / probe / health / reload / actions` — **no edit,
no delete.** So the native-action capability is real at the adapter and has **zero
operator-reachable surface**. `channel actions` will happily report Discord can edit and
delete; nothing in the shipped binary can ask it to.

Consequence for this lane, stated up front rather than discovered at the end: capabilities 2
and 3 cannot be driven through a shipped operator verb. They are driven through the
**production factory** (`channel_factory_for`, the same constructor the binary uses) against
real Discord, and the missing surface is reported as a defect rather than papered over.

## Log

- [t0] Worktree created, SHA asserted, brief + delivery-semantics + ledger 24-C3 rows read.
- [t0] NOTES committed before any network work.
- [t1] Observer control passed both directions. hetzner egress + build (1m49s) OK.
- [t1] Premise refutation #1 and finding F24-C3-D1 recorded.
