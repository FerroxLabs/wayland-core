# 24-GATEWAY-SURFACE — NOTES (living; append after every measurement)

Lane: `lane/24-gateway-surface`. Base `plan/f20-unified-audit-repair` @ `e77b44b0`.
Worktree: `/Users/seandonahoe/dev/waylandcore-frontier-worktrees/lane-24-gateway-surface`.
Started 2026-07-29.

## What I am measuring

Drive all 7 adapters x 6 legs = **42 cells** (computed from `ADAPTERS.length * LEGS.length`,
not hard-coded) through `scripts/f24-inbound.mjs --runtime gateway`, i.e. against the
**`gateway run`** surface an operator actually installs — then compare cell-for-cell against
the already-committed `--json-stream` results. Any cell that passes on `--json-stream` and
fails on `gateway run` is the finding that matters most: the shipped surface differing from
the tested one.

## Established so far

- Worktree created and verified (`git rev-parse --show-toplevel` matches the lane path).
- Harness already supports the surface switch: `--runtime json-stream|gateway`,
  `f24-inbound.mjs:2683` rejects any other value rather than silently falling back
  (so a typo cannot quietly re-measure `--json-stream` and be reported as `gateway`).
- `f24-inbound.mjs:1030` — `argv = runtime === 'gateway' ? ['gateway','run'] : ['--json-stream']`,
  foreground (no `--detach`) so the driver owns and can reap the child.
- `ADAPTERS` (7) = slack, whatsapp, sms, telegram, email, matrix, signal;
  `LEGS` (6) = admit, dedupe, access, bind, route, steady. Expected total 42.
- `TRANSPORT`: webhook = slack/whatsapp/sms; poll = telegram/email/matrix; subprocess = signal.
  `failWebhookLegs` is scoped to webhook adapters only — deliberately, so a runtime that binds
  no webhook host can still be measured on the polling adapters.
- Liveness uses `pidIsLive`, not `kill(pid,0)` (the zombie trap from the brief) — already in
  the harness.
- **Not re-deriving F24-C3-H2.** Per coordinator: `gateway run` DOES opt into inbound dispatch;
  it is built and merged. Reading `24-C3-H2-SUMMARY.md` rather than re-establishing from source.

## Established — measurement setup (T+~35min)

- **Baseline (committed, `--json-stream`)**, from `24-MATRIX-SIGNAL.md` §2a, 3 runs:
  **36/42 legs ran, `failed=0`, 6 NOT MEASURED (email, pre-existing SMTP/webpki-roots
  blocker)**, `restart_verdict=LOSS` (a known open HIGH on matrix inbound restart, NOT mine
  to fix). Binary source there was `aa4351aa`.
- **Confound spotted and being controlled for:** the committed baseline was taken at binary
  `aa4351aa`; my base is `e77b44b0`, which is LATER and contains the double-manager and
  reload-denial fixes. A gateway-vs-baseline diff would therefore confound *surface* with
  *commit*. So I run **BOTH surfaces at MY commit** — same binary, same driver, same
  fixtures, only `--runtime` differs. That makes any divergence attributable to the surface
  alone. The committed baseline is retained as the third point of comparison.
- **hetzner**: `/root/wayland-24-gwsurface` @ `3fe3832a`, built rc=0 in 5m40s.
  `wayland-core 0.12.25 (source 3fe3832a...)`, sha256
  `851c049a957c8a8c28fcf6e056c0c9873950ffe528bc253f0814bda7598417fa`. node v22.21.1.
  Host was IDLE at start (load 1.04, **zero** cargo/rustc running), so §6's contention
  caveat does not apply to these figures.
- **Instrument proven able to redden** — `scripts/f24-matrix-signal-selftest.mjs`:
  **`SELFTEST GREEN passed=41 failed=0`**. Read the counts, not the exit status. It carries
  the third-assertion pattern §6b-ii demands:
  - `T3` universal denial CANNOT manufacture a steady green (the brief's headline trap);
  - `T4` the five original legs all pass on an adapter that goes deaf after the burst —
    i.e. the steady leg is the only one that can see that class;
  - `R3`/`V3`/`Z3` the OLD grader/verdict/liveness check each disagree with the repaired one
    on the same input, which is what proves the repairs do anything.
- `--runtime` is validated (`f24-inbound.mjs:2683`): a typo exits 2 rather than silently
  measuring `--json-stream` and labelling it `gateway`.

## Still to establish

1. Run A: 42-cell matrix, `--runtime gateway`, at `3fe3832a`.
2. Run B: 42-cell matrix, `--runtime json-stream`, at `3fe3832a` (paired same-commit control).
3. Surface-to-surface diff; report any divergence with defect-grade rigour.

## Standing constraints for this lane

- 24-C3 is NOT MET and stays NOT MET: media + native actions are at zero evidence on every
  adapter and nothing in this lane changes those two clauses.
- Do NOT edit `crates/wcore-channel-matrix/`, `.github/workflows/ci.yml`,
  `crates/wcore-cli/src/{lib,main}.rs`, `.planning/BACKLOG.md` (other lanes own them).
- Every number reported comes from an unproxied tool (`/usr/bin/grep`, `/usr/bin/git`).
