# NOTES — lane `twilio-whatsapp-identity`

Branch `lane/twilio-whatsapp-identity`, based on integration `4caaa31c`.
Append-and-recommit after every measurement (LANE-BRIEF §6b-i).

---

## Minute 0-15 — premise verification, before any edit

### P1. Twilio and WhatsApp adapters DO inherit the pass-through `send_message_idempotent`

Measured, unproxied grep over the four adapters that could plausibly override it:

```
/usr/bin/grep -n "send_message_idempotent|supports_outbound_idempotency|idempotency" \
  crates/wcore-channel-{slack,discord,sms,whatsapp}/src/lib.rs
```

25 hits. **All 25 are in slack/lib.rs or discord/lib.rs.** Zero in
`wcore-channel-sms/src/lib.rs`, zero in `wcore-channel-whatsapp/src/lib.rs`.
Known-positive in the same invocation: slack:234 `async fn send_message_idempotent`
and discord:325 the same — so the instrument was alive and the needle correct.

=> Both inherit `wcore-channels/src/lib.rs:123-129` pass-through (`_key` discarded)
and the trait default `supports_outbound_idempotency() -> false` at `:139`.
**The delivery key never reaches the wire for either.** Brief's premise HOLDS.

### P2. The sink hardcodes `idempotency_key: null` for both endpoints

`scripts/f24-sink.mjs`:
- `:132` — `chat.postMessage` reads `req.headers['idempotency-key']`.
- `:176` — `whatsapp.messages` calls `record(..., /*key*/ null, false)`.
- `:193` — `twilio.messages` calls `record(..., /*key*/ null, false)`.

=> **This is a SECOND, independent cause of `unidentified`.** Even if the adapter
put a key on the wire tomorrow, the sink would still journal `null` and the gate
would still read NOT-PROVEN. Any fix that touches only the adapter is untestable
through the journey gate. The brief did not mention this half.

### P3. The journey gate's identity comes from the SINK journal, not our ledger

`scripts/f24-journey.mjs:1235` reads `a.idempotency_key` off arrivals; `journey.rs`
`DeliveryIdentity.unidentified` counts arrivals with no identity.

=> **A platform-returned id (`sid` / `wamid.*`) recorded in OUR journal does NOT
reduce `unidentified`.** The sink is the destination; it mints those ids itself.
The brief's proposed mechanism ("record the platform-returned id against the
delivery id") is a real improvement to *our* reconciliation but is **not** the
thing the journey gate measures. Flagged as a premise to argue with, see SUMMARY.

### P4. `docs/delivery-semantics.md` already says Twilio/WhatsApp are at-most-once

Rows already read `at-most-once` / `abandoned` and the evidence column already
claims "**Yes** — a replayed key produced **two** messages". That claim needs
scrutiny: we hold no Twilio or Meta credentials, so where did a *real-destination*
replay measurement come from? Open question O1.

---

## Open questions

- **O1** — provenance of the "replayed key produced two messages" evidence cell for
  Twilio and WhatsApp in `docs/delivery-semantics.md`. If it came from the sink and
  not a real destination, the doc overstates.
- **O2** — what field can each platform carry a client-supplied token in?
  Candidates to verify against real docs, not memory:
  WhatsApp Cloud API `biz_opaque_callback_data`; Twilio `Messages.json` — unknown.
- **O3** — what does Wayland Desktop actually do? Read-only inspection pending.
