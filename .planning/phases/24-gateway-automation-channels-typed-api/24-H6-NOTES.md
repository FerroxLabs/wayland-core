# 24-H6 NOTES — running log (committed at T+0, before any measurement)

**Fixing:** F24-C3-H6 — matrix `/sync` cursor is process-local (`sync.rs:190`) and the
initial sync's timeline is discarded (`sync.rs:217`), so every message delivered while the
process is down is silently lost on restart.

**Known (inherited, NOT re-derived):** mechanism proven 3/3 with three controls in
`24-MATRIX-SIGNAL.md`. Reference implementation to follow is `imap.rs:120` +
`uid_store.rs` in `wcore-channel-email` — persist a resume position keyed per account.

**Need:** persist the `/sync` cursor across restarts, keyed (homeserver × user_id ×
channel name); prove four things — gap message arrives after restart, no duplicate on
restart, corrupt/missing cursor degrades safely and says so, steady state unaffected
(counted). Plus: prove the gate reddens on a real loss before trusting a pass. Use
`wcore_types::process_liveness`, never a hand-rolled liveness check.

---

## T+~50min — fix landed, gates proven able to fail, instrument repaired

**Fix** (`ae9fbbd4`): `sync_store.rs` (new) + `sync.rs` resume/persist/wedge-guard + `lib.rs` wiring.
Three-state read (`Cursor` / `Absent` / `Corrupt`) so a corrupt file cannot read as a first run.
Wedge guard: a persisted cursor the homeserver answers 400 to is discarded ONCE with a warning,
then re-seeded — without it the loop backs off on it forever.

**Crate suite, hetzner `/root/wayland-24-h6`:** `cargo test -p wcore-channel-matrix`
→ **36 passed / 0 failed / 0 ignored**. Executed count read back, not exit status.

**MUTATION PROOF — every new gate reddens (LANE-BRIEF §3.2):**
| mutation | result | tests that failed |
|---|---|---|
| M1 ignore the persisted cursor (the defect itself) | REDDENED 3 failed | gap-survives-restart, wedge, steady |
| M2 never persist | REDDENED 5 failed | all five loop tests |
| M3 Corrupt reads as Absent | REDDENED 2 failed | corrupt-classification, corrupt-reseed |
| M4 remove the wedge guard | REDDENED 1 failed | rejected-cursor-does-not-wedge |
Each mutation asserted to apply exactly once; tree restored (0 bytes diff) after the sweep.

**INSTRUMENT DEFECT FOUND AND REPAIRED (my own lane's, §6b-ii).** The prior lane's restart
probe keyed its H2 exclusion on `initial_syncs[].served` and its liveness leg on
`initial_sync_total`. Both are only true while the product is BROKEN — a resuming adapter
issues an INCREMENTAL sync after a restart and never an initial one. So the probe **could
report the defect but could not express the fix**: it would have graded INCOMPLETE and burned
90s failing its liveness leg. Repaired to `servedAfterRestartFrom(report.syncs, ...)` (any
sync, records which KIND) and `sync_total`. Three assertions added (R5/R6/R7) including
`legacyServedInInitialOnly` kept executable to prove the repair changes an outcome.

**P3/P4 inverted.** The prior lane wrote them to redden and name themselves if anyone fixed
`sync.rs`. They did exactly that on the first post-fix run. Inverted to assert the fix (and
that the replay guard SURVIVED it), not deleted, so a regression reddens again.

**Driver self-test:** `node scripts/f24-matrix-signal-selftest.mjs` → **GREEN passed=44 failed=0**
(41 before this lane; +R5 +R6 +R7).

**Still to do:** live run of the unmodified restart probe against the fixed release binary on
hetzner; expect `verdict=PASS gap_served_on=incremental`.

---

## T+~110min — LIVE RUN 1, restart probe PASS

`hetzner-dsm`, `/root/wayland-24-h6`, `wayland-core 0.12.25`, sha256
`d618d3766db2f30eaf75f68a0865b15ac5d831a6aec40189d49f8a05f610f364`, `--json-stream`.

`INBOUND MATRIX RED legs=36/42 failed=0 not_measured=6 accounted=42/42
probe_failed=false restart_verdict=PASS` — RED only because email's six legs remain
NOT MEASURED (pre-existing SMTP/webpki-roots blocker, untouched by me). **failed=0.**

**Restart probe 6/6 PASS** (was 5/6 with the leg FAILing at LOSS before the fix):
- `restarted-binary-resyncs`: `sync_total 280 -> 282`, **`initial_sync_total 1 -> 1`** — the
  restarted process issued NO initial sync. It resumed.
- `gap-event-was-served-to-the-restarted-process`: sync **281, `initial:false`**, carried
  `$f240d3685degap` — **on an incremental sync**. The adapter ASKED for the window it missed.
- `gap-message-survives-the-restart`: **verdict=PASS arrivals=1**.
- `gap_arrivals=1` exactly — not 2. No duplicate.

**From the fixture's own journal, another OS process, with known-positive controls in the
same extraction** (`journal-mechanism.txt`, 282 `sync.close` records):
```
total initial syncs in the WHOLE run (both incarnations): 1
PRE  $f240d3685depre : initial_syncs_serving=0  incremental_syncs_serving=1
GAP  $f240d3685degap : initial_syncs_serving=0  incremental_syncs_serving=1
POST $f240d3685depost: initial_syncs_serving=0  incremental_syncs_serving=1
```
The finding lane measured the GAP row as `initial=1 incremental=0` with arrivals=0. It is
now `initial=0 incremental=1` with arrivals=1. Same instrument, opposite result.

**Operator-visible, from the product's own logs in two incarnations:**
- `core.log`: `INFO no persisted /sync cursor (first start for this account); seeding from an initial sync — existing room backlog will NOT be replayed`
- `core-restarted.log`: `INFO resumed persisted /sync cursor; messages delivered while this process was down will be served`
- on disk: `home/channel-state/matrix-5fbec553f4d71022.since` = `s18`, **beside**
  `imap-*.uid` and `telegram-*.offset` — the project's existing mechanism, not a second one.

**Steady state, counted, live:** `matrix/steady 3/3 [1,1,1]` after 30s quiet; all six
measurable adapters 3/3. Matrix 6/6, signal 6/6.
