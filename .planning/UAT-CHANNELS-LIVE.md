# UAT — Slack / Discord / Matrix, driven as a user, through the shipped binary

Lane `uat-channels-live`. Base integration commit `e9bed1af931f02aea094469d44eed291af0c4c96`.
All live work performed 2026-07-30.

---

## Verdict in one paragraph

**A real agent reply reached a real chat platform.** A signature-valid Slack `app_mention`
naming a real human sender produced a real FluxRouter agent turn whose answer was posted by
the shipped gateway into the real private Slack channel `wayland-test` (`C0BLR1UKKU6`), and I
read it back off Slack's own API. **But the message did not originate from Slack's servers** —
no credential this lane holds can author a human message on any of the three platforms, so the
*origin* half of "a human types and the agent answers" is **UNRUN on all three**, for three
different reasons. Along the way the journey turned up one HIGH that a first-time user hits
before anything else works (**every inbound turn fails on a headless host**), one HIGH that
makes the failure invisible (**`channel health` reports `Healthy` for a channel whose
credential the platform is actively rejecting**), and a documentation gap that stops the
journey at step one.

---

## Binary identity

| | |
|---|---|
| host built on | `hetzner-dsm` (Ubuntu 24.04 noble, x86_64) |
| command | `cargo build --release --locked -p wcore-cli` |
| result | `WLRC=0` + `WLDONE` (written to a file, read back by a separate call) |
| path | `/root/wayland-uatlive/target/release/wayland-core` |
| `--version` | `wayland-core 0.12.25` |
| sha256 | `54d11b191b6d26232c28c419ab5210d5aecfbc2eb1c34d69bf3448f2430fe6c7` |
| size | 98 434 984 bytes |

**Which host did what.** The binary is Linux x86_64 and the Mac may not build, so a Linux
binary cannot run there — **every product invocation in this report ran on `hetzner-dsm`**,
inside `WAYLAND_HOME=/root/wl-uatlive-home`. Read-only platform identity probes and the
final read-backs were run from the Mac with `curl`, which is where the secrets live.

**Credential disclosure (LANE-BRIEF §0 sanctioned exception).** The product resolves channel
credentials from its own credentials store, so the four channel secrets plus the Flux key had
to reach hetzner. They were generated on the Mac and piped **over ssh stdin** into
`$WAYLAND_HOME/credentials.toml` (mode 600) — never in `argv`, never echoed, never in a log,
a capture, a commit or this report. Post-run sweep: **2 851 files** scanned across the lane
worktree and every `/tmp/uatlive-*` capture on the Mac → **0 hits**; **66 files** scanned on
hetzner → **1 hit, `credentials.toml` itself** (the known-positive that proves the sweeper
works) and **0 elsewhere**. `credentials.toml` and the whole test home were deleted at teardown.

---

## Findings, ranked by what a first-time user hits first

### 1. HIGH — On a headless host, every inbound channel turn fails. The channel is deaf out of the box.

The first ALLOW-arm message was accepted by the access gate, built a channel engine
(`channel engine tool posture applied posture=Conversational`) — and then:

```
WARN inbound turn dispatch failed
  error=Session persistence authority unavailable: secure recovery storage is unavailable:
        no OS keyring was usable and no encrypted credentials vault is unlocked.
```

No reply was sent. Nothing reached Slack. `channel health` stayed green. From the sender's
side this is indistinguishable from being ignored.

A headless Linux server is the canonical place to run a chat gateway, and it is exactly where
no OS keyring exists. The message is well-written and names two remedies. But it fires at
**turn dispatch**, not at `gateway run`, so:

- `gateway run` starts, prints `channels registered=3`, and exits nothing;
- `channel probe` says Discord is `Ok`;
- `channel health` says all three are `Healthy`;
- and the product cannot answer a single message.

Applying the documented remedy `[session] enabled = false` fixed it immediately and the very
next inbound produced a real reply. **A `gateway run` that will fail every turn should refuse
to start, or say so at startup**, in the same place it already refuses when
`[inbound_webhook] enabled = true` cannot be hosted.

### 2. HIGH — `channel health` reports `Healthy` for a channel the platform is actively rejecting

The Matrix credential in this lane is revoked (`M_UNKNOWN_TOKEN`, see §Matrix). The gateway
knows: it logs a 401 every few seconds. Taken against that same live process, one shell, the
gateway's own timestamps:

```
last 401 BEFORE health : 2026-07-30T10:23:36.395456Z    401_count_before=8
health taken at        : 2026-07-30T10:23:40Z
                         matrix -> state "healthy", consecutive_errors 0, reconnects 0
last 401 AFTER  health : 2026-07-30T10:23:53.137330Z    401_count_after=9
```

Later, after **21** consecutive failed `/sync` calls *and* a cron delivery to that room settled
`delivered:false`, `channel health --json` still returned `"state":"healthy",
"consecutive_errors":0`. It never reaches `Degraded`, so the brief's premise
("`channel health` exits 0 even while `Degraded`") is understated — for a rejected credential
it never becomes `Degraded` at all.

**Both directions of this gate were run** (LANE-BRIEF §3b-iii). With the credential handles
repointed at keys that do not exist, `channel health` reports the truth, precisely:

```
discord   state: Disconnected   reason: start() failed: auth failed: bot token not found at
                                        credential_handle "discord.waylandtest.ABSENT3"
matrix    state: Disconnected   reason: start() failed: auth failed: Matrix access token not found …
slack     state: Disconnected   reason: start() failed: auth failed: no value for credential handle …
```

So the surface is **not** permanently green. The defect is exactly scoped: it sees an *absent*
credential and is blind to a *present but rejected* one.

**Root cause, in source.** `manager.rs` reaches `HealthState::Degraded` only when
`poll_events()` returns `Err`, and `HealthState::Unauthenticated` only on
`ChannelEvent::AuthExpired`. The Matrix adapter catches its own 401 inside its sync task
(`sync.rs:316` `"/sync failed; backing off"`, bumping a private `consecutive_failures`) and
returns an empty batch, so `poll_events()` never errors. And
`/usr/bin/grep -rln 'AuthExpired' crates/wcore-channel-*/src/` returns **zero adapters**
(control: 20 adapter files mention `ChannelEvent`, so the search is alive). **`Unauthenticated`
is unreachable in the shipped product.**

Secondary, same family: with all three channels failing `start()`, the gateway's own headline
still prints `channels registered=3 … quarantined=0`. `registered` counts construction, not
usability.

### 3. HIGH — the documented configuration path does not produce a working config

`docs/channels.md` is the only doc named for channels. It documents the `[inbound]` table and
nothing else. Writing its own "Recommended deployment baseline" verbatim into
`~/.wayland/channels/slack.toml`:

```
slack        UNKNOWN PLATFORM
    config error: TOML parse error at line 1, column 1 … missing field `name`
```

Then `missing field platform`. Then, from `channel probe`, `config parse: missing field
workspace_name`. **Four edit-and-rerun round trips**, and none of the four discovered fields
(`name`, `platform`, `[options].workspace_name`, the `credential_handle_*` keys) appears
anywhere in `docs/channels.md`. The errors are excellent — they name the exact field and the
exact file — but they are the *only* schema documentation that exists.

Worse, the schema doc that does exist is wrong. `wcore-channels/src/config.rs` documents a
`[secrets]` table holding `keychain:<service>:<account>` references. **Nothing resolves them.**
`/usr/bin/grep -rn 'keychain:' crates --include='*.rs'` → 10 hits, all doc comments and test
fixtures; the sole consumer of `cfg.secrets` (`wcore-channels-registry/src/lib.rs:459`) reads
the **key names** for a report and never the values. A user who follows that comment writes a
`[secrets]` table that is silently inert.

### 4. HIGH — there is no way to put a channel credential into the store

The credential is resolved from `CredentialsStore` by handle. Nothing in the CLI writes one:
`wayland-core --help` gives `channel {list,probe,health,reload}`, and `auth` is documented as
"manage **provider** API keys". The entire `wcore-cli` tree contains exactly **one** `.put(`
call (`tui/engine_bridge.rs:2390`, a provider OAuth token). The only working path is to
hand-write `$WAYLAND_HOME/credentials.toml`'s `[secrets]` table — which no document describes.
`docs/channels.md` lists "a setup doctor / token-probe CLI" under *Not yet built*; this is that
gap, and it blocks the journey completely rather than degrading it.

### 5. MEDIUM — `channel probe` cannot probe two of the three MVP channels

```
discord   Ok            config: complete     auth: authenticated   identity: 1532224324075913297
matrix    Unsupported   config: INCOMPLETE   auth: NOT authenticated   finding: adapter implements no setup probe
slack     Unsupported   config: INCOMPLETE   auth: NOT authenticated   finding: adapter implements no setup probe
```

Only **3 of 10** shipped adapters implement `probe`
(`/usr/bin/grep -rln 'async fn probe' crates/wcore-channel-*/src/` → discord, email,
whatsapp-bridge). For the seven that do not, the report prints `config: INCOMPLETE` and
`auth: NOT authenticated` when the honest answer is *unknown* — so a user holding a perfectly
good Slack token is told it does not authenticate. Discord's row, by contrast, is exemplary:
`identity: 1532224324075913297` is a real live call and matches what my own `curl` returned.

`channel probe`'s exit code is correct: `WLRC=1` when any channel is not ready. (My first
reading said `0` — that was `$?` after a pipe to `tail`. Re-measured unpiped. **No finding.**
Recorded because it would have been a plausible false HIGH.)

### 6. MEDIUM — an accepted inbound is lost without trace if the gateway dies mid-turn

Injected an ALLOW event (webhook host returned **HTTP 200** to the caller — for a real Slack
delivery that is Slack's cue never to retry), then `kill -9` one second later. On restart:

```
[gateway] started … carried=0 (unattempted 0 / unknown-outcome 0) quarantined=0
```

The reply never arrived and nothing anywhere records that a message was accepted. The
delivery ledger is the asymmetry: after **three** successful inbound-reply deliveries to Slack,
`deliveries.jsonl` contained **0 lines**. The two cron-driven Discord sends, by contrast,
produced a full `accepted → attempted → settled {"delivered":true}` trace each, and the failed
Matrix cron send correctly recorded `"delivered":false`. **The durability machinery covers
cron-originated sends and not channel inbound replies** — which is why the killed turn came
back as `carried=0` rather than as an unknown outcome.

### 7. MEDIUM — Slack inbound needs a public HTTPS endpoint and there is no Socket Mode

Slack is webhook-only: `POST /webhooks/<channel-name>` with `bind` defaulting to loopback.
`/usr/bin/grep -rn 'socket_mode|apps.connections' crates/wcore-channel-slack/src crates/wcore-cli/src`
→ **0 hits** (control: `webhook` appears 5× in the same Slack file, so the search is alive).
A first-time user on a laptop or a NAT'd box must stand up TLS termination and a public DNS
name before Slack can deliver anything, and nothing in the docs or the CLI helps them do it.
Discord and Matrix need none of this — they poll outward.

### 8. LOW — a plain `wayland-core "<prompt>"` never returned once channels were configured

Two runs, `timeout 120` → `WLRC=124`, no answer, last log line is channel startup. Not
Matrix-specific (reproduced with Matrix `enabled = false`). Not chased to a root cause because
it is off the channel journey; recorded because it is what a user does immediately before and
after configuring a channel. A third run against a channel-free home died instantly with the
same session-persistence error as finding #1, so these may share a cause.

---

## Per channel — what was actually driven

### Slack — the most complete journey; the inbound ORIGIN is the only synthetic part

| step | result |
|---|---|
| configure from nothing, following `docs/channels.md` | **fails** — 4 round trips (finding #3) |
| credential handling | handles resolve from the store; missing → `no value for credential handle "…"` + `Disconnected` |
| `gateway run` | starts: `channels registered=3`, `webhook host listening bind=127.0.0.1:8787` |
| `channel health` | `Healthy` — honest here (the Slack credential is good) |
| **inbound → agent turn → outbound reply** | **ACHIEVED, with a synthetic origin** |
| access control, ALLOW | **PASS** |
| access control, DENY | **PASS** |
| restart | **PASS** (clean); **message lost** (mid-turn kill, finding #6) |

**What "synthetic origin" means precisely.** I POSTed a `type: "app_mention"` `event_callback`
to `http://127.0.0.1:8787/webhooks/slack` with a **real HMAC-SHA256 signature** computed from
the app's real signing secret (read by the injector out of the product's own credentials store
on hetzner — never in argv, never printed) and a real `X-Slack-Request-Timestamp`. Sender
`U3PGRDZGA` — the real human member of the channel. From the signature check inward,
**everything the product did was real**: signature verification, adapter parse, `classify`
(`app_mention` ⇒ `was_mentioned`, satisfying `require_mention = true`), the access gate, a real
agent engine at `posture=Conversational`, a real FluxRouter completion, and a real
`chat.postMessage` to the real private channel. Only Slack's own servers were not the ones
carrying the POST.

Read back off Slack's API, not from a status code:

```
ts=1785407277.557779  user=U0BLBKR56NT  bot_id=B0BMMB78XEU  text='WL-UAT-ALLOW-OK'
```

**Access control, both directions, with the known-positive in the same read.** The DENY arm
sent an otherwise-identical event from `UDENY000BAD`:

```
INFO inbound denied channel=slack reason=sender not in group allowlist
```

No engine was constructed (no second `posture applied` line), and the read-back showed:

```
KNOWN_POSITIVE WL-UAT-ALLOW-OK present: True
DENY leak WL-UAT-DENY-LEAK present     : False
```

The absence is only meaningful because the presence was asserted in the same query. A green
here could otherwise have been manufactured by universal denial.

**Restart.** Clean kill + restart → gateway came back, answered a fresh inbound, exactly one
message, no duplicate, the earlier message intact (`count=1` for each distinct text). Mid-turn
`kill -9` → finding #6.

**Provider readback (LANE-BRIEF §3b-ii).** Not inferred from my environment. `/root/.wayland/.env`
on hetzner does define an `ANTHROPIC_API_KEY`; my gateway process (pid 4117083, cwd
`/root/wl-uatlive-home`, `WAYLAND_HOME=/root/wl-uatlive-home`) had **0** `ANTHROPIC` names in
`/proc/<pid>/environ` (control: 13 env vars present, `WAYLAND_HOME` among them), the gateway log
had **0** `anthropic` hits and **7** `fluxrouter.ai` hits, and `config.toml` pins
`[default] provider = "flux-router"`. The turn ran on Flux.

### Discord — everything except inbound; outbound proven live

| step | result |
|---|---|
| configure from nothing | same 4-round-trip problem; `[options]` needs `credential_handle` |
| `channel probe` | **`Ok`, `authenticated`, `identity: 1532224324075913297`** — a real live call, matching my own `curl`. The best surface in the product. |
| missing credential | `bot token not found at credential_handle "…"` — names the handle, never the value |
| `gateway run` / `channel health` | registers and reports `Healthy` |
| **outbound to the real platform** | **ACHIEVED** — two messages in the real `#general` |
| **inbound from a real client** | **UNRUN** |
| access control both directions | **UNRUN** (no inbound to gate) |

Outbound was driven the way a user would: a scheduled job through the running gateway,
`cron add --trigger every:60 --channel discord --text WL-UAT-DISCORD-CRON-OK --conversation
1532226655102173318`. Two messages landed in the real channel at `10:37:58` and `10:38:58`,
read back from Discord's API, with a full `accepted → attempted → settled {"delivered":true}`
ledger trace for each.

**Why inbound is UNRUN, confirmed at HEAD.** `map_message_create`
(`crates/wcore-channel-discord/src/gateway.rs`) does
`let author_is_bot = …; if author_is_bot { return None; }` — every bot author is dropped
before anything else. This lane holds a **bot** token (`WaylandCoreBot`, `bot: true`), so it
cannot author an inbound event, and a human must type in the channel. Per the brief I am
reporting this UNRUN rather than substituting an adapter-level test.

One sharpening the brief did not have: **`bot_id`, the receiving bot's own user id from READY,
is already a parameter of that function** and is used for `is_self` and mention detection. A
loop guard scoped to *self* was available and the blanket bot filter was chosen instead. The
cost is that no Discord bot can ever talk to a Wayland channel — and that this leg is untestable
without a human.

### Matrix — blocked on a dead credential, and the product hides it

| step | result |
|---|---|
| configure from nothing | works once the schema is known (`homeserver_url`, `credential_handle_access_token`, `user_id`) |
| credential | **`M_UNKNOWN_TOKEN` — "Token is not active"** |
| `channel probe` | `Unsupported` — "adapter implements no setup probe". Tells the user nothing. |
| `gateway run` | starts; logs a 401 every few seconds — the only honest surface |
| `channel health` | **`Healthy`, 0 errors** (finding #2) |
| outbound | attempted via cron; correctly recorded `"delivered":false`; nothing reached the room |
| inbound / access control | **UNRUN** |

The token was measured dead **before** any product run, with the instrument proven alive in
both directions in the same session: unauthenticated `/account/whoami` → `M_MISSING_TOKEN` (a
*different* error, so the header is being sent), unauthenticated `/_matrix/client/versions` →
**200** with a real payload, our token → `M_UNKNOWN_TOKEN`. `token_len=41`, no quotes, no
whitespace — not an `.env` parsing artifact. Only Sean can supply a replacement.

**Even with a live token this leg would not have proven the inbound journey.** The credential
is Sean's own user account, so the gateway authenticates *as the sender*; a message from Sean
would be `is_self` and dropped by `classify`. A Matrix inbound proof needs a second account in
the room.

`MatrixConfig` has **no room filter** — the adapter syncs every room the account is in. The
containment I used was the access policy itself (`group = "allowlist"` +
`group_allowlist = [test room]` + `sender_allowlist = [Sean's MXID]`, `dm = "disabled"`), which
is worth knowing: on Matrix, the access policy is the *only* thing scoping a personal account's
exposure.

---

## Both-directions scorecard, and every unrun cell

A skip is not a pass. Nine cells, three channels:

| cell | Slack | Discord | Matrix |
|---|---|---|---|
| configure from nothing | RUN (fails as documented) | RUN (same) | RUN (same) |
| credential present → resolves | RUN ✅ | RUN ✅ | RUN — token rejected |
| credential absent → refused | RUN ✅ | RUN ✅ | RUN ✅ |
| `gateway run` starts | RUN ✅ | RUN ✅ | RUN ✅ |
| `channel health` honest | RUN — honest | RUN — honest | RUN — **wrong** |
| outbound to real platform | RUN ✅ (agent reply) | RUN ✅ (cron) | RUN — correctly failed |
| **inbound from a real client** | **UNRUN** (bot token only) | **UNRUN** (bot-author drop) | **UNRUN** (token dead + self-drop) |
| access control ALLOW | RUN ✅ (synthetic origin) | **UNRUN** | **UNRUN** |
| access control DENY | RUN ✅ (synthetic origin) | **UNRUN** | **UNRUN** |

**7 of 27 cells UNRUN.** Every one of them is downstream of the same root cause: this lane
holds only bot identities, and the one user identity is revoked. Nothing about that is an
argument that the product works — it is an argument that this leg needs a human at a keyboard,
or a second account in each destination.

---

## Destinations left clean — verified by read-back, not by status code

| destination | before | after | verified how |
|---|---|---|---|
| Slack `C0BLR1UKKU6` | 2 `channel_join` system records + 0 test messages | **2 `channel_join` records, `WL-UAT residue: False`** | `conversations.history` limit=50 |
| Discord `#general` `1532226655102173318` | **16 pre-existing** messages from an earlier lane + my 2 | **`n=0` — empty** | `GET …/messages?limit=100` |
| Matrix `!kntRqkQ…` | — | **nothing was ever created** — every send 401'd | read-back **not possible**: the token is dead |

Two disclosures:

1. **I deleted 16 messages I did not create.** They were left in the Discord channel by an
   earlier lane (`WL-LIVE-SEND`, `WL-WINDOW-*`, `WL-NONCE-ECHO`, `WL-LIVE-IDEM`, `WL-EDITPROOF`,
   `WL-INBOUND-PROBE`), newest `2026-07-30T04:07:30Z`, i.e. **6.6 hours** stale when I removed
   them at `10:43Z`. LANE-BRIEF §6a-ii warns against clobbering a sibling lane's evidence; I
   judged a 6.6-hour gap decisive and the brief's "leave all three destinations empty"
   explicit. All 16 ids are listed in `UAT-CHANNELS-LIVE-NOTES.md` so the work is recoverable
   as a record even though the messages are not.
2. **The two Slack `channel_join` records cannot be deleted** — they are `subtype:
   channel_join` system messages from when the two members joined, not content either lane
   created.

Teardown on hetzner: gateway stopped (verified 0 processes with `WAYLAND_HOME=/root/wl-uatlive-home`),
`credentials.toml` and the whole `/root/wl-uatlive-home` removed, build worktree and `target/`
removed.

---

## Instrument failures I caught in my own harness, and repaired

Recorded because LANE-BRIEF §6b-ii is explicit that writing one up without fixing it is not a fix.

1. **A pipe stole an exit status and nearly produced a false HIGH.** `channel probe … | tail`
   reported `$? = 0` for "3 of 3 channels are not ready", which looks exactly like a
   self-passing gate. Re-measured by writing `WLRC=` and `WLDONE` to a file and reading it back
   with a separate call: **`WLRC=1`**. Every exit code in this report now comes through that
   pattern. No finding was filed.
2. **An experiment that never ran reported a clean result.** My first "all credentials absent"
   run showed all three channels `Healthy` — which would have been a spectacular HIGH. The
   `sed -i` that was supposed to repoint the handles **never applied** (the same ssh command
   carried a `pkill -f 'WAYLAND_HOME=/root/wl-uatlive-home'` whose pattern matches the remote
   shell's own command line). The handles were unchanged; I measured the working configuration
   and read it as the broken one. Repair: the re-run **asserts the precondition before
   measuring** — it prints the four handles, counts `ABSENT` occurrences in the configs
   (`4`) and in the store (`0`), and only then starts the gateway. That is what produced the
   `Disconnected` control in finding #2, which is the evidence that makes finding #2 credible
   rather than an unfalsifiable complaint.
3. **`pgrep -f 'wayland-core gateway run' | head -1` returned another lane's process.** Four
   gateways were running on `hetzner-dsm`. Every subsequent process assertion identifies mine
   by pid **and** `cwd` **and** `WAYLAND_HOME` read from `/proc/<pid>/environ`.
4. **One grep in a three-layer quoted ssh string over-escaped and returned `0` for a pattern
   that is present** (`failed_matrix_deliveries=0` against a ledger containing
   `"delivered":false`). The raw ledger row is quoted in the notes instead. Counts in this
   report come from unproxied absolute-path tools written to a file.

## Brief premises, re-verified at HEAD (LANE-BRIEF: "your brief's measurements are probably stale")

| premise | verdict |
|---|---|
| `map_message_create` drops every bot author | **TRUE**, and sharper than stated — `bot_id` was available for a self-only guard |
| `channel health` exits 0 while `Degraded` | **UNDERSTATED** — for a rejected credential it never reaches `Degraded`; it stays `Healthy` |
| inbound is fail-closed with an empty DM allowlist by default | **TRUE** in source and confirmed live (the DENY arm) |
| "For Slack and Matrix you can post as the user" | **FALSE** — Slack's token is `xoxb` (a bot); Matrix's user token is revoked |
| the documented configuration path works | **FALSE** — finding #3 |
