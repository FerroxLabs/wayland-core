# FIX-EVIDENCE-AUDIT — running notes

Lane `fix-evidence-audit`. Worktree
`/Users/seandonahoe/dev/waylandcore-frontier-worktrees/lane-fix-evidence-audit`,
branch `lane/fix-evidence-audit`, base integration `e7bc6d88`.

Read-and-audit lane. No behaviour changes. Every number below comes from a file
written by an unproxied tool (`/usr/bin/grep`, `/usr/bin/git`) and read back with
the Read tool, per LANE-BRIEF §3b.

---

## M0 — worktree identity

`/usr/bin/git rev-parse --show-toplevel` →
`/Users/seandonahoe/dev/waylandcore-frontier-worktrees/lane-fix-evidence-audit`.
`/usr/bin/git log --oneline -1` → `e7bc6d88 merge(fix-tui-first-message): ...`.
Confirms base. NOT `/Users/seandonahoe/dev/waylandcore`.

## M1 — `supports_outbound_idempotency` census at `e7bc6d88`

Capture: `/usr/bin/grep -rn "fn supports_outbound_idempotency" --include="*.rs" crates/`
→ 5 overrides + 1 trait default (`/tmp/lfea-supports.txt`, read back).
Bodies read at `/tmp/lfea-idem-bodies.txt`:

| adapter | file:line | value at HEAD |
|---|---|---|
| trait default | `wcore-channels/src/lib.rs:144` | `false` |
| Slack | `wcore-channel-slack/src/lib.rs:283` | **`false`** (was `true`; flipped after live proof) |
| Discord | `wcore-channel-discord/src/lib.rs:368` | **`false`** (was `true`; flipped after live proof) |
| Matrix | `wcore-channel-matrix/src/lib.rs:294` | **`true`** — the only `true` |
| Twilio SMS | `wcore-channel-sms/src/lib.rs:338` | `false` (explicit) |
| WhatsApp | `wcore-channel-whatsapp/src/lib.rs:384` | `false` (explicit) |
| Telegram, Email, iMessage, Signal, MS Teams | no override | inherit `false` |

**Brief premise HELD.** Exactly-once is 1 of 10 at HEAD, and the one survivor is
Matrix — the adapter that was driven live before it was believed.

---

## M2 — the method, and its BOTH-DIRECTION control

Two detectors, both self-testing, both committed alongside this file.

### Detector 1 — `scan_declarations.py` (literal-bodied capability functions)

A capability declaration is a function whose whole body is one literal. The
literal is a CLAIM about somebody else's system; nothing in the type system
checks it. Self-test (3 assertions, §6b-ii): bool literal detected, `Some(N)`
detected, multi-line body NOT detected, delegating call NOT detected.
`SELFTEST … result=PASS`.

Run at `e7bc6d88`: **487** literal-bodied fns under `crates/`, **177**
capability-named, **152** of those in `src/` (not test files).

### Detector 2 — `scan_invented_shapes.py` (payload shapes nobody produces)

A consumer reads named fields out of a `serde_json::Value`. If no PRODUCTION
code anywhere writes that field, the shape exists only in the consumer's head
and in the tests its author wrote. Self-test asserts the `#[cfg(test)]` span
tracker marks a test-side write, and does NOT mark production writes.

Run: 1771 `.rs` files, 491 distinct keys read in production, **188** with no
production writer, **111** whose only writer is a test.

### CONTROL — does the method rediscover the two KNOWN cases?

**Known case 1 (exactly-once bits): YES.** Detector 1 finds
`supports_outbound_idempotency` (6 sites, values `false`/`true`) directly.

**Known case 2 (TUI bash formatter): initially NO — and that is the finding
about the method.** Detector 2 as first written MISSED it. `exit_code` is
written in production by `wcore-agent/src/child_transaction/gate_executor.rs:423`
— an unrelated subsystem — and that one name collision bought the formatter a
pass on the very key that names the defect.

**Instrument repaired in-lane, not merely noted (§6b-ii).** Scoring moved from
per-KEY to per-CONSUMER-FILE: a file reading N keys with a producer for only one
is reading an invented shape regardless of the collision. Re-run:

```
CONTROL(known positive) crates/wcore-cli/src/tui/tool_formatters/bash.rs:
  2/3 unbacked, ratio=0.667, unbacked_keys=['cmd', 'stdout']
```

Known-negative side, same run: **44** consumer files score ratio `0.00` with ≥4
keys read (`protocol_bridge.rs` 0/15, `yuanbao_tools.rs` 0/15,
`cronjob_tools.rs` 0/15). The instrument discriminates in both directions —
it is not simply flagging everything.

**Both known cases are rediscovered by the repaired method.**

## M3 — capability-named literal declarations, grouped

From `/tmp/lfea-decls.json`, 152 in `src/`:

| name | sites | values |
|---|---|---|
| `is_concurrency_safe` | **79** | `false` / `true` |
| `is_available` | 15 | `false` / `true` |
| `media_bounds` | 10 | `MEDIA_BOUNDS` / `MediaBounds::default()` |
| `max_message_len` | 9 | `None`, `Some(1600)`, `Some(2000)`, `Some(28_000)`, `Some(32_768)`, `Some(39_000)` |
| `max_result_size` | 9 | `4096`, `10_000`, `20_000`, `50_000`, `100_000`, `MAX_OUTPUT_BYTES` |
| `is_deferred` | 8 | `false` / `true` |
| `supports_outbound_idempotency` | 6 | `false` / `true` |
| `supports_streaming` | 3 | `false` / `true` |
| `is_wired`, `is_alive` | 2 each | |
| `dispatch_is_idempotent`, `default_hook_timeout`, `native_actions` | 1 each | |

---

(appended as measurements land)
