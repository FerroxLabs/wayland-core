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

## M3 — what landed (commits on `lane/24c3-channels`)

| Commit | Content |
|---|---|
| `a455a736` | these notes |
| `a957daed` | `NativeActions` capability type + Slack `chat.update` / `chat.delete` |
| `814a54ee` | Discord `PATCH`/`DELETE` on the message resource; Telegram `editMessageText` / `deleteMessage` |
| `c2c9e460` | Matrix `m.replace` / redaction; MS Teams activity `PUT`/`DELETE` |
| `4f12f99e` | the five negative declarations + the cross-adapter conformance matrix |

**edit/delete went 0/10 → 5/10 implemented**, and the other five are now
*declared* rather than merely defaulted — with the reason each one is absent.

## M4 — iMessage: a Darwin-only measurement (2026-07-30, this Mac)

`macOS 26.3` build `25D125`, `Messages.app 26.0`. Instrument: `/usr/bin/sdef`.

```
$ sdef /System/Applications/Messages.app | grep -o '<command name="[^"]*"' | sort -u
<command name="login"
<command name="logout"
<command name="send"
```

- **known-positive** `<command name="send"` → **1 hit** (instrument alive)
- target `edit|unsend|delete|redact|recall|remove` → **0 hits**
- class list: `account`, `chat`, `file transfer`, `participant` — **there is no
  `message` class at all**

So iMessage edit/delete is not an unwritten adapter method: there is **no
scriptable object representing a sent message** to address an operation to.
Messages.app has had human-facing edit/unsend since macOS 13 and exposes none of
it to AppleScript, which is this adapter's only outbound path. `PlatformHasNoApi`
with evidence.

**This is the Darwin-behaviour exception being used as intended** (LANE-BRIEF §0):
no permitted build host runs macOS, and this fact is unobtainable anywhere else.
It required no `cargo` at all — only `sdef` against the system app bundle.

## M5 — BOTH DIRECTIONS of the matrix gate, measured (hetzner, `4f12f99e`)

The gold standard set by `F24-C-ARRIVAL` is *the gate failed first*. Three
one-variable runs at the same SHA, tree verified clean before and after:

| Run | Change | RC | Result |
|---|---|---|---|
| **baseline** | none | **0** | `3 passed; 0 failed` — **the gate CAN pass** |
| **mutation A** | Slack `.edit(Implemented)` → `PlatformHasNoApi` (1 line) | **101** | `FAILED. 2 passed; 1 failed` |
| **mutation B** | MS Teams `.react(PlatformHasNoApi)` → `Implemented` (1 line) | **101** | `FAILED. 2 passed; 1 failed` |

Mutation A's message: *"`slack.edit`: declared `platform-has-no-api` but the call
did NOT answer Unsupported … outcome = `Err(Auth("bot token not loaded"))`"*.
Mutation B's: *"`msteams.react`: declared `implemented` but the call fell through
to the trait's Unsupported default — the override is missing."*

So the gate detects **both** an overclaimed capability and a stale absence, and
it is green only when the declaration and the wire agree. `FINAL_DIRTY=0`.

**Instrument defect found and REPAIRED in-lane, not written up (§6b-ii):** the
first mutation run used `set -e`, so the failing `cargo test` aborted the script
**before the revert**, leaving the mutation on disk (`PRE_DIRTY=1` on the next
run caught it). Re-driven with a `trap … EXIT` revert; `FINAL_DIRTY=0` asserted
at the end of the run rather than assumed.

## Standing risks recorded up front

- `rtk` rewrites tool output including `git diff --numstat` and `grep -c`. **Every number
  in the final report is captured to a file and read with the Read tool.**
- A skip is not a pass. The matrix publishes unrun cells as `UNRUN`, never blank.
- Every gate gets run in BOTH directions (§3b-iii): construct the failing state and the
  passing state.
