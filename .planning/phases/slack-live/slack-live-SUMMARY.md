# slack-live — SUMMARY

**Verdict: goal ACHIEVED, and the brief's own premise about one leg was false in the product's
favour.** All five message actions are proven live against the real Slack Web API through the
adapter the production registry factory builds. The idempotency leg found a **HIGH** defect —
a customer-facing delivery guarantee that measurement contradicts — which is fixed and re-proven
in the same lane.

Branch `lane/slack-live`, HEAD `3316fbe5915a2d1d3da88986142d8a2acdfe527b`, pushed to `gh`.

---

## 1. The five legs, live

One test: `crates/wcore-channels-registry/tests/live_slack_actions.rs`. Built by
`channel_factory_for("slack")` — the same factory `auto_register_from_dir` uses at boot — and
driven entirely through the `Channel` trait. Every write is **read back from
`conversations.history`** before it is believed; an HTTP 200 is a statement that a request was
accepted, not that state changed.

Final run, hetzner, commit `3316fbe5` (identical result at `28098809`):

```
  PASS  send         sent ts=1785385973.483039 and read it back from history;
                     a fabricated channel was refused with channel_not_found
  PASS  edit         edited ts=1785385974.539399; history text changed "…edit-before" -> "…edit-after";
                     a fabricated ts was refused with message_not_found
  PASS  delete       deleted ts=1785385975.855699 and confirmed its ABSENCE from history;
                     the second delete of that same ts was refused with message_not_found
  PASS  receive      read ts=1785385977.303459 back from conversations.history; the real record,
                     independently signed and replayed through ingest_webhook, surfaced as
                     MessageReceived; a corrupted signature was rejected and enqueued nothing
  PASS  idempotency  declared=false; replayed key -> 2 arrival(s) (ts …977.719929 then …977.987069);
                     a distinct key -> 3
  clean channel: no marker messages remain
  5/5 legs passed
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
```

**Zero skipped cells.** The read scopes landed mid-run and I re-measured them myself rather than
trusting the notice (§4 below), so every leg ran live.

### Both-directions evidence, per leg

| leg | can it PASS (measured) | can it FAIL (measured, same invocation) |
|---|---|---|
| send | message read back from history at the returned `ts`, text byte-equal | send to `C00000000000BOGUS` → `channel_not_found`. Plus an instrument control: a body never posted is found **0** times, while the real one is found **1** |
| edit | history text goes `edit-before` → `edit-after`; the OLD text is asserted present *first*, so the change is caused by the edit | `chat.update` on `9999999999.000000` → `message_not_found` |
| delete | the `ts` is **absent** from history afterwards | a SECOND delete of the same `ts` → `message_not_found` |
| receive | real record replayed through `ingest_webhook` surfaces as `MessageReceived` with the real ts/text/channel | one byte of the signature flipped → rejected, **and** `poll_events` yields nothing |
| idempotency | a genuinely different key raises the count 2 → 3 | the count must equal what `supports_outbound_idempotency()` declares — and it **did redden**, see §2 |

The idempotency assertion is not a constant. It reads the adapter's declaration and requires the
platform to match it, so it reddens if we over-claim **and** if Slack ever starts honouring the
key. Both states are reachable, and **both were actually observed in this lane** — the same
assertion was RED at `81718bb2` (declared `true`, 2 arrivals) and GREEN at `28098809` (declared
`false`, 2 arrivals). That is the strongest form of §3b-iii: one gate, driven to both outcomes by
a product change.

---

## 2. HIGH — Slack was declared exactly-once and is not. Fixed.

`supports_outbound_idempotency()` returned `true` for Slack. The gateway reads that bit in
`LedgeredHandler::dispatch_fire` to decide whether an `Attempted`, outcome-unknown delivery may be
**re-sent** on restart. Slack ignores the `Idempotency-Key` header, so every such re-send was a
duplicate — and an invisible one, because our ledger recorded a single delivery.

Measured three independent ways in one run, and reproduced identically by raw `curl` outside the
adapter:

1. two `chat.postMessage` calls with the **same** key returned **two distinct `ts`**;
2. `conversations.history` held **2** records with that body;
3. `chat.delete` **succeeded on both** (a delete that succeeds proves the message existed) — with
   `chat.delete` on a fabricated `ts` returning `message_not_found` as the control proving that
   instrument can fail.

### How the false claim survived

The evidence behind it was `mockito`. A mock answers what it was told to answer: it can show the
header leaves us and can show **nothing** about what Slack does with it. The published table said
so in plain sight — `docs/delivery-semantics.md`'s Slack row justified *"one message"* with
*"the key was present on both attempts"*. Those are different claims, and the three rows beneath
it (Telegram, Twilio, WhatsApp) show what the real measurement reads like: *"produced **two**
messages"*, a count. The adapter's own `api.rs:73-76` already stated the header was "inert against
real Slack"; the capability bit six hundred lines away said the opposite, and the bit was the one
the gateway read.

### Fix (commit `28098809`)

| file | change |
|---|---|
| `crates/wcore-channel-slack/src/lib.rs` | `supports_outbound_idempotency()` → `false`, documented with the measurement |
| same, unit test | `slack_declares_idempotency_only_because_it_sends_the_header` → `a_keyed_send_puts_the_key_on_the_wire_though_slack_ignores_it`; it now asserts the two claims separately |
| `docs/delivery-semantics.md` | Slack row → at-most-once/abandoned; headline 3-of-10 → 2-of-10; §5 "all three" → "both"; machine-readable block; a dated correction note explaining the reasoning error |
| `crates/wcore-channels-registry/tests/.../delivery_semantics_declaration.rs` | `exactly_three_adapters_are_exactly_once` → `exactly_two…`; `comparator_rejects_a_downgraded_row` re-keyed onto Matrix (it needs an adapter that really declares `true`, or that direction is not exercised) |
| `crates/wcore-cli/tests/f24_c1_outbound_idempotency.rs` | the two tests asserting Slack `true` now assert `false`, renamed truthfully, header corrected |

The header is **still transmitted** — a Slack-compatible destination reached through
`api_base_url` may honour it, and the mockito test keeps it from being dropped silently. Only the
claim about `slack.com` changed.

Behaviour change to be aware of: Slack outcome-unknown deliveries are now **abandoned** instead of
re-sent. That is the conservative path the other eight adapters already take; it converts a silent
duplicate into a recorded, operator-resendable abandonment (`wayland-core gateway abandoned`).

---

## 3. What I did NOT prove — stated plainly

**The receive leg does not exercise Slack's own webhook delivery, and no scope grant would let
it.** The adapter receives via a signed Events API POST. Slack cannot POST to a test: that needs a
publicly reachable HTTPS endpoint and an event subscription on the app, which is Slack-app
configuration reserved to Sean, not a permission. So the leg proves, with real data:

- the message really is in Slack and readable (`conversations.history`, real read scope);
- the adapter's **real** signature verification, replay window and parser accept the **real**
  record and produce the right `IncomingMessage`;
- a corrupted signature is rejected and enqueues nothing.

It does not prove Slack's transport. The envelope is constructed in the test from the real history
record. I signed it with an HMAC-SHA256 implementation written from Slack's published spec rather
than calling the adapter's own `auth::expected_signature`, so the two implementations agreeing is
information rather than a tautology — but that is a mitigation, not a substitute for Slack signing
it.

**Also not done:** nothing on Windows; no gateway-level restart test proving the abandon path
end-to-end (the unit-level bit the spine reads is covered by the wcore-cli tests, and the live
proof stops at the adapter).

---

## 4. Premise check — what the dispatch brief got right and wrong

| brief claim | held? |
|---|---|
| follow `live_discord_actions.rs` | **FALSE** — it does not exist on my base, and no `live_*` test exists under any `wcore-channel*` crate. Followed the closest analog instead: `wcore-channels-registry/tests/{native_action_matrix,delivery_semantics_declaration}.rs`, which build adapters through the production factory |
| scopes limited to `chat:write,channels:history,channels:read` | **TRUE at first probe**, then superseded. My own re-probe confirmed `groups:history` + `groups:read` had landed |
| `groups:*` needed because the channel is private | **TRUE** — measured `missing_scope needed: groups:history` against `C0BLR1UKKU6` while `channels:history` was held |
| "only 8 of 24 arrivals carry an idempotency_key" | **not applicable to Slack outbound as stated.** The Slack adapter attaches the key only on `send_message_idempotent`, and deliberately **not** on a plain send — bound by a mockito test asserting the header is absent when unkeyed. So a low ratio is expected behaviour on that path, not a defect. I did not assume anything about it; I measured the header directly |
| coordinator: "2 messages in the channel are probably join events, not my leftovers" | **TRUE**, verified independently: both are `channel_join`, from the bot `U0BLBKR56NT` and Sean `U3PGRDZGA`. **No leftover probe messages of the coordinator's were found**, so none needed deleting |

---

## 5. Channel hygiene

Left with exactly the two `channel_join` system events it started with — verified by a direct
`conversations.history` read from the Mac *after* the final test run, not from the test's own
output:

```
ok=True total=2
  1785382174.613739 channel_join '<@U0BLBKR56NT> has joined the channel'
  1785382157.617439 channel_join '<@U3PGRDZGA> has joined the channel'
WL markers remaining: 0
```

Join events are not deletable by `chat.delete` and are not ours to remove. Every message this lane
created — 3 raw-probe, 12 across two test runs — was deleted. The test's sweep is per-run-tag
scoped and runs unconditionally after every leg, including on failure, which is why the failing
first run also left the channel clean.

No channel other than `C0BLR1UKKU6` was posted to, read, joined, left or enumerated. No DMs.

---

## 6. Gates, with real numbers

| gate | where | result |
|---|---|---|
| `cargo fmt --all -- --check` | Mac | rc=0 |
| `cargo check -p wcore-channels-registry --all-targets` | hetzner | rc=0 |
| `cargo check --workspace --all-targets` | hetzner | rc=0, 0 errors, **120 crates checked** (count read back so a no-op run could not pass as a clean one) |
| `cargo test -p wcore-channel-slack` | hetzner | **50 passed; 0 failed; 0 ignored** |
| `cargo test -p wcore-channels-registry` | hetzner | **8 + 3 passed; 0 failed**; `live_slack_actions` correctly shows **0 passed, 1 ignored** without `--ignored` |
| `cargo test -p wcore-cli --test f24_c1_outbound_idempotency` | hetzner | **6 passed; 0 failed; 0 ignored** |
| `cargo clippy -p wcore-channel-slack -p wcore-channels-registry -p wcore-cli --all-targets -- -D warnings` | hetzner | rc=0, 0 errors (first pass caught a `needless_borrows_for_generic_args` in my test — fixed in `3316fbe5`) |
| live matrix, `--ignored` | hetzner | **1 passed; 0 failed; 0 ignored; 0 filtered out**, 5/5 legs |

Counts read back per §3.2 rather than trusting exit status. The live binary is a single
`#[ignore]`d test, and it **cannot** silently run zero work: with `--ignored` requested and the
environment incomplete it panics naming the missing variable instead of returning early.

---

## 7. Credential handling — disclosed per §0

The Mac cannot compile, so the live run happened on `hetzner-dsm`. The bot token and signing
secret were **injected on stdin only** — one per line into a script containing no credential,
never in `argv`, never written to disk, never into a log or a commit.

Secret sweep against the live values: **0 hits** on the hetzner run log, and **0 hits**
independently on the Mac across every retained evidence file and every changed file. The sweep
instrument was proven alive on a known-positive in the same capture (`WL-LIVE-SLACK` found in the
log).

---

## 8. For the orchestrator to serialize

- **`docs/delivery-semantics.md` is edited by this lane.** The `matrix-live` lane is likely to
  touch the same file. My edits are the Slack row, the §1 headline counts, one sentence in §5, the
  §2 correction note, and the `slack =` line of the machine-readable block. A Matrix-row edit
  should merge cleanly, but the §1 counts are shared.
- **`crates/wcore-cli/tests/f24_c1_outbound_idempotency.rs`** belongs to phase 24-C1. Two of its
  assertions were inverted here because they asserted a measured-false claim.
- No `crates/wcore-cli/src/lib.rs` or `main.rs` edits — the §6 fence is untouched.
- Nothing merged into `plan/f20-unified-audit-repair`; pushed only to `lane/slack-live`.

## 9. BACKLOG (MEDIUM, non-blocking)

- **`BL-SLACKLIVE-KNOWN-POSITIVE`** — after the fix, both capability tests in
  `f24_c1_outbound_idempotency.rs` assert only negatives across their four fixtures, which is
  self-passing on a dead instrument. The surviving known-positive is
  `exactly_two_adapters_are_exactly_once` in the registry (Discord + Matrix must return `true`
  through the same production factory), and I cross-referenced it in the test body. An in-file
  positive fixture would be stronger; it needs an adapter whose `start()` does no network, which
  none of that file's four are.
- **Discord's `exactly-once` row is still mock-only** (`BL-24C1-DISCORD-WINDOW`). It is now one of
  only two such claims, and it rests on the same class of evidence that turned out to be wrong for
  Slack. It should be driven at a real destination before it is trusted — the `discord-live` lane
  is the natural place.
