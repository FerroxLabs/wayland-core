# f24-c3-h5-reload — NOTES (append-only, committed as I go)

Lane branch: `lane/f24-c3-h5-reload`
Base: `gh/plan/f20-unified-audit-repair` @ `d622cb09de01329cef6f20d6f9183df171462daf`
(SHA asserted against `/usr/bin/git ls-remote gh plan/f20-unified-audit-repair` — match).

## Minute 0-15 — the brief's central premise is FALSE at HEAD

My brief states `F24-C3-H5` is **"open and unfixed at HEAD"** and that my job is "the repair,
not the discovery". **Measured: the repair already landed and is an ancestor of HEAD.**

```
$ /usr/bin/git merge-base --is-ancestor 5d4bf4b9 HEAD  -> ANCESTOR-OF-HEAD
$ /usr/bin/git merge-base --is-ancestor 44a7cc16 HEAD  -> ANCESTOR-OF-HEAD
$ /usr/bin/git merge-base --is-ancestor 7c512fe2 HEAD  -> ANCESTOR-OF-HEAD
control (must be false): HEAD ancestor-of 5d4bf4b9 -> NOT ancestor  [instrument alive]
```

- `5d4bf4b9 fix(24-h5): a reloaded channel carries its access policy AND its tool posture`
- `44a7cc16 fix(24-h5): restore InboundPolicy import for the channel_inbound test module`
- `7c512fe2 docs(24-h5): SUMMARY and live evidence — reload now admits, with the configured posture`

`.planning/.../24-H5-SUMMARY.md` exists at HEAD with `status: complete`, verdict
"FIXED and live-proven, both facets", and a pre-fix/post-fix one-variable table
(pre-fix `8/11` legs FAIL, fixed `11/11` PASS) taken against binaries built from
`d34b2fe1` (pre) and the lane head (post).

**Why the ledger says otherwise:** the `2026-07-30` re-grade block in
`CRITERIA-GAP-LEDGER.md:818-832` was written by `lane/ledger-regrade` (`71acfd19`) and
reads the *finding* lane's `24-C3-FINISH.md`, which was accurate when written. The
`24-h5` repair lane merged after. The ledger row is stale, not wrong-at-the-time —
exactly the decay LANE-BRIEF §"Your brief's MEASUREMENTS are probably stale" predicts.

**So this lane is NOT the repair lane.** Remaining honest work, in priority order:

1. Re-verify the fix independently at HEAD (do not take the prior summary's word).
2. Answer the adjacent question my brief asks and the prior lane may not have:
   **is the access policy the only state `reload` fails to reload?** Sweep for siblings.
3. Answer: **can `reload`'s health report fail when the path is dead?**
4. Whatever siblings turn up: fix or name.

## Minute ~40 — the sibling sweep, and a NEW HIGH in the same code block

### Q: can `reload`'s health report fail when the path is dead?

**Yes — the machinery exists and is reached.** `ChannelHealthReport::is_complete()`
(`crates/wcore-cli/src/channel.rs:183`) is `registration_error.is_none() && registered >=
configured`, and `channel health` **`bail!`s** on `!is_complete()` (`channel.rs:445`). So
health has a real fail state and a non-zero exit. `configured` is counted from the config
DIRECTORY and `registered` from what the gateway built, so the two numbers are
independently sourced. That part is well built.

### Q: is the access policy the only thing `reload` fails to reload?

**No.** Sweep of everything `channel reload` touches (`gateway.rs:1430-1550`):

| state | refreshed by reload? | verdict |
|---|---|---|
| adapter set (`ChannelManager::reload`) | yes — removes+stops, replaces on fingerprint, `start_all` | OK |
| `registered_n` | yes | OK |
| inbound access policy | yes (`reload_policies`) — the 24-h5 fix | FIXED |
| tool posture | yes, same single swap — cannot be forgotten separately | FIXED |
| `config_fingerprint()` | **no production adapter overrides it** — the trait default at `wcore-channels/src/lib.rs:239` returns `None`, which reload treats as CHANGED. So `unchanged` is always empty and every reload reconnects every adapter. Deliberate and documented, fail-safe direction. | OK-by-design, noted |
| **`registration_error`** | **CLEARED UNCONDITIONALLY — see below** | **NEW HIGH** |
| poll lease (`channel_lease::attempt`) | **no** — acquired once at `gateway.rs:1284`, reload never re-attempts | contributes to the HIGH |

### NEW HIGH — `F24-C3-H6`: a successful `channel reload` ERASES the record of a dead inbound path

`gateway.rs:1465`, inside the reload success branch, three lines above the 24-h5 fix:

```rust
registered_n = names;
registration_error = None;          // <- unconditional
```

`registration_error` is the single field `channel health` fails on. At startup it accumulates
facts that a reload does **not** re-evaluate and **cannot** fix:

- `gateway.rs:1256` — `"inbound dispatch unavailable: {e}"`. `channel_inbound_host::spawn`
  failed (no resolvable provider) and `inbound_host` is `None`. **There is no inbound stack in
  the process at all.**
- `gateway.rs:1291` — `"inbound polling owned by another process"`. The single-owner poll lease
  was lost, so *"this gateway will send but not poll"*. The lease is never re-attempted.
- `gateway.rs:1302` — `"start_all: {e}"`.

Reload re-runs only *adapter registration*. It then wipes all three.

So the sequence is: gateway starts degraded → `channel health` correctly **fails** non-zero →
the operator runs `channel reload` (documented, innocuous, and the natural thing to try) →
adapters re-register fine → `registration_error = None` → `channel health` now **exits 0 and
reports complete**, while the inbound path is exactly as dead as it was one second earlier.

This is the same defect class as `F24-C3-H5` — an affirmative health claim over a silently
dead path — reached from the other direction, in the same code block, on the same command.
The 24-h5 lane added `reload_policies` immediately below this line and did not notice that the
line above wipes the error surface `reload_policies` itself depends on (its own `KEPT-STALE`
branch at `gateway.rs:1512` writes into the field that `:1465` has just cleared — that one
survives only because it is assigned *after*).

**Worse than H5 in one respect:** H5 was fail-CLOSED (messages denied). This is fail-OPEN on
the *report* — the operator is actively told the degradation is gone.

Fix direction: reload must clear only the component it re-evaluated (adapter registration) and
preserve the startup facts it did not. Test must be DRIVEN through the real binary's
`channel health` exit code, not asserted over the reload function.
