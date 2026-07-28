---
phase: 24-gateway-automation-channels-typed-api
criterion: "24-C3 (reference channels / the inbound matrix)"
finding: F24-C3-H2
lane: 24-c3-h2
branch: lane/24-c3-h2
status: complete
end-state: "1 — BUILT. run_gateway now constructs the inbound subscriber and the webhook host, and the installed gateway receives real inbound end to end. The fail-loudly half was ALSO taken, but only where it belongs: when [inbound_webhook] enabled = true and the stack genuinely cannot be built, the gateway refuses to start naming what is unsupported, instead of starting healthy over a dead socket."
grade-24-C3: "STILL NOT MET. F24-C3-H2 is closed and the persistent runtime can now receive — but closing it does not deliver the criterion. Neither designated reference adapter (discord, email) has an inbound fixture seam and neither was driven; four of the criterion's eight clauses (media, native actions, reconnect/reload, health) remain untouched on the inbound path; and macOS and Windows have no post-fix matrix. This lane measured Linux only."
merge-base: 0b16f86791a707c614c14a1e1ee9f1a0c17d27d9
head: ca67f13c
new-finding: "F24-C3-H4 — the gateway registers and starts TWO ChannelManagers; measured, not fixed"
---

# 24-C3-H2 — the persistent gateway runtime now hosts inbound

**One sentence: the runtime an operator actually installs — the systemd unit,
the launchd plist, the scheduled task — polled its channel adapters and dropped
every inbound event on the floor while its own config read `[inbound_webhook]
enabled = true`; it now constructs the subscriber and the webhook host, and the
same driver that scores 15/15 on `--json-stream` scores 15/15 on `gateway run`
with nine real replies read out of an independent process's journal.**

Nothing here was merged, pushed to `main`, tagged, released, or used to close an
issue. No requirement is marked complete. No credential belonging to anyone was
read, embedded or transmitted; every secret in every run was minted by the
driver at run time. `crates/wcore-cli/src/{lib,main}.rs` were not touched, so
this lane has **zero §6 fence exposure**.

---

## 1. The decision, and what it cost

**End state (1): build it.** The brief asked me to establish the cost before
committing, because my predecessor deliberately did not bodge this — it judged
that the gateway needed "a provider, an engine pool and a turn dispatcher it
does not currently construct".

That judgement was two-thirds right and the remaining third is why (1) turned
out tractable:

| Piece | Status before I started |
|---|---|
| provider | `bootstrap::create_provider_with_oauth(&Config)` — already `pub` |
| policies + tool postures | `bootstrap::load_channel_policy_configs()` — already `pub`, and already the F24-C3-H1 seam |
| turn dispatcher | `channel_dispatch::ChannelTurnDispatcher` — already `pub` |
| **engine pool** | **not needed.** `ChannelTurnDispatcher` owns `engines: Arc<Mutex<HashMap<..>>>` and builds one engine per session lazily. The gateway never has to construct or hold one. |
| media enricher, subscriber, webhook host | all already `pub` |

So no new crate, no new dependency, no new seam, and no lifecycle the gateway
did not already have. The whole change is **137 lines in `gateway.rs`** and a
**324-line new module**, and the only structural edit is lifting the manager to
`Arc<RwLock<..>>` because the subscriber and the webhook host both hold it for
the life of the process — six mechanical call sites.

**I did not take end state (2) as the answer**, because retiring a false promise
is not the same as delivering the capability and the brief was explicit that (2)
must not be recorded as closing Criterion 3. But I did build (2) **as the guard
on (1)**, which is the combination that has no silent state left in it:

- `[inbound_webhook] enabled = true` is an explicit operator opt-in. If the
  stack cannot be built, the gateway **refuses to start** and the error names
  what is unsupported. Live-proven below.
- With the webhook not enabled, the same failure is degraded rather than fatal —
  a gateway with no model still runs its schedule — but it is carried into
  `registration_error`, which `channel health` already reads, rather than left
  as a log line.
- `channel_inbound_host::spawn` returns either a live host or a typed error.
  **There is no success value in which the subscriber is absent.** That is the
  invariant that makes the original defect unrepresentable rather than merely
  fixed.

### Ordering, which is load-bearing

`start_all()` now runs **after** the subscriber acquires its broadcast receiver,
not before. Tokio's broadcast drops events published before a receiver exists,
so arming the poll loops first would silently lose every message that arrived in
the gap — a second, quieter version of the same defect.

---

## 2. The live observation

The instrument is `scripts/f24-inbound.mjs`, reused as instructed, with one
additive switch: `--runtime json-stream|gateway`. Arrivals are derived from the
journal of `scripts/f24-sink.mjs`, a separate OS process the binary cannot write
to except by completing a real TCP round trip, and cross-checked against a second
journal written by the fixture model. An unknown `--runtime` value **exits 2**
rather than falling back, so a typo cannot measure the surface that already
worked and report it as the gateway's result.

`scripts/f24-c3-h2-gateway-inbound.sh` runs three legs. At `ca67f13c`, on
`hetzner-dsm`, Linux:

```
A prefix/json-stream : legs=15 failed=0  arrivals=9 webhook_bound=true
B prefix/gateway     : legs=15 failed=15 arrivals=0 webhook_bound=false
C postfix/gateway    : legs=15 failed=0  arrivals=9 webhook_bound=true turns=9
=== VERDICT: PASS ===
```

**A and B are the same binary.** `wayland-core 0.12.25 (source e88cf43f)`,
sha256 `b390e600…` — byte-identical to the binary that produced 24-C3's 15/15
green table. Same driver, same fixtures, same config, same legs, same host, same
minute. Only the runtime surface differs. That is what isolates the fault to
`run_gateway` and not to the binary, the instrument, the fixtures or the host.

C is `wayland-core 0.12.25 (source ca67f13c)`, sha256 `7930d7b0…`. Its arrivals
journal in full — nine real HTTP round trips into another process, driven by
**`gateway run`**:

```
1 chat.postMessage    D24C3ONE      'F24C3-REPLY f24c3-slack-admit-408f0e61'
2 chat.postMessage    D24C3ONE      'F24C3-REPLY f24c3-slack-dedupe-control-408f0e61'
3 chat.postMessage    D24C3TWO      'F24C3-REPLY f24c3-slack-bind-408f0e61'
4 whatsapp.messages   15552220000   'F24C3-REPLY f24c3-whatsapp-admit-e672c713'
5 whatsapp.messages   15552220000   'F24C3-REPLY f24c3-whatsapp-dedupe-control-e672c713'
6 whatsapp.messages   15552221111   'F24C3-REPLY f24c3-whatsapp-bind-e672c713'
7 twilio.messages     +15553330000  'F24C3-REPLY f24c3-sms-admit-00535c99'
8 twilio.messages     +15553330000  'F24C3-REPLY f24c3-sms-dedupe-control-00535c99'
9 twilio.messages     +15553331111  'F24C3-REPLY f24c3-sms-bind-00535c99'
```

Each reply carries the correlation token of the message that caused it, so
"a reply arrived" and "**this** message's reply arrived" stay different claims.

The gateway's own account, from the C run:

```
[gateway] channels registered=3
[gateway] inbound: subscriber spawned, webhook host listening bind=127.0.0.1:18787 policies=3
```

`policies=3` is not decoration: it is the F24-C3-H1 seam reporting that the
`[inbound]` access policy resolved from the same directory the adapters were
registered from. A `policies=0` beside `registered=3` is that finding recurring.

### The refusal, live

```
=== case unservable-bind: enabled=true bind=not-a-socket-address ===
EXIT=1
wayland-core gateway: gateway refusing to start: [inbound_webhook] enabled = true
but this runtime cannot host inbound. [inbound_webhook] enabled = true but bind
address "not-a-socket-address" is not a valid socket address, so nothing can
listen on it
```

With its control, which is the half that matters — the **identical** unservable
bind with `enabled = false` must NOT take the runtime down, or the refusal test
would also pass on a gateway that refused unconditionally:

```
=== case control-disabled: enabled=false bind=not-a-socket-address ===
[gateway] started pid=3516367 role=Owner profile=default carried=0 …
[gateway] inbound: subscriber spawned, webhook host disabled policies=0
control_refusals=0
```

---

## 3. The guard, and proof it can fail

**The guard needs three legs, not one, and A is the reason.** A red B produced
by the defect and a red B produced by a broken fixture, a held port or a config
the binary rejects are the same colour. A runs the identical everything against
the identical binary on the surface that already worked; without it, B's red is
uninterpretable. The guard grades a failing A as **INCOMPLETE, not PASS** — it
refuses to convert an unmeasured run into a passed one. It also reports a green
B as a **contradiction to be resolved** ("the finding is wrong, or the pre-fix
binary is not pre-fix") rather than swallowing it, and grades C green-with-zero-
arrivals as **FAIL**, because a green over a dead path is the entire defect class
this guard exists for.

That last rule is aimed squarely at the two self-passing gates my predecessor
recorded: its `access` leg passed on all three adapters at the pre-fix binary
**because everything was denied**, and its `bind` leg compared a conversation to
itself. Neither was caught by a gate failing; both were caught by reading the
numbers. So the guard reads `arrivals_total`, `turns_total`, `webhook_host_bound`
and the leg count out of the result **JSON**, never out of the banner the driver
prints about itself, and every exit status is captured on the line after its
command rather than through a pipe.

I read my own numbers, and the reading changed the work twice:

1. **`arrivals=9, turns=9` on C, not just `failed=0`.** Fifteen passing legs with
   zero arrivals is representable and would have been a green over a dead path.
   It is now an explicit FAIL branch.
2. **The driver used to throw when the webhook host never bound**, producing a
   stack trace and *no result document* — so the single most important
   measurement it can make, "the runtime bound NOTHING", was the one outcome it
   could not record. It now records `webhook_host_bound: false`, fails all 15
   legs with the one reason that caused them, and writes the same document shape
   a full run writes.

### Mutation M3 — the guard reddens

The fix reverted in source (`run_gateway` constructs no inbound host, exactly as
before), rebuilt to a **distinct** binary, and the guard re-run with that mutant
as its post-fix argument:

```
MUT_BUILD_RC=0   mutant sha256 38826aef…   (≠ 7930d7b0…)
A prefix/json-stream : legs=15 failed=0  arrivals=9 webhook_bound=true
B prefix/gateway     : legs=15 failed=15 arrivals=0 webhook_bound=false
C postfix/gateway    : legs=15 failed=15 arrivals=0 webhook_bound=false turns=0
  !! C: the fixed gateway bound no inbound webhook host
=== VERDICT: FAIL ===   WLRC_MUT2=1
TREE_RESTORED_RC=0
```

A first, weaker mutation — passing the pre-fix binary as the post-fix argument —
was **rejected by the guard's own binary-identity check at rc=3 (INCOMPLETE)**
before any leg ran, because two byte-identical binaries cannot distinguish a fix
from its absence. That is the guard refusing a test I tried to give it, which is
its own small piece of evidence. M3 is the real mutation and it reddens.

---

## 4. Gates — every executed count read back

Targets run **by file / by explicit path**, never by a bare filter, and every
`N passed` read back. Run at `ca67f13c` on `hetzner-dsm`.

| Gate | Result |
|---|---|
| `cargo test -p wcore-agent --lib channel_inbound_host::` | **3 passed**, 0 failed, **0 ignored** — all three names echoed |
| `cargo test -p wcore-agent --test f24_c3_inbound_policy_home_test` | **1 passed** — 24-C3's H1 guard still holds under my change |
| `cargo test -p wcore-cli --lib` | **1830 passed**, 0 failed, 1 ignored, rc=0 |
| `cargo test -p wcore-channels` | **114 passed** + **17 passed**, 0 failed |
| `cargo clippy -p wcore-agent -p wcore-cli --all-targets -- -D warnings` | rc=0 |
| `cargo fmt --all -- --check` (Mac) | rc=0 |
| Guard, tip commit | **PASS**, rc=0 |
| Mutation M3 | **FAIL**, rc=1; tree restored `git diff --quiet` rc=0 |
| Startup refusal + control | EXIT=1 with cause named; control started, 0 refusals |
| §6 fence vs merge-base `0b16f867` | **0 files** — `wcore-cli/src/{lib,main}.rs` untouched |

**One line in the `wcore-cli` output reads `test result: FAILED. 0 passed; 1
failed`, and it is not a failure.** It is a nested `failing_fixture` crate
spawned by `plugin::scaffold::tests::plugin_test_propagates_a_failing_suite` — a
deliberate fixture proving the plugin scaffold propagates a red suite. Its child
output interleaves into the parent's stdout. The parent test passes and
`RC_CLI=0`. I checked this rather than assuming, because an unexplained FAILED
line beside a green rc is exactly the shape one should not wave through.

**Clippy scope.** The brief warned of 4 pre-existing clippy errors in
`journey.rs`. That file is `crates/wcore-eval-scenarios/src/journey.rs` —
another crate and another lane's. My clippy scope is `-p wcore-agent -p
wcore-cli`, which excludes it. I did not fix, silence, `#[allow]` or inherit
those errors, and I make no claim about them.

---

## 5. A new finding I measured but did not fix

### F24-C3-H4 — the gateway registers and starts TWO ChannelManagers

From the C run's own log, **six registration events for three channels**:

```
17:19:19.718  channel auto-registered  f24c3slack / f24c3sms / f24c3whatsapp
17:19:20.359  channel auto-registered  f24c3slack / f24c3sms / f24c3whatsapp
```

`run_gateway` calls `wcore_agent::cron::build_headless_cron_handler(&cwd)`, which
builds its **own** `ChannelManager`, auto-registers every adapter into it and
calls `start_all()` on it. `run_gateway` then registers the same adapters again
into its own manager. Two managers, both polling, in one process — and only one
of them (mine) has a subscriber.

This predates my change and my change does not cause it. For the three **webhook**
adapters it is harmless, and the evidence says so: webhook POSTs route to the
manager the host holds, and the run recorded exactly nine arrivals with the
dedupe legs passing — no duplication.

**For polling adapters it is not obviously harmless, and I did not measure it.**
Email IMAP, Telegram `getUpdates` and the Discord gateway all *consume* as they
poll — a seen-flag, an update offset, a session cursor. A second manager polling
the same account can take delivery of a message that then never reaches the
subscriber, which would be the accepts-and-never-does-the-thing shape again on
the one path this lane could not instrument.

I am reporting the part I measured (double registration and double `start_all`,
proven) separately from the part I did not (the consumption race, **unmeasured**).
I did not grade it HIGH because I have no polling fixture to grade it with — and
the adapters it would affect are precisely the ones with no fixture seam. Fixing
it blind, at the end of a lane, is how a fix becomes the next lane's defect,
which is the reasoning my predecessor applied to H2 and was right to.

---

## 6. What Criterion 3 still lacks — stated without narrowing

**Criterion 3 is STILL NOT MET.** Closing F24-C3-H2 removes the objection that
"a criterion about channels proving themselves cannot be met while the runtime an
operator installs has no inbound receiver". It does not supply the rest.

- **Neither designated reference adapter's inbound path has been driven.**
  `24-03` named Discord and email as the reference pair — one persistent
  connection, one polling. Discord's API base is overridable only through a
  `#[doc(hidden)]` test constructor, so from the shipped binary its inbound can
  only ever reach `discord.com`. Email needs a CA the host trusts. Neither moved
  this lane. Grading the criterion on the three adapters that happen to have a
  fixture seam would be narrowing it to the adapters that worked.
- **Discord and Telegram are NOT MEASURED for want of a vendor credential**,
  which is Sean's alone to supply. That is not a zero and not a pass, and I did
  not embed, copy or invent one.
- **Four of the criterion's eight clauses remain untouched on the inbound path**:
  media, native actions, reconnect/reload, health. The matrix covers access,
  routing, and idempotency-as-inbound-dedupe.
- **macOS and Windows have no post-fix gateway matrix.** This lane measured
  **Linux only**. The `--runtime gateway` switch works on any platform, but I ran
  it on none of them and claim nothing about them.
- **F24-C3-H4 is open** and the polling-inbound path in the gateway is
  unmeasured.

---

## 7. Open, and for whom

| Item | Severity | Owner |
|---|---|---|
| **F24-C3-H4** — gateway registers + starts two `ChannelManager`s; polling-consumption race unmeasured | MEDIUM (measured) / potentially HIGH (unmeasured) | a Core lane with a polling fixture |
| macOS + Windows gateway inbound matrix (`--runtime gateway` is ready; nothing else needed) | MEDIUM | needs a build at the candidate commit on each |
| Email inbound against a TLS IMAP fixture via `SSL_CERT_FILE` (Linux) | MEDIUM | carried from 24-C3, still feasible |
| Matrix inbound against a `/sync` fixture | MEDIUM | carried from 24-C3 |
| Discord/Telegram have no fixture seam — a config-level base-URL override, or accept they are only provable with a vendor credential | MEDIUM (design) | Sean's call; the credential half is Sean-only |
| Inbound media / native actions / reconnect-reload / health legs | MEDIUM | the rest of the criterion's clause list |

No protocol seam changed. No contract fixture was regenerated. No shared-file
fence edit. Nothing requires the orchestrator to serialize this lane against
another beyond the ordinary merge.
