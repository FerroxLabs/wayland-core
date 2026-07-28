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

---

## T+~75min — Task 1 first live run on hetzner, and what it caught

Binary: `/root/wayland-24-c3-tg-email/target/release/wayland-core`, built from `ddc7cfe0`,
`cargo build -p wcore-cli --release`, `BUILDRC=0`. Driver at `237636c9`.
Run dir `/root/f24tg-run1`.

```
INBOUND MATRIX RED platform=linux runtime=json-stream legs=20/20 failed=1
arrivals_total=9 telegram_arrivals=3 turns_total=12 instrument_fault=false
telegram fixture: submitted=5 still_pending=[] polls=176 max_concurrent_getupdates=1
```

Per-leg, telegram:

| leg | result | evidence |
|---|---|---|
| admit | PASS | `SUBMIT 200 {"ok":true,"update_id":1}` / arrivals(journal)=1 want=1 / turns=1 |
| route | **FAIL** | `reply_text="F24C3\-REPLY f24c3\-telegram\-admit\-67ac190c" carries_correlation=false` |
| dedupe | PASS | before=1 after=1 / positive-control fresh-id arrivals=1 |
| access | PASS | arrivals=0 want=0, turns=0 want=0, CONTROL admit-leg-arrived=1 (control held) |
| bind | PASS | conv1="24030001" conv2="24030002" distinct=true |

The three webhook adapters stayed 15/15 PASS through the refactor.

### The `route` FAIL is an INSTRUMENT fault, and it is the most useful thing this run produced

Read the evidence string. The reply **arrived**, in the **right conversation**
(`conversation_id="24030001"` equals `want`), carrying the **right token** — in its
MarkdownV2-escaped form, exactly as predicted in the T+0 note before anything was run.
`carries_correlation=false` is the harness being wrong about a working product.

The cause is a **partial repair**. `arrivalsFor()` was moved onto `f24-correlate.mjs`, which is
why `admit` counted its arrival correctly. But `runMatrix`'s route check re-tested that same
arrival on a separate line that still read:

```js
const routed = seen1.length === 1 && (seen1[0].text ?? '').includes(c1);
```

So one call site was repaired and one was missed, and the missed one failed **in the direction
that blames the product**. That is LANE-BRIEF §6b-ii demonstrated inside a single lane rather
than across two.

Fixes applied, both of them:
1. the route check now goes through `correlationMatches()`;
2. a **source scan** in `f24-correlate.test.mjs` fails if any line in the driver compares a
   correlation variable with `String.includes` — because a comment would not have caught this,
   and the scan does. The scan asserts the source is >10kB first, so it cannot pass vacuously.

Self-test: **16 passed, 0 failed** (Mac and hetzner, node v22.21.1).

### Independent observables recorded this run

- `max_concurrent_getupdates=1` — counted by the fixture, in another process, from overlapping
  open requests. **1** is correct: one poller on the token. **2** would mean two `ChannelManager`s
  competing and silently destroying each other's reads (F24-C3-H4); **0** would mean the runtime
  polled nothing. So a "fix" that works by starting nothing cannot pass this.
- `submitted=5 still_pending=[]` — every update the fixture was given was consumed.
- 5 submitted, 3 arrivals: admit + dedupe-control + bind arrived; the dedupe replay and the
  denied sender correctly did not. That accounting balances exactly.

## T+~80min — Task 2 scope correction, measured from the lockfile

The brief's premise is **half right**, and the half that is wrong needs saying before any
fixture is built:

- **IMAP inbound uses `native-tls`** (`crates/wcore-channel-email/Cargo.toml:13`), which is
  OpenSSL on Linux. `native_tls::TlsConnector::new()` at `imap.rs:194` therefore picks up
  OpenSSL's default verify paths, and **OpenSSL honours `SSL_CERT_FILE`**. A self-signed IMAP
  fixture IS reachable from a child-scoped env var on Linux. The brief is correct here.
- **SMTP outbound does NOT use native-tls.** `Cargo.toml:11` pins
  `lettre = { default-features = false, features = ["smtp-transport", "tokio1-rustls-tls", "builder"] }`
  and `smtp.rs:78` builds `AsyncSmtpTransport::<Tokio1Executor>::starttls_relay(host)`. The
  resolved dependency set in `Cargo.lock` (the `lettre` block) contains **`webpki-roots 1.0.7`
  and NOT `rustls-native-certs`**.

`webpki-roots` is a **compiled-in Mozilla root set**. It reads no file and no environment
variable. So `SSL_CERT_FILE` has **no effect whatsoever** on the SMTP path — not on Linux, not
on macOS, not on Windows. This is a different and stronger blocker than the macOS one the brief
flagged, and it applies to the leg that captures the REPLY.

Consequence for the five legs on email: `route` (and the reply half of `admit`) cannot be
captured through SMTP by any environment-variable mechanism. To be proven executably, not
asserted, before it is reported.

---

## T+~100min — Task 1 CLOSED. Run 2, hetzner, driver `6a6e8c4e`

```
INBOUND MATRIX GREEN platform=linux runtime=json-stream legs=20/20 failed=0
arrivals_total=9 telegram_arrivals=3 turns_total=12 instrument_fault=false
telegram fixture: submitted=5 still_pending=[] polls=176 max_concurrent_getupdates=1
```

`PASS telegram/route: reply_text="F24C3\-REPLY f24c3\-telegram\-admit\-3e7f85e6" carries_correlation=true`
— the same escaped text that read `false` one commit earlier now reads `true`. The repair is
proven against the product's real transformation, not against a mock of it.

Binary `wayland-core 0.12.25 (source ddc7cfe0…)`, sha256
`0e5468a23ea23a447178a23b9cba937e2b4f634588feed973343c8f2f26c724b`.
Journal bytes: arrivals=2424, turns=3387, telegram=46900, core_log=13227.

Telegram fixture per-update accounting `[update_id, message_id, serve_count, deleted_by]`:

```
[[1,1,1,131],[2,1,1,133],[3,2,1,153],[4,3,1,155],[5,4,1,175]]
```

Read it: update 2 carries **message_id 1**, i.e. the dedupe replay genuinely re-used the platform
message id under a fresh transport cursor. **`serve_count == 1` on every update** — nothing was
served twice, so there was no double-poll. `max_concurrent_getupdates=1` says the same thing from
the other side, counted out-of-process.

Update 4 (the denied sender) was **served and consumed** — the refusal happens at the dispatch
access gate, not by failing to read. That is what makes the `access` PASS a positive result
rather than the universal-denial green the brief warns about.

## T+~120min — Task 2 run 3: the SMTP blocker, proven executably, and a trap I nearly fell into

### The blocker is real, and it is NOT the macOS one

One run, one process, one certificate, one `SSL_CERT_FILE` — and the two protocols disagree:

```
19:48:14.388  imap.uid_fetch  uid=1000 bytes=298 set_seen=true      <- IMAP accepted the cert
19:48:15.030  smtp.session.open  session=86
19:48:15.031  smtp.command  verb=EHLO   secure=false
19:48:15.072  smtp.command  verb=STARTTLS  secure=false
19:48:15.077  smtp.starttls.rejected
   error: "SSL routines:ssl3_read_bytes:tls alert certificate unknown ... SSL alert number 46"
```

Alert 46 is `certificate_unknown`, sent **by the client** — the binary's rustls refusing my
fixture. Fifteen SMTP sessions, all refused identically; four IMAP fetches, all accepted.

That isolates the difference to the TLS backend and nothing else, and it confirms the lockfile
read: IMAP is `native-tls`/OpenSSL (honours `SSL_CERT_FILE`), SMTP is `lettre` +
`tokio1-rustls-tls` resolving to **`webpki-roots`**, a compiled-in root set that reads no file
and no environment variable. **This blocks the reply half on every platform, not just macOS.**

Independent instrument check first: Python `imaplib.IMAP4_SSL` with the fixture cert as CA drove
`LOGIN OK / SELECT OK / SEARCH [b'1000'] / FETCH OK / BODY_HAS_TOKEN True / LOGOUT BYE`. So the
fixture was proven correct against a client that is not mine before any product claim was made.

### The trap: run 3 reported `email/dedupe FAIL`, and the product was right

Run 3's turns journal showed `f24c3-email-admit-…` **twice**, 90 seconds apart, each tied to its
own `imap.uid_fetch` of a distinct UID carrying the **same RFC Message-ID**. Read naively that is
a dedupe defect on a polling adapter — the exact class this lane went looking for.

It is not. The dedupe cache is keyed `(platform, account_id, message_id)`
(`dispatch/mod.rs:79-85`) with a TTL measured from `first_seen` (`dispatch/dedupe.rs:107`), and
both construction sites pass **60_000 ms** (`bootstrap.rs:3234`, `channel_inbound_host.rs`). The
delivery timestamps are `19:48:12.520` and `19:49:42.660` — **90.14 s apart**. The entry had
expired 30 seconds before the replay arrived. A second turn is the cache working as designed.

The 90 s came from **the driver**: email's `admit` leg burned its full `ARRIVAL_BUDGET_MS` of
90 s waiting for a reply that the SMTP blocker guarantees can never arrive.

So the driver was manufacturing a product defect out of its own latency. Repairs, in this lane:

1. `DEDUPE_TTL_MS` is read from the product and a replay landing outside it is graded
   **INCOMPLETE**, never FAIL — with the measured delay printed.
2. per-adapter arrival budgets, so a blocked reply path cannot inflate the delay.
3. the dedupe leg now additionally requires the **turns** count to be unchanged, which detects a
   duplicate turn even when the reply cannot leave — a strictly stronger check than the
   arrivals-only one it replaces.

**Had this been reported, it would have been a fabricated HIGH against working code.** It is the
mirror image of the telegram/route defect four hours earlier: there the instrument under-counted
a real arrival, here it over-counted a real duplicate. Same class, opposite sign.
