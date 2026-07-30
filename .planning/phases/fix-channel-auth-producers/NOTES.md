# NOTES — lane/fix-channel-auth-producers

Base: e7bc6d883027102ff1e5bbaa2dd19f9265268cab

## Measured so far (2026-07-30)

### The two enums the brief conflates

`Unauthenticated` names TWO different variants in this codebase:

- `wcore_channels::HealthState::Unauthenticated` (health.rs:37) — the running
  health surface. This is the UAT-C2 defect's subject.
- `wcore_channels::ProbeOutcome::Unauthenticated` (probe.rs:57) — the *setup
  probe* verdict, a separate surface that sends no traffic.

Discord ALREADY reaches `ProbeOutcome::Unauthenticated` (lib.rs:398 `probe()`,
test at lib.rs:1063). It does NOT reach `HealthState::Unauthenticated`. A census
keyed on the bare word "Unauthenticated" therefore over-counts Discord.

### Routes into `HealthState::Unauthenticated` — exactly two, both via poll_events()

manager.rs:396-433, inside the per-channel poll task:

1. `ChannelEvent::ConnectionStateChanged { state: ConnectionState::AuthError }`
   → `HealthState::from_connection_state` (health.rs:57) → Unauthenticated.
2. `ChannelEvent::AuthExpired { reason }` → Unauthenticated directly
   (manager.rs:416-426).

Both set `auth_rejected`, which `break`s the poll loop (manager.rs:446-455).
There is NO third route: a `poll_events()` **error** maps to Degraded +
supervised reconnect, never to Unauthenticated.

So: an adapter can only reach Unauthenticated by EMITTING one of those two
events from `poll_events()`.

### Census — which of 10 adapters emit an auth event (src/ only)

Query (unproxied, glob quoted):
```
/usr/bin/grep -rn "AuthExpired|ConnectionState::AuthError" --include="*.rs" \
  crates/wcore-channel-<c>/src/
```
Instrument liveness: the SAME invocation returned hits for matrix and telegram
(known-positive), zero for the other eight.

| adapter  | emits auth event | where |
|---|---|---|
| telegram | YES | longpoll.rs:96 `ConnectionState::AuthError` |
| matrix   | YES | sync.rs:344 `ChannelEvent::AuthExpired` |
| discord  | no  | — (has probe() only) |
| slack    | no  | — |
| email, imessage, msteams, signal, sms, whatsapp | no | — |

**True census: 2 of 10.** The orchestrator's earlier "0 of 10" was wrong (it
missed telegram). The brief's claim that telegram+matrix are the two existing
producers HOLDS.

### Slack — brief claim "no inbound poll loop at all"

Partially true, and the wording matters. Slack DOES implement `poll_events`
(lib.rs:220) but it only drains a local `inbox` fed by **webhooks**
(`inbound::parse_webhook`, lib.rs:140). There is no outbound poll to slack.com.

Critically: `start()` (lib.rs:163-202) resolves the bot token + signing secret
**from the credential store only** and immediately sets `Connected`. It never
contacts slack.com. So a rejected token is indistinguishable from a good one,
and the channel reports Healthy forever. That is the defect.

Slack has NO `probe()` impl (unlike Discord).

### Discord

`start()` (lib.rs:217-283) spawns `gateway::gateway_loop`. It calls
`rest::get_current_user_id` but that is **best-effort** — on failure it warns
and proceeds with `None`, so a rejected token does not fail start.
The gateway `4004` close code is the hook to check next.

## Still to establish

- What manager/CLI does on `start()` Err — is that the arm-2 `disconnected`
  path that names the handle?
- Discord gateway close-code handling in gateway.rs (does it see 4004?).
- Whether Slack's honest fix is an auth watchdog (`auth.test`) vs a full
  inbound loop.
