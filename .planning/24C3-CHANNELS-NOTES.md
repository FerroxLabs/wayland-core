# 24-C3 CHANNELS LANE — NOTES (append-only, committed continuously)

Lane: `lane/24c3-channels`. Base: `b2ddf113681647221dc9e5bbfc7de79b1da90b54`.
Worktree: `/Users/seandonahoe/dev/waylandcore-frontier-worktrees/lane-24c3-channels`.

Criterion: **24-C3** — *"Reference channels prove setup/auth, access, routing, media,
native actions, idempotency, reconnect/reload, and health."* (`ROADMAP.md:119`)

---

## M1 — brief premise verification (2026-07-30, first 15 min)

### Claim: "edit/delete is 0 of 10 adapters" — **HOLDS**

Instrument: `/usr/bin/grep` (unproxied), output redirected to `/tmp/24c3-editdelete.txt`
and read with the Read tool, never through Bash stdout. Capture carries a
known-positive AND the target search in the same invocation.

| Search | Result |
|---|---|
| **known-positive** `async fn send_message` in `crates/wcore-channel-*/src/` | **40 hits across 14 files**, incl. one `impl Channel` override per adapter crate (discord `lib.rs:317`, email `:346`, imessage `channel.rs:225`, matrix `:265`, msteams `:302`, signal `:252`, slack `:230`, sms `:237`, telegram `:267`, whatsapp `:229`) — **instrument alive** |
| `async fn edit_message` in `crates/wcore-channel-*/src/` | **0 hits**, rc=1 |
| `async fn delete_message` in `crates/wcore-channel-*/src/` | **0 hits**, rc=1 |
| concept search `fn (edit\|delete\|revoke\|unsend\|redact\|remove)_` in adapters | **1 hit**, `telegram/src/api.rs:862 delete_webhook` — unrelated (Bot-API webhook dereg, not message delete) |

So: **edit = 0/10, delete = 0/10.** Both trait methods (`wcore-channels/src/lib.rs:198`
`edit_message`, `:215` `delete_message`) are defaulted to
`ChannelError::Unsupported { op, platform }` and **no adapter overrides either.**

Note the default is already *honest* (a named `Unsupported`, not a silent `Ok`) — the
gap is capability, not truthfulness. That matters for what "closing" means (below).

### Claim: `F24-C3-H5` already fixed — accepted, not re-litigated
Ledger `CRITERIA-GAP-LEDGER.md:835-841` carries the LATE CORRECTION with independently
verified ancestry. Brief repeats it. Not re-touching.

---

## M2 — the shape of the edit/delete hole

`edit_message`/`delete_message` are declared on `Channel` but there is **no capability
declaration** for them — unlike outbound idempotency, which has
`supports_outbound_idempotency()` (`lib.rs:139`) precisely so a caller can discriminate
*"this platform cannot"* from *"nobody wrote it yet"* **without making a call**.

Today those two states are indistinguishable: both surface `Unsupported` at call time.
That is the same defect family the ledger keeps recording — a truthful-looking negative
that carries no information about whether it is permanent.

Per-platform reality (to be verified against each platform's API before implementing;
this is the hypothesis, not the result):

| Adapter | Edit API? | Delete API? |
|---|---|---|
| slack | `chat.update` | `chat.delete` |
| discord | `PATCH /channels/{c}/messages/{m}` | `DELETE /channels/{c}/messages/{m}` |
| telegram | `editMessageText` | `deleteMessage` |
| matrix | `m.replace` relation event | `PUT /rooms/{r}/redact/{e}/{txn}` |
| msteams | `PUT /v3/conversations/{c}/activities/{a}` | `DELETE .../activities/{a}` |
| signal | signal-cli edit (`--edit-timestamp`) | signal-cli remote-delete |
| whatsapp | none (Cloud API) — **permanent** | none — **permanent** |
| email (SMTP) | none — **permanent** | none — **permanent** |
| sms (twilio) | body-redact only, does not un-deliver | record delete only, does not un-deliver |
| imessage | not scriptable via AppleScript | not scriptable via AppleScript |

So the honest close is NOT "10/10 implemented". It is *"implemented where the platform
has the surface, declared permanently-absent where it does not, and the two are
distinguishable without making a call."*

---

## Standing risks recorded up front

- `rtk` rewrites tool output including `git diff --numstat` and `grep -c`. **Every number
  in the final report is captured to a file and read with the Read tool.**
- A skip is not a pass. The matrix publishes unrun cells as `UNRUN`, never blank.
- Every gate gets run in BOTH directions (§3b-iii): construct the failing state and the
  passing state.
