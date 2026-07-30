# SUMMARY — lane `twilio-whatsapp-identity`

Branch `lane/twilio-whatsapp-identity`, based on integration `4caaa31c`.
Build host `hetzner-dsm`, worktree `/root/wayland-twilio-whatsapp-identity`.

**Verdict: Phase A landed and is measured. Phase B is blocked on credentials and
two live cells are UNRUN. The brief's central premise held; its proposed
mechanism did not, and is argued against below with evidence.**

---

## 1. Does Wayland Desktop solve this? No. It only sends.

Read-only inspection of `/Users/seandonahoe/dev/wayland`. Nothing mutated,
nothing executed.

- **Twilio** (`SmsTwilioPlugin.ts`) calls the official npm SDK's
  `client.messages.create({to, body, from | messagingServiceSid, mediaUrl})` and
  returns `result.sid` upward. No idempotency token, no opaque field, and the sid
  is not persisted against anything.
- Worse: `sendWithRetry` retries **429 and 5xx with no token at all**, so a 5xx
  arriving after Twilio accepted the message produces a duplicate. Core does not
  do that — it abandons an outcome-unknown delivery and surfaces it.
- **WhatsApp** (`WhatsAppPlugin.ts` → `backends/meta-business.js`) posts
  `{messaging_product, to, type, text}` and reads `res.messages[0].id`.
  `rememberSentId(id)` is **echo suppression** (ignore our own message returning
  through `messages.upsert`), not delivery identity.
- `biz_opaque_callback_data` outside `node_modules`: **0 hits**. Control
  `messaging_product`: **3 hits** — instrument alive, absence real.
- Outbound delivery ledger: `deliveryId` 0 files, `delivery_id` 0, `DeliveryLedger`
  0, `outboundLedger` 0. Control `IUnifiedOutgoingMessage`: **55 files**.
  `exactly-once` appears in 27 files, 24 of them shipped `skills-library`
  markdown and the rest chat send-on-wake / workflow session — **none** outbound
  channel delivery.
- The seven files under `app/src/process/channels/` that mention "idempot" are
  `connection-tokens.ts`, `PairingService.ts`, `EmailImapConnection.ts`,
  `WeixinTyping.ts`, `GoogleChatPubSub.ts`, `tunnel/types.ts`,
  `gateway/PluginManager.ts`. **Neither tier1 plugin is among them.**

"Desktop has them working" is true and it means *messages arrive*, which was
already true of Core. **Nothing was taken from Desktop**, so no
`THIRD-PARTY-NOTICES.md` entry is owed, and the word that caused this week's
audit finding does not appear in this lane's output.

## 2. The brief's premise: three of four claims held, one is false

| brief claim | verdict |
|---|---|
| Both adapters inherit the pass-through and put no identity on the wire | **HELD.** All 25 `idempotency` hits across the four candidate adapters are in slack or discord; zero in sms or whatsapp |
| Both already parse a stable platform id and do nothing durable with it | **HELD.** `MessageReceipt.id` carries the sid / wamid, and `LedgeredHandler::dispatch_fire` receives only `Result<()>` — the receipt is discarded before the ledger |
| Both are `at-most-once` and abandon rather than retry | **HELD** |
| *"If the platform-returned id is recorded against the delivery id in the journal … the journey gate can classify it"* | **FALSE — see §3** |

## 3. The premise I am arguing against, and the decomposition I used instead

The journey gate's `unidentified` count is computed from the **sink's** arrivals
journal (`f24-journey.mjs:1235` reads `a.idempotency_key`), not from our ledger.
The sink **is** the destination; it mints the `sid` / `wamid` itself. **Recording
a platform-returned id in our journal therefore moves `unidentified` by exactly
zero.** It is a real improvement to *our* reconciliation — an operator could join
a delivery to a Twilio console entry — but it is not the thing the gate measures,
and the brief's chain of reasoning connects the two.

What the gate needs is the **delivery id on the wire**, so the arrival is
self-identifying at the destination. That is what Slack, Discord and Matrix
already do and why the same journey with `--adapters slack` returns `rc=0`.

**The brief also missed a second, independent cause.** `scripts/f24-sink.mjs`
hardcoded `idempotency_key: null` for `whatsapp.messages` (`:176`) and
`twilio.messages` (`:193`). An adapter-only fix would have left the number
unchanged while every unit test went green.

I did **not** do the ledger plumbing. It requires changing `dispatch_fire`'s
signature through the `wcore-cron` dispatcher trait — high blast radius, exactly
the workspace-break shape the brief warns about — for zero movement on the gate.
Handed over as a scoped follow-up in §8 rather than half-done.

## 4. What landed (Phase A — no credentials needed)

| | Twilio SMS | WhatsApp |
|---|---|---|
| carrier | `Idempotency-Key` request header | `biz_opaque_callback_data` (Cloud API tracking string, ≤512 chars) |
| why that one | the `Messages` create resource has **no** client-supplied dedup or opaque parameter — its 18 documented optionals are `StatusCallback`, `ApplicationSid`, `MaxPrice`, `ProvideFeedback`, `Attempt`, `ValidityPeriod`, `ForceDelivery`, `ContentRetention`, `AddressRetention`, `SmartEncoded`, `PersistentAction`, `TrafficType`, `ShortenUrls`, `ScheduleType`, `SendAt`, `SendAsMms`, `ContentVariables`, `RiskCheck` | Meta echoes it back in the `statuses` object of the `messages` webhook, so it is useful at the **real** platform rather than merely inert |
| value at the real platform | inert; mirrors Slack's header | real: a status arriving hours later joins to its `cron:{job}:{millis}` |
| capability bit | **`false`** | **`false`** |

Also: the key is re-attached inside Twilio's transport retry loop (a retry is the
same logical delivery); the WhatsApp value is shared by every part of a
multi-attachment send and clamped on a char boundary; an empty key leaves the
field **omitted**, never blank, so "unidentified" stays one unambiguous state.

## 5. The measurement — both directions, on real code

`crates/wcore-channels-registry/tests/identity_at_the_sink.rs`, hetzner,
`30bdb74e`. Full transcript: `IDENTITY-DIFFERENTIAL.md`.

```text
arrivals=12 arms=3
arm         endpoint            arr  replay  recur  indet  unid  verdict
A-UNKEYED   twilio.messages        2       0      0      1     2 NOT-PROVEN
A-UNKEYED   whatsapp.messages      2       0      0      1     2 NOT-PROVEN
B-KEYED     twilio.messages        2       0      1      0     0 RECURRENCE
B-KEYED     whatsapp.messages      2       0      1      0     0 RECURRENCE
C-REPLAY    twilio.messages        2       1      0      0     0 EXACTLY-ONCE-VIOLATED
C-REPLAY    whatsapp.messages      2       1      0      0     0 EXACTLY-ONCE-VIOLATED
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
```

- **CAN fail:** `A-UNKEYED` reproduces the pre-change state on the post-change
  binary. Without this the improvement is equally explained by a kinder harness.
- **CAN pass:** `B-KEYED` reaches `RECURRENCE` with `unidentified=0`. For these
  two adapters that verdict was previously unreachable *in principle*.
- **Not blinded:** `C-REPLAY` still returns `EXACTLY-ONCE-VIOLATED`. The cheap
  failure mode — everything reclassifying as a benign recurrence — did not occur.

The classifier is **imported** from `f24-journey.mjs`, not reimplemented.

### Does the journey gate's `indeterminate` improve, and by how much?

**Per repeat, from unjudgeable to judged — measured above, 1→0 on both
endpoints.** Extrapolating to the reported Windows run
(`recurrences=4 indeterminate=8 unidentified=16`): the 16 unidentified arrivals
were exactly the `twilio.messages` + `whatsapp.messages` ones, and the 8
indeterminate repeats were theirs. Both should now classify.

**I did not run the full 17-step Windows journey, so I am not claiming the
`8 → 0` end-to-end number.** What is measured is every arrival on that path,
under the real classifier, in both directions. Someone should re-run the Windows
journey and confirm; the arithmetic is a prediction until they do, and this
programme has been wrong about exactly that kind of prediction before.

## 6. HIGH finding: two published evidence cells were false

`docs/delivery-semantics.md` claimed for both Twilio and WhatsApp: *"Replay
measured at a real destination? **Yes** — a replayed key produced **two**
messages."*

The run behind them is `crates/wcore-cli/tests/f24_c1_outbound_idempotency.rs` —
**mockito**. Measured with a control in the same invocation:

```
grep -ln "TWILIO_ACCOUNT_SID|WHATSAPP_ACCESS_TOKEN|WA_ACCESS_TOKEN|live_twilio|live_whatsapp"
  over crates/**/*.rs   ->  0 files
control: grep -ln "SLACK_BOT_TOKEN"  ->  live_slack_actions.rs   (1 file)
```

This is the **identical** defect the doc's own Slack correction diagnoses —
*"the evidence column must state the same claim as the guarantee column"* —
sitting in the two rows immediately beneath it. It survived that correction
because the reviewer was looking at Slack. Corrected to `NOT MEASURED at a real
destination`. **No guarantee changed**; `at-most-once` never rested on the
fixture. The exactly-once set is untouched (`vec!["matrix"]`).

**Telegram's row carries the same overstatement and I left it standing**, with a
note naming the fix. It is not my lane's row and I did not measure it.

## 7. What I did NOT do

- **No live Twilio or Meta run. Two cells UNRUN.** `live_twilio_whatsapp_identity.rs`
  is written and gated; it **panics** rather than skipping, and its census test
  is deliberately NOT `#[ignore]`d so `UNRUN_LIVE_CELLS count=2 — a skip is NOT a
  pass` prints on every ordinary `cargo test`. Needs
  `WL_LIVE_TWILIO_HOME` + `WL_LIVE_TWILIO_TO` (a Twilio SID + auth token + a
  provisioned From number; each send bills real money) and
  `WL_LIVE_WHATSAPP_HOME` + `WL_LIVE_WHATSAPP_TO` (a Meta Business app with the
  WhatsApp product, a phone-number id, a system-user token, and a recipient
  inside the 24-hour window).
- **No capability bit set `true`.** Both remain `false`.
- No full 17-step journey run; no Windows leg.
- No ledger plumbing of the platform-returned sid / wamid (§3, §8).
- No Telegram row edit.
- No WhatsApp bridge work — that is `lane/whatsapp-bridge`'s scope.

## 8. Honest cost of this change, and follow-ups

**The `false` got weaker, and that is stated rather than absorbed.** It used to
rest on a mechanical argument — *we transmit nothing, therefore nothing can
dedupe*. That argument is gone; the bit is now a **conservative default pending
measurement**. The asymmetry makes the trade sound: a wrong `false` abandons a
delivery *visibly* (`wayland-core gateway abandoned`), a wrong `true` duplicates
one *silently*. But the next reader must not infer the older, stronger claim, so
it is written into `f24_c1_outbound_idempotency.rs`'s header, both adapters'
`supports_outbound_idempotency` docs, and the delivery-semantics doc.

Follow-ups, none blocking:
1. **MEDIUM** — persist `MessageReceipt.id` (sid / wamid) against the delivery id
   in the ledger. Needs a `dispatch_fire` signature change through the
   `wcore-cron` dispatcher trait. Real operator value for
   `resend --confirm-not-delivered`; zero effect on the journey gate.
2. **MEDIUM** — Telegram's evidence cell (§6).
3. **LOW** — `IDEMPOTENCY_HEADER` is now defined in both `wcore-channel-slack`
   and `wcore-channel-sms`. AGENTS.md says extract to the lowest shared crate
   (`wcore-channels`); I did not, because that means editing slack/lib.rs, which
   another lane may hold.
4. **LOW** — Twilio's only real echo channel is query params on a
   `StatusCallback` URL. The SMS crate has **zero** references to
   `StatusCallback`; it would need config plumbing and a public URL.

## 9. Coordination — files touched in `wcore-channel-whatsapp`

Per the mid-flight coordination message, the orchestrator must serialise this
lane against `lane/whatsapp-bridge`. **Exactly two files** in that crate:

- `crates/wcore-channel-whatsapp/src/api.rs` — adds
  `biz_opaque_callback_data` to `SendMessageRequest` and `SendMediaRequest`,
  `with_tracking_data`, `clamp_tracking_data`, `MAX_TRACKING_DATA_CHARS`.
- `crates/wcore-channel-whatsapp/src/lib.rs` — extracts the existing
  `send_message` body into an inherent `post(msg, key)`; adds
  `send_message_idempotent` and an explicit `supports_outbound_idempotency`.

**The backend assumption is named in code, not left implicit.**
`biz_opaque_callback_data` is a **Cloud-API-only** field; it does not exist in
the WhatsApp Web protocol, so a Baileys / whatsapp-web backend arriving through a
future transport seam cannot carry a delivery id there and would silently
reproduce the unattributable-arrival state one layer down. That is written on
`with_tracking_data` and on `supports_outbound_idempotency`, which is **one bit
for the whole adapter** even when the transports beneath it differ — a backend
that cannot carry an id must not inherit a declaration reasoned about one that
can. I did not design the seam; that is the other lane's scope.

No edits to `crates/wcore-cli/src/lib.rs` or `main.rs`, so no shared-file fence
contention. All 12 changed paths are listed in §10.

## 10. Gate results — every number read back from a file, not an exit code

| gate | commit | result |
|---|---|---|
| `cargo test -p wcore-channel-sms` | `13571bf1` | **28 passed, 0 failed, 0 ignored, 0 filtered out** |
| `cargo test -p wcore-channel-whatsapp` | `13571bf1` | **42 passed, 0 failed, 0 ignored, 0 filtered out** |
| `cargo test -p wcore-cli --test f24_c1_outbound_idempotency` | `13571bf1` | **6 passed, 0 failed, 0 ignored, 0 filtered out** |
| `identity_at_the_sink -- --ignored` | `30bdb74e` | **1 passed, 0 failed, 0 ignored, 0 filtered out** (§5) |
| `cargo fmt --all -- --check` (Mac) | `530bd6df` | rc=0, 0 lines |

Changed paths, diffed against the captured merge-base `4caaa31c`:

```
.planning/evidence/twilio-whatsapp-identity/IDENTITY-DIFFERENTIAL.md
.planning/evidence/twilio-whatsapp-identity/TWILIO-WHATSAPP-IDENTITY-NOTES.md
crates/wcore-channel-sms/src/api.rs
crates/wcore-channel-sms/src/lib.rs
crates/wcore-channel-whatsapp/src/api.rs
crates/wcore-channel-whatsapp/src/lib.rs
crates/wcore-channels-registry/tests/identity_at_the_sink.rs
crates/wcore-channels-registry/tests/live_twilio_whatsapp_identity.rs
crates/wcore-cli/tests/f24_c1_outbound_idempotency.rs
docs/delivery-semantics.md
scripts/f24-identity-arms.mjs
scripts/f24-sink.mjs
```

## 11. Instrument defect found and repaired in-lane

The background-task harness reported **exit code 0** for a run whose real status
was **101** (ten compile errors). LANE-BRIEF §2a, in the wild. The repair is not
"noted and moved on" (§6b-ii): every remote command in this lane now writes
`WLRC=<code>` and `WLDONE` into a log file that a separate call reads back, and
**every number in this summary is quoted from a file, never from an exit code.**
