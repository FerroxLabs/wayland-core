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

## 6. Consumer check — the attachment path IS live (not advertised-but-dead)

`crates/wcore-agent/src/channel_dispatch.rs:278-300` — `build_turn_prompt()` renders
`IncomingMessage.attachments` into the agent's turn text as
`"\n\n[attachments received with this message:\n  1. {Kind} ({content_type}) — {url}]"`,
preferring `att.transcribed` when the media enricher populated it. Production call
sites: `channel_dispatch.rs:139` and `:141` (the real dispatch path, not a test).

So parsing Teams `attachments[]` into that vec has a genuine downstream consumer, and
the live assertion for this lane is **the attachment line appearing in the turn prompt**,
not merely a populated struct. Good: that makes the clause falsifiable end-to-end.

## 7. FINDING — advertised-but-dead: the media-bounds module has NO production consumer

Measured with a known-positive control in the SAME shape, same invocation style
(`/usr/bin/grep -rn "<sym>()" --include='*.rs' crates/`):

| symbol | call sites | production consumer? |
|---|---|---|
| `max_message_len()` — **known-positive control** | 7 | YES — `wcore-channels/src/manager.rs:690` |
| `media_bounds()` | 1 | **NO** — only `wcore-channels/tests/framework_matrix.rs:373` (a test) |
| `normalize_all(...)` | 2 | **NO** — only its own definition (`media.rs:193`) and its own unit test (`media.rs:330`) |
| `normalize(...)` (outside `media.rs`) | 1 | **NO** — only `framework_matrix.rs:379/391` (a test) |

The control returning 7 with a real production hit proves the instrument and the query
shape are alive; the targets returning test-only hits is therefore a measurement, not a
dead-tool zero.

What this means substantively: `wcore-channels/src/media.rs` opens with
"**the rule this module exists to enforce: never drop silently**" and `normalize_all`'s
doc says attachments past the declared bound are "DEGRADED with a reason rather than
truncated away, because a truncated list is a message the agent answers with no idea it
was incomplete." **None of that runs in production.** Every channel that parses inbound
attachments today (slack `inbound.rs:106`, sms `inbound.rs:162`, telegram
`longpoll.rs:217`, email `imap.rs:467`) hand-rolls its own MIME→`MediaKind` mapping and
never consults `MediaBounds`. `discord` and `email` bother to *declare* `media_bounds()`
and nothing ever reads the declaration.

This is the advertised-but-dead class the dispatch brief named — a declared capability
with no production consumer — and it lands directly on the surface I was sent to build.
It also shapes the build: implementing msteams attachments through `normalize_all` makes
msteams the **first production consumer** of the module, so the bound is actually
enforced on at least one adapter instead of zero.

Severity: MEDIUM (a latent unenforced bound, not an active data-loss bug on a shipped
path) → BACKLOG per §5, EXCEPT for the part I close myself on msteams.
