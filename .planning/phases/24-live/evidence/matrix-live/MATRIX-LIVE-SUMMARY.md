# MATRIX-LIVE — the Matrix half of `24-C3`, driven against matrix.org

Lane `lane/matrix-live`. Merge-base `43c69ca71bc788dcd925fc070204d6918c2d7e0f`.
All live runs on `hetzner-dsm`, `/root/wayland-matrix-live`, room
`!REDACTED-MATRIX-ROOM:matrix.org`, binary `wayland-core 0.12.25`.

> **Redaction note.** This evidence was captured against the maintainer's real
> matrix.org account, so the account MXID and the room ID appeared verbatim
> throughout it and throughout the sibling logs. Both were replaced in place by
> lane `identifier-scrub` (2026-08-02) with `@REDACTED-MATRIX-USER:matrix.org`
> and `!REDACTED-MATRIX-ROOM:matrix.org` — one placeholder per distinct real
> value, in the identifier's own syntax, so every claim below still reads and
> still correlates across files. Nothing else in the evidence changed.
> `scripts/check-no-personal-identifiers.py` is what keeps them out from now on.
> **Git history at and before `c0906590` still contains the real values.**

**Verdict: all five capabilities are PASS, none NOT-RUN — and the run found one HIGH
and two MEDIUMs, one of which made the very row it was proving unreachable.**

---

## 1. The five capabilities

| # | capability | verdict | what corroborated it |
|---|---|---|---|
| 1 | **send** | **PASS** | shipped binary, `cron add --channel --conversation` + `gateway run`. Ledger `delivered:true`; an independent process reading matrix.org directly counted **1** event carrying the nonce, with the pre-fix run's nonce reading **0** in the same capture |
| 2 | **edit** | **PASS** | `m.replace` reached the platform; graded by the **homeserver's own** bundled `unsigned.m.relations.m.replace` on the ORIGINAL event, which named the replacement and carried the new body. Not by the 200 |
| 3 | **delete** | **PASS** | redaction graded by **read-back of the body**: `state=REDACTED body_present=false redacted_because=true`, with a sibling event reading `NOT-REDACTED` in the same capture |
| 4 | **receive** | **PASS** | an event already in the room reached the product through the production `/sync` loop and `ChannelManager::subscribe()` with `text_len=47` and the sender preserved |
| 5 | **outbound idempotency across a real restart** | **PASS** | below — the headline |

Nothing was skipped. Every leg ran.

### Capability 5, in full

The product was made to crash with a send whose outcome it genuinely could not know: a
recording forwarder passed the `PUT` upstream to matrix.org **for real**, read the real
response, and never wrote it back. The event landed; the product could not tell.

| | txn id on the wire | homeserver's `event_id` |
|---|---|---|
| life 1 (pid 3132637), response withheld, then `kill -9` | `cron:bf4c989c-…:1785385265000` | `$BAnrbBtxNCqVOn0q…` |
| life 2 (pid 3138250), `carried=1 (unattempted 0 / unknown-outcome 1)` | `cron:bf4c989c-…:1785385265000` — **identical** | `$BAnrbBtxNCqVOn0q…` — **identical** |
| **control**: same body, **different** delivery id | `cron:99a26815-…:1785385376000` | `$8rWWSSH7nc9lgq3F…` — different |

Ledger across the kill: `accepted → attempted → [kill] → attempted → attempted → settled
delivered:true`. Crash sentinel `/…/.dirty-death.3132637` confirms the death was unclean.

**Independent read of the room: 2 events, not 3.**

**The control is why that number means anything.** A count of one would have been equally
explained by exactly-once working and by the replay never having been attempted — the
LANE-BRIEF §3b-iii failure. Two, one of which was demonstrably produced by a *different*
delivery id, distinguishes them. It is simultaneously the live demonstration of
`delivery-semantics.md` §4: a different key is not a replay.

**So `docs/delivery-semantics.md` does NOT need its Matrix guarantee changed.** Exactly-once
holds across a real process restart at a real destination. What did need changing is the
evidence cell — it claimed "replay driven against a real, fresh Synapse", which was **curl**
against a container and established what the protocol does, not what this adapter does. §8 of
that document now records the product doing it.

---

## 2. Findings

### F-ML-5 — HIGH — a scheduled channel delivery addressed the CHANNEL NAME as the destination

`wcore-agent/src/cron.rs:137` passed `channel_name` as the outgoing `conversation_id`.
`Target::Channel` had no destination field and `cron add` had no flag to supply one (full flag
census run). The shipped binary, first live attempt:

```text
PUT /_matrix/client/v3/rooms/mxlive/send/m.room.message/cron:bd8831fa-…:1785384138000
403 M_FORBIDDEN "User @REDACTED-MATRIX-USER:matrix.org not in room mxlive"
```

`mxlive` is the channel's name.

**Not Matrix-specific.** Slack (`lib.rs:416`), WhatsApp (`:238`) and SMS (`:250`) each fall
back to a configured default destination — but only when `conversation_id` **is empty**, which
cron never produced. So no adapter's configured default was reachable from a scheduled
delivery; one could only arrive where the channel name and the destination id happened to be
the same string.

**Why six lanes of live proof missed it.** `24-C1-abandon-surface/f24c1-live3.sh` counted real
arrivals for `--channel f24c1csink` — because the destination was a **fixture sink that accepts
any channel id**. A real Slack would have answered `channel_not_found`. The defect is invisible
to any harness whose destination is permissive, which is every harness this programme built
before a real account existed.

**Consequence for the document.** §4 scopes exactly-once to `cron:{job}:{scheduled_millis}`,
and cron is the only production minter of those ids. Until this fix the Matrix exactly-once row
described a path that **could not address a Matrix room at all**. The row was true and
unreachable at the same time.

**Fixed here.** `Target::Channel::conversation_id: Option<String>` + `cron add --conversation`.
`None` now yields an empty conversation id — the value those three fallbacks already test for.
Tests: legacy on-disk records without the field still load and are not rewritten with a null; an
addressed target round-trips; `--conversation` reaches the persisted job and is asserted **not
equal** to the channel name. `conversation_id` is also fed to `scan_target`, being operator text
that reaches a URL path segment. Proven live: the post-fix wire shows
`PUT /rooms/%21REDACTED-MATRIX-ROOM%3Amatrix.org/send/…` and `delivered:true`.

### F-ML-3 — MEDIUM — a Matrix redaction of a nonexistent event returns 200

Found by a negative control that **failed**. `rest.rs:342-349` states the delete "reports
success when the homeserver accepted the redaction, which is the strongest guarantee the
protocol offers". matrix.org answers `200 {"event_id": …}` to a redaction of an event id that
never existed — corroborated by curl outside the product, and again unprompted during cleanup
when an **empty** event id also returned 200. Acceptance guarantees nothing, so `Ok(())` from
`delete_message` carries no information about whether anything was deleted.

The edit path is **not** symmetric: matrix.org rejects a relation to an unknown event with
`400 M_UNKNOWN "Can't send relation to unknown event"`, so `edit_on` returning `Ok` *is*
informative. Both are now asserted, in their respective directions, in
`matrix_live_room.rs`. The failing prediction was **inverted to pin the measured behaviour
rather than deleted** — it still reddens if matrix.org changes.

### F-ML-2 — MEDIUM — `edit_message` / `delete_message` have no production caller

Measured with `/usr/bin/grep` against a same-shape known-positive:

| manager method | production (non-test) callers |
|---|---|
| `react_on` | 2 — `channel_inbound.rs:520,553` |
| `send_to` | 2 — `channel_send_transport.rs:90`, `channel_inbound.rs:588` |
| **`edit_on`** | **0** |
| **`delete_on`** | **0** |

No CLI verb, agent tool, gateway path or protocol command can invoke a message edit or delete,
while `wayland-core channel actions --require edit` exists and gates deployments on it. Not
fixed here — adding operator verbs is product scope this lane was not asked for. Capabilities 2
and 3 were therefore driven through the production factory and the production `ChannelManager`,
which is the same `MatrixChannel` the gateway builds, and this is stated rather than hidden.

### F-ML-1 — disclosed constraint, and used as a control

`sync.rs:414-416` drops events whose sender equals the configured `user_id`. One account exists,
so the inbound probe's sender is that account and the filter would discard it. The channel's
`user_id` was therefore configured to a different mxid — a real configuration, since the token
and `user_id` are independent inputs. **The same event was then offered to a second production
adapter configured AS the sender and must not arrive**, which it did not
(`MLR_INBOUND_SELF_ECHO_ADMITTED=false`). Without that, "the message arrived" would be
indistinguishable from "the filter is broken".

### F-ML-4 — LOW, not investigated further — a redacted event arrives as an empty message

The inbound run shows `MLR_INBOUND_SAW id=$I38AF… text_len=0` for an event that had been
redacted. The adapter emits a `MessageReceived` with empty text for a redaction stub. Whether
anything downstream drops it was not measured. Recorded, not claimed.

---

## 3. Controls, in both directions (LANE-BRIEF §3b-iii)

Every instrument was proven able to **pass** and to **fail**.

| instrument | can it fail? | can it pass? |
|---|---|---|
| observer nonce count | a nonce never sent reads `0` while the control reads ≥1 | the posted nonce reads exactly `1` |
| observer instrument-death detector | a control nonce that does not exist → `OBSERVER=INSTRUMENT-DEAD`, exit 3, target count **withheld** | a live control lets the report through |
| redaction read-back | the same event pre-redaction reads `NOT-REDACTED`; a nonexistent id reads `NOT-FOUND`, never `REDACTED` | post-redaction reads `REDACTED` |
| `--replacements` | an event with no edit reads `0` | an event with an edit reads `1` |
| restart idempotency | the control's different delivery id produced a **second** event | the replayed key produced **no** second event |
| self-echo filter | configured as the sender → not admitted | configured as another mxid → admitted |
| secret sweep | a file containing the token returns `1` | the whole tree returns `0` |
| shared-fence diff | the same command over a file I did change returns 102 lines | the fence files return 0 |

Harness self-tests: proxy **11/11**, observer **13/13** (one assertion of mine was wrong and the
self-test caught it before any live run).

### Instrument defect found and repaired in-lane (§6b-ii)

`--replacements` used the **v3** relations route. matrix.org serves that route at **v1**, so it
returned a flat 404 for an event that demonstrably *had* a replacement — a gate with no reachable
pass state, exactly the §3b-iii shape, and one that would have reported "no replacement" forever.
Caught only because the bundled `unsigned.m.relations` in the same capture disagreed. Repaired to
v1 and proven three ways: passes on an event with a replacement (`1`), fails on one without (`0`),
and the old v3 route returns `404` for the same event the repaired one resolves.

---

## 4. Gate numbers

| suite | result |
|---|---|
| `wcore-cron` (lib + 5 integration bins) | `74 / 11 / 3 / 6 / 11 / 13` passed, **0 failed** |
| `wcore-gateway --lib` | **49 passed, 0 failed** |
| `wcore-channels-registry` (all) | `11 / 8 / 1 / 3` passed, **0 failed**; `delivery_semantics_declaration` **8/8** so the document and the capability bits still agree |
| `wcore-agent --lib cron::` | **19 passed, 0 failed** (2228 filtered out — count read back, per §3.2 flavour (c)) |
| `wcore-cli --lib cron::` | **7 passed, 0 failed** (1893 filtered out), including the new `add_channel_carries_the_destination_conversation` |
| `matrix_live_room` unasked | `1 passed, 2 ignored` — the two live tests do not run by default and **panic rather than skip** when run without configuration |

A `cargo build --release` does **not** compile `cfg(test)` code, so the binary that proved the
fix live would have shipped with `wcore-cli`'s lib tests red. `cargo test -p wcore-cli --lib
cron::` caught five more initializers. Recorded because "the release build was green" is not the
same statement as "the tests compile".

---

## 5. Cleanup

Every event this lane created is **redacted, verified by read-back rather than by the 200s**
(F-ML-3 is precisely why): 9 events, all `state=REDACTED body_present=false
redacted_because=true`. Final room read: `wayland-core live probe` → `originals=0`, and that
zero is proven live — posting one more made the same query read `1` before it too was redacted.

The room was the only room touched. No room was joined or left, no invite sent, no display
name, avatar, account or room setting changed, and `joined_rooms` was never enumerated. The
`/dev/shm` home is removed and the hetzner worktree torn down.

---

## 6. Secret handling — disclosed deviation from LANE-BRIEF §0

§0 requires a real credential to reach a build host **on stdin only and never be written to
disk**. Half of that was achievable and half was not, and the difference is stated rather than
glossed:

- **stdin only — held.** The token was piped to `ssh`, read with `read -r`, and exported. It is
  in no argv, no command line, no log.
- **never written to disk — NOT achievable for the binary legs.** The product reads channel
  credentials from a credentials store and has no environment path for a channel handle, so
  `gateway run` cannot be driven without one. The store was therefore created at
  `/dev/shm/matrix-live-secure/home/credentials.toml`, mode 600 inside a mode-700 directory —
  **`/dev/shm` is tmpfs, i.e. RAM**, so the value never reached persistent storage, and the tree
  is deleted. The Rust legs avoided even that with an in-memory `CredentialsStore`.

**Sweep, with a known-positive control returning 1 in the same capture:** 0 hits across the
evidence tree, the full merge-base diff, and the entire branch history — exact match and 12-char
prefix. The forwarder redacts `Authorization` before anything is journalled and its self-test
asserts that the unredacted header set *would* have leaked it.

---

## 7. What I did NOT do

- **Did not mark `24-C3` MET.** This closes the **Matrix** half of the criterion's
  send/receive/native-actions/idempotency clauses. `media`, `reconnect/reload` and `health` were
  not touched for Matrix, and the other nine adapters are not mine.
- **Did not add operator verbs for edit/delete** (F-ML-2). Reported, not built.
- **Did not investigate F-ML-4** beyond recording it.
- **Did not run a full-workspace build or test** — only `-p wcore-cron`, `-p wcore-gateway`,
  `-p wcore-channels-registry`, `-p wcore-channels`, and targeted `wcore-cli`/`wcore-agent` lib
  filters, per the disk/contention rule.
- **Did not touch the shared fence.** `git diff $BASE -- crates/wcore-cli/src/lib.rs
  crates/wcore-cli/src/main.rs` is **0 lines** (`$BASE` = the captured merge-base SHA
  `43c69ca7`, not the branch name), with a known-positive returning 102 on a file I did change.
- **Did not run `wcore-contract generate`**, merge, open a PR, tag, release, or close an issue.
- **Did not `git rebase`, `git reset`, `git stash`, `git clean`, or `git add -A`.**
- **Did not verify anything on Windows or macOS.** Every live figure is Linux/hetzner. The
  `docs/delivery-semantics.md` §5 Windows caveat is untouched and still applies.

## 8. For the orchestrator to serialize

- **Cross-crate change: `Target::Channel` gained a field.** `wcore-cron/src/job.rs`,
  `wcore-agent/src/cron.rs`, `wcore-agent/src/tool_backends/cron.rs`, `wcore-cli/src/cron.rs`,
  `wcore-cli/src/gateway.rs`, `wcore-gateway/src/automation.rs`,
  `wcore-cli/tests/f24_c1_outbound_idempotency.rs`. Any other lane constructing or destructuring
  `Target::Channel` will conflict. **No `Cargo.toml` / `Cargo.lock` change, no new dependency.**
- **`Target` is persisted.** The field is `#[serde(default, skip_serializing_if = "Option::is_none")]`,
  so old job stores load and are not rewritten with a null. A **Desktop** writer that mirrors this
  enum should learn about `conversation_id`; no wire contract was regenerated.
- **`cron add` grew `--conversation`.** Additive; existing invocations parse unchanged, but their
  *behaviour* changes — they now address the adapter's default rather than the channel name. That
  is the fix, and it is the only behavioural change a downstream consumer could notice.
- **`docs/delivery-semantics.md`** gained §8 and two cell edits. The Guarantee column and the
  machine-readable block are byte-identical, so `delivery_semantics_declaration.rs` reads the same
  pair — re-verified 8/8.
