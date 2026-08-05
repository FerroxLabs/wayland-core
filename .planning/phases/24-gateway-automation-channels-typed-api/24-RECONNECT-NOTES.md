# 24-RECONNECT — running notes

Lane `24-reconnect`. Branch `lane/24-reconnect`. Merge-base captured ONCE, as a SHA:

```
BASE=15cda12d6a189d7cad3daf0998eded4710f809af
```

(`/usr/bin/git merge-base HEAD plan/f20-unified-audit-repair` at T+0; `/usr/bin/git`, unproxied.)

Committed at T+13 min before any measurement, per LANE-BRIEF §6b-i. Appended and re-committed
after every measurement.

---

## T+0 — assignment as I read it

`24-C3`'s `reconnect/reload` clause has two halves.

- **reload** — a new adapter added to config and picked up by `channel reload`. Driven by
  `24-C3-FINISH` (found `F24-C3-H5`: reload registered the adapter, reported healthy, denied
  every message because the inbound access policy was never reloaded), fixed and live-proven by
  `24-H5`.
- **reconnect** — the *remote* side goes away under a running process. **Never driven.** Every
  prior lane says so explicitly:
  - `24-C3-FINISH` §7: "Did not measure the `reconnect` half of reconnect/reload, only `reload`."
  - `24-H5-SUMMARY` §8: "Did not measure the `reconnect` half ... Only `reload`."
  - `24-H6-SUMMARY` §6: "Did not measure the upstream-drop half of `reconnect`. My probe
    restarts the *process*; dropping the *connection* under a running process is untouched."

`24-H6` restarted the **process**. That is a different event from the **connection** dropping
under a process that stays up: the process-restart path is now cursor-persisted on disk, whereas
an in-process reconnect can recover from in-memory state and never touch the file. So H6's fix
does NOT imply this half.

**I am NOT grading `24-C3`.** Seven lanes have declined it correctly and nothing I can do in one
lane moves `media` or `native actions`, which have zero evidence on every adapter. I grade the
`reconnect` half only, per adapter, and name the adapters I did not reach.

## T+0 — the defect shape I am hunting

Five silent inbound message-loss defects in two days, all invisible from source review, all only
visible when the real product was driven. The most recent (`F24-C3-H6`, matrix) was exactly this
shape: **everything delivered while the process was down was lost**, because the sync cursor was
process-local. Assume a sixth exists until measured otherwise.

For an upstream disconnect the loss window is the same but the mechanism differs. The question is
not "does it reconnect" — a health probe answers that and answers it green for a broken adapter.
The question is **"is every message delivered around the disconnect window accounted for, exactly
once."** So the measurement is a census: N delivered before + M during + K after, and I derive
losses and duplicates myself from the sink journal. **I do not trust any figure the product
reports about itself.**

## T+0 — adapter taxonomy (to be verified against source, not assumed)

Upstream-drop is only a meaningful event for adapters that *hold* or *initiate* an upstream
connection. Adapters we merely receive webhooks on have no upstream to drop from our side.

| adapter | expected upstream shape | drop is expressible? |
|---|---|---|
| discord | WebSocket gateway, `op6 RESUME` w/ replay already in fixture | **yes — the sharpest target** |
| matrix | long-poll `/sync`, cursor now persisted (H6) | **yes** |
| telegram | long-poll `getUpdates`, `offset_store` on disk | **yes** |
| email/imap | IMAP connection + `uid_store` on disk | yes, but email has a pre-existing SMTP/webpki blocker on every prior run |
| slack / whatsapp / sms | inbound webhook — *they* connect to *us* | probably not; verify |
| signal | verify | ? |
| msteams / imessage | undriven / platform-blocked | no |

TO VERIFY IN SOURCE before building anything.

## T+0 — rules of evidence I am binding myself to

1. **Every absence claim needs a known-positive in the same invocation and a decoy proving the
   detector fires.** "No messages were lost" is self-passing on a dead instrument. Broken grep,
   typo'd path, unquoted glob, wrong tree — every one of those returns a zero for free.
2. Every claim gets a **one-variable negative control proven to redden**.
3. Load-bearing numbers come from `/usr/bin/grep`, `/usr/bin/git`, `/usr/bin/env cargo`.
   `rtk` rewrites `git log` (drops merges), `grep` (reported 9 matches in 7 files for a one-file
   search whose true answer was 0), `cargo` (strips `0 ignored` / `0 filtered out`), and `wc -c`
   (returned 0 for a 72-byte file — cross-check byte counts two ways).
4. Read back `N passed` counts; never trust exit status. A suite exits 0 having run zero tests.
5. Compile only on `hetzner-dsm`. Never on the Mac.

## T+0 — concurrency hazard, inherited and NOT yet verified fixed

`24-H6` §8.0 reported that `f24-inbound.mjs` binds a **fixed** `127.0.0.1:18787` and its launcher
`pkill`s three **global** patterns, so two lanes destroy each other's runs. My dispatch says a
prior lane repaired this — `F24_WEBHOOK_PORT` is now honoured and the pkill patterns are scoped.
**I will verify that in the source before running anything**, because taking it on trust is how
I'd both lose my own run and damage four other lanes'.

**I will pick a port no other lane would pick.** Four other lanes are live.

## T+0 — plan

1. Verify the harness concurrency repair actually landed (read the source, do not trust).
2. Read each candidate adapter's connection loop in source; establish where an upstream drop is
   even expressible and what state (if any) survives it.
3. Build a probe that induces a genuine upstream disconnect against the hermetic fixture, with a
   before/during/after message census and self-derived loss+duplicate counts.
4. Prove the probe can fail: decoy message it must miss, known-positive it must find, and a
   one-variable product mutation that reddens it.
5. Drive `gateway run` (the real installed surface), not `--json-stream` only.
6. Grade per adapter. Name what I did not reach.

---

## Log

- **T+0** worktree verified `/Users/seandonahoe/dev/waylandcore-frontier-worktrees/lane-24-reconnect`,
  branch `lane/24-reconnect`, HEAD == BASE == `15cda12d`. Fence exposure at this point: zero by
  construction (no commits yet).
- **T+13** this file committed before any measurement.

### T+25 — harness concurrency repair VERIFIED landed (not trusted)

Read the source rather than taking the dispatch's word:

- `scripts/f24-inbound.mjs:176` — `const WEBHOOK_PORT = Number(process.env.F24_WEBHOOK_PORT ?? 18787);`
  Default unchanged, overridable. **Repair is real.**
- `scripts/f24-inbound-run.sh` — the three `pkill`s are now scoped to `${BINARY}` and
  `${RUN_DIR}` rather than the global `wayland-core --json-stream` / `f24-sink.mjs` /
  `f24-llm-fixture.mjs` patterns. **Repair is real.**

Separately, and better for me: **`f24-discord-fixture.mjs` binds an ephemeral port**
(`this.server.listen(0, '127.0.0.1', …)`, line ~198) and reports it back. So the discord driver
has no fixed-port collision surface at all and is inherently concurrency-safe. That removes the
strongest reason to avoid running while four other lanes are live.

### T+30 — adapter taxonomy, verified in source

`scripts/f24-inbound.mjs:102` `TRANSPORT`:

| adapter | transport | is an upstream drop expressible? |
|---|---|---|
| slack, whatsapp, sms | `webhook` | **NO** — the remote dials *us*. There is no upstream connection for our side to lose. Out of scope by construction, and I will say so rather than score them. |
| telegram, email, matrix | `poll` | yes — the fixture can refuse/close mid-poll |
| signal | `subprocess` | yes, but the "upstream" is a spawned child, not a network peer |
| **discord** | WebSocket gateway (separate driver, `f24-discord-inbound.mjs`) | **YES — and it is the only adapter with a real session-resume protocol** |

`f24-discord-inbound.mjs:284` spawns `this.args.binary, ['gateway', 'run']`. **The discord driver
already drives the real installed surface**, which is what my dispatch demands. It is the target.

Discord's product-side reconnect surface (`crates/wcore-channel-discord/src/gateway.rs`) is fully
built: `OP_RESUME=6`, `OP_RECONNECT=7`, `ResumeState{session_id, resume_gateway_url, seq}`,
`decide_handshake()`, and an outer `gateway_loop` that carries `resume` across a dropped socket
by `&mut` so it survives an `Err` return. That is exactly the machinery that has never been
driven.

### T+45 — FIRST RESULT, and it is against my own instrument, not the product

**`f24-discord-fixture.mjs` CANNOT express a gap replay.** A message dispatched while no client
is connected is allocated a sequence number that COLLIDES with an already-delivered one, so
`RESUME` never replays it. Measured, not reasoned:

```
$ node .planning/.../24-RECONNECT-evidence/fixture-seq-repro.mjs
{"fixture_seq_table":[{"id":"PRE-1","s":2},{"id":"PRE-2","s":3},
                      {"id":"GAP-1","s":3},{"id":"POST-1","s":4}],
 "client1_last_seq":3, "client1_saw":["PRE-1","PRE-2"],
 "live_conns_after_drop":0, "gap_dispatch_reached_sockets":0,
 "client2_resumed":true, "client2_saw":["POST-1"],
 "duplicate_seq_numbers_in_fixture_table":1,
 "KNOWN_POSITIVE_pre_messages_seen":true,
 "KNOWN_POSITIVE_post_resume_message_seen":true,
 "GAP_REPLAYED_ON_RESUME":false}                                    rc=1
```

**Mechanism**, `f24-discord-fixture.mjs:486`:

```js
this.dispatched.push({ id, s: s || this.dispatched.length + 1, payload, sockets: targets.length, ... });
```

With zero identified connections `targets` is empty, so the `for` loop never runs, `s` stays `0`,
and the fallback `this.dispatched.length + 1` is used. That fallback **forgets that READY
consumed sequence 1**: at the gap, `dispatched.length` is 2, so `GAP-1` is numbered `3` — the
number `PRE-2` already holds. The client resumes from `seq=3`, the replay filter is
`x.s > after`, and the gap message is filtered out **by its own sequence number**.

**Why this matters more than the bug itself:** a reconnect probe built on this fixture would
report `GAP-1` missing for *every* product, including a perfectly correct one. It would read as
inbound message loss — a fabricated HIGH. This is the same shape as `24-H6` §5a (a probe that
could report the defect but not the fix) and `24-H5` §6 (an ANSI-blind matcher reading `null`
and blaming the product). **Both known-positives passed in the same invocation**, which is the
only reason the zero is worth anything: the client is provably alive and provably receiving.

Per LANE-BRIEF §6b-ii I repair the instrument **in this lane** — writing it up and moving on is
how the identical defect recurred once already on this program. The repair needs three
assertions, not two: known-positive passes, known-negative fails, **and the pre-repair fixture is
proven to miss it**.

Evidence: `24-RECONNECT-evidence/fixture-seq-repro.mjs`,
`fixture-seq-repro-BEFORE.json` (529 bytes, cross-checked `/usr/bin/wc -c` and `ls -la`, since
`wc -c` has returned 0 for a 72-byte file on this program).

### T+1h10 — instrument repaired, and proven able to fail

`nextSeq()` is now the only place `s` is minted. `f24-media-actions.mjs` had a **copy** of the
same derivation (`s || this.dispatched.length + 1`) and therefore a latent copy of the same bug;
it now calls the shared allocator. Also added `/__control/drop` (destroy sockets, no WS close
frame), `/__control/replay` (suppress the replay — the negative-control lever), and four report
journals so a driver never infers the drop or the replay.

`scripts/f24-reconnect-selftest.mjs`, **17 passed / 0 failed**, with R3 extracting the pre-repair
fixture **byte-exact** from `15cda12d` rather than re-implementing it.

Four-mutation sweep of the repaired instrument, each asserted to apply exactly once, tree
restored byte-identical (sha256 equal before/after):

| mutation | reddened | assertions that failed |
|---|---|---|
| MI1 the drop is announced but never performed | yes | R1b, R1d |
| MI2 RESUME replays the whole journal | yes | R1d, **R2b** |
| MI3 the replay is not journalled | yes | R1d |
| MI4 the replay kill-switch is inert | yes | **R4** |

Distinct attribution per mutation — not one blanket assertion wearing four names. The sweep also
caught **my own harness defect**: `check()` did not wrap `fn()`, so a thrown assertion escaped
`main()`, exited 2 (the USAGE code), never incremented `failed`, and never printed the
`passed=N failed=M` line. All three mutations initially "reddened" with `NO VERDICT LINE`,
grading correctly only by accident of the exit code. Repaired, not noted.

### T+1h40 — RUN 1: NOT MEASURED, and it presented as total inbound message loss

Live on `hetzner-dsm`, real `gateway run`, release binary sha256
`41d4a639ca1287d5dc989c84ac63cc1777b3852782edff39bd28c1418fca2218`.

The driver reported **0/2 on its own pre-drop control** and 0/2 on the gap. Read naively that is
catastrophic inbound loss. It was not. From the LLM fixture's own journal, in a different OS
process:

```
{"seq":1,...,"user_text":"hello f24rc-0c425c-before-1\n...","correlation":"no-correlation",...}
{"seq":3,...,"user_text":"hello f24rc-0c425c-during-3\n...","correlation":"no-correlation",...}
```

**All six messages arrived. Five turns executed.** The one denial in the gateway log
(`inbound denied channel=f24rcdisc reason=sender not in dm allowlist`) was my decoy, correctly
denied.

**Root cause: my token shape.** The shared `f24-llm-fixture.mjs:89` echoes only
`/f24c3-[a-z0-9-]+/i`; my `f24rc-` tokens produced `F24C3-REPLY no-correlation` for every turn, so
the census could match nothing. **The sixth instrument fault on this criterion, and the fifth to
fail in the direction that blames the product.**

It was caught *only* because the known-positive control failed first and refused to grade. Had I
written the run without it — or graded the gap leg before the control — this lane would have
filed a fabricated CRITICAL against a working adapter.

Repaired three ways rather than noted: `mintToken()` is now the single source of the shape; a
**live preflight** asks the running fixture to echo a minted token (and a wrong-shaped one, which
must NOT come back) and aborts with NOT MEASURED in milliseconds; and T1/T2/T3 assert the shape
against the **live** shared fixture, with T3 proving the old shape yields a `lost` from a real
fixture reply.

### T+2h — RUN 2: the measurement. Discord reconnect PASSES.

`F24RECONNECT PASS legs=7/7 lost=0 duplicated=0 leaked=0 mode=measurement`

| leg | result |
|---|---|
| pre-drop-control | PASS 2/2 replied (KNOWN-POSITIVE) |
| upstream-drop-really-happened | PASS 1 socket destroyed, no WS close frame, live 1 → 0 |
| adapter-reconnected | PASS **~750 ms, via RESUME (+1)**, connections 1 → 2 |
| **gap-messages-survive-the-upstream-drop** | **PASS 2/2**, each dispatched to **0 live sockets**, fixture replayed 3 on RESUME |
| no-duplicate-turns-around-the-window | PASS — raw deliveries **6 vs 6** dispatches |
| post-reconnect-control | PASS 1/1 (KNOWN-POSITIVE #2) |
| decoy-and-phantom-score-zero | PASS `leaked=[]` |

From the product's own log:
```
WARN  gateway session ended; backing off before reconnect
      error=ws read: WebSocket protocol error: Connection reset without closing handshake
      backoff_ms=1000 resumable=true
DEBUG sent RESUME session_id=f24c3-sess-1-… seq=3
DEBUG RESUMED received; replayed events will flow as dispatches
```

`each reached 0 live socket(s) at dispatch time` is the load-bearing number: there genuinely was
no connection when the gap messages were dispatched, so their arrival can only be the replay.

### T+2h15 — the one-variable negative control REDDENS

Same binary, same config, same message plan, same token shape. **One variable: the fixture
accepts RESUME and replays nothing.**

`F24RECONNECT FAIL legs=6/7 lost=2 duplicated=0 leaked=0 mode=CONTROL-no-replay`

```
FAIL gap-messages-survive-the-upstream-drop — 0/2 … fixture replayed 0 message(s) on RESUME.
     lost=[f24c3-rc-6d0292-during-3, f24c3-rc-6d0292-during-4]
PASS pre-drop-control            (still 2/2)
PASS post-reconnect-control      (still 1/1)
PASS adapter-reconnected         (still ~750ms via RESUME)
PASS decoy-and-phantom-score-zero
```

**Exactly one leg flipped.** Both known-positives stayed green, the reconnect still happened, and
raw deliveries fell 6 → 3. So the loss detector fires, and it fires *specifically*, which is what
makes run 2's `lost=0` a measurement rather than a free zero.

Captures: `24-RECONNECT-evidence/live/{run2-discord,control-no-replay,run1-not-measured}/`.
Byte counts cross-checked `/usr/bin/wc -c` vs `stat -f%z`. Secret sweep over every capture for the
per-run minted vault passphrase: **0 hits**, with a known-positive (`sent RESUME`, 1 hit) proving
the grep was alive.

### T+3h30 — matrix, via an out-of-process TCP kill-proxy

First attempt embedded the proxy in the driver and reported `sync_total=0` /
`/sync failed; backing off — error sending request`, i.e. "the matrix adapter cannot reach its
homeserver". **`Atomics.wait` blocks the whole event loop**, so the in-process proxy forwarded
nothing while the driver slept. `f24-discord-inbound.mjs:55-62` documents this exact failure and
it recurred anyway — F24-RC-I4. Repaired structurally into `f24-killproxy.mjs`.

- **matrix measurement:** `PASS legs=7/7 lost=0 duplicated=0 leaked=0`. Outage proven: 2
  connections destroyed, **3 reconnect attempts refused**, homeserver `sync_total` FROZE across
  the window and resumed `4 → 6` after restore.
- **matrix negative control** (link never restored): **`lost=3`**, proxy `refused=12`.

Corroborated in source *after* the measurement: `sync.rs:287-294` advances the cursor only on
`Ok` and pushes events to the inbox before `save_to`, so a failed `/sync` cannot move past an
undelivered window.

### T+3h45 — final state

Fence vs captured BASE SHA `15cda12d`: **0 bytes** (`crates/wcore-cli/src/{lib,main}.rs`,
`ci.yml`, `BACKLOG.md`), **0 Rust files**, **0 Cargo changes** — with a known-positive control in
the same invocation (a file I did change: 9493 bytes), so the diff instrument is proven alive.

Report written to `24-RECONNECT.md`. No product finding. Four instrument findings, all repaired
in-lane. `24-C3` NOT claimed.
