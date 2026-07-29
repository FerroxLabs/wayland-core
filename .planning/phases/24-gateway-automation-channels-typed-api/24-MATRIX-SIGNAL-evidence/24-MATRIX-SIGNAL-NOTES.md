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

## Status

T+0: worktree created, brief + 24-C3-FINISH read, harness outlined. Nothing measured yet.
