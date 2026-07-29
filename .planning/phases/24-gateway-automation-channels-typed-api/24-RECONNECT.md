---
phase: 24-gateway-automation-channels-typed-api
lane: 24-reconnect
branch: lane/24-reconnect
merge-base: 15cda12d6a189d7cad3daf0998eded4710f809af
criterion: "24-C3 (reference channels / the inbound matrix)"
clause: "reconnect/reload — the RECONNECT half only (upstream disconnect under a still-running process)"
grade-24-C3: "STILL NOT MET, and this lane does not claim it. Eight lanes have now declined it. media and native actions still have ZERO evidence on every adapter; every figure in the criterion is still Linux. This lane closes the reconnect half for TWO adapters and names the five it did not reach."
grade-reconnect-half: "discord PASS (7/7, lost=0, duplicated=0). matrix PASS (7/7, lost=0, duplicated=0). slack/whatsapp/sms OUT OF SCOPE BY CONSTRUCTION (webhook transport — the remote dials us, there is no upstream for our side to lose). telegram, email, signal, msteams, imessage NOT REACHED."
new-finding: "NONE IN THE PRODUCT. Four instrument defects found and ALL FOUR REPAIRED IN-LANE (F24-RC-I1..I4), three of which failed in the direction that blames the product and one of which is a RECURRENCE of a defect this repo had already documented in a comment. The product's reconnect path was not defective on either adapter driven."
fence-exposure: "zero — 0 bytes in crates/wcore-cli/src/{lib,main}.rs, .github/workflows/ci.yml and .planning/BACKLOG.md vs the captured merge-base SHA 15cda12d. 0 Rust files changed, 0 Cargo.toml/lock changes. Verified with a known-positive control in the same invocation (a file I DID change reported 9493 bytes)."
live-runs-graded: 4
live-runs-not-measured: 1
status: complete
---

# 24-RECONNECT — the upstream-drop half of `reconnect/reload`

**Verdict up front.** The `reconnect` half is **PASS on discord and PASS on matrix**, live, on
the real `gateway run` surface, with every zero paired to a control and a one-variable negative
control proven to redden for each. **`24-C3` is still NOT MET and I do not claim it.** I found
**no sixth product defect** in this path — and that is an absence claim, so §2 is entirely about
what makes it readable.

Nothing merged, no PR, nothing tagged, no issue closed, `wcore-contract generate` not run. No
vendor credential was read, required or embedded; every secret in every run was minted at run
time and died with it.

---

## 1. What I drove

Real binary, real installed surface. `hetzner-dsm`, Linux, `/root/wayland-24-reconnect`,
`wayland-core 0.12.25` release, sha256
`41d4a639ca1287d5dc989c84ac63cc1777b3852782edff39bd28c1418fca2218`, started as
**`wayland-core gateway run`** — not `--json-stream`, which is the surface every prior figure in
this criterion came from.

**The event is a genuine upstream disappearance, not an error the fixture politely returns.**

| adapter | transport | how the upstream was made to vanish |
|---|---|---|
| discord | WebSocket gateway | `socket.destroy()` on the fixture side — **no WS close frame**. The product logs `Connection reset without closing handshake`. |
| matrix | long-poll `/sync` | a **TCP kill-proxy** between the binary and the homeserver: destroy every live connection, then **refuse** new ones with a RST for a 12 s outage. The homeserver keeps accepting messages the whole time. |

The matrix shape is the one that matters for this criterion. `F24-C3-H6` was matrix losing
everything delivered while the **process** was down; H6 fixed it by persisting the `/sync` cursor
to disk. **That fix cannot answer this question**, because a process that stays up never reads
that file. The in-process path is separate code and had never been driven.

### The census

Messages **before**, **during** and **after** the window, each carrying its own correlation
token. Losses and duplicates are **derived here** from the fixtures' own journals. No figure the
product reports about itself is used for any verdict.

---

## 2. Why the zeros are readable — this section is the result

"No messages were lost" is the single easiest claim to pass without doing any work. A dead
client, a fixture that never dispatched, a drop that never happened, a matcher that matches
nothing — every one returns it for free, and this lane produced **three** of those states before
it produced a measurement.

Every run therefore carries, **in the same invocation**:

1. **A pre-drop known-positive.** If messages before the drop do not produce turns, the run is
   NOT MEASURED and no other zero is read. *This fired for real — see §4.*
2. **A post-reconnect known-positive.** Proves the adapter is ALIVE after the drop, so a zero on
   the gap cannot be explained by a dead adapter.
3. **A DECOY** — a message from a non-allowlisted sender, dispatched **inside** the window. It
   must score zero. If it scores, the matcher over-matches and every zero in the run is void.
4. **A PHANTOM** — a token registered in the census and **never dispatched at all**. If the
   detector answers yes to it, the detector answers yes to anything.
5. **A confirmed drop.** Socket count and refusal count read from the instrument's journal.
   `refused ≥ 1` is what proves the adapter kept *trying* rather than having quietly died.
6. **A one-variable negative control**, below.

### The negative controls both redden, and redden specifically

| run | the one variable | result |
|---|---|---|
| discord measurement | — | `PASS legs=7/7 lost=0 duplicated=0 leaked=0` |
| **discord control** | fixture accepts RESUME and **replays nothing** | **`lost=2`**, `legs=6/7`. Both known-positives still green, reconnect still ~750 ms, raw deliveries fell 6 → 3. **Exactly one leg flipped.** |
| matrix measurement | — | `PASS legs=7/7 lost=0 duplicated=0 leaked=0` |
| **matrix control** | the link is **never restored** | **`lost=3`**, proxy `refused=12`. |

Binary, config, message plan and token shape identical between each pair.

---

## 3. The measurements

### 3a. discord — PASS 7/7

| leg | result |
|---|---|
| pre-drop-control | PASS 2/2 (KNOWN-POSITIVE) |
| upstream-drop-really-happened | PASS — 1 socket destroyed, no close frame, live `1 → 0` |
| adapter-reconnected | PASS — **~750 ms, via RESUME**, connections `1 → 2` |
| **gap-messages-survive-the-upstream-drop** | **PASS 2/2**, each dispatched to **0 live sockets**; fixture replayed 3 on RESUME |
| no-duplicate-turns-around-the-window | PASS — raw deliveries **6 vs 6** dispatches |
| post-reconnect-control | PASS 1/1 (KNOWN-POSITIVE #2) |
| decoy-and-phantom-score-zero | PASS `leaked=[]` |

From the product's own log across the window:

```
WARN  gateway session ended; backing off before reconnect
      error=ws read: WebSocket protocol error: Connection reset without closing handshake
      backoff_ms=1000 resumable=true
DEBUG sent RESUME session_id=f24c3-sess-1-… seq=3
DEBUG RESUMED received; replayed events will flow as dispatches
INFO  inbound denied channel=f24rcdisc reason=sender not in dm allowlist   ← the decoy
```

`each reached 0 live socket(s) at dispatch time` is the load-bearing number: there genuinely was
no connection when the gap messages were dispatched, so their arrival can only be the replay.

**Duplicate-freedom is measured two ways**, because the product's 60 s inbound dedupe cache would
mask a duplicate *delivery* behind a single *turn*: turn count per token (`duplicated=[]`) **and**
the fixture's raw socket-delivery ledger (`6 deliveries / 6 dispatches`).

### 3b. matrix — PASS 7/7

| leg | result |
|---|---|
| pre-drop-control | PASS 2/2 |
| upstream-drop-really-happened | PASS — 2 connections destroyed, **3 reconnect attempts refused**, homeserver `sync_total` **froze** across the outage |
| adapter-reconnected | PASS — `sync_total 4 → 6` after restore |
| **gap-messages-survive-the-upstream-drop** | **PASS 2/2** — messages the homeserver received while our link was dead all produced turns |
| no-duplicate-turns-around-the-window | PASS `duplicated=[]` |
| post-reconnect-control | PASS 1/1 |
| decoy-and-phantom-score-zero | PASS `leaked=[]` |

The frozen `sync_total` is what makes the outage a fact rather than a hope: the adapter provably
could not reach the homeserver, and the homeserver provably kept receiving.

**Why it holds, confirmed in source after the measurement rather than before it**
(`sync.rs:287-294`): the cursor advances **only** on `Ok`, and events are pushed to the inbox
**before** `save_to`. So a failed `/sync` cannot move the cursor past an undelivered window, and
a crash between delivery and persistence re-delivers rather than skips — at-least-once, with the
dedupe layer collapsing the duplicate. I state this second because source review has missed all
five prior defects on this criterion; it is corroboration, not evidence.

---

## 4. Four instrument defects. All four repaired in-lane. Three blamed the product.

LANE-BRIEF §6b-ii: a written-up instrument defect is a defect you have agreed to keep. Each
repair carries **three** assertions — known-positive passes, known-negative fails, **and the
pre-repair instrument is proven to miss it**.

### F24-RC-I1 — the discord fixture could not express a gap replay at all

`f24-discord-fixture.mjs` allocated a dispatch sequence per-connection, with
`s || this.dispatched.length + 1` as the fallback for a dispatch reaching zero sockets. **That
fallback forgets READY consumed sequence 1**, so a message dispatched during a disconnect window
was numbered one too low and **collided with an already-delivered sequence**:

```
BEFORE-1 s=2   BEFORE-2 s=3   DURING-1 s=3  ← collision   AFTER-1 s=4
```

The client resumes from `seq=3`; the replay filter is `x.s > after`; the gap message is discarded
**by its own sequence number**. So the fixture was **structurally incapable** of expressing
"delivered while disconnected, replayed on RESUME" — and a reconnect probe built on it would have
reported inbound message loss **for every product, including a correct one**.

Repaired to a single session-global allocator, `nextSeq()`. `f24-media-actions.mjs` carried a
**copy** of the same derivation and therefore a latent copy of the same bug; it now calls the
shared allocator so the two cannot drift.

**Third assertion (R3/R3b):** the pre-repair fixture is extracted **byte-exact** from
`15cda12d` with `git show` — not re-implemented, because a re-derived "legacy" would be a
re-derivation of the bug — imported, and run through the identical scenario. It fails to replay,
and R3b proves the failure is the collision (`during.s == before2.s`, and `during.s ≤ resumeFrom.seq`)
rather than a dead legacy client.

### F24-RC-I2 — my own self-test swallowed assertion failures as crashes

`check()` did not wrap `fn()`. A thrown assertion escaped `main()`, exited **2** (this file's
USAGE code), never incremented `failed`, and **never printed the `passed=N failed=M` line** a
reader is instructed to read back. Found by the mutation sweep: all three mutations "reddened"
with rc=2 and `NO VERDICT LINE` — grading correctly only by accident of the exit code.

### F24-RC-I3 — the token shape, which cost a full run and looked like total inbound loss

The shared `f24-llm-fixture.mjs:89` echoes only `/f24c3-[a-z0-9-]+/i`. My driver minted `f24rc-`
tokens, so every reply came back `no-correlation` and the census could match nothing. **Run 1
dispatched 6 messages, admitted all 6, and executed 5 turns** — and the driver reported **0/2 on
its own pre-drop control**, then 0/2 on the gap.

Read naively that is catastrophic inbound message loss on a working adapter. It was caught **only
because the known-positive failed first and refused to grade the run**. Repaired three ways:
`mintToken()` is the single source of the shape; a **live preflight** asks the running fixture to
echo a minted token (and a wrong-shaped one, which must *not* come back) and aborts in
milliseconds with NOT MEASURED; and T1/T2/T3 assert the shape against the **live** shared fixture,
with T3 proving the old shape yields a `lost` from a real fixture reply.

### F24-RC-I4 — a recurrence of a defect this repo had already documented in a comment

My first kill-proxy ran **inside the driver**. Every driver here sleeps with `Atomics.wait`, which
blocks the whole Node event loop, so the proxy forwarded nothing while the driver waited. The run
reported:

```
the binary never established a /sync loop (sync_total=0) — NOT MEASURED
WARN /sync failed; backing off — error=network: error sending request for url (…:41743/…)
```

which reads as **"the matrix adapter cannot reach its homeserver."** A product defect, from an
instrument that was not listening.

`f24-discord-inbound.mjs:55-62` **already documents this exact failure** ("that is exactly how
this driver's first two runs failed"). It recurred anyway, in a new file, to a reader who had read
the warning. Repaired structurally: `f24-killproxy.mjs` is its own OS process, its control plane
is a **second listener on its own port** (a control plane behind the kill switch could not be
asked to restore), and the driver **proves the proxy forwards** before the binary depends on it.

### The instrument is proven able to fail

`scripts/f24-reconnect-selftest.mjs`: **17 passed / 0 failed**. Four-mutation sweep of the
repaired instrument, each replacement asserted to apply **exactly once**, tree restored
byte-identical (sha256 equal before and after):

| mutation | reddened | assertions that failed |
|---|---|---|
| MI1 the drop is announced but never performed | yes | R1b, R1d |
| MI2 RESUME replays the whole journal | yes | R1d, **R2b** |
| MI3 the replay is not journalled | yes | R1d |
| MI4 the replay kill-switch is inert | yes | **R4** |

Distinct attribution per mutation — not one blanket assertion wearing four names. The sweep's own
exactly-once guard also caught MI2 silently ceasing to match after a later edit and **refused to
report a mutation that never applied**.

---

## 5. Gate results, with the numbers read back

| gate | result |
|---|---|
| `node scripts/f24-reconnect-selftest.mjs` (Mac and hetzner) | **17 passed / 0 failed** |
| `node scripts/f24-discord-selftest.mjs` (pre-existing, my fixture change) | **16 passed / 0 failed** |
| instrument mutation sweep | **4/4 reddened**, restored byte-identical |
| live discord, `gateway run` | **PASS 7/7** `lost=0 duplicated=0 leaked=0` |
| live discord CONTROL | **`lost=2`**, one leg flipped |
| live matrix, `gateway run` | **PASS 7/7** `lost=0 duplicated=0 leaked=0` |
| live matrix CONTROL | **`lost=3`**, proxy `refused=12` |
| fence vs captured BASE **SHA** `15cda12d` | **0 bytes**, with a known-positive (9493 bytes on a file I did change) |
| Rust files changed | **0** |

No `cargo` gate is reported because **this lane changed zero Rust files**; a green suite would
have certified nothing it did. The release binary was built once on hetzner at `b4cedc25` and the
Rust tree is byte-identical at final HEAD, so the binary is the correct artifact for every figure
above. `df -h /root` checked before the build: 704 G free.

---

## 6. What I did NOT do

- **Did not claim `24-C3`.** Eight lanes have now declined it. `media` and `native actions` still
  have **zero evidence on every adapter**; both are untouched by this lane.
- **Did not reach five adapters:** **telegram** (poll — the probe shape is built and would port
  cheaply), **email** (poll — the pre-existing SMTP/webpki-roots blocker, untouched), **signal**
  (subprocess — the "upstream" is a spawned child, a genuinely different question), **msteams**,
  **imessage** (platform-blocked). I did not measure them and I do not grade them.
- **slack / whatsapp / sms are OUT OF SCOPE BY CONSTRUCTION, not skipped.** `TRANSPORT` in
  `f24-inbound.mjs:102` types them `webhook`: the remote dials *us*. There is no upstream
  connection for our side to lose, so "does it reconnect" is not a question that can be asked of
  them. Stating this rather than scoring them zero.
- **Did not measure anything on macOS or Windows.** Every figure here, like every figure in this
  criterion from every lane, is **Linux**. I did not use the Darwin-behaviour exception; nothing
  here is Darwin-specific.
- **Did not touch the `reload` half** — done by 24-C3-FINISH and 24-H5.
- **Did not re-verify `F24-C3-H6`'s process-restart fix.** Different event, different code path.
- **Did not weaken, ignore, delete or re-gate a single test.** No `#[ignore]`, no `#[allow]`, no
  raised timeout. Run 1 is reported as NOT MEASURED rather than re-run quietly.
- **Did not change a single Rust file**, did not touch the §6 fence, `ci.yml`, `BACKLOG.md`,
  `Cargo.toml` or `Cargo.lock`.
- **Did not run `wcore-contract generate`**, merge, open a PR, tag, or close an issue.
- **Did not use, read or require any vendor credential.**
- **Did not run a full-workspace build or test** — one targeted release build, per the
  disk/contention rule.
- **Did not use a global `pkill` pattern.** Every cleanup was scoped to my own run directory and
  my own minted token, because `24-H6` §8.0 measured a global pattern destroying another lane's
  in-flight run.

---

## 7. For the orchestrator to serialize

**No protocol seam, no contract fixture, no dependency change, no Rust, no fence bytes.**

**One shared-file change that other lanes must know about.** `scripts/f24-discord-fixture.mjs` is
shared with `f24-discord-inbound.mjs`, `f24-discord-selftest.mjs` and `f24-media-actions.mjs`
(which subclasses it).

- Dispatch sequence numbers are now **session-global and monotonic** rather than per-connection.
  `nextSeq()` is the only place `s` is minted. With two concurrent sockets one logical dispatch
  now carries the **same** `s` to both, which is more correct and leaves the duplication detector
  (`sockets` per dispatch) unchanged.
- Additive: `/__control/drop`, `/__control/replay`, and report fields `dispatch_ledger`,
  `duplicate_seq_numbers`, `forced_drops`, `forced_drop_sockets`, `resume_replays`,
  `resume_replayed_total`. **No existing field was removed or renamed.**
- A RESUME replay now increments that dispatch's `sockets` count, so
  `dispatch_socket_deliveries` counts replayed deliveries. A consumer comparing it to
  `dispatched_total` will see replays; before, replays were invisible to it.
- `f24-media-actions.mjs`'s `dispatchMessageWithAttachments` calls the shared allocator. Its own
  behaviour is unchanged (it dispatches to a connected client and never resumes).
- Both pre-existing selftests are green at final HEAD: discord **16/0**.

**Three new files, all mine and all additive:** `scripts/f24-reconnect.mjs`,
`scripts/f24-reconnect-poll.mjs`, `scripts/f24-killproxy.mjs`, plus
`scripts/f24-reconnect-selftest.mjs`.

**`f24-killproxy.mjs` is reusable** and is the cheapest way for the next lane to drive telegram's
or email's upstream drop: it is transport-agnostic TCP and needs no change to any fixture.

**Concurrency:** this lane binds **no fixed port**. The discord fixture, the matrix fixture, the
LLM fixture and the kill proxy all bind ephemeral ports, and `[inbound_webhook] enabled = false`
in both configs, so nothing here can collide with another lane. I did not need
`F24_WEBHOOK_PORT`, and I verified the prior lane's repair of it landed
(`f24-inbound.mjs:176`, and the scoped `pkill`s in `f24-inbound-run.sh`) rather than trusting it.

---

## 8. Evidence

`.planning/phases/24-gateway-automation-channels-typed-api/24-RECONNECT-evidence/`

| path | what it holds |
|---|---|
| `24-RECONNECT-NOTES.md` (parent dir) | the running log, committed at T+13 before any measurement and re-committed after each |
| `fixture-seq-repro.mjs`, `-BEFORE.json`, `-AFTER.json` | F24-RC-I1, measured before and after the repair |
| `mutate-instrument.py`, `instrument-mutation.json` | the four-mutation sweep, exactly-once guarded, sha256-verified restore |
| `selftest-GREEN.txt` | 17/0 |
| `live/run2-discord/` | **the discord measurement** — `result.json`, `gateway.log`, `llm-journal.jsonl`, `run.log` |
| `live/control-no-replay/` | the discord negative control, `lost=2` |
| `live/run3-matrix/` | **the matrix measurement** — plus `killproxy.log` |
| `live/control-matrix-no-recovery/` | the matrix negative control, `lost=3` |
| `live/run1-not-measured/` | F24-RC-I3 — the run that looked like total inbound loss and was not |

Byte counts cross-checked `/usr/bin/wc -c` against `stat -f%z`, because `wc -c` has returned 0 for
a 72-byte file on this program. Secret sweep over every capture for the per-run minted vault
passphrase: **0 hits**, with a known-positive in the same invocation (`sent RESUME`, 1 hit)
proving the grep was alive.
