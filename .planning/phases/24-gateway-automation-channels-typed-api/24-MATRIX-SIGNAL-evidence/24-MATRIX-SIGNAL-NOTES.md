# 24-MATRIX-SIGNAL — running notes (committed early per LANE-BRIEF §6b-i)

Lane: `lane/24-matrix-signal`, branch base `plan/f20-unified-audit-repair` @ `d34b2fe1`.
Worktree `/Users/seandonahoe/dev/waylandcore-frontier-worktrees/lane-24-matrix-signal`.
Started 2026-07-29T02:05Z.

## Mandate

Drive **matrix** and **signal** inbound across the same five legs
(`admit / dedupe / access / bind / route`) the other adapters are driven across, using
`scripts/f24-inbound.mjs`. Add a **steady-state leg** (messages after a settle period).
Answer the matrix **inbound restart / sync-token** question. Grade honestly — `24-C3`
is NOT MET and must not be claimed.

## T+0 — what I have read

- `LANE-BRIEF.md` in full.
- `24-C3-FINISH.md` in full. Its §4b is the costing this lane inherits:
  - **matrix** — `MatrixConfig.homeserver_url` (`config.rs:9`) required, no
    `#[serde(default)]`, no production constant; consumed by `new()` (`lib.rs:61-62`);
    `make_matrix` calls `new()` (`registry:179`). Transport = HTTP long-poll `/sync`.
    **Zero Rust needed.** No production default to preserve ⇒ no control test needed.
  - **signal** — `SignalConfig.signal_cli_path` (`config.rs:18`) →
    `RealLauncher::launch` `Command::new(cli_path).arg("-a").arg(account).arg("jsonRpc")`
    (`subprocess.rs:53-62`). Transport = **stdio JSON-RPC subprocess**. The fixture is an
    executable, not an HTTP server. **Costed cheapest in the phase.** TO BE VERIFIED
    against source myself — I do not inherit a claim I have not read.
- `scripts/f24-inbound.mjs` (1854 lines). Shape understood:
  - `ADAPTERS = ['slack','whatsapp','sms','telegram','email']`, `TRANSPORT` map,
    `LEGS = ['admit','dedupe','access','bind','route']`.
  - `runMatrix(adapter, cfg)` is the generic 5-leg driver. A **webhook** adapter supplies
    `cfg.build` (signed request POSTed to `/webhooks/:channel`); a **poll** adapter
    supplies `cfg.inject` (hand to fixture control plane, binary comes and gets it).
    Telegram is the reference `inject` adapter.
  - Arrivals are read from an **out-of-process sink journal** (`f24-sink.mjs`); turns from
    a second journal (`f24-llm-fixture.mjs`). `readerFor(adapter)` selects the journal.
  - `DEDUPE_TTL_MS = 60_000` and an explicit `replayDelayMs >= TTL ⇒ recordIncomplete`
    guard — the trap the brief warned about is already closed in the shared driver.
  - `access` leg's pass condition **includes** `accessControlHeld = seen1.length === 1`,
    so universal denial cannot manufacture a green. Confirmed by reading, lines 1233-1246.
  - `instrument_fault` ⇒ exit 3 INCOMPLETE, distinct from RED. Already present.
  - Correlation tokens must match `f24-llm-fixture.mjs`'s regex — 24-C3-FINISH §5.2
    burned a run on `f24c3fin-` vs `/f24c3-[a-z0-9-]+/i`. My tokens go through
    `runMatrix`, which already builds `f24c3-${adapter}-...`, so this is inherited safe;
    I will still assert it.

## Open questions I must answer

1. Is signal's subprocess seam as cheap as costed? (verify `subprocess.rs` myself)
2. Does the **matrix inbound** side reuse or reset a `since`/sync token across a restart
   in a way that loses or replays messages? The outbound txn-id defect (HTTP 200 with the
   OLD event id) has a plausible inbound twin. NOT YET LOOKED AT.
3. Does a steady-state leg (post-settle arrivals) show silent ongoing loss, as it did for
   Telegram (that is what raised F24-C3-H4 from MEDIUM to HIGH)?

## Instrument discipline for this lane

- Every absence claim gets a **known-positive in the same invocation** (§3b-i).
- Load-bearing measurement via `/usr/bin/grep`, `/usr/bin/git`, never bare `grep`/`git`.
- Byte-count every capture.
- New legs must be able to FAIL — self-test with three assertions including "the old
  broken matcher would have missed it".

## T+35 — SOURCE READ COMPLETE. Both seams verified myself; one likely HIGH found.

### Seam 1 — signal. CONFIRMED, and it IS the cheapest in the phase.

- `crates/wcore-channel-signal/src/config.rs:18` — `signal_cli_path: PathBuf`, but
  **with** `#[serde(default = "default_signal_cli_path")]` returning `PathBuf::from("signal-cli")`.
  **CORRECTION to 24-C3-FINISH §4b:** it says matrix has "no production default to preserve,
  so there is no control test to write" — true of matrix, and by omission it reads as though
  signal is the same. Signal DOES have a production default (bare `signal-cli`, PATH lookup).
  A control assertion is therefore warranted for signal and not for matrix.
- `subprocess.rs:52-63` — `RealLauncher::launch` = `Command::new(cli_path).arg("-a")
  .arg(account).arg("jsonRpc")` with stdin/stdout/stderr piped, `kill_on_drop(true)`.
- `lib.rs:82-83` — `SignalChannel::new` → `Self::with_launcher(name, config, Arc::new(RealLauncher))`.
  `with_launcher` is the `#[doc(hidden)]` test seam; `new()` is the shipped one and it hardwires
  `RealLauncher`. **The fixture must therefore be a real executable on disk** — the trait seam
  is NOT reachable from config, but the PATH is, and that is enough.
- `wcore-channels-registry/src/lib.rs:157-169` — `make_signal` calls `SignalChannel::new`. Shipped path.
- Wire: line-delimited JSON-RPC 2.0 on stdio.
  - inbound notification: `{"jsonrpc":"2.0","method":"receive","params":{"account":..,
    "envelope":{"source":"+1..","sourceName":"..","timestamp":<ms>,"dataMessage":{"message":"..",
    "timestamp":<ms>}}}}`
  - outbound: `{"jsonrpc":"2.0","id":N,"method":"send","params":{"recipient":["+1.."],"message":".."}}`
    → must answer `{"jsonrpc":"2.0","id":N,"result":{"timestamp":<ms>,"results":[{"type":"SUCCESS"}]}}`
    (`jsonrpc.rs:135-168`, `classify_delivery`).
- Identity mapping (`subprocess.rs:262-331`): dedupe `id` = `format!("{ts_ms}")` — **the envelope
  timestamp IS the message id**. `conversation_id` = groupId ?? source ?? sourceUuid.
  `sender_id` = sourceUuid ?? source ?? sourceName. So sending only `source` (no `sourceUuid`)
  makes signal **peer-keyed**, the same shape as whatsapp/sms/telegram.
  `chat_type` = Direct when `groupInfo` absent.
- **No HTTP, no TLS, no port, no certificate.** Verdict: costing was RIGHT.

### Seam 2 — matrix. CONFIRMED.

- `config.rs:9` `homeserver_url: String`, required, no default. `lib.rs:61` `new()` does
  `let api_base = config.homeserver_url.clone();` → `with_base`. `with_base` is the
  `#[doc(hidden)]` test seam, and `new()` feeds it straight from config. Registry `make_matrix`
  (`registry:173-184`) calls `new()`. Shipped path, zero Rust.
- Inbound: `GET {api_base}/_matrix/client/v3/sync?timeout=30000[&since=..]`, bearer auth.
- Outbound: `PUT {api_base}/_matrix/client/v3/rooms/{room}/send/m.room.message/{txnId}` (`rest.rs:135`).
  Fixture journals this as the arrival, `conversation_id` = room id.
- Identity mapping (`sync.rs:323-372`): dedupe `id` = **`event_id`**; `conversation_id` = room id;
  `sender_id` = `ev.sender` mxid. **Matrix is ROOM-keyed** (like slack), not peer-keyed —
  bind leg = two rooms, one sender.
- **CRITICAL fixture detail:** `chat_type` is `Direct` ONLY when
  `rooms.join[room].summary."m.joined_member_count" == 2` (`sync.rs:328-331`); anything else,
  including an omitted summary, is `Group`. The other adapters' configs set `group = "disabled"`,
  so a fixture that omits the summary would have every message dropped by GROUP policy and the
  run would read as inbound loss for a reason that is my fixture's fault. Summary must be emitted
  on every sync response, not just the initial one.
- Bot self-echo skip: `ev.sender == bot_user_id` → skipped. Sender must not be the bot mxid.

### FINDING CANDIDATE — matrix inbound restart. The answer is YES, there is an equivalent.

`sync.rs:190` — `let mut since: Option<String> = None;` is a **process-local variable inside
`sync_loop`**. It is never written anywhere.

`sync.rs:212-226` — `let is_initial = since.is_none();` and events are emitted **only when
`!is_initial`**. The initial sync is consumed for its cursor and its timeline is discarded
(documented "initial-sync replay guard", `sync.rs:8-12`).

Composition: **on every process restart `since` resets to `None`, so the first `/sync` is an
initial sync, so its entire timeline is discarded.** A real homeserver returns recent room
timeline in an initial sync — including everything delivered while the process was down. Those
messages are dropped silently: no error, no retry, no log an operator reads, and the channel
reports healthy.

This is the inbound twin of the outbound txn-id defect (reuse after restart → HTTP 200 with the
OLD event id → new message vanishes reporting success). Same root shape: **state that must
survive a restart does not.**

**It is NOT an unavoidable tradeoff, and the proof is in this repo.** Same concept search over
the sibling polling adapter:

```
/usr/bin/grep -rniE 'persist|watermark|checkpoint|state_dir|cursor|resume|fs::write|fs::read' \
    crates/wcore-channel-matrix/src/   ->  5 hits, ALL comments/CredentialsStore, zero persistence
    crates/wcore-channel-email/src/    -> 24 hits, real persistence
```
Instrument proven alive on a known-positive in the same shape: `next_batch` in the matrix crate
→ **16 hits**. So the matrix zero is a measured zero, not a dead grep.

`crates/wcore-channel-email/src/imap.rs:120` states the intent verbatim:
"**Resume the UID watermark from disk so a restart neither replays the [backlog] nor [loses
the gap]**". Email solves exactly this problem deliberately. Matrix implements the replay-guard
half and omits the resume half.

**Status: READ, NOT YET PROVEN LIVE.** Must not be reported until a live run separates:
- H1 (product) restart drops the gap;
- H2 (my fixture) the fixture never put the gap message in the initial-sync timeline, so there
  was nothing to lose.
The control must show the SAME message arriving when delivered with no restart, and the fixture
must independently report that the initial sync it served DID contain the gap event.

## Plan

1. `scripts/f24-matrix-fixture.mjs` — homeserver: `/sync` long-poll with real `since` cursor
   semantics + room summary, `PUT .../send/...` journalling, `__control/{submit,report,health}`.
2. `scripts/f24-signal-fixture.mjs` — an **executable** speaking JSON-RPC on stdio, plus a
   sidecar control socket so the driver can inject while the binary owns the process's stdio.
3. Wire both into `f24-inbound.mjs` (`ADAPTERS`, `TRANSPORT`, readers, configs, `runMatrix` cfgs).
4. Add the **steady-state leg** to `runMatrix` for every adapter.
5. Add a **matrix restart leg** proving/disproving the finding above.
6. Self-test with the mandatory three assertions.
7. Live run on hetzner.

## T+95 — INSTRUMENTS BUILT AND SELF-TESTED. Two of my own defects found and repaired.

Committed `aa4351aa`, pushed. hetzner worktree `/root/wayland-24-matrix-signal` at the same SHA.

### What was built

| file | what |
|---|---|
| `scripts/f24-matrix-fixture.mjs` | homeserver with real `/sync` cursor semantics + `initial_syncs[].served` |
| `scripts/f24-signal-fixture.mjs` | fake `signal-cli`, JSON-RPC on stdio, spawned BY THE PRODUCT |
| `scripts/f24-inbound.mjs` | both adapters wired; new `steady` leg for EVERY adapter; matrix restart probe |
| `scripts/f24-matrix-signal-selftest.mjs` | 33 assertions |

`LEGS` is now `admit / dedupe / access / bind / route / steady`; `ADAPTERS` is now 7.
Expected legs = 7 × 6 = 42.

### TWO INSTRUMENT DEFECTS, MINE, FOUND BY THE SELF-TEST BEFORE ANY LIVE RUN

Both failed **in the direction that blames the product** — which is what an under-detecting
instrument does by default, and is why each had to be caught by an assertion rather than by
a red.

1. **The fixture sent `{rooms: {<roomId>: ...}}` instead of `{rooms: {join: {<roomId>: ...}}}`.**
   `sync.rs:61-65` deserialises `Rooms { #[serde(default)] join }`, so the missing `join` key
   would have defaulted to an EMPTY map, `parse_sync_events` would have iterated nothing, and
   **every matrix leg would have reported ZERO ARRIVALS as a product defect.** This is exactly
   the fabricated-HIGH shape the brief warns about. Caught by M2.
2. **The self-test read the fixture's stdout with `child.stdout.on('data')` while blocking the
   event loop with `Atomics.wait`.** The handler never ran, so it reported "no frame appeared
   on stdout" while the fixture was emitting correctly. Caught by S1/S3/S4 failing.

Both **repaired here, not written up and carried** (§6b-ii). The repair for (2) is structural
— all stdio interaction happens in one top-level-await block and the observations are frozen
into plain data, so the synchronous `test()` calls keep the hard-fail-on-thenable guard.

### The self-test is PROVEN ABLE TO FAIL

Not by mutation, but by history: its first execution was **26 passed / 7 failed** and the 7
were two genuine defects. Restored: **33 passed / 0 failed** on macOS.

### The mandatory THIRD assertions (§6b-ii)

- `T4` — the five ORIGINAL legs **all pass** on a simulated adapter that delivers the startup
  burst perfectly and then goes deaf, while `steady` catches it. This is the assertion that
  proves the new leg does anything; without it T1-T3 would pass on a driver that never added
  it. `T5` is a drift guard asserting the five transcribed conditions still exist verbatim in
  `runMatrix`.
- `R3` — `naiveGradeRestart` (the grader without the H2 exclusion, kept executable) reports
  **LOSS** on the exact observation where `gradeRestart` reports **INCOMPLETE**. That is the
  fabricated HIGH, demonstrated rather than asserted.

### Product-contract assertions (P1-P8)

Read from source so a product change reddens here and names itself rather than silently
changing what a green run means. Notably `P3` asserts `sync.rs` STILL holds the cursor
process-local — if someone repairs it, the probe would go green and P3 is what tells the
reader the green means "repaired", not "never broken". `P4` is P3's known-positive control.

## Status

T+0: worktree created, brief + 24-C3-FINISH read, harness outlined.
T+35: both seams verified from the shipped construction path. Matrix restart defect candidate
found by source read, with a sibling reference implementation proving it is not a tradeoff.
T+95: instruments built, self-tested 33/33 on macOS, two of my own defects found and repaired.
Committed + pushed `aa4351aa`. hetzner build launched. **Nothing live yet — no product number
has been taken.**
