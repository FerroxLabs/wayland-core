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

(appended as measurements land)
