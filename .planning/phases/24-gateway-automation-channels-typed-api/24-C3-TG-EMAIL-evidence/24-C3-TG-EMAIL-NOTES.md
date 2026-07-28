# 24-C3-TG-EMAIL — running NOTES (append-only, re-committed after every measurement)

Lane: `lane/24-c3-tg-email`. Base: `ef1d97beb61f1b084bdfba745e8f49830924d757`
(`plan/f20-unified-audit-repair` at branch time). Worktree
`/Users/seandonahoe/dev/waylandcore-frontier-worktrees/lane-24-c3-tg-email`.

Goal: close as much of `24-C3` as is closable with NO vendor credential, by adding
`telegram` (Task 1) and email/IMAP (Task 2) to the five-leg inbound matrix.

---

## T+0 — source read, before any code change

### What the inbound matrix is today

`scripts/f24-inbound.mjs:54` — `export const ADAPTERS = ['slack', 'whatsapp', 'sms'];`
`scripts/f24-inbound.mjs:55` — `export const LEGS = ['admit','dedupe','access','bind','route'];`

All three current adapters are **webhook** adapters. `runMatrix()` (line 555) is hard-wired to
that transport in three separate ways:

1. **Injection** is always `post(cfg.build({url: http://127.0.0.1:18787/webhooks/<name>, ...}))`
   — a signed HTTP POST into the binary's inbound webhook host.
2. **Arrival reading** is always `this.arrivals()` = `f24-sink.mjs`'s journal, because all three
   adapters' outbound `api_base_url` is pointed at that one sink.
3. **Preflight** is `waitForWebhookHost()` (line 440), which probes `/healthz` on the webhook
   host and, when it is absent, calls `failEveryLeg()` over `ADAPTERS × LEGS`.

A polling adapter satisfies none of the three. So adding `'telegram'` to the array alone would
produce 5 spurious FAILs, not a measurement. The generalisation needed is an **injector +
arrival-reader pair per adapter**, and a preflight that is per-adapter rather than global.

### Why telegram is drivable with no vendor credential — verified, not assumed

- `TelegramConfig::api_base_url` exists at `crates/wcore-channel-telegram/src/config.rs:48`,
  `#[serde(default = "default_api_base")]`, default `TELEGRAM_API_BASE`.
- `TelegramChannel::new` at `crates/wcore-channel-telegram/src/lib.rs:68-75` reads
  `config.api_base_url` into `api_base` — so the SHIPPED constructor (not a `#[doc(hidden)]`
  test constructor) honours it. Confirmed by reading `new`, not by inference from the field.
- `scripts/f24-tg-fixture.mjs` serves `deleteWebhook`, `getUpdates` (with real destructive
  offset-confirm semantics), `sendMessage`, plus a `/__control/{health,submit,report}` plane.
  It takes `--token` and the harness **mints** that token at run time (same pattern as the
  slack/whatsapp/twilio secrets minted at `f24-inbound.mjs:217-223`). The fixture IS the API,
  so there is no vendor credential anywhere in the loop.

So telegram's arrival journal is the **TG fixture's own journal** (`kind:"sendMessage"` records,
`chat_id` + `text`), not `f24-sink.mjs`. That preserves the load-bearing property — an
out-of-process journal the binary cannot write to except by completing a real round trip.

### The instrument fault I expect to hit, identified BEFORE running (LANE-BRIEF §6b-ii)

`crates/wcore-channel-telegram/src/config.rs:60-62` — `default_parse_mode()` is
`ParseMode::MarkdownV2`. `lib.rs:274-277` applies `escape_markdown_v2()` to the FULL outbound
text under that mode. `config.rs:105-107` puts `-` in the reserved set.

The existing correlation tokens are of the form `f24c3-<adapter>-admit-<tag>` — **four hyphens**.
Through the telegram outbound path that token arrives at the fixture as
`f24c3\-telegram\-admit\-<tag>`. `arrivalsFor()` (line 504) matches with
`(a.text ?? '').includes(correlation)` — an **exact substring test**. It would return 0 for a
reply that had in fact arrived intact.

That is the exact defect the brief records (`replied=0` for eight replies that all arrived), and
it is the eleventh instance of an instrument carrying the class it hunts. Per §6b-ii it gets
**repaired in this lane, not written up**: a three-tier matcher (exact → de-escaped → fuzzy),
where fuzzy-match-without-normalised-match raises an explicit `instrument_fault` and grades the
run **INCOMPLETE rather than LOSS**. Self-test with three assertions including "the old matcher
would have missed it".

Note the alternative — minting hyphen-free tokens — is REJECTED: it hides the defect instead of
instrumenting it, and leaves the next unanticipated mangling silent.

### Email/IMAP — what is established so far

- `crates/wcore-channel-email/src/imap.rs` is 1585 lines; the single named blocker is
  `native_tls::TlsConnector::new()` at line 194.
- `ImapConfig` host/port are already config (to be re-verified before building the fixture).
- Linux OpenSSL honours a child-scoped `SSL_CERT_FILE`; **macOS Security.framework does not**.
  So a self-signed-cert IMAP fixture is drivable on Linux (hetzner) with no Rust change, and is
  NOT drivable on macOS by the same trick. This must be stated explicitly in the summary.

### Still to establish

- [ ] Is the telegram adapter actually reachable from the inbound dispatch path in the shipped
      binary (registry construction + `[inbound]` access policy applying to a polling channel)?
- [ ] Does `[inbound] dm = "allowlist"` gate telegram, or does telegram gate only via
      `allowed_chat_ids`? These are different access mechanisms and the `access` leg must drive
      whichever one is real.
- [ ] `ImapConfig` exact field names for host/port/TLS.
- [ ] hetzner worktree + targeted build.

Nothing measured yet. No claim of any kind is made in this file about a leg passing.
