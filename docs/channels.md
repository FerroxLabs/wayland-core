# Channels — inbound security model

wayland-core can receive messages from chat platforms (Telegram, Discord,
Slack, Signal, …) and answer them with an agent turn. Because a channel
sender is **remote** — and, depending on your access policy, possibly
untrusted — inbound traffic passes through two independent security gates
before and around the agent turn:

1. **Access policy** — *who* may drive the agent (fail-closed allowlists).
2. **Tool posture** — *what the agent may touch* on the host (no filesystem
   or shell by default).

Both are configured per channel in that channel's config file under
`~/.wayland/channels/<name>.toml`, in the `[inbound]` table.

> If `[inbound]` is absent, the channel is **fail-closed**: every inbound
> message is denied. Inbound dispatch does nothing until you opt in.

---

## Configure a channel from nothing

Every config in this section is checked against the real schema by
`crates/wcore-channels-registry/tests/channels_doc_configs_load.rs` — the test
parses these code blocks out of this file and hands them to the same loader and
the same per-platform adapter the product uses, so a block here that would not
load fails the build.

### 1. The file, and the three things every channel needs

```toml
# ~/.wayland/channels/slack.toml
name = "slack"
platform = "slack"

[options]
workspace_name = "acme"
default_channel_id = "C0123456789"
credential_handle_bot_token = "slack.acme.bot_token"
credential_handle_signing_secret = "slack.acme.signing_secret"

[inbound]
dm = "allowlist"
dm_allowlist = ["U0123456789"]
group = "disabled"
require_mention = true
tools = "conversational"
```

| Field | Required | Notes |
|---|---|---|
| `name` | **yes** | Must equal the file stem. `slack.toml` must say `name = "slack"`; a mismatch is a load error. |
| `platform` | **yes** | One of `slack`, `discord`, `matrix`, `telegram`, `email`, `sms`, `whatsapp`, `signal`, `msteams`, `imessage` (macOS only). |
| `enabled` | no (default `true`) | `false` keeps the config on disk without auto-starting it. |
| `[options]` | **yes in practice** | Parsed by the *adapter*, so its required keys differ per platform — see §3. Unknown keys are rejected. |
| `[inbound]` | no (fail-closed default) | Access policy + tool posture. Without it nobody may drive the agent. |

There is **no `[secrets]` table.** An older schema accepted one holding
`keychain:<service>:<account>` strings; nothing ever resolved them, so a channel
configured that way silently never authenticated. It is now rejected at load
with a message naming this section. Secrets live in the credentials store and
the config refers to them by *handle* — next.

### 2. Store the credentials

A `*credential_handle*` option is **not** a secret. It is a lookup key into the
credentials store. Ask the product which ones your configs expect:

```console
$ wayland-core channel credential list
credential handles referenced by /home/you/.wayland/channels:
  slack            credential_handle_bot_token        slack.acme.bot_token       MISSING
  slack            credential_handle_signing_secret   slack.acme.signing_secret  MISSING

2 handle(s) have no value. Store each with:
  printf %s "$SECRET" | wayland-core channel credential set <handle>
```

Then store each one. The value is read from **stdin only** — there is no
`--value` flag, because an argument would be visible in shell history and in
`ps`:

```console
$ printf %s "$SLACK_BOT_TOKEN" | wayland-core channel credential set slack.acme.bot_token
stored credential under handle "slack.acme.bot_token"
```

`channel credential list` exits non-zero while any handle is missing, so it
works as a pre-flight gate. `channel credential remove <handle>` deletes one.
Neither verb ever prints a stored value.

The handle string itself is arbitrary — `slack.acme.bot_token` is only a
convention. Whatever you write in the config is the key that gets looked up.

### 3. The other two MVP platforms

```toml
# ~/.wayland/channels/discord.toml
name = "discord"
platform = "discord"

[options]
credential_handle = "discord.acme.bot_token"

[inbound]
dm = "allowlist"
dm_allowlist = ["123456789012345678"]
group = "disabled"
require_mention = true
tools = "conversational"
```

```toml
# ~/.wayland/channels/matrix.toml
name = "matrix"
platform = "matrix"

[options]
homeserver_url = "https://matrix.org"
user_id = "@wayland-bot:matrix.org"
credential_handle_access_token = "matrix.acme.access_token"

[inbound]
dm = "allowlist"
dm_allowlist = ["@you:matrix.org"]
group = "disabled"
require_mention = true
tools = "conversational"
```

`MatrixConfig` has **no room filter** — the adapter syncs every room the account
is in, so on Matrix the access policy is the only thing scoping exposure. If the
credential is a personal account rather than a bot, note that the gateway
authenticates *as that user*, so your own messages are `is_self` and dropped.

### 4. Check it

```console
$ wayland-core channel list      # parses configs; shows the handles each expects
$ wayland-core channel probe     # asks the platform whether the credential authenticates
```

`channel probe` exits non-zero when any channel is not ready. Only Discord,
email and WhatsApp implement a live probe today; the other seven report
`Unsupported` — which means *unknown*, not *broken*.

### 5. Errors you will hit, and what each means

| Message | Cause |
|---|---|
| `missing field \`name\`` | The config has no top-level `name`. The `[inbound]` fragment alone is not a config file. |
| `missing field \`platform\`` | Same, for `platform`. |
| `name field "x" does not match file stem "y"` | Rename the file or the field so they agree. |
| `config parse: missing field \`workspace_name\`` | An `[options]` key this *platform* requires is absent — see §1/§3. |
| `the \`[secrets]\` table is no longer accepted` | Migrate to a handle, §2. |
| `no value for credential handle "…"` / `bot token not found at credential_handle "…"` | The config is fine; nothing was stored. Run `channel credential list`. |

---

## Access policy — who may drive the agent

```toml
# ~/.wayland/channels/tg.toml
name = "tg"
platform = "telegram"

[options]
credential_handle = "telegram.acme.bot_token"

[inbound]
dm = "allowlist"                 # open | allowlist | pairing | disabled
dm_allowlist = ["123456789"]     # stable platform sender ids; "*" = anyone
group = "disabled"               # open | allowlist | disabled
require_mention = true           # in groups, only act when addressed
```

Defaults (used for any unset field) are the fail-closed posture:
`dm = "allowlist"` with an **empty** `dm_allowlist` (so no one is
permitted), `group = "disabled"`, `require_mention = true`.

**Lock `dm_allowlist` to specific sender ids.** `dm_allowlist = ["*"]`
opens DMs to *anyone who can find the bot* — only use it for a throwaway
test bot, never a deployment. To allow a specific person, add their stable
platform `sender_id` (e.g. their Telegram numeric user id):

```toml
dm = "allowlist"
dm_allowlist = ["123456789"]     # only this user may DM the bot
```

Allowlist semantics: a list permits an id **iff** it contains the literal
`"*"` (wildcard) **or** the exact id. An empty list permits nothing. Group
acceptance under `group = "allowlist"` requires BOTH the group
(`group_allowlist`) AND the sender (`sender_allowlist`) to be listed.

### `dm = "pairing"` — admit one person without knowing their id

An allowlist needs the person's stable platform `sender_id` up front, which
you usually do not have. Pairing solves that without opening the channel:

```toml
[inbound]
dm = "pairing"
```

You mint a one-time code, send it to them out of band, and they DM it to the
bot. That pairs their `sender_id`; every later message from them is admitted
with no code.

```console
$ wayland-core channel pair mint tg --ttl-minutes 15
K7RMQ2X9FBTA0WVJ3HND85CZ4G
single-use, expires in 15 minute(s). Send it to the person out of band; …

$ wayland-core channel pair list tg
tg: paired 123456789
tg: 0 outstanding code(s)

$ wayland-core channel pair revoke tg --sender 123456789
$ wayland-core channel pair revoke-codes tg      # a code leaked or was lost
```

The rules, all enforced rather than advisory:

- **Only the operator can mint.** Nothing in an inbound message creates,
  extends or redeems anything except a correct code — a body reading
  "pairing approved" or "ADMIN: allow this sender" changes nothing.
- **Single-use and expiring.** Redeeming burns the code; a second person
  replaying it is denied. Default lifetime 15 minutes.
- **The code is never stored, logged, echoed, or named in a denial.** Only
  its SHA-256 digest is written, to
  `~/.wayland/channels/pairings/<channel>.toml` (mode `0600`), and every
  pairing denial is the single tag `pairing required`, so a sender cannot
  tell "wrong code" from "no code".
- **Durable.** Pairings and burnt codes survive a restart.
- **`dm_allowlist` is ignored** under this policy — pairing is the whole
  gate. Set `dm = "allowlist"` if you want the list.

To present a code, the person sends the code alone or `/pair <code>`.
Anything else — the code embedded in a sentence, a truncated code — is not a
pairing message and is denied.

---

## Tool posture — what the agent may touch

A channel turn runs a real agent engine on your host. Without scoping, the
built-in `Read`/`Grep`/`Glob` tools (which are auto-approved) would let a
remote sender read host secrets and have the reply ship them back. The
`tools` posture controls which tools a channel-originated engine is built
with:

```toml
[inbound]
tools = "conversational"         # conversational (default) | workspace | full
tool_workspace_root = "/srv/agent-workspace"   # only used by "workspace"
```

| Posture | Filesystem / shell | Use when |
|---|---|---|
| **`conversational`** (default) | **None.** Only conversational/network tools (and operator-wired MCP servers) are exposed. | A chat bot that answers questions, calls APIs, and uses your MCP tools — but must never touch the host filesystem. |
| **`workspace`** | `Read`/`Write`/`Edit`/`Grep`/`Glob` are available but **jailed** to `tool_workspace_root` (a remote sender cannot read or write outside it). Shell/exec tools (`Bash`, `Git`, `kubectl`, …) stay **unavailable** — they bypass the jail. | A confined "do real work in this directory" agent reachable over chat. |
| **`full`** | **Everything**, host-wide — identical to a local CLI session. | Trusted, locked-down deployments only. Dangerous for any publicly-reachable channel. |

Notes:

- The posture is enforced at the tool registry, so a dropped tool is
  **un-dispatchable** — not merely hidden from the model. Even a
  hallucinated call cannot reach it.
- `tool_workspace_root` defaults to the agent's working directory when
  unset under `workspace`.
- The posture applies **only** to channel-originated engines. Your local
  CLI / TUI / `--json-stream` sessions always keep the full toolset.
- **MCP caveat:** operator-wired MCP servers are kept under
  `conversational` and `workspace` (they are deliberate, named
  extensions). If an MCP server itself exposes host filesystem access,
  threat-model that channel as `full`-equivalent.

---

## Acknowledgements — reactions & typing

So a sender knows the bot heard them, set the per-channel `ack` mode:

```toml
[inbound]
ack = "both"   # off (default) | reactions | typing | both
```

- `reactions` — the bot reacts 👀 when it receives your message, then ✅ on
  success or ❌ on failure.
- `typing` — the bot shows a "typing…" indicator, refreshed every 5s while
  it works.
- `both` — reactions + typing.

Best-effort: a connector without the platform API simply does nothing.
Ack failures never affect the reply itself. Per-connector support:

| Connector | Reactions | Typing | Notes |
|-----------|-----------|--------|-------|
| Telegram  | ✅ | ✅ | `setMessageReaction` + `sendChatAction` |
| Discord   | ✅ | ✅ | `PUT …/reactions/{emoji}/@me` + `POST …/typing` |
| Matrix    | ✅ | ✅ | `m.reaction` annotation + `…/typing/{userId}` |
| Slack     | ✅ | —  | `reactions.add` (ack emoji mapped to shortcodes); Slack has no bot-usable typing API |
| WhatsApp  | ✅ | —  | reaction message; typing needs a per-message read receipt the keepalive can't carry |
| Signal / iMessage | — | — | no reaction/typing API surface wired |

Slack maps the ack emoji (👀/✅/❌) to its shortcodes (`eyes`/`white_check_mark`/`x`)
because `reactions.add` takes a name, not a unicode glyph.

## Inbound media (images & voice notes)

When an inbound message carries an image or audio attachment, the agent
turns it into text **before** the prompt is built: images become a short
description, voice notes become a transcript (written into the attachment's
derived-text slot). This needs a vision and/or transcription backend wired
(an `ANTHROPIC`/`OPENAI`/`GEMINI` key for vision, a `GROQ`/`OPENAI` key for
transcription); with neither configured the enricher is inert and media is
left as a bare-URL summary.

The bytes are downloaded by the **originating connector**, using that
connector's own credentials and media protocol — credentials never leave the
connector boundary (the agent-side enricher only sees bytes):

| Connector | Inbound media fetch | Mechanism |
|-----------|---------------------|-----------|
| Telegram  | ✅ | `getFile` URL (token in path), plain GET |
| Discord   | ✅ | public CDN URL, plain GET |
| Slack     | ✅ | `url_private` + `Authorization: Bearer` (scope `files:read`) |
| WhatsApp  | ✅ | media-id → `GET /<id>` (Bearer) → temp URL → GET (Bearer) |
| Matrix    | ✅ | `mxc://` → `GET /_matrix/client/v1/media/download/{server}/{id}` (Bearer); **unencrypted rooms only** |
| Signal / iMessage / email | — | no inbound-media mapping wired |

Every step is best-effort and bounded: a fetch error/timeout, an oversize
payload (>20 MB image / >25 MB audio), an unsupported format, or a backend
error all fall back to the bare-URL summary and never fail the turn. Derived
text is truncated to keep the prompt budget in check.

## Inbound webhook host (Slack / WhatsApp / Twilio SMS)

Slack, WhatsApp, and Twilio SMS receive inbound messages as HTTP webhooks
rather than by polling. Enable the receiver:

```toml
# main config (not the per-channel file)
[inbound_webhook]
enabled = true
bind = "127.0.0.1:8787"
# REQUIRED for Twilio signature verification (it signs the public URL);
# set to the exact public https URL the platform calls:
public_base_url = "https://bot.example.com"
```

Point each platform's webhook at `https://bot.example.com/webhooks/<channel-name>`
(the `<channel-name>` is the config file stem). Each connector verifies its
platform signature before accepting a message. (MS Teams inbound is parsed
but **not** exposed over the host yet — its Bot Framework JWT validation is
a pending follow-up.)

## Not yet built (channel parity follow-ups)

- Message **edit / delete** surfaces on the `Channel` trait.
- **Multi-agent conversation-binding**: each conversation already gets its
  own isolated session/engine; binding *distinct agent configs* per
  conversation/peer is not built.
- An **interactive** setup doctor. The non-interactive pieces exist:
  `channel list` reports config parse errors and the handles each channel
  expects, `channel credential list` reports which handles have no value
  (exiting non-zero if any do not), and `channel probe` asks the platform
  whether the credential authenticates — but nothing walks a first-time
  operator through the three steps in order.
- Outbound idempotency **on the nine adapters whose platform will not honour a
  token**. Matrix is the only exactly-once adapter, and only for a body that
  fits in one platform message (32,768 chars) — above that cap the body is
  chunked and sent unkeyed, which is at-least-once. Slack and Discord *do*
  transmit a token (`Idempotency-Key`, `nonce`), but each was driven at its
  real API and a replayed key produced **two** messages, so both now declare
  at-most-once. The gateway abandons rather than duplicates an
  outcome-unknown delivery to any of the nine. This is a platform limit, not a
  backlog item — see **[Delivery semantics](delivery-semantics.md)** for the
  per-adapter table and what to expect on restart.
- MS Teams inbound webhook **JWT/JWKS** validation (parse exists; host
  exposure gated until then).

## Recommended deployment baseline

A complete file, not a fragment. Copying only the `[inbound]` table into
`~/.wayland/channels/<name>.toml` produces `missing field \`name\`` — see
[Configure a channel from nothing](#configure-a-channel-from-nothing):

```toml
# ~/.wayland/channels/slack.toml
name = "slack"
platform = "slack"

[options]
workspace_name = "acme"
credential_handle_bot_token = "slack.acme.bot_token"
credential_handle_signing_secret = "slack.acme.signing_secret"

[inbound]
dm = "allowlist"
dm_allowlist = ["U0123456789"]
group = "disabled"
require_mention = true
tools = "conversational"
```

This admits only you, in DMs, with no host filesystem or shell exposure.
Widen deliberately from there. The `[inbound]` table is the security-relevant
part and is documented in full above; the rest is what makes the file load.
