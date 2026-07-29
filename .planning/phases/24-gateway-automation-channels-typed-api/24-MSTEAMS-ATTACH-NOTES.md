# 24-MSTEAMS-ATTACH — working notes (append-only, committed continuously)

Lane: `24-msteams-attach` · branch `lane/24-msteams-attach` · merge-base `15cda12d`
Started 2026-07-29.

## 0. Plan (first 15 minutes)

1. Establish the TRUE prior state of the msteams adapter before proposing any build.
   The dispatch brief warned that an earlier briefing called msteams "discord-shaped,
   ~2 sessions" and that this was stale. Verify directly.
2. Determine whether an inbound attachment path exists at all, and — separately —
   whether anything downstream *consumes* `IncomingMessage.attachments`
   (advertised-but-dead check: 8 prior instances on this program).
3. If the path is absent, build it: parse Bot Framework `attachments[]` into
   `Vec<Attachment>` in `activity_to_incoming`.
4. Prove it live through the real `wayland-core` binary via `gateway run` against the
   hermetic fixture, with a one-variable negative control per clause.
5. Grade ONLY the msteams clauses actually measured. Do NOT claim 24-C3.

Port choice: `F24_WEBHOOK_PORT` — pick a value no other lane would pick (see §4).

## 1. TRUE prior state of the msteams adapter (measured, not assumed)

Instrument discipline: all searches below use `/usr/bin/grep` (unproxied) with
**quoted** globs (`--include='*.rs'`), because zsh eats an unquoted `--include=*.rs`
and the command then fails with `no matches found` while looking like a clean zero.
That failure mode was hit once in this lane and corrected — recorded here because a
silent zero is exactly the self-passing-negative class §3b-i names.

### 1a. Known-positive control for the instrument

```
/usr/bin/grep -rn "msteams" --include='*.rs' crates/ | wc -l   →  44
```
Non-zero ⇒ the instrument is alive on this tree with these flags.

### 1b. The adapter itself: BUILT AND EXPOSED (premise HELD)

`crates/wcore-channel-msteams/src/` = `auth.rs` (23.5K), `config.rs`, `error.rs`,
`inbound.rs` (12.3K), `lib.rs` (32.0K), `token.rs`, `schemas/msteams.json`.
Inbound Bot Framework Activity parsing, JWT/JWKS auth gate with a `serviceUrl`
cross-check, Connector-API send path, `send_typing`, `config_schema`,
`max_message_len` — all present. The "already built and exposed" correction in the
dispatch brief is CORRECT for the adapter as a whole. The stale claim was the
*two-sessions-of-greenfield* cost, not the adapter's existence.

### 1c. The ATTACHMENT path specifically: ABSENT, and declared absent in source

Concept search (not one keyword) over the crate:
```
/usr/bin/grep -rnE "attach|Attach|contentUrl|content_url|media|Media" crates/wcore-channel-msteams/src/
```
Every hit is a *statement of absence*, not an implementation:

- `inbound.rs:18-21` — module doc: "**Attachments are NOT parsed in v1.** Teams
  delivers files as `attachments[]` entries with `contentType`/`contentUrl`, but
  fetching them requires a separate auth-gated download against the Graph/Connector
  API; deferred to a follow-up. `attachments` is left empty here."
- `inbound.rs:253` — the existing unit test *asserts the emptiness*:
  `assert!(msg.attachments.is_empty());`
- `lib.rs:351-353` — "`fetch_media` likewise stays default until inbound attachment
  parsing lands, since the connector surfaces no attachments to fetch yet."

Structural confirmation: the `Activity` struct (`inbound.rs:31-50`) has **no**
`attachments` field at all, so the Bot Framework `attachments[]` array is dropped at
deserialization time. Cross-crate:
```
/usr/bin/grep -rniE "msteams|ms_teams" --include='*.rs' crates/ | grep -iE "attach|media"   → 0 lines
```
(run with the known-positive above proving the instrument live in the same shape).

**Conclusion: the gap is real and the work is a BUILD, not a MEASURE.** This is the
one sub-surface where "already built" does not hold. Recorded before building.

## 2. The advertised-but-dead risk I must check BEFORE building

`wcore-channels/src/event.rs:102-129` defines `Attachment { url, path, content_type,
kind, transcribed }` and `IncomingMessage.attachments: Vec<Attachment>`. Parsing
Teams `attachments[]` into that vec is only worth anything if a **production
consumer** reads it. If nothing downstream renders/uses attachments, then adding
msteams parsing produces exactly the failure class this program keeps finding: a
declared, reachable surface that does nothing.

TODO(next): find the production consumer of `IncomingMessage.attachments` on the
gateway path, and prove it live — not from source review alone (five instances of
silent inbound loss on this program were invisible from source).

## 3. Open questions

- Does the gateway inbound path surface `attachments` to the agent turn, or drop them?
- Does `fetch_media` need to land for the attachment to be *useful*, or is
  url+content_type+kind enough for the clause under test?

## 4. Harness / port

Shared harness `scripts/f24-inbound.mjs` honours `F24_WEBHOOK_PORT` and has scoped
pkill patterns. Four other lanes concurrent — port chosen to avoid the obvious
neighbourhood; recorded in the summary with the run evidence.

## 5. Fence exposure

`crates/wcore-cli/src/lib.rs` and `main.rs` — expected ZERO. Diffed against the
captured merge-base SHA `15cda12d`, never against the branch name.
