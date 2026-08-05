# 24-C3 LIVE — lane/discord-live NOTES (append-only, committed continuously)

Base SHA asserted: `43c69ca71bc788dcd925fc070204d6918c2d7e0f` (matches brief's `43c69ca7`).
Worktree: `/Users/seandonahoe/dev/waylandcore-frontier-worktrees/lane-discord-live`.

## Mission

Close the gap six lanes declined: **no message was ever sent or received against a live
Discord.** Prove 5 capabilities through the PRODUCT, corroborated by an independent observer.

1. send  2. edit  3. delete  4. receive (inbound, non-empty content)  5. **outbound
idempotency across a real restart** — the high-value one.

## Premise verification (LANE-BRIEF: "your brief's MEASUREMENTS are probably stale")

| Brief claim | Verified? | Note |
|---|---|---|
| `docs/delivery-semantics.md` puts Discord in exactly-once | **TRUE** | §2 row + machine-readable block line `discord = exactly-once` |
| that row rests on "a mockito test with an unbounded dedup window" | **TRUE** | row's last column reads verbatim `**No — mock only.**`, window open as `BL-24C1-DISCORD-WINDOW` |
| exactly-once scoped to delivery id `cron:{job}:{scheduled_millis}` | **TRUE** | `docs/delivery-semantics.md` §4 cites `wcore-cron/src/runner.rs:324-338` |
| `24-C3` declined by prior lanes | **TRUE** | ledger:868 "`24-C3` is still NOT MET and the repairing lane declines to claim it" |

So the doc is **already honest** that Discord is mock-only. My job is not to catch a lie; it is
to replace a labelled unknown with a measurement, and to change the Guarantee cell if the
measurement dissents.

## Architecture traced (before touching the network)

- `LedgeredHandler::dispatch_fire`, `crates/wcore-gateway/src/automation.rs:143-237`.
- Restart path that matters: state `Attempted` + outcome UNKNOWN + `destination_dedupes==true`
  → falls THROUGH the abandon arm (`:201`, guarded on `!destination_dedupes`) → `begin_attempt`
  (`:218`) → `self.inner.dispatch_fire` re-sends **with the same delivery id**.
- Discord's key on the wire: `rest::nonce_for_key`, used `wcore-channel-discord/src/lib.rs:170-172`.
- **The claim under test is therefore precisely:** a second send carrying an identical `nonce`,
  separated by a real process restart, yields ONE message at Discord. If Discord's dedup window
  is shorter than a restart, this is at-least-once in practice and the table must change.

## Instrument discipline for this lane

Per LANE-BRIEF §3b: every number redirected to a file and read with the Read tool, never
through Bash. Every absence gets a known-positive in the same capture. Observer must be proven
able to see a FAILURE (read a message id that does not exist → expect 404) before any of its
200s are trusted.

## Observer control — PASSED IN BOTH DIRECTIONS before any product run

`/tmp/lane-discord-live/observer-control.txt`:

```
me_code=200            <- known-positive: GET /users/@me
nonexistent_code=404   <- known-NEGATIVE: GET .../messages/000000000000000001
chan_code=200
bot_id 1532224324075913297 user WaylandCoreBot
chan_id 1532226655102173318 name general guild 1532226655102173315
```

The 404 is the load-bearing one: it proves the observer can report a message's ABSENCE, so a
later 404-after-delete is a real reading rather than a dead instrument. Bot id, channel id and
guild id all match the brief exactly.

hetzner egress verified before assuming any failure is the product's:
`curl https://discord.com/api/v10/gateway` → `discord_https=200`, 986G free, 96 cores.

## PREMISE REFUTED #1 — the ledger's "native actions" claim is STALE

Ledger `.planning/CRITERIA-GAP-LEDGER.md:824-825` and `:868`:
*"**media and native actions remain untouched for every adapter**"* and *"media and native
actions remain at zero"*.

**False for Discord at HEAD.** `wcore-channel-discord/src/lib.rs:465-472` declares
`.edit(Implemented).delete(Implemented).react(Implemented).typing(Implemented)`, and
`async fn edit_message` (`:475`) / `async fn delete_message` (`:502`) are real overrides
calling `rest::edit_message` / `rest::delete_message`.

## FINDING F24-C3-D1 — implemented, declared, and UNREACHABLE from the product

Searched with `/usr/bin/grep`, captures in `/tmp/lane-discord-live/action-surface.txt` and
`edit-on-callers.txt`:

| Search | Result |
|---|---|
| `.edit_message(` / `.delete_message(` in wcore-cli, wcore-gateway, wcore-agent, wcore-tools, wcore-protocol | **0** (rc=1) |
| known-positive, same tool, same dirs: `.send_message(` | **6 hits** — instrument alive |
| manager wrappers `edit_on` / `delete_on` callers, whole `crates/` | **only tests** — `framework_matrix.rs:416,421,441,443`, `native_action_matrix.rs:265,270` |

`wayland-core channel --help` offers `list / probe / health / reload / actions` — **no edit,
no delete.** So the native-action capability is real at the adapter and has **zero
operator-reachable surface**. `channel actions` will happily report Discord can edit and
delete; nothing in the shipped binary can ask it to.

Consequence for this lane, stated up front rather than discovered at the end: capabilities 2
and 3 cannot be driven through a shipped operator verb. They are driven through the
**production factory** (`channel_factory_for`, the same constructor the binary uses) against
real Discord, and the missing surface is reported as a defect rather than papered over.

## Live environment (hetzner, `/root/wl-discord-live-home`)

Real config, real credential, real destination. Token reached hetzner **on stdin only** — never
in `argv`, never in a log, never in this repo. Written by the product's own plaintext
credentials backend shape (`[secrets]`, mode 600) at `$WAYLAND_HOME/credentials.toml`.

Setup bug that was MINE, not the product's, recorded so the next lane does not lose the hour:
`discord.live.bot_token = "…"` is a TOML **dotted key** and nests as
`secrets.discord.live.bot_token`. `PlaintextCredentialsStore::get` looks up the FLAT key, so it
read `None` and the product correctly reported *"credential … is not present"*. The key must be
**quoted**: `"discord.live.bot_token" = "…"`.

## CAPABILITY: setup/auth — PASS

`wayland-core channel probe`, `PROBE_RC=0`:

```
1532226655102173318 (discord)
  outcome:  Ok
  config:   complete
  auth:     authenticated
  identity: 1532224324075913297
```

The identity the PRODUCT authenticated as equals the bot id I obtained independently from
`GET /users/@me` before the product ran. First live product↔Discord contact on this programme.

## CAPABILITY 1: send — PASS

Driven end-to-end through the shipped binary: `cron add --trigger once:… --channel … --text
WL-LIVE-SEND-1785382891`, then `gateway run`. No curl in the send path.

| | |
|---|---|
| baseline, marker absent before the run | `baseline_marker_hits= 0` (channel had 1 unrelated message) |
| after the product ran, read by the independent observer | `MARKER_ARRIVALS= 1` |
| author | `WaylandCoreBot`, `bot=True` |
| content | `'WL-LIVE-SEND-1785382891'` — exact |

Delivery id: job `7eacc300-1c2d-4661-984c-8bde6eb33348`, trigger `once:2026-07-30T03:42:26Z`.

Note on corroboration: `nonce` reads back as `None` on a **history** GET. That is not evidence
the nonce was absent — Discord does not echo `nonce` in `GET /channels/{id}/messages`. I do NOT
claim key-on-wire from this read; it is measured separately in the idempotency section.

## DEFECT F24-C3-D2 (HIGH) — the Discord inbound WebSocket cannot connect AT ALL

**This is why six lanes never proved inbound: in the shipped binary it cannot work.**

```
thread 'tokio-rt-worker' panicked at rustls-0.23.40/src/crypto/mod.rs:249:14:
Could not automatically determine the process-level CryptoProvider from Rustls crate features.
Call CryptoProvider::install_default() before this point to select a provider manually, or
make sure exactly one of the 'aws-lc-rs' and 'ring' features is enabled.
```

Measured over one ~120 s `gateway run`, with a known-negative control in the same capture:

| | |
|---|---|
| `Could not automatically determine the process-level CryptoProvider` | **84** |
| known-negative `ZZZ-NOT-PRESENT-CONTROL` (same file, same tool) | **0** — instrument alive |
| `forcing supervised reconnect` | **84** — one per panic, a tight loop |

**The discriminating control is that outbound worked in the same process.** REST send (reqwest)
succeeded and the message arrived; only the Gateway WebSocket path panics. So this is not
network, not the token, not hetzner egress — it is the WS TLS stack having no installed
`CryptoProvider`.

**The false line:** after every panic the supervisor logs
`INFO channel reconnected; resuming polling`. Nothing reconnected and nothing is polling. A
reader of that log sees recovery 84 times over.

**Credit where due, measured rather than assumed:** `channel health` does NOT claim healthy —
it reports `state: Degraded`, `reason: supervised reconnect in progress`, `errors: 5`,
`reconnects: 16`. Two real problems remain: the reason implies a transient state when the loop
is permanent, and **`HEALTH_RC=0`** — health exits 0 while Degraded, so it cannot be used as
the deployment gate `channel actions --require` is shaped to be.

## Privileged intent — verified MYSELF, as instructed

`GET /applications/@me` → `app_flags= 565248`:

```
GATEWAY_MESSAGE_CONTENT=False
GATEWAY_MESSAGE_CONTENT_LIMITED=True   <- the toggle IS enabled
```

Bit 19 (`_LIMITED`) is the flag Discord sets for an unverified app in <100 guilds whose
MESSAGE CONTENT toggle is on; bit 18 is the verified-app equivalent. So the owner's claim
holds and inbound content would be non-empty **if the socket could connect**. The inbound
failure is ours, not a missing intent.

## CAPABILITY 5: outbound idempotency — **FAIL. The doc is WRONG and must change.**

### 5a. The platform measurement — Discord's `nonce` does not deduplicate. At all.

`BL-24C1-DISCORD-WINDOW` asked how long Discord's dedup window is. **The window does not
exist.** Same channel, same author, byte-identical nonce, replayed at four delays:

| delay | first id | second id | verdict |
|---|---|---|---|
| 0 s | 1532233150594289704 | 1532233156847992891 | **DUPLICATE** |
| 5 s | 1532233158874108034 | 1532233181867278427 | **DUPLICATE** |
| 30 s | 1532233187801960489 | 1532233320211943434 | **DUPLICATE** |
| 90 s | 1532233322401370353 | 1532233706088038480 | **DUPLICATE** |

Not even at **zero** delay. Three controls, because a permanently-DUPLICATE verdict would be a
permanently-red gate (§3b-iii) and worth nothing:

1. **The nonce is accepted, not rejected.** `POST` returns 200 and Discord **echoes it back**:
   `nonce_sent= wl715feade0e1664b3`, `nonce_echoed_in_create_response= wl715feade0e1664b3`. So
   the field is valid, well-formed and under the 25-char cap — the absence of dedup is not a
   malformed token.
2. **The comparator CAN report identity** — two GETs of one message compare equal (`True`).
3. **A same-id outcome is reachable through this very API** — `PATCH` returns the same id as
   the `POST` (`True`). So "DEDUPED" is an achievable world; the gate can pass.

`nonce` is a client-side reconciliation echo, not a server-side idempotency key.

### 5b. The product measurement — a real restart produced a real duplicate

Outcome-unknown was manufactured honestly, through the adapter's **own** `api_base_url` seam
(the field `config.rs:52-70` says exists for exactly this): a local proxy forwards the create
to real Discord and then never responds. The message lands; the product never learns the
outcome. That is precisely the `F24-C-H1` shape, not a simulation of it.

| step | evidence |
|---|---|
| trigger is `once:` — **cannot fire twice** | `cron add --trigger once:2026-07-30T03:52:46Z`, job `97ce67c3-52f0-48da-92e1-80692363a555` |
| attempt 1 reached real Discord, with the key on the wire | proxy: `FORWARDED id=1532234475344498829 nonce=wle82e6651cfa60bb8` |
| gateway 1 start state | `carried=0 (unattempted 0 / unknown-outcome 0)` |
| gateway killed `-9` mid-send | `GW1_KILLED alive=no` |
| **gateway 2 start state — the product's own words** | **`carried=1 (unattempted 0 / unknown-outcome 1)`** |
| `gateway abandoned`, before and after | `No abandoned deliveries recorded` — the spine never abandons Discord, because Discord claims it dedupes |
| **arrivals at Discord, baseline 0** | **`ARRIVALS_AT_DISCORD= 2`** — `1532234475344498829` @03:52:46.462 and `1532234524996538478` @03:52:58.300 |
| known-negative control on the log scan | `0` for an absent string — instrument alive |

**The nonce on the wire is provably the product's derivation from the delivery id:**

```
nonce_for_key('cron:97ce67c3-52f0-48da-92e1-80692363a555:1785383566000') = wle82e6651cfa60bb8  MATCH
nonce_for_key(... millis+1)                                             = wle82e6751cfa60d6b  correctly no match
```

So this was one delivery id, replayed across a genuine process restart, carrying the identical
key — and it produced **two messages**. It is not the `F24-GWP-H1` "second scheduler mints a
new id" confound: a `once:` trigger has exactly one scheduled instant, and the gateway itself
reported the delivery as **carried, unknown-outcome** rather than as a new fire.

### 5c. Why this is WORSE than the seven at-most-once adapters

`supports_outbound_idempotency() == true` makes the spine take the `:216-220` re-attempt arm
instead of the `:201-215` abandon arm. So for Discord the gateway **deliberately re-sends a
possibly-delivered message**, on the strength of a suppression the destination does not
perform. The seven honest `false` adapters abandon and hand the operator a nameable delivery;
Discord silently posts the second copy. **A false `true` is more dangerous than an honest
`false`** — which is the exact argument `delivery-semantics.md` §6 already makes, applied to
the row that violates it.

## CAPABILITIES 2 and 3: edit and delete — PASS

Driven through the **production registration path** (`auto_register_from_dir` + `ChannelManager`
— the same functions `gateway run` uses at `wcore-cli/src/gateway.rs:929`), because no operator
verb reaches them (finding F24-C3-D1 above).

`cargo test -p wcore-channels-registry --test live_discord_actions -- --ignored`:

```
LIVE_SENT id=1532235755945070672 conv=1532226655102173318
LIVE_EDITED id=1532235755945070672
LIVE_EDIT_KNOWN_NEGATIVE_ERR=Rejected("404: Unknown Message")
LIVE_DELETED id=1532235755945070672
LIVE_DELETE_KNOWN_NEGATIVE_ERR=Rejected("404: delete: status 404 Not Found")
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
```

Executed count read back, per §3.2 — this is not a suite that ran zero tests.

**Both directions, in-process:** editing a nonexistent id fails 404; deleting an
already-deleted id fails 404. **Both directions, independently at Discord:**

| check | result |
|---|---|
| deleted message `1532235755945070672` | `deleted_msg_http=404` |
| known-positive in the same capture — a message that still exists | `known_positive_existing_msg_http=200` |
| edited message `1532236030105616524` content now | `'WL-EDITPROOF-1785383936-AFTER'` |
| the pre-edit text | absent (`BEFORE_absent= True`) |
| Discord's own edit marker | `edited_timestamp= 2026-07-30T03:58:57.461Z` |

The `edited_timestamp` matters: it is Discord recording an EDIT, which distinguishes a real
edit from "deleted the old one and posted a new one".

## CAPABILITY 4: receive — PARTIAL. Transport now proven; the last hop needs a human.

It was **impossible** before this lane: the socket panicked on every connect (F24-C3-D2). After
the fix, at SHA `cf857965`, with `RUST_LOG=wcore_channel_discord=debug`:

```
READY received; session captured for resume                     -> 1
MESSAGE_CREATE received from the Discord gateway
    channel_id=1532226655102173318 author_is_bot=true content_len=50
CryptoProvider panics                                           -> 0   (was 84)
known-negative control                                          -> 0
```

Two things are proven that never had been:

1. **The privileged intent is genuinely live.** The adapter IDENTIFYs with `intents=37376`
   (`GUILD_MESSAGES | MESSAGE_CONTENT | DIRECT_MESSAGES`). Discord closes a connection with
   **4014 Disallowed intent** when a privileged intent is requested but not granted. We got
   READY instead, which is the platform itself confirming the grant — independent of the
   app-flags read I did earlier, and stronger.
2. **A real MESSAGE_CREATE crossed the live socket carrying non-empty content**:
   `content_len=50`, exactly the length of the body I posted (`expected_content_len=50`).

**What is NOT proven, stated plainly:** that a **human-authored** message becomes an
`IncomingMessage` on `poll_events`. `map_message_create` (`gateway.rs:322-325`) drops **every**
bot-authored message, not merely its own — so the bot cannot source its own inbound event, and
neither can a channel webhook (Discord marks webhook messages `author.bot = true` too). I hold
no human Discord account and credentials are Sean-reserved, so I did not run this leg.

**It is now a one-line test for any human:** with the fixed binary running, type anything in
`#general` and the debug log will show `author_is_bot=false` with a non-zero `content_len`.
Before this lane that experiment would have measured nothing, because the socket never opened.

## Doc + capability correction (the required outcome)

`supports_outbound_idempotency()` for Discord: `true` → **`false`**.
`docs/delivery-semantics.md`: 3-of-10 → **2-of-10** exactly-once, 7 → **8** at-most-once, the
Discord row rewritten, the code-provenance table, the Windows §5 note, §6, the machine-readable
block (`discord = at-most-once`), plus a new **§8** recording the measurement and its controls.

**The enforcement gate proved it can fail before it was updated.** With the capability flipped
and the census test still expecting three, the run went red exactly where it should:

```
test exactly_three_adapters_are_exactly_once ... FAILED
delivery_semantics_declaration.rs:302
test result: FAILED. 7 passed; 1 failed
```

That is direction-1 evidence for the doc/code binding (§3.2) obtained for free. Renamed to
`exactly_two_adapters_are_exactly_once` and green after.

Final run, hetzner, SHA `6170d6d6`:
`wcore-channels-registry + wcore-channel-discord + wcore-gateway + wcore-channels` →
**304 passed, 0 failed, TEST_RC=0** (3 ignored = my two live tests + 1 pre-existing);
`wcore-cli --test f24_c1_outbound_idempotency` → **6 passed, 0 failed, rc=0**.

## Secret hygiene

Token reached hetzner on **stdin only**, never `argv`, never a log, never an evidence file. The
hang proxy has `log_message` stubbed out precisely because the Authorization header transits it.
Sweep over all 8 committed files with `/usr/bin/grep -c -F`:

```
0  (each of the 8 changed files)
1  known-positive-control.txt   <- the sweep is alive
0  files-with-hits-in-evidence-dir
```

## Log

- [t0] Worktree created, SHA asserted, brief + delivery-semantics + ledger 24-C3 rows read.
- [t0] NOTES committed before any network work.
- [t1] Observer control passed both directions. hetzner egress + build (1m49s) OK.
- [t1] Premise refutation #1 and finding F24-C3-D1 recorded.
- [t2] Live home built on hetzner; probe authenticated as the real bot.
- [t2] SEND PASS, corroborated 0→1 at Discord by the independent observer.
- [t3] DEFECT F24-C3-D2 found: inbound WS panics 84× in 120 s; health says Degraded, rc=0.
- [t3] MESSAGE CONTENT intent verified enabled from Discord's own app flags.
