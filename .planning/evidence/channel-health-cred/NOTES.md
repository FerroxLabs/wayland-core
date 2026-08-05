# NOTES — lane/channel-health-cred (residual of task #149)

Base: `c0906590`. Host for every build, test and live run: `hetzner-dsm`.

## What was already true — the brief's premise is stale

The brief says "neither Slack nor Discord has an auth-rejection producer". That
was true at `13f61ec6`. It is **not** true at this lane's base. Commit
`41c53619` ("fix(channels): give slack and discord an auth-rejection producer"),
plus follow-ups `0dd5ebb4`, `1e87a7b8`, `fa7e7ce6`, are all ancestors of
`c0906590` (`git merge-base --is-ancestor 41c53619 HEAD` — true). They deliver
exactly (a) and (b):

- **Discord** — `gateway.rs` captures the close frame's numeric code and
  classifies 4004 / 4013 / 4014 as terminal credential rejections
  (`auth_rejection_for_close_code`), publishing `ChannelEvent::AuthExpired`
  before the gateway loop exits.
- **Slack** — `start()` calls `api::auth_test` against slack.com under a 10s
  budget and queues `AuthExpired` on refusal, and the send path routes an
  outbound refusal into the inbox for a mid-run revocation.

That lane also left committed live four-quadrant evidence
(`.planning/evidence/fix-channel-auth-producers/live/`) and a per-hunk ablation.
**I re-proved both directions myself on this lane's binary anyway** — see below —
because inherited evidence describes an inherited binary.

The brief's remaining worry, the once-valid-then-revoked lifecycle, is partly
addressed by classification rather than by a revocation: `token_revoked`,
`token_expired` and `account_inactive` are all in `api::is_auth_rejection`, and
Slack's `auth.test` on a bogus token returns `invalid_auth`. An actual
revocation of the live Trade Canyon token was **not** performed — see "Not
proven" below.

## What was still broken, and is now fixed

### 1. Three hand-rolled auth lists disagreed with the one classifier

`api::is_auth_rejection` is documented as the answer to "did the platform refuse
this credential" and includes `account_inactive`, with a comment explaining
that a deactivated bot user cannot be recovered by retrying. Three call sites —
`post_message_keyed`, `add_reaction`, `post_mutate` (chat.update / chat.delete)
— restated that list by hand, and **all three omitted `account_inactive`**.

Consequence: on a running gateway, Slack's only producer of
`HealthState::Unauthenticated` is an outbound call returning `SlackError::Auth`,
which the adapter turns into `AuthExpired`. Classifying `account_inactive` as
`SlackError::Api` drops that producer, so a Slack admin deactivating the bot
user left the channel reading `Healthy` while every send failed. All three sites
now call `is_auth_rejection`.

### 2. Only `send_message` published — four other outbound surfaces did not

`react`, `edit_message`, `delete_message` and `fetch_media` mapped
`SlackError::Auth` straight to `ChannelError::Auth`: the caller learned the
token was refused, the health surface learned nothing.

`react` is the one that matters. The ack emoji is the FIRST outbound call the
engine makes on an inbound message, so the most likely discovery point for a
mid-run revocation was also the silent one. All five surfaces now route through
one `publish_auth_expired` helper, with a bounded push (a dead token produces
one refusal per inbound message until the manager stops the loop; drop-oldest
keeps the newest `AuthExpired`).

### 3. Slack had no setup probe at all

`channel probe` answers "is this channel ready" without starting a gateway and
without sending a message. Slack implemented none, so it took the trait default
and reported `Unsupported`. That default is honest — it is not `Ok` and it does
not read as ready — but it left the one platform whose inbound is webhooks, and
therefore the one platform with no connection a bad token can be rejected on, as
the one an operator could not pre-check. `api::auth_test` already answered the
question. Slack now implements `probe()` with the three verdicts kept distinct:
`Incomplete` (fill the store), `Unauthenticated` (rotate the token),
`Unreachable` (no verdict reached — retry). The signing secret is checked for
PRESENCE only; no Slack API will say whether it is the right one, and claiming
otherwise would be the probe attesting to something it did not measure.

## Mutant evidence — every hunk can fail

Individually reverted by exact-string replacement asserting **exactly one**
occurrence (a silently-missed revert cannot masquerade as a green), with the
tests left untouched, `os.utime` after every write so cargo cannot serve a stale
artifact, and a restore control at the end.

| reverted hunk | result | test that reddened |
|---|---|---|
| H1 `post_message_keyed` classifier | 59/2 FAILED | `every_credential_code_is_auth_on_chat_post_message`, `a_deactivated_bot_user_publishes_auth_expired_on_send` |
| H2 `add_reaction` classifier | 60/1 FAILED | `every_credential_code_is_auth_on_reactions_add` |
| H3 `post_mutate` classifier | 60/1 FAILED | `every_credential_code_is_auth_on_chat_update` |
| M1 `react` publish | 62/1 FAILED | `every_outbound_surface_publishes_auth_expired_on_a_refusal` |
| M2 `edit_message` publish | 62/1 FAILED | same |
| M3 `delete_message` publish | 62/1 FAILED | same |
| M4 `send_message` publish | 61/2 FAILED | `a_token_revoked_after_start_publishes_auth_expired_on_send`, `a_deactivated_bot_user_publishes_auth_expired_on_send` |
| **restore control** | 63/0 (then 67/0 with the probe) | — matches baseline exactly |

Negative controls are part of the suite, not an afterthought: a
`channel_not_found` stays `SlackError::Api`, a `message_not_found` on an edit
publishes NO `AuthExpired`, and a 5xx on the probe is `Unreachable` not
`Unauthenticated`. Without these, a site that answered "credential refused" to
everything would pass every positive test and would drive a live channel to
"rotate your token" the first time somebody posted to an archived channel.

## Live evidence — both directions, real platforms, no mocks

Credential premise verified in the same session before any arm was built on it
(`invalid_auth` / HTTP 401 for bogus, `ok:true` / HTTP 200 for real), so the
refusals below are issued by slack.com and by Discord's own gateway.

### Running gateway — `channel health` (binary `83ff14db`, HEAD `d9e7c8ee`)

| arm | credential | platform evidence in the gateway log | health | sticky 20s | `--require-healthy` |
|---|---|---|---|---|---|
| slack rejected | bogus | `invalid_auth`=1, manager terminal=1 | **`unauthenticated`** | yes | **rc=1** |
| slack real | **live token** | none | **`healthy`** | yes | **rc=0** |
| discord rejected | bogus | `4004`=1, gateway stop=1, manager terminal=1 | **`unauthenticated`** | yes | **rc=1** |
| discord real | **live token** | none | **`healthy`** | yes | **rc=0** |

Reasons carried: `adapter reported auth expired: slack auth.test rejected the
bot token: invalid_auth` and `... discord gateway closed with 4004
(authentication failed): the bot token was rejected at IDENTIFY`.

Each arm ran in its own `WAYLAND_HOME`, asserted the handle was actually in the
store before measuring, asserted the gateway was alive and that exactly one
process owned the home, and tore down by PID with the death verified. Raw logs:
`live/chc-*.log`.

### No gateway — `channel probe` (binary `645ac613`, HEAD `dd2a866f`)

| arm | slack | discord | rc |
|---|---|---|---|
| real | **`ok`**, identity `<user>/Trade Canyon, Inc.` | `ok`, bot id | 0 |
| bogus | **`unauthenticated`**, finding `invalid_auth` | `unauthenticated`, HTTP 401 | 1 |
| absent | **`incomplete`**, names both handles | `incomplete`, names the handle | 1 |

Raw log: `live/probe-three-arms.log`. This is the surface that did not exist for
Slack before this lane — the `real` and `bogus` rows are the first time a Slack
credential verdict has been available without starting a gateway.

## Not proven — stated plainly rather than papered over

1. **A genuine revocation was never performed.** Every rejected arm uses a
   well-formed bogus token containing no byte of any real credential. Revoking
   the live Trade Canyon Slack token or resetting the Discord bot token is a
   credential action reserved for Sean. What the arms prove is that the platform
   issues a real refusal and the product classifies it correctly; what they do
   not prove is the *transition* from accepted to refused on the same token.

2. **The outbound-refusal path (fix 1 and fix 2) is NOT live-proven.** Reaching
   it requires a token that passes `auth.test` at `start()` and is then refused
   on a later call — i.e. exactly the revocation above. Bogus tokens are caught
   at `start()` and never reach `react` / `edit` / `delete`. These two fixes rest
   on mock + mutant evidence only, and I am not going to call that live. The
   live arms above exercise the start-time and gateway paths, which is a
   different code path from the one I changed.

3. **Slack still cannot detect a mid-run revocation on an idle channel.** Slack
   inbound is webhooks; there is no poll to slack.com. If the token dies while
   the gateway is up and nothing is sent, health stays `Healthy` until the next
   outbound call or a restart. Fix 2 shortens that window to "the next reaction,
   edit, delete, send or media fetch" — for a channel the agent is actually
   answering on, that is the next inbound message. For a genuinely idle channel
   it is unbounded. Closing it fully needs a periodic `auth.test` watchdog; that
   is a new background task with its own rate-limit and flap questions, it was
   not asked for, and I would rather report the boundary than smuggle it in.

## Credential handling

Real credentials reached hetzner on **ssh stdin only** — never argv, never a
file this lane wrote. They reach disk only inside each arm's private
`WAYLAND_HOME`, through the product's own stdin-only `channel credential set`,
which is itself part of what is under test. Every arm home was removed.

The vault needed unlocking for `channel credential set` to store anything on a
headless host (`WAYLAND_VAULT_PASSPHRASE`, a per-run random value generated on
hetzner, never printed, never committed). The harness's `handle_in_store`
precondition CAUGHT the first run where this was missing and refused to report a
measurement — the instrument working as designed.

Leak sweep over this evidence directory: **0 hits** for each of the three real
secrets, with `unauthenticated` as a known-positive returning 6 hits (so the
sweeper was looking at the right bytes).
