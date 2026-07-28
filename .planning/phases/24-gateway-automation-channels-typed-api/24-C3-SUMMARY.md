---
phase: 24-gateway-automation-channels-typed-api
criterion: "24-C3 (reference channels / the inbound matrix)"
lane: 24-c3
branch: lane/24-c3
status: partial
grade-24-C3: "NOT MET. The inbound half now has an instrument and is proven end to end for admit/dedupe/access/bind/route on THREE webhook adapters on Linux, against fixtures, with arrivals derived from an independent sink's journal. It is NOT proven for either of the two adapters 24-03 designated as the REFERENCE pair — neither Discord's nor email's inbound path can be pointed at a fixture from configuration — and four of the criterion's eight named clauses (media, native actions, reconnect/reload, health) are untouched on the inbound path. macOS and Windows are reproductions of a defect at older commits, not matrix results."
merge-base: 15ad7b0e0ae51f052057ce8d211f5982c3e6f514
head: bf9590179b9c04a5f0d8db0a4e4a4a0b3e0dd8b0
findings: "F24-C3-H1 HIGH fixed+mutation-proved; F24-C3-H2 HIGH open; F24-C3-H3 HIGH fixed+mutation-proved"
---

# 24-C3 — the inbound channel matrix

**One sentence: the inbound half was not merely unproven, it was broken — under
an isolated profile every channel silently denied every message its operator
had allowlisted, an inbound SMS reply was addressed to the bot's own phone
number while every sender shared one agent session, and the persistent gateway
runtime — the thing installed as a systemd unit — binds no inbound receiver at
all; two of those three are now fixed and live-proven, the third is open, and
the criterion is still NOT MET because the two adapters this phase designated as
its reference pair have no fixture seam on their inbound path.**

Nothing here was merged, pushed to `main`, tagged, released, or used to close an
issue. No requirement is marked complete. No credential belonging to anyone was
read, embedded, or transmitted; every secret in every run was minted by the
driver at run time.

---

## 1. The criterion, verbatim

> **"Reference channels prove setup/auth, access, routing, media, native
> actions, idempotency, reconnect/reload, and health."** — `ROADMAP.md:119`

`24-03` graded this **PARTIALLY MET on Linux, NOT MET on macOS or Windows** and
named its own unmet clause precisely: *"the end-to-end inbound matrix from the
binary against a fixture (admit → dedupe → access → bind → route)"*. That is what
this lane went after.

---

## 2. What did not exist before, and now does

| Artifact | What it is |
|---|---|
| `scripts/f24-inbound.mjs` | The inbound matrix driver. Drives the **shipped binary** in `--json-stream` with a real, correctly-signed platform webhook per adapter, and derives every count from the independent sink's journal. |
| `scripts/f24-llm-fixture.mjs` | A deterministic OpenAI-wire chat-completions endpoint as its own OS process, with its own journal. The inbound path ends in an agent turn; a turn needs a model. |
| `scripts/f24-gateway-inbound-probe.sh` | Asks a **running gateway** whether anything is listening on its own configured webhook bind. |
| `scripts/f24-c3-tests.sh`, `scripts/f24-c3-mutations.sh` | The crate gates, and the mutation proofs for both fixes. |
| additive endpoints in `scripts/f24-sink.mjs` | WhatsApp `/{ver}/{phone}/messages` and Twilio `/2010-04-01/Accounts/{sid}/Messages.json`, in the same `Arrival` record shape, so all three adapters land in **one** journal and their numbers are comparable. The journey never calls them, so no count it takes can change. |

**Reuse, as instructed.** `f24-sink.mjs` is lane 24-journey's, unchanged in
behaviour; the two endpoints added are new paths only.

### What was refused as evidence

Not that a handler registered. Not that a config parsed. Not that a call
returned `Ok`. Not a status line the product printed about itself. Every arrival
number below is read out of the journal of a process the binary does not own and
cannot write to except by completing a real TCP round trip — and is
cross-checked against a **second** journal written by the fixture model, so a
leg reporting zero is distinguishable from a leg that never ran.

---

## 3. The matrix — Linux, at `e88cf43f`, 15/15 GREEN

```
INBOUND MATRIX GREEN platform=linux legs=15 failed=0 arrivals_total=9 turns_total=9
```

Binary identity, read out of the binary and hashed by the driver:

```
wayland-core 0.12.25 (source e88cf43f33551f2f7c005391004fbfdb9af50a46)
sha256 b390e6005009ef6f497513d4c6a32c9e795a7350ffc21e2fa73345da71bed92e
```

| leg | slack | whatsapp | sms (twilio) |
|---|---|---|---|
| **admit** | PASS 1 arrival, 1 turn | PASS 1/1 | PASS 1/1 |
| **route** | PASS `D24C3ONE` | PASS `15552220000` | PASS `+15553330000` |
| **dedupe** | PASS 1→1 on replay, control 1 | PASS | PASS |
| **access** | PASS 0 arrivals, 0 turns, control held | PASS | PASS |
| **bind** | PASS `D24C3ONE` ≠ `D24C3TWO` | PASS two peers distinct | PASS two peers distinct |

The arrivals journal, in full — nine lines, each one a real HTTP round trip to a
listener in another process:

```
1 chat.postMessage    D24C3ONE      'F24C3-REPLY f24c3-slack-admit-d4412178'
2 chat.postMessage    D24C3ONE      'F24C3-REPLY f24c3-slack-dedupe-control-d4412178'
3 chat.postMessage    D24C3TWO      'F24C3-REPLY f24c3-slack-bind-d4412178'
4 whatsapp.messages   15552220000   'F24C3-REPLY f24c3-whatsapp-admit-8b4bc079'
5 whatsapp.messages   15552220000   'F24C3-REPLY f24c3-whatsapp-dedupe-control-8b4bc079'
6 whatsapp.messages   15552221111   'F24C3-REPLY f24c3-whatsapp-bind-8b4bc079'
7 twilio.messages     +15553330000  'F24C3-REPLY f24c3-sms-admit-22d3ad01'
8 twilio.messages     +15553330000  'F24C3-REPLY f24c3-sms-dedupe-control-22d3ad01'
9 twilio.messages     +15553331111  'F24C3-REPLY f24c3-sms-bind-22d3ad01'
```

Each reply carries the correlation token of the message that caused it, so
"a reply arrived" and "**this** message's reply arrived" are different claims and
the table asserts the second.

### The same driver at the pre-fix binary: 12 of 15 RED

```
INBOUND MATRIX RED platform=linux legs=15 failed=12 arrivals_total=0 turns_total=0
```

Run at `15ad7b0e`, the merge-base, with the identical driver and the identical
fixtures. **That is the falsifier for everything in the green table**: the same
instrument, same host, same commit-day, reports zero when the defect is present.

---

## 4. Three findings

### F24-C3-H1 — HIGH — the inbound access policy ignored `WAYLAND_HOME`. FIXED, mutation-proved, reproduced on two more platforms.

`AgentBootstrap` registered the adapters through
`wcore_channels_registry::channels_dir()`, which honors `WAYLAND_HOME`, and then
loaded their `[inbound]` access policy and tool posture through
`ChannelConfigLoader::default_root()`, which joins `$HOME/.wayland/channels`
unconditionally.

Under an isolated profile — **every gateway unit, every `--profile`, the desktop
host** — the second lookup found nothing, so every channel fell back to
`InboundPolicy::default()`, which is fail-closed. The adapter registered,
started, polled and reported healthy while **denying every inbound message the
operator had allowlisted**. This is precisely the accepts-persists-lists-and-
never-does-the-thing shape.

Diagnosed by controlled experiment rather than by reading: with the profile's
own config also copied to `$HOME/.wayland/channels`, the identical message was
admitted and dispatched. Nothing else changed.

```
inbound denied  channel=f24c3slack  reason=sender not in dm allowlist
```

…for `U24C3ALLOWED`, named in that profile's own `dm_allowlist`.

**It broke in the other direction too.** On a host whose `$HOME/.wayland/channels`
does hold configs, an isolated profile applied the **host user's** allowlists and
tool posture — including `tools = "full"` — to a different profile's channels.
That is the same cross-profile leak F-019 closed for registration; the doc
comment on `channels_dir` already asserts the two loaders "never diverge", and
they did. The mutation proof reddens on exactly this direction:

```
M1 mutated:   rc=101 passed=0 failed=1   →  left: ["hostchannel"]  right: ["profilechannel"]
M1 restored:  rc=0   passed=1 failed=0
```

Fixed by routing the policy loader through the same `channels_dir()`, extracted
as `bootstrap::load_channel_policy_configs` so the invariant is testable.

### F24-C3-H2 — HIGH — the persistent gateway runtime hosts no inbound at all. OPEN, deliberately not bodged.

`run_gateway` registers the adapters and calls `start_all()`, so they poll — and
then constructs **no `InboundSubscriber`** on the `ChannelManager` broadcast and
spawns **no inbound webhook host**. Both live only in `AgentBootstrap`, which the
gateway does not use. Inbound dispatch is opted into at exactly three call sites,
all of them interactive/host sessions (`main.rs` `run`, `run_tui_mode`,
`run_json_stream_mode`); `gateway run` is not one of them.

So the surface Phase 24 installs as a systemd unit, a launchd plist and a
scheduled task — the thing an operator actually runs — polls its adapters and
drops every inbound event on the floor.

Asked of the running gateway directly, not read off the source:

```
=== the question: is anything listening on 127.0.0.1:18787/healthz ? ===
webhook probe rc=7 (0 = something answered, 7 = nothing listening)
RESULT: the running gateway binds NO inbound webhook host despite
        [inbound_webhook] enabled = true.
```

…with the gateway alive (`kill -0 rc=0`) and its own `[inbound_webhook] enabled
= true` in force. The probe fails in both directions: it exits 1 if the port
answers, and 3 if the gateway never came up, so it cannot pass by accident.

**Not fixed.** Wiring inbound into the gateway means giving it a provider, an
engine pool and a turn dispatcher it does not currently construct. That is a
scoped change to the gateway lifecycle, not a flag, and improvising it at the end
of a lane is how a fix becomes the next lane's defect.

### F24-C3-H3 — HIGH — an inbound SMS replied to the bot's own number, and every sender shared one session. FIXED, mutation-proved.

`pairs_to_incoming` set `conversation_id` to the Twilio `To` field — the bot's
own number — reasoning that this groups each `(From, To)` pair into one
conversation. **A deployment has one Twilio number**, so `To` is a constant.

Caught by the `route` leg reading the sink's journal:

```
FAIL sms/route: conversation_id="+15550009999" want="+15553330000"
```

`+15550009999` is the bot's configured `from_number`. The reply was addressed to
the bot itself; the human who texted received nothing.

The session consequence is the more serious one. `build_session_key` for
`ChatType::Direct` is `agent:main:{channel}:dm:{conversation_id}` with no sender
component — correct on every platform where a DM conversation id identifies the
peer, and a **cross-person context leak** on the one where it did not: every
distinct human texting the bot shared one session and one history.

Fixed by making the conversation the peer; `account_id` keeps `To`, so which bot
number the message arrived on is not lost. Two in-crate assertions encoded the
old value and are updated **at the site, with the reason** — they asserted the
defect, and the new integration test asserts both directions plus a positive
control that the same peer still resolves to one session.

```
M2 mutated  — the_conversation_is_the_peer_not_the_bots_own_number:        RED
M2 mutated  — two_people_texting_the_same_bot_number_do_not_share_a_session: RED
M2 restored — both:                                                        GREEN
tree restored to HEAD: git diff --quiet rc=0
```

---

## 5. The adapters this lane did NOT measure, and exactly why

**Never rendered as a zero and never as a pass.** Each row is a source fact, not
an impression.

| adapter | inbound transport | fixture-pointable from CONFIG? | result |
|---|---|---|---|
| slack | webhook (Events API) | yes | **MEASURED**, 5/5 |
| whatsapp | webhook (Meta Cloud API) | yes | **MEASURED**, 5/5 |
| sms | webhook (Twilio) | yes | **MEASURED**, 5/5 |
| **discord** | persistent WS gateway | **NO** | **NOT MEASURED** |
| **email** | IMAP polling | partially | **NOT MEASURED** |
| telegram | polling `getUpdates` | **NO** | **NOT MEASURED** |
| msteams | webhook | n/a | **NOT BUILT** |
| matrix | `/sync` | yes (`homeserver_url`) | **NOT MEASURED** — feasible, not attempted |
| signal, imessage | local agent / local DB | not assessed | **NOT MEASURED** |

- **Discord.** `DISCORD_API_BASE` and `DISCORD_GATEWAY_BASE` are overridable only
  through `with_bases`, a `#[doc(hidden)]` test-only constructor. `DiscordConfig`
  is `deny_unknown_fields` and has no URL field, and the production factory
  `make_discord` calls `DiscordChannel::new`. **From the shipped binary, Discord's
  inbound can only ever reach `discord.com` with a real bot token, which nobody
  on this program has.** Supplying one is Sean's alone.
- **Email.** Host and port are configurable, but `poll_once` builds
  `native_tls::TlsConnector::new()` with default verification and `EmailConfig`
  has no CA or insecure option, so a local IMAP fixture needs a certificate the
  host trusts. On Linux this is reachable without touching the host trust store —
  OpenSSL honours `SSL_CERT_FILE`, so the fixture CA can be scoped to the child
  process. **Feasible; I did not get to it.** On macOS `native_tls` uses
  Security.framework, which ignores `SSL_CERT_FILE`, so that route is Linux-only.
- **Telegram.** No base-URL field in `TelegramConfig` at all — the API base is a
  constant. Same shape as Discord.
- **MS Teams.** `docs/channels.md` states it plainly: *"MS Teams inbound is parsed
  but **not** exposed over the host yet — its Bot Framework JWT validation is a
  pending follow-up."* Not a gap I found; a gap the project already declares.

**Discord and email are the two adapters `24-03` designated as its REFERENCE
pair** — "one driven by a persistent connection, one driven by polling". Their
inbound halves are exactly the two this lane could not reach. That is the single
most load-bearing sentence in this document and it is why the grade below is what
it is.

---

## 6. macOS and Windows — reproductions, not matrix results

Neither platform has a binary at the candidate commit, and cargo may not run on
the Mac. What both runs DO establish is that **the inbound webhook host binds and
the subscriber spawns on all three OS families** — previously unknown — and that
**F24-C3-H1 is not Linux-specific**.

- **macOS**, driving `wayland-core 0.12.25 (source a8ed7322)`, an existing debug
  binary: host bound on `127.0.0.1:18787`, subscriber spawned, three channels
  registered, then `inbound denied channel=f24c3slack reason=sender not in dm
  allowlist` — H1, reproduced verbatim on a second platform.
- **Windows**, driving `wayland-core 0.12.25 (source 978f49d7)` copied out of
  `C:\ferrox-win` (read only; sha256 `49A0A55E…`): reached the same point, host
  bound, then the same denial.

Both binaries **predate both fixes**, so their matrix legs are a foregone red for
a cause already diagnosed. Reporting them as matrix results would be dishonest,
so they are reported as reproductions. **The post-fix matrix on macOS and Windows
is NOT MEASURED**: building at the candidate commit on `seandesktop` would
contend with a lane that was actively writing to that tree an hour before this
run, which is the contention I was told to back off from.

---

## 7. Gates — every executed count read back

| Gate | Result |
|---|---|
| `cargo test -p wcore-channel-sms` | **26 passed** in-crate + **2 passed** in the new integration binary, 0 failed |
| `cargo test -p wcore-agent --test f24_c3_inbound_policy_home_test` | **1 passed**, and the test NAME echoed back |
| `cargo test -p wcore-channels` | **114 passed** + **17 passed**, 0 failed |
| `cargo clippy -p wcore-channel-sms -p wcore-agent -p wcore-channels --all-targets -- -D warnings` | rc=0 — **after** reddening at rc=101 on my own doc-comment seam, which is how I know it can fail |
| `cargo fmt --all -- --check` | rc=0 |
| Mutation M1 (policy home) | mutated **RED**, restored **GREEN** |
| Mutation M2 (SMS conversation), ×2 tests | mutated **RED** ×2, restored **GREEN** ×2 |
| Tree restored after mutations | `git diff --quiet` rc=0 |
| Inbound matrix, pre-fix binary | **RED, 12/15 failed** |
| Inbound matrix, post-fix binary | **GREEN, 15/15**, 9 arrivals / 9 turns |
| Gateway inbound probe | rc=0 = *nothing listening* (the finding); exits 1 if the port answers |

**Zero-test defence.** Each target is run **by file** (`--test <name>`), never by
filter — a filter matching no test name exits 0 having run nothing. The counts
above are read back from `N passed`, and because a count alone is satisfied by a
support module, the gate also echoes the specific test names. The mutation
harness grades **ran-nothing as its own third state**, distinct from pass and
fail, for the same reason.

**Two self-passing gates I wrote and then had to fix**, recorded because the
class matters more than the instances:

1. **The `access` leg passed on all three adapters at the pre-fix binary** — while
   every other leg failed. "The denied sender did not get through" is trivially
   true of a path nothing gets through. The admit control was printed *beside*
   the result instead of being *part of* it. Now conjoined; it reddens when the
   path is dead.
2. **The `bind` leg compared a conversation to itself** on the two peer-keyed
   platforms, so `distinct` could never hold. That was a driver defect reported
   as a product failure, and I caught it only by reading the values in the
   detail line rather than the PASS/FAIL.

Neither was found by the gate failing. Both were found by reading the numbers.

---

## 8. Honest grade

**Criterion 3: NOT MET.**

What moved: the inbound half now has an instrument, and `admit → dedupe → access
→ bind → route` is proven end to end from the shipped binary against fixtures for
**three** adapters on Linux, with arrivals derived from an independent process's
journal and cross-checked against a second journal. Two HIGH defects that made
the inbound path actively wrong — not merely unproven — are fixed and
mutation-proved.

What is still not true, stated without narrowing:

- **Neither designated reference adapter's inbound path has been driven**, and
  neither *can* be from configuration. Grading the criterion on the three
  adapters that happened to have a fixture seam would be narrowing it to the
  adapters that worked.
- **Four of the criterion's eight named clauses are untouched on the inbound
  path**: media, native actions, reconnect/reload, and health. The matrix covers
  access, routing, and idempotency-as-inbound-dedupe.
- **The persistent gateway cannot receive inbound at all** (F24-C3-H2, open). A
  criterion about channels proving themselves cannot be met while the runtime an
  operator installs has no inbound receiver.
- **macOS and Windows have no post-fix matrix**, only a reproduction of a defect
  at older commits.

`24-03` graded this PARTIALLY MET on Linux. I would now grade it **NOT MET
everywhere** and call that an improvement, because the earlier grade was taken
without knowing that the isolated-profile path denied everything.

---

## 9. Open, and for whom

| Item | Severity | Owner |
|---|---|---|
| **F24-C3-H2** — wire `InboundSubscriber` + inbound webhook host into `run_gateway` | **HIGH** | a scoped Core lane; needs a provider/engine/dispatcher in the gateway |
| Email inbound against a TLS IMAP fixture via `SSL_CERT_FILE` (Linux) | MEDIUM | feasible now; ~1 session |
| Matrix inbound against a `/sync` fixture (`homeserver_url` is configurable) | MEDIUM | feasible now |
| Discord/Telegram inbound have **no fixture seam** — a config-level base-URL override, or accept they are only ever provable with a vendor credential | MEDIUM (design) | Sean's call; the credential half is Sean-only |
| Inbound **media**, **native actions**, **reconnect/reload**, **health** legs | MEDIUM | the rest of the criterion's clause list |
| macOS + Windows post-fix matrix | MEDIUM | needs a build at the candidate commit on each |
| An isolated profile with no vault passphrase stores credentials plaintext-0600 and then refuses **every** turn with "Session persistence authority unavailable" | LOW (self-inconsistent default, host-wide not channel-specific — a plain one-shot fails identically) | BACKLOG |
| `channel auto-registered from ~/.wayland/channels` is logged even when the directory is `$WAYLAND_HOME/channels` | LOW | BACKLOG |
| `inbound webhook host listening` logged twice on one bind | LOW | BACKLOG |

Nothing in `crates/wcore-cli/src/{lib,main}.rs` was touched, so this lane has no
§6 fence exposure. No protocol seam changed; no contract fixture was regenerated.
