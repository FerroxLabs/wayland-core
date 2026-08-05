# 24-C1 declaration — working NOTES

Lane `lane/24c1-declaration`. Base `0d48b551`. Started 2026-07-30.

Goal: write the per-adapter delivery-semantics declaration in `docs/`, from measurement,
and make it enforceable with a drift test.

## M1 — `supports_outbound_idempotency` overrides at base (`/usr/bin/grep -rn crates/`)

Capture: 22 hits, rc=0. Overrides of the trait method (`fn supports_outbound_idempotency(&self) -> bool`
NOT on the trait itself):

| Crate | file:line |
|---|---|
| `wcore-channel-slack` | `src/lib.rs:249` |
| `wcore-channel-matrix` | `src/lib.rs:294` |
| `wcore-channel-discord` | `src/lib.rs:344` |

Trait default `false` at `crates/wcore-channels/src/lib.rs:139`.

**Brief's "exactly-once is 3 of 10" — HOLDS at base.** Matrix `:294` and Discord `:344` are
byte-exact against the brief. Instrument alive: the same grep returned 22 hits including known
consumers (`gateway.rs:956`, `manager.rs:716`, `cron.rs:177`).

## M2 — the adapter population is 10

`ls crates/ | grep channel` → 10 adapter crates + `wcore-channels` (trait) + `wcore-channels-registry`
(factory). iMessage is `#[cfg(target_os = "macos")]`-gated in the registry (`lib.rs:60-61`), so on
Linux/Windows it is not constructible at all — that is a *row in the table*, not an omission.

## M3 — construction path for a drift test

`wcore-channels-registry` is the only crate depending on all ten (`Cargo.toml`). Its
`channel_factory_for(platform) -> Option<ChannelFactory>` (`lib.rs:46`) is the production
construction path and every `make_*` is pure (`parse_options` + `::new`) — no network, no
credential read at construction. So a drift test can build all ten from hermetic fixture configs.

## M4 — the delivery spine, read end to end (`wcore-gateway/src/automation.rs:143-237`)

`LedgeredHandler::dispatch_fire`, the only ledgered delivery path:

| ledger state on `accept()` | `supports_outbound_idempotency` | action | file:line |
|---|---|---|---|
| `Accepted` (first sight) | either | attempt | `:218` |
| `Duplicate` + `Settled` | either | **return Ok, no send** | `:169-171` |
| `Duplicate` + `Attempted` (outcome UNKNOWN) | `false` | **`abandon(OutcomeUnknownNoDedup)`**, warn, return Ok | `:201-215` |
| `Duplicate` + `Attempted` (outcome UNKNOWN) | `true` | fall through → re-attempt WITH the key | `:216-220` |

`settle(&id, outcome.is_ok())` at `:231` settles **both** arms — so a KNOWN failure is
terminal for that occurrence and is never retried by the spine (`:227-230`).

So the mapping is exact and mechanical:
- `supports == true` → **exactly-once**;
- `supports == false` → **at-most-once** (abandoned, never auto-retried).
There is no adapter for which the spine produces at-least-once automatically.

## M5 — THE SCOPE OF THE GUARANTEE, and why F24-GWP-H1 breaks it on Windows

`FireContext::delivery_id()` (`wcore-cron/src/runner.rs:324-338`):

```
cron:{job_id}:{scheduled_for.timestamp_millis()}[:{occurrence}]
```

**The whole guarantee is keyed on that string.** It is a (job, scheduled instant) pair — NOT
the customer's notion of "this message".

`24-GATEWAY-PLATFORMS-SUMMARY.md:155-194` measured, on Windows: **27 arrival lines, 13 distinct
texts, `{2:12, 3:1}`** against macOS `{1:13}`. Crucially (`:179-181`): *"27 **distinct** delivery
ids, each settled exactly once"*. Process count never exceeded 1, so it is not a lock failure —
a runtime restarted at the Task Scheduler `PT1M` boundary **re-fires jobs that already fired**,
minting a NEW delivery id for the same logical message.

**A new delivery id is, to the ledger and to every adapter, a new delivery.** No adapter's dedup
can suppress it — including the three that dedupe correctly. So F24-GWP-H1 is a **platform row
that applies to all ten adapters**, and the table must say so rather than printing
"exactly-once" unqualified.

And `F24-GWP-M1`: the receipt headline said `duplicates: 0` for that run. A table graded on the
headline would have been wrong. This is why the doc's Windows row is written from the sink's
journal, not the product's own count.

## M6 — the seven, both directions

`/usr/bin/grep -rni "idempot"` across the 7 non-overriding adapter crates: **8 hits, all
lifecycle** (`start`/`stop` "already running — idempotent"), zero delivery idempotency.
Known-positive in the same invocation: slack `12`, matrix `10`, discord `12` hits.

Per-adapter override census (`send_message_idempotent` / `supports_outbound_idempotency`):
slack 1/1, matrix 1/1, discord 1/1; telegram, sms, whatsapp, email, signal, imessage,
msteams all **0/0**. Instrument alive in both directions in one run.

Restart-stability of the two that were fixed, verified at source:
- Matrix — `rest.rs:63 txn_id_for_key(key)`, FNV-1a of the key, used at `:133-135`; the
  unkeyed path `next_unkeyed_txn_id()` (`:85`) is the counter, and it is no longer on the
  keyed path.
- Discord — `rest::nonce_for_key(k)` at `lib.rs:170-172`; `next_nonce()` only unkeyed.
Both bind the claim with a named test (`..._declares_idempotency_only_because_the_..._is_derived_from_the_key`).

## M7 — nearest miss: SMTP

`smtp.rs:287 make_outbound_message_id(from)` mints a **fresh** Message-ID per send, and
`smtp.rs:730 outbound_message_ids_are_unique_per_send` asserts that. Deriving it from the
delivery key would be trivial. **It still would not earn `true`**: the trait requires a key the
destination *will honour* (`wcore-channels/src/lib.rs:131-138`), and no RFC or MTA guarantees
dedup by Message-ID. Declaring `true` on "Gmail probably will" is exactly the reassuring
sentence over unimplemented code this lane exists to prevent. Left unchanged; recorded as the
one candidate a future product decision could revisit.

## M8 — cross-audit panel on "can any of the 7 be fixed": UNANIMOUS NO (3/3)

Question put to all three: does each platform's send API accept a client-supplied
idempotency/dedup token the destination will HONOUR on replay?

| Panelist | Verdict |
|---|---|
| codex 5.6 Sol | 7/7 **NO**, with primary sources (Twilio's own "is the request safe to retry" page; RFC 5321 §6.1 + RFC 5322 §3.6.4; signal-cli jsonrpc man page; Bot Framework activity spec) |
| gemini 3.1 Pro | 7/7 **NO** |
| kimi K3 | 7/7 **NO** (self-corrected mid-answer on Twilio, then confirmed) |

**One nuance worth keeping**, from codex: Telegram's lower-level **MTProto** API has `random_id`;
the **Bot API** `sendMessage` does not expose it. Our adapter is a Bot API client, so the token is
unreachable from where we stand. Recorded in the doc — "the platform" ≠ "the API we use".

Internal adversarial pass, arguing AGAINST: *"you never re-send to those seven, so it is
effectively exactly-once, and 'at-most-once' undersells it."* **Rejected** — at-most-once is
precisely right: the message may not have arrived, and the abandon path exists precisely because
we do not know. Calling that exactly-once would be the overclaim.

**So the brief's judgement HOLDS: every adapter fixable in code has been fixed.** I fixed no
adapter, and say so. SMTP is the nearest miss and is documented as such (M7).

## M9 — the drift test, run in BOTH directions on the REAL artifacts

Not just in-memory comparator mutations — the real document and the real adapters, on hetzner at
`003661d8` / `39f53536`, `CARGO_BUILD_JOBS=10`.

| Run | What was mutated | Result |
|---|---|---|
| **known-positive** | nothing | `8 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out` rc=**0** |
| **mutation A** | `docs/delivery-semantics.md`: `telegram = at-most-once` → `exactly-once` | rc=**101**, `2 passed; 6 failed`; message: *telegram: the document says "exactly-once" … but the adapter returns false* |
| **mutation B** | `wcore-channel-telegram/src/lib.rs`: adapter gains `supports_outbound_idempotency() -> true` | rc=**101**, `2 passed; 6 failed`; message: *telegram: the document says "at-most-once" … but the adapter returns true* |
| **mutation C** | `wcore-channel-matrix`: `true` → `false` | see below |

All three mutations reverted; `git status --porcelain | wc -l` = **0** after each.

**Mutation C is the §6b-ii third assertion — "the old instrument would have missed it", executed
rather than reasoned about.** With Matrix silently downgraded:

| Test | Result |
|---|---|
| pre-existing `wcore-cli/tests/f24_c1_outbound_idempotency` (4 adapters) | **`6 passed`, rc=0 — BLIND** |
| new `delivery_semantics_declaration` (all ten) | **rc=101**, `3 passed; 5 failed`, naming `matrix` |

That test's name was `capability_is_declared_true_by_slack_alone_across_the_configurable_matrix`
— a name claiming a census its body does not run. Renamed, and its three stale header facts
(slack `:234` → `:249`; "Slack is the **only** adapter"; "the other **nine**") corrected.

Crate health at `39f53536`: `wcore-channels-registry` **11 + 8 passed, 0 failed, 0 ignored,
0 filtered out**; `cargo clippy -p wcore-channels-registry --all-targets -- -D warnings` rc=**0**
(the single `warning:` line is the `imap-proto` future-incompat note, not a lint);
`wcore-cli --test f24_c1_outbound_idempotency` **6 passed**, rc=0.

## Established — nothing outstanding

- [x] Gateway behaviour on `supports == false` → **abandon**, `automation.rs:201-215` (M4).
- [x] Matrix/Discord restart-stability → both derive the token from the key by hash (M6).
- [x] Per-platform primitive for the other 7 → none; panel-unanimous, code-cited (M6, M8).
- [x] F24-GWP-H1 → a platform row affecting **all ten**, because a re-fire mints a NEW delivery
      id and a different key is not a replay (M5).
- [x] Any `false` adapter fixable? → **no**; judgement tested and upheld (M8).
- [x] Drift test, both directions, on real artifacts (M9).
