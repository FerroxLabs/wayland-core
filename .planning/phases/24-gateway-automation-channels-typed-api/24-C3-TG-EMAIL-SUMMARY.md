# 24-C3-TG-EMAIL — telegram and email inbound, measured without a vendor credential

Lane `lane/24-c3-tg-email`, HEAD `f12cffb7` (plus this summary's commit). Base
`ef1d97be`. All live runs on `hetzner-dsm`, `/root/wayland-24-c3-tg-email`.

**Verdict up front: `24-C3` is NOT MET, and this lane does not claim it.** The polling half
is now measured and it is what closed. Two reference adapters remain undriven, one clause of
the criterion is unmeasurable for email by any configuration-only mechanism, and a HIGH found
by the new steady-state leg was fixed and re-proven live.

---

## 1. Per adapter, per leg

The criterion's five legs are defined on **arrivals**: a reply that leaves the binary and lands
in a journal owned by another process. Nothing below is graded on a status line the product
printed about itself.

Final run: **run 6**, driver `f12cffb7`, binary
`wayland-core 0.12.25 (source f12cffb764d962eb8356033fc6d04ff6f51c0ab6)`,
sha256 `b2afc30d…76f44db`.

| adapter | transport | admit | dedupe | access | bind | route |
|---|---|---|---|---|---|---|
| slack | webhook | PASS | PASS | PASS | PASS | PASS |
| whatsapp | webhook | PASS | PASS | PASS | PASS | PASS |
| sms | webhook | PASS | PASS | PASS | PASS | PASS |
| **telegram** | **poll** | **PASS** | **PASS** | **PASS** | **PASS** | **PASS** |
| **email** | **poll** | **NOT MEASURED** | **NOT MEASURED** | **NOT MEASURED** | **NOT MEASURED** | **NOT MEASURED** |

```
INBOUND MATRIX RED platform=linux runtime=json-stream legs=20/25 failed=0
not_measured=5 accounted=25/25 arrivals_total=9 telegram_arrivals=3 mail_arrivals=0
turns_total=18 instrument_fault=false
```

`RED` means "not everything could be asked", not "something misbehaved". **Zero legs failed.**
The banner reports `accounted=25/25` so a reader can see no leg went missing.

### Telegram — 5/5, and it is the first fully-measured polling adapter in this matrix

Every number below comes from the fixture's journal, in another OS process.

- `admit` — `arrivals(journal)=1 want=1`, cross-checked `turns(fixture-journal)=1`.
- `route` — `reply_text="F24C3\-REPLY f24c3\-telegram\-admit\-…" carries_correlation=true
  conversation_id="24030001" want="24030001"`.
- `dedupe` — replay of the **same `message_id` under a fresh `update_id`** at +1056 ms, inside
  the product's 60 000 ms TTL: `arrivals before=1 after=1`, `turns before=1 after=1`,
  positive-control fresh id `arrivals=1`.
- `access` — `arrivals=0 want=0, turns=0 want=0`, with the admit control held at 1.
- `bind` — `conv1="24030001" conv2="24030002" distinct=true`.

Independent observables: `max_concurrent_getupdates=1` counted out-of-process from overlapping
open requests (2 would mean two managers competing for one destructive read — F24-C3-H4; 0 would
mean nothing polled), `submitted=5 still_pending=[]`, and `serve_count=[1,1,1,1,1]` — no update
served twice.

**The `access` green is positive, not manufactured.** The denied sender's update was *served and
consumed* by the poller (`deleted_by` set, `serve_count=1`), so the zero arrivals are a refusal
at the access gate, not an unread queue. The brief's universal-denial trap is closed by making
the admit control part of the pass condition, not merely printed beside it.

No vendor credential anywhere: the bot token is minted per run and the fixture *is* the API.
**No Rust change was needed for telegram** — `TelegramConfig::api_base_url` already existed and
`TelegramChannel::new` already honoured it.

### Email — inbound proven, reply half unmeasurable, five legs NOT MEASURED

Recorded NOT MEASURED rather than FAIL, from the fixture's **observed** SMTP failures rather than
from a lockfile inference. What was established instead is reported under a deliberately
different name so it cannot be read as the five legs:

| email-admission probe | result | evidence |
|---|---|---|
| fetch | PASS | `imap.uid_fetch before=0 after=1` — shipped binary completed a TLS IMAP session against the run's throwaway cert |
| admit | PASS | `turns(fixture-journal)=1 want=1` for an allowlisted `From:` |
| dedupe | PASS | same RFC Message-ID at **+2052 ms** (inside the 60 s TTL): turns before=1 after=1; control fresh Message-ID turns=1 |
| access | PASS | denied `From:` turns=0; admit control=1; **and the denied message WAS fetched** (`uid_fetch 3→4`), so the zero is a refusal, not an unread mailbox |
| second-sender-admitted | PASS | turns=1. **Explicitly NOT the bind leg** — see §3 |
| **steady-state** | **PASS (post-fix)** | 6 back-to-back: `turns per message=[1,1,1,1,1,1]`, never-fetched=0, fetched-more-than-once=0, `max_concurrent_imap=1` |

All 11 mailbox messages show `fetch_count=1` — read exactly once each, none twice, none never.

---

## 2. The HIGH the steady-state leg found — fixed and re-proven live

**`crates/wcore-channel-email/src/imap.rs` silently destroyed every message in a batch except
the largest.** This is a second, independent loss mode from F24-C3-H4's two-poller race, and it
fires with a *single* poller.

Measured at `0ed5a5d7`, run 5. Six messages delivered back-to-back; the fixture's own trace:

```
20:07:15.875 DELIVER uid 1005  …  20:07:16.005 DELIVER uid 1010
20:07:17.096 SEARCH q=1005:*  hits [1005,1006,1007,1008,1009,1010]   <- server answered ALL SIX
20:07:17.137 FETCH  uid 1010                                          <- exactly ONE fetch
20:07:19.229 SEARCH q=1011:*                                          <- watermark past the rest
```

`turns per message=[0,0,0,0,0,1]`. Five inbound messages gone, no error, no retry, no log line.

Root cause: `Session::uid_search` returns `HashSet<Uid>` (imap-2.4.1 `client.rs:1276`), whose
iteration order is arbitrary and re-randomised per process. The loop filtered each candidate
against a `high_water` it **mutated inside that same loop**, so visiting the largest UID first
made `uid <= high_water` true for every remaining new message. `uid_store::save` then persisted
that maximum, making the loss permanent — the skipped mail is below the watermark forever.

Impact: any poll cycle finding more than one new message, i.e. the normal case for a real
mailbox. Severity **HIGH**.

Fix `f12cffb7`: freeze the watermark for the duration of the batch, and sort ascending — which
also repairs a second, milder defect the same `HashSet` caused, mail reaching the engine in
arbitrary rather than arrival order. Selection extracted to `new_uids` so the invariant is
testable without a live session.

- `cargo test -p wcore-channel-email --lib` on hetzner: **80 passed, 0 failed**. All four new
  tests confirmed present **by name** (`every_new_uid_is_selected_regardless_of_set_iteration_order`,
  `the_old_filter_loses_five_of_six_when_the_largest_uid_comes_first`,
  `already_seen_uids_are_still_skipped`,
  `selection_is_ascending_so_mail_reaches_the_engine_in_arrival_order`), guarding the
  "filter matched no test" trap.
- The **pre-fix filter is kept executable** in the test module and asserted to return `vec![1010]`
  from the hostile order, so the repair is proven to change an outcome rather than to restate
  the new behaviour.
- Live re-proof, run 6, same driver and same fixture, only the binary changed:
  `[0,0,0,0,0,1]` → `[1,1,1,1,1,1]`.

---

## 3. Exact remaining distance to `24-C3` MET

Criterion: *"Reference channels prove setup/auth, access, routing, media, native actions,
idempotency, reconnect/reload, and health."*

**Closed by this lane:** the polling class now has two adapters driven end to end, one of them
(telegram) on all five arrival legs. `access` and `idempotency` are positively proven on a
polling transport for the first time, with controls that can fail.

**Still open, honestly:**

1. **Email `route` / `bind` — the reply half, blocked at TLS trust.** SMTP is
   `lettre` + `tokio1-rustls-tls` (`crates/wcore-channel-email/Cargo.toml:11`) whose resolved
   `Cargo.lock` deps include **`webpki-roots` and not `rustls-native-certs`**. `webpki-roots` is
   a compiled-in Mozilla root set: it reads no file and no environment variable, **on any
   platform**. Proven executably, not inferred — in one run, one process, one certificate, one
   `SSL_CERT_FILE`, IMAP accepted the cert (`imap.uid_fetch … set_seen=true`) and 0.6 s later
   SMTP was refused: `EHLO` → `STARTTLS` → `tls alert certificate unknown … SSL alert number 46`,
   82 sessions, all identical. Closing this needs either a publicly-chained cert or a
   cert-source knob in the adapter; it is **not reachable from configuration**.
2. **Email `bind` is unproven even at the turn level.** The turns journal carries no conversation
   id, so "two senders produced two turns" cannot show they bound to *distinct sessions*. Stated
   rather than papered over.
3. **Discord, Signal, Matrix, MS Teams, iMessage** — still entirely undriven inbound.
4. **`media`, `native actions`, `reconnect/reload`, `health`** — four clauses of the criterion
   this matrix does not touch at all, for any adapter.
5. **`gateway run`** — every figure here is `--runtime json-stream`. The gateway surface was
   F24-C3-H2's finding and was not re-measured this lane, though the driver now supports it and
   will run the polling legs even when no webhook host binds.

So: **the polling half is closed and 24-C3 is not.** Three prior lanes correctly declined to
mark it MET and this one declines too.

---

## 4. The macOS warning — do not plan around this

**`SSL_CERT_FILE` reaches the IMAP path on Linux only. It does NOT work on macOS.**

`crates/wcore-channel-email/Cargo.toml:13` pulls `native-tls`; `imap.rs:194` calls
`native_tls::TlsConnector::new()`. That resolves per platform:

| platform | native-tls backend | honours `SSL_CERT_FILE` |
|---|---|---|
| Linux | OpenSSL | **yes** — this is the whole seam |
| macOS | Security.framework / system keychain | **no** |
| Windows | SChannel / system cert store | **no** |

A macOS email leg needs a **different mechanism** — installing the fixture root into the login
keychain, or adding a cert-source knob to the adapter. It is not achievable by exporting an
environment variable, and any plan that assumes otherwise will fail at the first run. The driver
encodes this: on a non-Linux host it records the email legs NOT MEASURED with this reason rather
than producing zeros that would read as a product defect.

**And separately: the SMTP half is unreachable by `SSL_CERT_FILE` on *every* platform**, Linux
included, for the `webpki-roots` reason in §3.1. These are two different blockers and conflating
them will waste a cycle.

---

## 5. Instrument defects found in this lane, and repaired in it (§6b-ii)

Three, all in my own harness. Each was repaired here, not written up and passed on.

1. **Correlation matcher under-counted a real arrival.** Predicted from source *before* the first
   run: telegram defaults to MarkdownV2 and escapes the full outbound body, so
   `f24c3-telegram-admit-…` arrives as `f24c3\-telegram\-admit\-…` and `String.includes` scores a
   delivered reply zero. Replaced by `scripts/f24-correlate.mjs`, a three-tier matcher
   (exact → de-escaped → fuzzy) with an explicit **`instrument_fault`** state: a token present in
   an encoding the driver cannot decode grades the run **INCOMPLETE (exit 3)**, never LOSS.
   Self-test `scripts/f24-correlate.test.mjs`: **16 passed, 0 failed** on both Mac and hetzner,
   asserted three ways per the brief — known-positive passes, known-negative fails *and is not
   excused as a fault*, and **the old matcher would have missed it** (asserted as the field's own
   `replied=0` against 8 real arrivals). The rejected alternative — minting hyphen-free tokens —
   would have hidden the defect instead of instrumenting it.
2. **The repair was PARTIAL, and the live run caught it.** `arrivalsFor` was moved onto the
   module; `runMatrix`'s route check was not. Run 1 reported
   `carries_correlation=false` about a reply that had arrived, in the right conversation, with the
   right token. One call site repaired, one missed, and the missed one failed **in the direction
   that blames the product**. Fixed, plus a **source scan** in the self-test that fails on any
   raw `String.includes` against a correlation variable — guarded against passing vacuously by
   asserting the source is >10 kB first. A comment would not have caught this.
3. **The driver nearly manufactured a HIGH out of its own latency.** Run 3 reported
   `email/dedupe FAIL` on a duplicate turn 90.1 s after the original. The product was right: the
   dedupe TTL is 60 000 ms (`bootstrap.rs:3234`, `dispatch/dedupe.rs:107` measures from
   `first_seen`), and email's admit leg was burning its full 90 s arrival budget waiting for a
   reply the SMTP blocker guarantees can never come. `DEDUPE_TTL_MS` is now read from the
   product; a replay outside it grades **INCOMPLETE, never FAIL**, printing the measured delay.
   Per-adapter arrival budgets stop a blocked reply path inflating the delay. The dedupe leg was
   also **strengthened** to require the turns count unchanged, which catches a duplicate turn
   even when the reply cannot leave.

Note the symmetry: (1) under-counted a real arrival, (3) over-counted a real duplicate. Same
class, opposite sign, one lane apart. Neither would have been visible from a green.

The percent-encoding case in the self-test also exposed a hole in the *first* draft of the
detector — skeleton-substring only catches non-alphanumeric noise, and `%2D` inserts
alphanumerics. Rather than weaken the test case, the detector was strengthened to a bounded
subsequence window. Recorded because writing the self-test first is what found it.

---

## 6. What I did NOT do

- **Did not mark `24-C3` MET.** See §3.
- **Did not run the `gateway` runtime.** Every figure is `--runtime json-stream`.
- **Did not drive discord, signal, matrix, msteams, imessage.**
- **Did not measure media, native actions, reconnect/reload, or health** — four criterion clauses.
- **Did not touch the shared fence.** `git diff $BASE -- crates/wcore-cli/src/lib.rs
  crates/wcore-cli/src/main.rs` is empty (`$BASE` = the captured merge-base SHA `ef1d97be`, not
  the branch name).
- **Did not run `wcore-contract generate`**, merge, open a PR, tag, or close an issue.
- **Did not use, read, or require any vendor credential.** Every secret in these runs is minted
  at run time and dies with it.
- **Did not run a full-workspace build or test.** Only `-p wcore-channel-email` and
  `-p wcore-cli`, per the lane brief's disk/contention rule.
- **Did not verify the email fix on macOS or Windows.** The `HashSet` ordering defect is
  platform-independent by inspection, but the live re-proof is Linux-only.

## 7. For the orchestrator to serialize

- **One Rust file changed: `crates/wcore-channel-email/src/imap.rs`** (the HIGH fix + 4 tests).
  No other lane should be in that file. No `Cargo.toml`/`Cargo.lock` change, no new dependency,
  no protocol or contract change.
- Four script files: `scripts/f24-inbound.mjs` (modified), `scripts/f24-tg-fixture.mjs`
  (modified — optional explicit `message_id`), `scripts/f24-correlate.mjs`,
  `scripts/f24-correlate.test.mjs`, `scripts/f24-mail-fixture.mjs` (new).
- `f24-inbound.mjs` exit codes changed: **0 GREEN, 1 RED, 3 INCOMPLETE (instrument fault)**. Any
  gate treating non-zero as a uniform failure still works; one distinguishing harness fault from
  product regression should read the code.
- **`ADAPTERS` grew from 3 to 5**, so a caller asserting 15 legs must now expect 25.

## 8. Evidence

`.planning/phases/24-gateway-automation-channels-typed-api/24-C3-TG-EMAIL-evidence/`

| directory | what it holds |
|---|---|
| `24-C3-TG-EMAIL-NOTES.md` | the running log, committed at T+15min and after every measurement |
| `run2-telegram-green/` | telegram 5/5, 20/20 legs, `instrument_fault=false` |
| `run5-email-steady-loss/` | the HIGH: `steady-state-loss-trace.jsonl` has the six-UID search and the single fetch |
| `run6-post-fix-green/` | post-fix: `[1,1,1,1,1,1]`, all 11 messages `fetch_count=1`, 0 failed legs |

Journal byte counts, run 6: `arrivals=2424, turns=6048, telegram=62320, mail=117556,
core_log=17297`. Recorded because an empty journal and an absent journal both read as
"0 arrivals" if only parsed records are counted.
