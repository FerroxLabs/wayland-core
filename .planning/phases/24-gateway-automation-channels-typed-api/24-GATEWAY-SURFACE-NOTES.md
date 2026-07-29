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

## Still to establish

1. Read `24-C3-H2-SUMMARY.md` (2 min) + locate the committed `--json-stream` 42-cell baseline.
2. Prove my gate can redden before trusting any pass (§3.2) — incl. that a zero-arrival green
   grades FAIL (`gradeSteady` requires `arrived === want && want > 0`, needs confirming live).
3. Run the 42-cell matrix on hetzner against `gateway run`.
4. Surface-to-surface diff; report divergence with defect-grade rigour.

## Standing constraints for this lane

- 24-C3 is NOT MET and stays NOT MET: media + native actions are at zero evidence on every
  adapter and nothing in this lane changes those two clauses.
- Do NOT edit `crates/wcore-channel-matrix/`, `.github/workflows/ci.yml`,
  `crates/wcore-cli/src/{lib,main}.rs`, `.planning/BACKLOG.md` (other lanes own them).
- Every number reported comes from an unproxied tool (`/usr/bin/grep`, `/usr/bin/git`).
