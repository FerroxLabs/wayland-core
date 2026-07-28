---
phase: 24-gateway-automation-channels-typed-api
criterion: "24-C3 (reference channels / the inbound matrix)"
finding: F24-C3-H4
lane: 24-c3-h4
branch: lane/24-c3-h4
status: complete
verdict: "REPRODUCED, MEASURED, AND FIXED. The double start is real; the consumption race it was suspected of is real and worse than filed — 8/8 lost at startup and 5/6 lost in steady state, silently. One manager, one owner, and both proof legs are positive: inbound arrives (8/8 turns, 8/8 replies) and cron still fires."
severity-as-filed: "MEDIUM (measured) / potentially HIGH (unmeasured)"
severity-as-measured: "HIGH"
merge-base: e6abc748ecca0127545e8b34e949d6ad1d741cf5
head: fb7822ac (summary commit; final tip recorded in the lane report)
new-seam: "TelegramConfig.api_base_url — the first fixture seam for a POLLING inbound adapter"
---

# 24-C3-H4 — the gateway's second ChannelManager was eating inbound messages

**One sentence: `gateway run` started two `ChannelManager`s over the same
account and only one of them had a subscriber, so on a polling adapter the
other one took delivery and dropped it on the floor — 8 of 8 messages lost at
startup and 5 of 6 lost in steady state, with no error, no log and no retry
anywhere; there is now one manager with one owner, and the run that used to
lose everything answers all of it.**

Nothing here was merged, pushed to `main`, tagged, released, or used to close an
issue. `wcore-contract generate` was not run. No requirement is marked complete.
No credential belonging to anyone was read, embedded or transmitted: the bot
token, the vault passphrase and the model key in every run were minted by the
driver at run time. `crates/wcore-cli/src/{lib,main}.rs` were not touched, so
this lane has **zero §6 fence exposure** (`git diff $(git merge-base HEAD
plan/f20-unified-audit-repair) -- crates/wcore-cli/src/lib.rs
crates/wcore-cli/src/main.rs` → empty).

---

## 1. The reproduction, before anything was changed

Two managers, both registering from the same directory, only one subscribed:

| | manager #1 | manager #2 |
|---|---|---|
| built by | `cron::build_headless_cron_handler(&cwd)` | `run_gateway` itself |
| registers from | `wayland_config_dir()/channels` | `home.join("channels")` |
| `start_all` | yes | yes |
| **subscriber** | **no** | yes |

`run_gateway` sets `WAYLAND_HOME=home` when `--home` is passed, so
`wayland_config_dir()` and `home` resolve to the SAME directory — which is why
24-C3-H2 saw six registration events for three channels rather than three.

I did not take that on report. The count is measured **outside the binary**, by
a fixture in another OS process, from overlapping open `getUpdates` requests:
`max_concurrent_getupdates` = **2** pre-fix, **1** post-fix. A log line the
product prints about itself would not have been evidence.

---

## 2. The consumption race — real, and worse than it was filed at

### The mechanism, in the fixture's own journal

```
17:45:01.531  channel auto-registered f24c3h4tg      <- manager #1 registers
17:45:03.331  deleteWebhook                          <- manager #1 begins polling
17:45:03.334  getUpdates poll=1 offset=0   served=[1,2,3,4,5,6,7]
17:45:03.3351 channel auto-registered f24c3h4tg      <- manager #2 registers
17:45:03.3357 [gateway] inbound: subscriber spawned
17:45:03.337  getUpdates poll=2 offset=8   DELETED=[1,2,3,4,5,6,7]  served=[8]
17:45:03.365  getUpdates poll=3 offset=9   DELETED=[8]
17:45:07.028  deleteWebhook                          <- manager #2 begins polling
17:45:07.030  getUpdates poll=7 offset=9   served=[]  <- nothing left
```

Manager #1 swept the queue and confirmed it away **3 milliseconds before manager
#2 had even registered**, and 3.7 seconds before manager #2 polled at all.
Telegram's `offset=N` is a destructive confirm: those updates are gone
server-side, for everyone, forever. **LLM journal for that run: 0 turns.** Eight
inbound messages, and nothing anywhere reports a problem.

### The full matrix

| Leg | Binary | Scenario | Pollers | Sub | Turns | Replied | Lost | Cron |
|---|---|---|---|---|---|---|---|---|
| A | pre `402f7c70` | 4 queued + 4 live @4s | **2** | 8 | 0 | 0 | **8** | — |
| B | post `7a042868` | same, + a cron job | **1** | 8 | 8 | 8 | **0** | **1** |
| M-DENY | mutant `5f360e0a` | same | **0** | 8 | 0 | 0 | 8 | 0 |
| steady-pre | pre | 6 live after a 45s settle | **2** | 6 | 1 | 1 | **5** | — |
| steady-post | post | same | **1** | 6 | 6 | 6 | **0** | — |

### Why the steady-state leg exists, and why it changes the severity

Legs A and B put every message inside the gateway's startup window, where the
two managers come up seconds apart. On their own they would only have proved a
startup sweep — bad, but bounded to "messages queued while the gateway was
down". So a further leg submits nothing until **45 seconds after start**, with
both loops established and long-polling: **5 of 6 lost**. The loss is ongoing,
not a startup artifact.

Its control is the half that makes it readable: the identical 45-second settle
against the fixed binary is **6 of 6 answered, 0 lost**. Without that, "5/6 lost
after 45s" would have been equally consistent with "this adapter stops working
after 45 seconds", which would have been true of both binaries and would have
proved nothing about the race.

**Filed as MEDIUM/potentially-HIGH. It earns HIGH.** Silent majority-to-total
inbound message loss, no error path, on the persistent runtime Phase 24 installs
as a systemd unit / launchd plist / scheduled task — on the very feature the
phase is shipping. It is fixed, which is what the severity policy requires of a
HIGH.

### What the measurement does NOT cover, stated plainly

- **Telegram is the only polling adapter measured.** Email IMAP (`\Seen`) and
  the Discord gateway (one session per token) are destructive-on-read by the
  same mechanism and are affected by the same double start, but I did not drive
  them. Email needs a TLS IMAP fixture the host trusts; Discord's API base is
  still overridable only through a `#[doc(hidden)]` constructor. Those are
  carried below, unmeasured, and I claim nothing about them beyond the
  mechanism they share.
- **Linux only.** No macOS or Windows leg.
- The fixture deliberately does **not** answer a second concurrent `getUpdates`
  with real Telegram's `409 Conflict`. 409ing would make the second poller fail
  loudly, which is the easy case; serving both is the quiet case that produces
  silent loss, and it is the one worth measuring. Against the real API the
  observable behaviour would differ in shape — 409 storms alongside the loss —
  but the destructive confirm that causes the loss is Telegram's documented
  semantics, not the fixture's invention.

---

## 3. The fix — one manager, one owner

`build_headless_cron_handler_with_channels(cwd, Some(arc))` adopts a manager the
caller already owns and **registers nothing and starts nothing**; the original
`build_headless_cron_handler(cwd)` is unchanged for `cron daemon`, which has no
manager of its own. `run_gateway` now builds its channel stack — register → Arc
→ subscriber → `start_all` — **before** the automation plane, and hands the
plane's handler that same `Arc`.

Two orderings are load-bearing and neither is cosmetic:

1. **subscriber before `start_all`** (inherited from 24-C3-H2): tokio's
   broadcast drops events published before a receiver exists.
2. **channels before the plane** (new): `plane.resume()` dispatches carried
   deliveries through the channel sink, and adapters that resolve their
   credential in `start()` cannot send before `start_all` has run. Building the
   plane first would have traded inbound loss for broken delivery recovery.

The invariant is that the gateway is the sole owner of exactly one manager, so
the double start is now unrepresentable from that call site rather than merely
absent from it.

---

## 4. The seam — and why its absence was the actual blocker

Telegram was the **only** HTTP channel adapter with no config-level base URL.
`SlackConfig`, `WhatsAppConfig` and `SmsConfig` all already carry
`#[serde(default = "default_api_base")] pub api_base_url: String`, which is
exactly why 24-C3's matrix could drive those three and not telegram.
`TelegramChannel::with_api_base` existed but is `#[doc(hidden)]` and
`wcore_channels_registry::make_telegram` calls `new` — so **no config a shipped
binary could load pointed the polling adapter anywhere but `api.telegram.org`.**

That is the reason the polling inbound path had gone unmeasured for the whole
phase, and it is a two-field change to close: `TelegramConfig.api_base_url`
defaulting to `TELEGRAM_API_BASE`, and `new` honouring it. An untouched
production config reaches production Telegram byte-for-byte as before — asserted
by `new_without_an_override_still_points_at_production_telegram`, which is the
control for `new_honours_the_configs_api_base_url`.

Trust note: this is operator-owned configuration at the same level as
`credential_handle`. Anyone who can write that file can already name the
credential the adapter sends. It is not reachable from a message.

**This seam outlives the finding.** It is the first fixture seam for a polling
inbound adapter in this program, and any future email/Discord equivalent has a
precedent to copy.

---

## 5. Gates — every executed count read back

Run on `hetzner-dsm`, by package/path, never by a bare filter.

| Gate | Result |
|---|---|
| `cargo test -p wcore-channel-telegram` | **66 passed**, 0 failed, **0 ignored**; both new tests echoed by name |
| `cargo test -p wcore-channels-registry` | **11 passed**, 0 failed |
| `cargo test -p wcore-channels` | **114 passed** + **17 passed**, 0 failed |
| `cargo test -p wcore-cli --lib` | **1830 passed**, 0 failed, 1 ignored, rc=0 |
| `cargo test -p wcore-agent --lib` | see below — **not clean in parallel, 2124/0 serial** |
| `cargo clippy -p wcore-channel-telegram -p wcore-agent -p wcore-cli --all-targets -- -D warnings` | rc=0 |
| `cargo fmt --all -- --check` (Mac) | rc=0 |
| Guard `f24-c3-h4-guard.sh`, tip | **PASS**, rc=0 |
| Mutation M-DENY | **FAIL**, rc=1; tree restored `git diff --quiet` rc=0 |
| Fixture self-test | **8/8**, rc=0 |
| §6 fence vs merge-base `e6abc748` | **0 files** |

**The one `test result: FAILED. 0 passed; 1 failed` line in the `wcore-cli`
output is not a failure.** It is the nested `failing_fixture` crate that
`plugin::scaffold::tests::plugin_test_propagates_a_failing_suite` deliberately
shells out to; the failing test inside it is named `always_fails`, its output
interleaves into the parent's stdout, and the parent run is rc=0 with 1830
passed. I read the lines rather than counting them.

### `cargo test -p wcore-agent --lib` is red in parallel, and it is not this lane

It reported 13, then 14 failures, all in `engine::` / `orchestration::` /
`session::` / `session_journal::`, none of them near this diff, and all of the
form `session journal writer lease is already held at /tmp/...`. Three runs
settle it:

```
HEAD, parallel                                  : 2110 passed; 14 failed
HEAD, --test-threads=1                          : 2124 passed;  0 failed  rc=0
CONTROL — my cron.rs reverted to base, parallel : 2109 passed; 15 failed
```

The control — the identical tree with only my `wcore-agent` change removed —
fails **more** than HEAD does, with a shifting and partly different set, and
everything passes single-threaded. That is a pre-existing process-global-lease
contention artifact in that suite, not a regression here. I did not weaken,
`#[ignore]`, `#[allow]` or re-gate anything to make it green, and I am reporting
the red with its numbers rather than the serial green alone.

---

## 6. The instrument, and a fault in it I nearly shipped as a defect

The fixture is proven against a known-positive and a known-negative **before**
it measures anything (`f24-c3-h4-fixture-selftest.mjs`, 8/8): a single poller
must be served all four updates; a competing poller that confirms first must
leave the second one ZERO; two overlapping long-polls must read as 2; and a run
with **no** poller must read as **0, not 1** — that last one is the branch that
stops a fix which works by making nothing start from passing as "one manager".

Then the fault. Both first-pass runs printed `replied=0`. For the post-fix run
that was **wrong**: all eight replies had arrived. Telegram's default parse mode
is MarkdownV2 and the adapter escapes every reserved character, so the
correlation token leaves the product as `f24c3\-h4\-pre\-0\-…` and a plain
`includes(token)` never matches. **I was one step from writing up a fully
working path as total inbound loss** — the instrument carrying the exact defect
class it was hunting.

Closed two ways: un-escape before matching, and an explicit `instrument_fault`
state — "the adapter delivered N replies but none matched a submitted token" is
now reported as an instrument fault that makes the run INCOMPLETE, and a loss
claim requires that nothing came back at all. The guard refuses to grade a run
carrying that flag.

---

## 7. Both proof legs, positively

A fix that made nothing start would pass every "no duplicate registration"
check, so neither leg is a formality:

- **Inbound arrives.** 8 submitted → 8 model turns in the fixture's journal → 8
  replies at the adapter, each carrying the correlation token of the message
  that caused it. Turns and replies are counted from two different OS processes
  and are never collapsed into one number.
- **Cron still fires.** The cron handler no longer owns a manager, so the
  scheduler could have been quietly cut off from every channel. A job added
  through the product's own `wayland-core cron add "* * * * *" --channel … `
  surface fired **1** time through the shared manager and its text arrived at
  the adapter. The mutation run shows `cron_fires=0`, so this check can fail.

---

## 8. Open, and for whom

| Item | Severity | Owner |
|---|---|---|
| Email IMAP inbound in the gateway — same destructive-read mechanism, **unmeasured**; needs a TLS IMAP fixture via `SSL_CERT_FILE` | MEDIUM | a Core lane; carried from 24-C3 |
| Discord gateway inbound — same mechanism, **unmeasured**; API base still overridable only via `#[doc(hidden)]`. `TelegramConfig.api_base_url` is now the precedent for closing this at config level | MEDIUM (design) | Sean's call on the config-vs-credential split |
| `cargo test -p wcore-agent --lib` fails 13–15 nondeterministically in parallel on a process-global session-journal writer lease; 2124/0 serial. Pre-existing, reproduced with my change reverted | MEDIUM | not this lane — surfaced for the orchestrator |
| macOS + Windows gateway polling matrix (`f24-c3-h4-guard.sh` is portable; nothing else needed) | MEDIUM | needs a build on each |
| Inbound media / native actions / reconnect-reload / health legs | MEDIUM | the rest of criterion 3's clause list |

**Criterion 24-C3 is still NOT MET**, and closing F24-C3-H4 does not move it
much: it removes a defect that would have made any polling-inbound result
meaningless, and it supplies the polling fixture seam the criterion needs, but
neither designated reference adapter (Discord, email) has been driven and four
of the criterion's eight clauses remain untouched on the inbound path.

No protocol seam changed. No contract fixture was regenerated. No shared-file
fence edit. Nothing requires the orchestrator to serialise this lane against
another beyond the ordinary merge.

## Evidence

`.planning/phases/24-gateway-automation-channels-typed-api/24-C3-H4-evidence/`
— `guard-PASS.txt`, `guard-MUTATION-DENY-FAIL.txt`, the four
`result-*.json`, `prefix-fixture-journal-head.jsonl` (the mechanism),
`postfix-replies.jsonl`, `code-gates.txt`,
`wcore-agent-parallelism-control.txt`, and `24-C3-H4-NOTES.md` (the running
log, committed within the first 15 minutes and re-committed after every
measurement).
