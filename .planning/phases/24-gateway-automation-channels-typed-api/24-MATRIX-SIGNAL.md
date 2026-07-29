---
phase: 24-gateway-automation-channels-typed-api
criterion: "24-C3 (reference channels / the inbound matrix)"
lane: 24-matrix-signal
branch: lane/24-matrix-signal
merge-base: d34b2fe119916b7e35ad47d28783634955d75664
status: complete
grade-24-C3: "STILL NOT MET, and this lane does not claim it. Two more adapters (matrix, signal) are now driven across all five original legs plus a new steady-state leg — 12/12 PASS each, reproduced three times. A NEW cross-cutting leg was added to every adapter. A new HIGH was found on the matrix inbound restart path, proven with three independent controls and reproduced three times, and is NOT fixed. Two of the eight clauses (media, native actions) still have zero evidence on any adapter."
new-finding: "F24-C3-H6 — matrix holds its `/sync` cursor in a process-local variable and discards the initial sync's timeline, so every message delivered while the process is down is silently lost on restart. Measured, triple-controlled, reproduced 3/3, NOT fixed."
seam-determination: "signal's subprocess-path seam is as cheap as costed and is the cheapest in the phase — CONFIRMED, with one correction to the costing. matrix's seam is as costed — CONFIRMED."
fence-exposure: "zero — 0 Rust files changed, 0 bytes in crates/wcore-cli/src/{lib,main}.rs, gateway.rs, ci.yml or BACKLOG.md vs the merge-base SHA d34b2fe1"
instrument-defects-found-in-own-harness: 4
---

# 24-MATRIX-SIGNAL — two adapters driven, one new leg, one new HIGH, four instrument defects of my own

**Verdict up front: `24-C3` is NOT MET and I do not claim it.** Six lanes have now
declined to mark it. This one drives the two adapters that needed no Rust, adds the leg
that a startup-only matrix structurally cannot cover, and answers the matrix inbound
restart question with a measured **yes** — it has an equivalent to the outbound defect,
and it is worse in one specific way.

Nothing was merged, no PR opened, nothing tagged, no issue closed, no credential read or
embedded. **Zero Rust files changed.**

---

## 1. The three questions this lane existed to answer

### 1a. Was signal's subprocess-path seam as cheap as costed? **YES — confirmed, and it is the cheapest in the phase. One correction to the costing.**

`24-C3-FINISH.md` §4b costed signal at zero Rust via a **subprocess-path** seam rather
than a base-URL one, and flagged that anyone grepping the never-driven adapters for
`*_base_url` would find nothing in signal and wrongly conclude it had no seam. **That
determination is correct.** Verified from the shipped construction path myself rather
than inherited:

| step | source | what it establishes |
|---|---|---|
| `SignalConfig.signal_cli_path: PathBuf` | `config.rs:16-18` | the seam is a config field |
| `SignalChannel::new` → `Arc::new(RealLauncher)` | `lib.rs:82-83` | the SHIPPED constructor hardwires the real launcher |
| `Command::new(cli_path).arg("-a").arg(account).arg("jsonRpc")` | `subprocess.rs:54-62` | the path is exec'd directly |
| `make_signal` → `SignalChannel::new` | `registry:157-169` | the registry takes that path |

The fixture is **an executable on a path**. No HTTP, no TLS, no port to bind, no
certificate to mint, no `openssl` dependency, no platform trust-store problem. Compare
email, whose five arrival legs remain NOT MEASURED on every platform because SMTP resolves
to compiled-in `webpki-roots` that reads no file and no env var. **Signal cost less than
any other adapter in this phase**, and the fixture is 200 lines.

**The correction.** 24-C3-FINISH says of matrix: *"there is no production default to
preserve, so there is no control test to write"* — true of matrix, and by omission it
reads as though signal is the same. **It is not.** `signal_cli_path` carries
`#[serde(default = "default_signal_cli_path")]` resolving to a bare `signal-cli` on
`$PATH`. So signal **does** have a production default that a fixture run could mask, and
a control assertion is warranted. It is asserted in the self-test (`P1`) rather than in
the run, so this lane's green cannot be read as evidence that a config naming no path
still works.

Note also what was deliberately **not** used: `SignalChannel::with_launcher` is a real
seam and is `#[doc(hidden)]`. Driving through it would have proven nothing an operator can
reproduce — the exact distinction that left Discord's `with_token_url` unusable. Every
figure below goes through `signal_cli_path`.

### 1b. Does matrix inbound have an equivalent of the outbound restart defect? **YES. Measured, triple-controlled, reproduced 3/3, and it is a new HIGH.** See §3.

### 1c. What did my own instrument defects nearly cost? **Four of them, three failing in the direction that blames the product, and one that would have been a fabricated finding had I reported it.** See §5.

---

## 2. Per-adapter, per-leg results, with counts

All live figures: `hetzner-dsm`, Linux, `/root/wayland-24-matrix-signal`,
`wayland-core 0.12.25 (source aa4351aa...)`, sha256
`9265efbf574dcb3fcb55847f2078c4634518b4b677b9ba3f679eba918a321df6`, runtime
`--json-stream`.

### 2a. The three runs

| run | driver commit | verdict | legs | failed | not_measured | matrix arrivals | signal arrivals | instrument_fault | restart verdict |
|---|---|---|---|---|---|---|---|---|---|
| 1 | `aa4351aa` | RED rc=1 | 36/42 | **0** | 6 (email) | 8 | 6 | **false** | **LOSS** |
| 2 | `c5cb6b45` | RED rc=1 | 36/42 | **0** | 6 (email) | 8 | 6 | **false** | **LOSS** |
| 3 | `7343a935` | RED rc=1 | 36/42 | **0** | 6 (email) | 8 | 6 | **false** | **LOSS** |

`failed=0` across all three: **every leg that ran, passed.** The runs are RED because
email's six legs are NOT MEASURED (unchanged, pre-existing SMTP/webpki-roots blocker) and,
from run 2 onward, because the restart probe reports LOSS — see §5c, where making the
verdict capable of that was itself a repair.

Byte counts, because an empty journal and an absent journal both read as "0 arrivals" if
only parsed records are counted (run 3):
`arrivals=4860 turns=13387 telegram=146700 mail=240853 matrix=80874 signal=3164`.

### 2b. Matrix — 6/6 PASS, all three runs

Room-keyed, like Slack. Inbound is an HTTP long-poll on `/sync` — the **third** polling
transport in this matrix and the first **non-destructive** one (a `/sync` does not consume
what it reads, unlike `getUpdates` and IMAP `FETCH`).

| leg | run 3 result | count |
|---|---|---|
| admit | **PASS** | arrivals=1 want=1, turns=1 |
| route | **PASS** | `carries_correlation=true`, `conversation_id="!f24room1:f24.invalid"` = want |
| dedupe | **PASS** | replay at +1057ms (inside the 60000ms TTL); arrivals 1→1, turns 1→1; positive control fresh-id arrivals=1 |
| access | **PASS** | denied-sender arrivals=0 turns=0, **CONTROL admit-leg-arrived=1 (held)** |
| bind | **PASS** | conv1=`!f24room1` conv2=`!f24room2` distinct=true |
| **steady** | **PASS** | after 30000ms quiet: **3/3**, per-message `[1,1,1]` |

### 2c. Signal — 6/6 PASS, all three runs

Peer-keyed, like whatsapp/sms/telegram. Inbound is JSON-RPC over the stdio of a
subprocess the binary spawns and owns.

| leg | run 3 result | count |
|---|---|---|
| admit | **PASS** | arrivals=1 want=1, turns=1; fixture answered `{"ok":true,"timestamp":1700000001000,"pid":...}` |
| route | **PASS** | `carries_correlation=true`, `conversation_id="+15552240001"` = want |
| dedupe | **PASS** | replay at +1026ms (inside TTL); arrivals 1→1, turns 1→1; positive control arrivals=1 |
| access | **PASS** | denied-sender arrivals=0 turns=0, **CONTROL admit-leg-arrived=1 (held)** |
| bind | **PASS** | conv1=`+15552240001` conv2=`+15552240002` distinct=true |
| **steady** | **PASS** | after 30000ms quiet: **3/3**, per-message `[1,1,1]` |

**The universal-denial trap is closed structurally, not by inspection.** The `access` leg's
pass condition *includes* `accessControlHeld = seen1.length === 1` — a leg whose assertion
holds while its control is zero grades FAIL. That was already in the shared driver; I
verified it by reading (`f24-inbound.mjs:1233-1246`) rather than assuming, and the new
`steady` leg is inherently self-controlling because it demands arrivals > 0. A path that
denied everything would score `[0,0,0]` and FAIL. Asserted in the self-test as `T3`.

### 2d. The new `steady` leg — added to EVERY adapter, and every adapter passed it

`LEGS` went from 5 to 6 and applies to all 7 adapters (42 expected legs, all accounted).

The five original legs all fire in a continuous burst within the first seconds of a
channel's life. **F24-C3-H4 was raised from MEDIUM to HIGH precisely because a steady-state
run lost 5 of 6 messages while the startup burst looked perfect.** A poller that dies,
desynchronises its cursor, or loses its subscriber *after* its first successful exchange
passes all five and then drops everything.

The leg goes genuinely quiet for 30s — longer than any adapter's poll interval, so the
channel idles rather than pausing mid-cycle — then delivers 3 messages 4s apart, each with
its own correlation token so a single swallowed message shows as a count, not a boolean.

| adapter | steady, run 3 |
|---|---|
| slack | **PASS 3/3** `[1,1,1]` |
| whatsapp | **PASS 3/3** `[1,1,1]` |
| sms | **PASS 3/3** `[1,1,1]` |
| telegram | **PASS 3/3** `[1,1,1]` |
| matrix | **PASS 3/3** `[1,1,1]` |
| signal | **PASS 3/3** `[1,1,1]` |
| email | NOT MEASURED (arrival legs blocked at SMTP) |

**This is a real, if negative, result and it is worth stating plainly: there is no
steady-state inbound loss on `--json-stream` at this commit, on any of the six measurable
adapters.** F24-C3-H4's class does not reproduce here. That does not clear it on
`gateway run`, which I did not measure (§7).

`telegram fixture: submitted=8 still_pending=[] polls=559 max_concurrent_getupdates=1` —
`1` is correct; `2` would mean two managers competing for one token; `0` would mean
nothing polled at all, so a "fix" that works by starting nothing could not pass.

### 2e. What this changes for the criterion's eight clauses

| clause | before this lane | after | note |
|---|---|---|---|
| setup/auth | PROVEN ×5 | **PROVEN ×7** | breadth, not a new clause |
| access | PROVEN ×5 | **PROVEN ×7** | breadth |
| routing | PROVEN ×5 | **PROVEN ×7** | breadth |
| idempotency | PROVEN ×5 | **PROVEN ×7** | breadth |
| health | PROVEN (Linux, prior lane) | unchanged | not re-measured |
| reconnect/reload | PARTIAL (F24-C3-H5) | **PARTIAL, and now worse** | new HIGH on the restart path (§3); the *upstream-drop* half is still untouched |
| **media** | **UNTOUCHED** | **UNTOUCHED** | zero adapters |
| **native actions** | **UNTOUCHED** | **UNTOUCHED** | zero adapters |

**Adding two adapters to clauses five already prove is worth less than proving a clause
zero adapters prove.** I did this anyway because it was the lane's assignment and because
it was the cheapest way to reach the restart question — but it should not be mistaken for
closing distance on the criterion. `media` and `native actions` are exactly where they
were, and they are two of the eight.

---

## 3. NEW HIGH — F24-C3-H6: matrix loses every message delivered while the process is down

**One sentence: an operator restarts the agent — a deploy, a crash, a reboot — and every
Matrix message that arrived during the downtime is silently and permanently dropped, with
no error, no retry, no log an operator would look at, and a channel that reports healthy.**

### 3a. The mechanism, in source

- `sync.rs:190` — `let mut since: Option<String> = None;` The `/sync` cursor is a
  **process-local variable inside `sync_loop`**. It is never written anywhere.
- `sync.rs:212-226` — `let is_initial = since.is_none();` and timeline events are emitted
  **only when `!is_initial`**. The initial sync is consumed for its cursor and its timeline
  is discarded. This is the documented "initial-sync replay guard" (`sync.rs:8-12`), and
  its stated purpose — not replaying the whole room backlog at boot — is legitimate.

Composed: **a restart resets `since` to `None`, so the first `/sync` after a restart is an
initial sync, so its entire timeline is discarded** — including everything the homeserver
accumulated while the process was down.

### 3b. It is NOT an unavoidable tradeoff, and the proof is in this repo

The obvious defence is that you must choose between replaying the backlog and losing the
gap. **The sibling polling adapter in this same workspace refuses that choice.**
`crates/wcore-channel-email/src/imap.rs:120`, verbatim:

> `// Resume the UID watermark from disk so a restart neither replays the`

Email persists a watermark keyed by (host, user, mailbox) precisely so a restart does
neither. **Matrix implements the replay-guard half and omits the resume half.** The
project's own standard for this exact problem, in a sibling crate, is the third option.

Absence proven per §3b-i, with the instrument shown alive on a known-positive in the same
shape, and the query stated so it can be re-run:

```
/usr/bin/grep -rniE 'persist|watermark|checkpoint|state_dir|cursor|resume|fs::write|fs::read' \
  crates/wcore-channel-matrix/src/   ->  5 hits, ALL comments or CredentialsStore. Zero persistence.
  crates/wcore-channel-email/src/    -> 24 hits, real persistence.
KNOWN-POSITIVE (same tree, same tool): 'next_batch' in crates/wcore-channel-matrix/ -> 16 hits.
```

Asserted as self-test `P3` (matrix still holds the cursor process-local) with `P4` as its
known-positive control. If someone repairs `sync.rs`, `P3` reddens and names itself —
so a future green cannot be misread as "the defect was never there".

### 3c. The measurement, and the three controls that make it a product finding

Reproduced **3 times out of 3**, on three different driver commits. Run 3 (`7343a935`,
the repaired instrument), from the probe's own output:

| # | leg | result | what it excludes |
|---|---|---|---|
| 1 | `pre-restart-live-control` | **PASS** arrivals=1 | the channel never worked at all |
| 2 | `binary-down-and-quiet-during-the-gap` | **PASS** `stopped=true`; fixture `sync_total` 281 → 281 → 281 | the dying process consumed the gap message |
| 3 | `restarted-binary-resyncs` | **PASS** `initial_sync_total` 1 → 2 | the process never came back |
| 4 | **`gap-event-was-in-the-initial-sync-timeline`** | **PASS** — fixture's own report lists `$f240d705391gap` in the post-restart initial sync's `served` | **H2: the fixture never served it** |
| 5 | `post-restart-live-control` | **PASS** arrivals=1 | the process came back broken |
| 6 | **`gap-message-survives-the-restart`** | **FAIL — verdict=LOSS, arrivals=0** | — |

**Leg 4 is the H2 exclusion and it is the reason this is a finding rather than a
fabrication.** A lane on this program once traced a dedupe FAIL to its own 90s replay
against a 60s TTL; reporting it would have been a fabricated HIGH against working code. So
"the fixture actually served the gap event" is not assumed, not inspected, and not asserted
by me — it is read from the **fixture's own report, in another OS process**, which records
exactly which event ids each initial sync's timeline carried. The grader is explicitly
three-state: **a run where leg 4 fails grades INCOMPLETE, never LOSS.**

### 3d. The mechanism closed end-to-end, from the independent journal

The strongest single datum, extracted from run 3's fixture journal with `/usr/bin/grep`
and a known-positive control in the same invocation:

```
KNOWN-POSITIVE — the post-restart event was served on an incremental sync:  1
the GAP event:  initial_syncs_serving_gap=1   incremental_syncs_serving_gap=0
```

The gap event was served **exactly once, on the initial sync the product discards, and on
no incremental sync ever**. The post-restart control event was served on an incremental
sync and arrived. That is the mechanism, demonstrated rather than argued: `since` is set
from the initial sync's `next_batch`, whose cursor is already past the gap event, so the
gap event can never be served again for the life of the process.

This also excludes the last surviving hypothesis — "it arrives late" — without needing a
longer wait: there is no later sync in which it could arrive.

### 3e. Comparison with the outbound defect, and with H5

The lane brief named the outbound shape: a transaction id reused after a restart made the
homeserver return **HTTP 200 with the OLD event id**, so a genuinely new message vanished
while reporting success.

**The inbound side has the same root shape — state that must survive a restart does not —
and differs in one way that makes it harder to notice.** The outbound defect at least
produced a wire response an instrumented client could compare against. This one produces
*nothing at all*: no request, no response, no error, no log line, and a `channel health`
that reports the channel healthy because the channel *is* healthy. It is only visible by
comparing what the homeserver holds against what the agent acted on.

### 3f. Severity, argued rather than asserted

**HIGH.**

- The loss is **silent** and **permanent for the affected messages**: no error, no retry,
  nothing an operator would see. The data still exists server-side; the adapter simply
  never reads it.
- The trigger is a **routine operation**, not an edge case. Every deploy, every crash,
  every reboot.
- It is the same class F24-C3-H4 and F24-C3-H5 each earned **HIGH** for: silent inbound
  message loss on a documented operator workflow, on the runtime an operator installs.
- Bounded, honestly: only the downtime window is lost, where H5 disables a channel
  permanently. That makes it no worse than H5 — it makes it **recur on every restart**
  instead of once.
- It fails in the losing direction, not the leaking direction. It is **not** a security
  hole.

### 3g. It is NOT fixed, and that is a deliberate call

Severity policy says a HIGH must be fixed or disproved. I proved it real and did not fix
it. Stated plainly rather than dressed up.

The repair is to persist the `/sync` cursor across restarts, mirroring `imap.rs`. That is
a **Rust change to product state-persistence semantics**, and it carries decisions this
lane is not the right place to make: where the cursor lives, what it is keyed by
(homeserver × user_id × channel name), what happens on credential rotation or homeserver
change, and — the dangerous one — **what the first-run seed does**. A partial fix that
persists the cursor but mishandles the seed would **pass a naive re-run of my own probe
while replaying an entire room's backlog into the agent on first start**, which is a worse
defect than the one being fixed. "The repair was PARTIAL, and the live run caught it" is a
recorded lesson from this very criterion, and 24-C3-FINISH declined to blind-fix H5 at the
end of a lane for the same reason; H4 was then fixed properly with a full mutation proof.

There is one respect in which this is **cheaper than H5**: it has a reference
implementation in-tree. `imap.rs:120-320` already solves seeding, watermark advance on
parse failure, and the never-replay-never-lose contract. The next lane is not designing
from scratch.

What the next lane inherits is not a hunch: the mechanism proven from an independent
journal, three controls, three reproductions, the exact call sites, the reference
implementation, and a driver that already reddens on it. **Estimated ~1 session**,
including the mutation proof and a first-run seeding test.

---

## 4. What the new `steady` leg and the restart probe cost the criterion's leg accounting

`LEGS` is now 6, `ADAPTERS` is 7, expected = 42, `accounted=42/42` on every run. The
restart probe is recorded **outside** `results` — deliberately, exactly as
`email_admission_probe` is — because the six legs are uniform across adapters so the
columns compare, and this question exists only for matrix. Folding it in would break leg
reconciliation and make one adapter's row mean something different from the others'.

That decision is what created instrument defect §5c, and it is worth noting that the fix
was to make the *verdict* account for the probe, not to collapse the probe into the legs.

---

## 5. FOUR instrument defects, all mine, all repaired in this lane

§6b-ii: a documented instrument defect is a defect you have agreed to keep, and the one
measured recurrence in this program happened because an earlier sighting was written up
instead of repaired. Each repair below carries the mandatory **three** assertions —
known-positive passes, known-negative fails, **and the old broken instrument would have
missed it**.

**Three of the four failed in the direction that blames the product.** That is not
coincidence; it is what an under-detecting instrument does by default, and it is why each
had to be caught by a control or an assertion rather than by a red.

**This is the fifth independent sighting of this class in this program tonight, and the
lane brief already numbered it the eleventh overall.** The class is not rare and it is not
getting rarer.

### 5a. The fixture put the room map one level too high — would have reported ZERO matrix arrivals as a product defect

`f24-matrix-fixture.mjs` sent `{ rooms: { "!room": ... } }`. `sync.rs:61-65` deserialises
`Rooms { #[serde(default)] join }`, so the missing `join` key would have **defaulted to an
empty map**, `parse_sync_events` would have iterated nothing, and **every matrix leg would
have reported zero arrivals** — a fabricated product defect caused entirely by one line of
my fixture. Caught by self-test `M2` **before any live run**.

Repaired to `{ rooms: { join: ... } }`. Assertions `M2`/`M3` require the summary and the
room map on both initial and incremental syncs; `M4` is the known-negative (an incremental
sync at the head must return **nothing**, so an arrival count can never be a tautology).

### 5b. The self-test read the fixture's stdout with a handler while blocking the event loop

`child.stdout.on('data')` plus an `Atomics.wait` sleep: the handler never ran, so the
self-test reported "no frame appeared on stdout" **while the fixture was emitting
correctly**. Caught by `S1`/`S3`/`S4` failing.

Repaired **structurally**, not with a bigger timeout: every stdio interaction happens in
one top-level-`await` block that yields to the loop, and the observations are frozen into
plain data so the synchronous `test()` calls keep the hard-fail-on-thenable guard. `S4`
additionally now asserts a minimum frame count, because in its broken form it passed
vacuously on zero lines.

### 5c. The run verdict could not fail on a restart LOSS — a self-passing gate

**Found by running, not by reading.** Run 1 graded the restart probe **LOSS** — a genuine
product finding — while `failed.length` stayed **0**, because the probe is recorded outside
`results`. The run exited RED anyway, **for an unrelated reason**: email's six legs were
NOT MEASURED.

So the gate *looked* correct while being **incapable of failing on the thing it had just
found**. The moment email becomes measurable, a proven silent inbound loss across a restart
would have exited **0 GREEN**. That is squarely §3.2's class.

Repaired: the verdict now folds in both probes. Self-test `V2` is the known-negative and
**`V3` is the third assertion — the old verdict expression, kept executable, calls the very
same observation GREEN.** Run 2 and run 3 report `probe_failed=true restart_verdict=LOSS`
in the banner.

### 5d. Liveness could not distinguish a zombie — this one would have been a FABRICATED FINDING

The restart probe's central claim is *"the binary was down when the gap message was
delivered."* It checked that with `process.kill(pid, 0)`, which is **wrong under this
driver**: node reaps children on the event loop, this driver's waits are blocking, so a
child that died instantly stays a **zombie** — for which `kill(pid, 0)` succeeds.

Measured directly rather than reasoned:

```
pid=99377 process.kill(pid,0)_says_alive=true exitCode=null ps_state="Z"
```

**What it nearly cost.** Run 1's probe reported `exit_secs=30 (SIGKILL)`, which reads as
**"`--json-stream` ignored SIGTERM for 30 seconds"** — a product claim about shutdown
behaviour, on a surface nobody in this phase has audited. I was one paragraph from writing
it up. It was very probably this bug. **It is not reported as a finding, and §6 records
what is and is not established about it.**

Repaired: `pidIsLive()` reads the OS's own process state (`/proc/<pid>/stat` on Linux,
`ps -o stat=` elsewhere) and treats `Z` as dead. `Z1` known-positive, `Z2` known-negative
(with a guard that the scenario really produced a zombie, so the assertion cannot be
vacuous), and **`Z3` is the third assertion — `process.kill(pid,0)` still reports that same
zombie ALIVE, so the repair demonstrably changes an outcome.**

### 5e. The self-test is proven able to fail — by history, not by mutation

**Its first execution was 26 passed / 7 failed, and the 7 were two genuine defects**
(§5a, §5b). Final state **41 passed / 0 failed**, on macOS and on Linux at the same commit
(`7343a935`), byte-identical driver on both hosts (`md5 d965fa9334a8414a81981ecd71801591`).

`instrument_fault = false` on all three graded runs. **The HIGH in §3 is not one of my
faults. I had four, I know exactly what they look like, and this is not one.**

---

## 6. Observation NOT established — `--json-stream` and SIGTERM

Run 3, with the zombie-aware check, still reported `exit_secs=30 (SIGKILL)`: the process
was reported genuinely alive (state not `Z`) for 30s after SIGTERM.

**I am not grading this and I am not calling it a defect**, for two reasons:

1. The instrument that produced it is the one I had *just* found to be wrong about
   liveness once (§5d). One repair does not earn back the benefit of the doubt.
2. There is a plausible correct-behaviour explanation I did not exclude: the driver holds
   the child's **stdin open as a pipe it never writes to and never closes**, and for a
   stdio protocol surface, staying alive until stdin closes is arguably correct. The
   product uses that idiom itself — `lib.rs:196` closes signal-cli's stdin to ask it to
   exit.

My attempt to reproduce it standalone **failed to start the binary correctly** and produced
nothing usable, so I have no clean measurement. **Recorded as unmeasured rather than
reconstructed.** The experiment a follow-up should run is one line: close stdin, then
measure time-to-exit, and compare against SIGTERM-with-stdin-held.

---

## 7. What I did NOT do

- **Did not mark `24-C3` MET.**
- **Did not fix F24-C3-H6.** Reasoned in §3g, not an oversight.
- **Did not fix F24-C3-H5** (the prior lane's unfixed HIGH). Not my lane's assignment.
- **Did not measure `media` or `native actions`** — two of the eight clauses, still zero
  adapters, exactly where the prior lane left them.
- **Did not measure the upstream-drop half of `reconnect`.** My probe restarts the
  *process*; dropping the *connection* under a running process is a different question and
  is still untouched.
- **Did not measure `health`.** Unchanged from the prior lane's Linux result.
- **Did not run on `gateway run`.** Every figure here is `--json-stream`. This matters:
  F24-C3-H2 and H5 are both `gateway run` findings, and my steady-state green does **not**
  clear that surface. A `gateway run` pass of this same driver is ~10 minutes and is the
  single cheapest next measurement available.
- **Did not measure anything on macOS or Windows.** Every figure in this criterion, from
  every lane, remains Linux.
- **Did not drive msteams or imessage.** Costed by the prior lane at ~2 sessions and
  platform-blocked respectively; unchanged.
- **Did not establish the SIGTERM observation** (§6).
- **Did not change a single Rust file.** `git diff d34b2fe1 --name-only -- '*.rs'` → **0**.
- **Did not touch the fence.** `git diff d34b2fe1` over
  `crates/wcore-cli/src/{lib,main,gateway}.rs`, `.github/workflows/ci.yml` and
  `.planning/BACKLOG.md` → **empty**, against the captured merge-base **SHA** `d34b2fe1`,
  never the branch name.
- **Did not modify any shared fixture** (`f24-sink.mjs`, `f24-llm-fixture.mjs`,
  `f24-correlate.mjs`, `f24-tg-fixture.mjs`, `f24-mail-fixture.mjs`) — other drivers depend
  on them; I conformed to their contracts. The only shared file changed is
  `f24-inbound.mjs`, additively.
- **Did not use, read or require any vendor credential.** Every secret in every run was
  minted at run time and died with it. No credential reached hetzner.
- **Did not run a full-workspace build or test.** `cargo build --release -p wcore-cli`
  only, per the disk/contention rule. Disk checked first: 714G free.
- **Did not run `wcore-contract generate`**, merge, open a PR, tag, or close an issue.

---

## 8. Exact remaining distance to `24-C3` MET

Updating the prior lane's table with what this one changed:

| # | gap | status after this lane |
|---|---|---|
| 1 | **`media`** | **UNCHANGED — zero adapters.** The `ChannelMediaEnricher` is already wired at `channel_inbound_host.rs:184-195` |
| 2 | **`native actions`** | **UNCHANGED — zero adapters** |
| 3 | **`reconnect/reload`** | reload half PARTIAL (H5, unfixed); **restart path now has H6, unfixed**; upstream-drop half still untouched |
| 4 | **F24-C3-H5** | **still unfixed** |
| 5 | **F24-C3-H6** | **NEW, proven 3/3, unfixed** (~1 session, reference impl in-tree) |
| 6 | email `route`/`bind` | unchanged — reachable, costed, not built |
| 7 | **matrix, signal** | **DONE — 6/6 each, ×3 runs** |
| 8 | msteams | unchanged (~2 sessions) |
| 9 | imessage | unchanged (platform-blocked) |
| 10 | **`gateway run` for the new leg + 2 adapters** | **NEW GAP created by this lane** — ~10 min, cheapest next measurement |
| 11 | macOS + Windows | unchanged — every figure is Linux |

**Honest total: still roughly 6–8 lane-sessions**, and that is before the two other
platforms. This lane closed item 7 and opened item 10; it did not move the needle on the
two clauses that have zero evidence, and it added an unfixed HIGH.

`24-C3` is a **release blocker and it is still open.** Two of its eight clauses have zero
evidence on any adapter, a third is PARTIAL with two unfixed HIGHs against it, and every
number in the criterion is single-platform.

**Marking it MET would be wrong.**

---

## 9. For the orchestrator to serialize

**Nothing.** Zero Rust files, zero fence bytes, no protocol seam, no contract fixture, no
dependency change, no `Cargo.lock` edit.

Three new files, all additive and all mine — `scripts/f24-matrix-fixture.mjs`,
`scripts/f24-signal-fixture.mjs`, `scripts/f24-matrix-signal-selftest.mjs` — plus additive
changes to `scripts/f24-inbound.mjs` and evidence under
`24-MATRIX-SIGNAL-evidence/`.

**One coordination note.** `f24-inbound.mjs` is shared with four other drivers, and this
lane changed `LEGS` (5 → 6) and `ADAPTERS` (5 → 7). Any lane holding a hard-coded expected
leg count of 25 will need to read 42. The driver computes it (`ADAPTERS.length * LEGS.length`)
and reconciles `accounted`, so nothing silently under-counts.

`f24-inbound.mjs` exit codes are unchanged in meaning: **0 GREEN, 1 RED, 2 USAGE, 3
INCOMPLETE (instrument fault)** — but note that RED now includes a restart-probe LOSS,
which it did not before.

---

## 10. Evidence

`.planning/phases/24-gateway-automation-channels-typed-api/24-MATRIX-SIGNAL-evidence/`

| path | what it holds | bytes |
|---|---|---|
| `24-MATRIX-SIGNAL-NOTES.md` | the running log, committed at T+0 before any measurement and re-committed after each | — |
| `run1-json-stream/` | `result.json`, `run.log`, `matrix-fixture.jsonl`, `signal-fixture.jsonl`, `core-restarted.log` | 495,763 |
| `run2-json-stream/` | same, at the repaired verdict | 473,893 |
| `run3-json-stream/` | same, at the repaired liveness check, **plus both incarnations' `core.log`** | 506,344 |

The restart probe's H2 exclusion is reproducible from `matrix-fixture.jsonl` alone:
`initial_syncs[].served` lists every event id each initial sync carried.

Binary: `wayland-core 0.12.25 (source aa4351aaafeb9b9e67b67c20190919e434060d9d)`,
sha256 `9265efbf574dcb3fcb55847f2078c4634518b4b677b9ba3f679eba918a321df6`.
