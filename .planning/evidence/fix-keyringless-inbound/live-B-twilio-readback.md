# Live inbound proof — platform-side read-back

The claim `c73ac417` explicitly did NOT make:

> "A real inbound turn on a real platform from a keyring-less host is UNRUN.
> This lane held no platform credential."

It is now run. Arbiter is the **Twilio Messages REST API**, not our own log.

## Topology

| role | number | why |
|------|--------|-----|
| gateway (receives) | `<GATEWAY_NUMBER>` = `TWILIO_FROM_NUMBER` | owned; `sms_url` was the inert `demo.twilio.com/welcome/sms/reply/`, so repointing it disturbs no production traffic |
| sender (originates) | `<SENDER_NUMBER>` | a **different** owned number, also on the inert demo URL |

Two distinct identities, which is the whole difficulty — see NOTES.md for why
Discord/Slack/Matrix cannot supply one.

The 7 numbers pointing at `services.leadconnectorhq.com` (Sean's live business
webhooks) were **not touched**.

## Path exercised

```
sender number
  -> Twilio
  -> HTTP POST http://95.216.244.213:8788/webhooks/sms      (public internet)
  -> wayland-core `gateway run`, inbound webhook host, bind 0.0.0.0:8788
  -> channel dispatch -> AgentBootstrap -> run_with_content -> preflight
  -> flux-router turn
  -> Twilio REST outbound
  -> sender number
```

`run_with_content -> preflight -> open_confidential_credentials_store` is
precisely the chain that killed every turn pre-fix.

## Host state at the time of the turn (read back from the product)

```
BUILD_INFO: wayland-core 0.12.25 (source e7bc6d883027102ff1e5bbaa2dd19f9265268cab)
VAULT_PASSPHRASE_SET=no
DBUS_SESSION_BUS_ADDRESS=UNSET          <- Secret Service unreachable, proven in NOTES.md
MISSECTIONED_KEYS=0                     <- config actually took effect (see harness defect #3)
[gateway] channels registered=1
inbound webhook host listening bind=0.0.0.0:8788
notice: durable session persistence is OFF for this run. ...
INFO vision: using flux-auto at https://api.fluxrouter.ai/... (active OpenAI-wire provider)
```

Provider is read back from the product's own resolver line, per LANE-BRIEF
§3b-ii — NOT inferred from what the harness exported.

## The message pair

Sent (Twilio API, HTTP 201, `SM8f161b6b…`):

> What is 17 times 3? Reply with the number and include the token FKRI-657418 in your answer.

Read back from `GET /Messages.json?To=<SENDER_NUMBER>`, HTTP 200:

```
sid=SM13c6c55c.. from=<GATEWAY_NUMBER> status=received  sent=Thu, 30 Jul 2026 14:18:55 +0000
   body: 51 FKRI-657418
sid=SM08515f1b.. from=<GATEWAY_NUMBER> status=delivered sent=Thu, 30 Jul 2026 14:18:54 +0000
   body: 51 FKRI-657418
```

`REPLY_FROM_GATEWAY_CONTAINING_NONCE = True`.

Two independent things are proven by the body, and both matter:

* **`51`** — 17x3. A canned ack, a retry echo or a delivery receipt cannot
  produce it; only a real model turn can. This is what makes the run a *turn*
  and not merely a webhook 200.
* **`FKRI-657418`** — a nonce generated from `/dev/urandom` seconds earlier, so
  the reply is bound to THIS inbound and cannot be a replayed artefact of an
  earlier session.

## The degrade was real, not bypassed

```
SESSIONS_DIR_AT_PROFILE_ROOT=no
JOURNAL_FILES=0
DEGRADE_NOTICE=1
CHANNEL_ENGINE_BUILT=1
SESSION_AUTHORITY_ERROR=0        <- the pre-fix killer, absent
CTRL_known_negative=0            <- instrument alive
```

So the turn ran with durable sessions genuinely OFF — it did not quietly
acquire a journal.

`memory/sessions/boot.db*` exists but is the **memory** subsystem's store, not
the durable session journal (see harness defect #4).

## Harness defects found and repaired during this run

1. **`ps aux | grep -c "[k]wallet"` returned 1 on a box with no kwallet** — `ps`
   was showing my own ssh command line carrying the pattern. Repaired by
   building needles at runtime from fragments inside a script file.
2. **`RC=$?` read after a pipe** reported `head`'s status; a failed `dbus-send`
   was recorded as `RC=0`. Repaired by redirecting to a file with no pipe.
3. **`provider`/`model`/`max_turns` written at TOML top level were silently
   ignored** — they belong under `[default]`. The first run would have proved a
   turn on the DEFAULT provider while the report claimed flux-router: exactly
   LANE-BRIEF §3b-ii. Caught only because the product emits
   `ignoring unknown or mis-sectioned config key`. A `MISSECTIONED_KEYS` gate is
   now asserted `0` with a known-positive alongside it.
4. **`find -type f -name '*journal*' -o -type f -path '*sessions*'`** — `-o`
   precedence made the count meaningless, and the needle was wrong anyway
   (it matched the memory store). Replaced with an explicit
   `sessions/`-at-profile-root check plus a `*.journal` count.
5. **A poll gate matching `-e "inbound"` was self-passing** — the startup line
   `inbound webhook host listening` matches it, so it would have reported
   "inbound seen" with no message ever delivered. Replaced by the Twilio-side
   read-back, which cannot pass without a real reply.
6. **`ufw status | head -5` produced a FALSE blocker.** Truncation hid the
   `80/tcp` and `443/tcp` rules and I nearly reported the host as having no
   public ingress. Re-read with `ufw status verbose` unpiped.
7. **The phone-number sanitizer initially "passed" with 0 placeholder hits** —
   indistinguishable from a broken `sed`. Proven alive on a seeded
   known-positive: 2 hits before, 0 after, 2 placeholders. The product turned
   out never to log the numbers at all (raw capture: 0 hits, with `gateway` as
   the known-positive at 4).

## Credential handling

`TWILIO_ACCOUNT_SID`, `TWILIO_AUTH_TOKEN` and `FLUX_API_KEY` reached
`hetzner-dsm` on **ssh stdin only** — never argv, never a file, never a log.
The product received the two Twilio values on its own stdin via
`channel credential set <handle>`, which is the shipped verb for this.

Sweep over every file this lane commits, against all four live values:
`CREDENTIAL_HITS_IN_COMMITTED_EVIDENCE=0`, with the sweeper proven alive on a
seeded token (`CREDSWEEP_KNOWN_POSITIVE=1`).

## Reversible changes made to shared infrastructure

| change | restored |
|--------|----------|
| `ufw allow 8788/tcp` on hetzner-dsm | see `infra-restore.txt` |
| `<GATEWAY_NUMBER>` `sms_url` -> our webhook | restored to `https://demo.twilio.com/welcome/sms/reply/` |
