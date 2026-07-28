---
phase: 24-gateway-automation-channels-typed-api
criterion: "24-C5 (setup-to-recovery journeys) + 24-C1 (upgrade/rollback, platform coverage)"
lane: 24-journey
branch: lane/24-journey
status: partial
grade-24-C5: "MET on Linux. NOT MET on Windows (red, cause isolated, two fixes landed, re-proof incomplete). NOT RUN on macOS (artifact never produced within the session)."
grade-24-C1: "upgrade and rollback now PERFORMED and observed — on Linux only. The platform half of the clause is unchanged."
merge-base: f5966d61e0b70cedd907acada1c9b5d4b135e6a7
candidate-proved: d89b81b6f4460d6c493552c3eb91eb0b8ad0eb56
head: cccdf14d6ea51de8377691d2afab156e6f012f01
---

# 24-C5 — the setup-to-recovery journey

**One sentence: the instrument now exists and is honest, Linux passes it end to
end with a recomputable receipt, Windows fails it and the failure found two real
Windows defects, and macOS was never driven because the artifact it depends on
never came out of the queue.**

Nothing here was merged, pushed to `main`, tagged, released, or used to close an
issue. No requirement is marked complete.

---

## 1. What did not exist before, and now does

`24-04` was never started, and `24-04-SUMMARY.md` says so in its own §6. There
was no journey driver, no receipt schema and no receipt on any platform. All of
the following are new.

| Artifact | What it is |
|---|---|
| `scripts/f24-journey.mjs` | The one ordered **17-step** journey. Identical on all three platforms; the only platform difference lives in an invocation table (how a process is killed, how the platform's service mechanism is queried, how a residual registration is detected). Writes five run-scoped files and **nothing** into the repository. |
| `scripts/f24-sink.mjs` | The independent delivery destination, as its own OS process. |
| `crates/wcore-eval-scenarios/src/journey.rs` | The receipt schema and verifier. |
| `crates/wcore-eval-scenarios/bin/wayland-journey.rs` | `verify \| scan \| redact \| bind`, every argument long-named, each exiting non-zero on refusal. |
| `crates/wcore-eval-scenarios/tests/journey_receipt_contract.rs` | 21 tests driving the compiled tool, one per named refusal. |
| `redaction` module | Promoted from crate-private to public, plus `from_secret_set`, which **refuses** a too-short secret instead of silently dropping it the way `from_secret` does. |

### The step list (canonical, enforced by the verifier)

```
preflight-clean  binary-identity  profile-setup  sink-start  gateway-install
gateway-start    status-running   automation-add deliveries-submit
arrival-before-kill  hard-kill  platform-recover  delivery-reconcile
upgrade-in-place  rollback  redaction-canary  drain-uninstall-clean
```

`upgrade-in-place` and `rollback` are in the list because **24-C1 names both and
neither had ever been performed on any platform.**

### The receipt schema, and what it refuses

`schema, platform, service_family, candidate_commit, binary_version,
binary_sha256, driver_commit, started_at, finished_at, arrival_source, counts{5},
steps[17]{name, command, output, ok}`.

`wayland-journey verify` exits non-zero on: an empty or unparsable receipt; a
wrong schema tag; a step list that is not exactly the canonical ordered list; any
step with an empty command, empty captured output, or `ok: false`; an
`arrival_source` other than `independent-sink`; counts that do not reconcile;
zero deliveries; a wrong platform; a wrong commit; and **a recorded binary digest
that differs from the one the verifier computes itself by hashing the file**.

Two design points are load-bearing rather than decorative:

- **`candidate_commit` is read out of the binary** (`--build-info`, which embeds
  `WAYLAND_SOURCE_SHA` at build time), not from the host's checkout. The two
  diverge exactly when a stale binary is being driven. **This caught a real stale
  build**: on Windows, `cargo build --release` reported success at a checkout of
  `e18da29b` while producing a binary that still reported `132a4725`. Every
  Windows figure after that point was taken from a `cargo clean -p wcore-cli`
  rebuild whose `--build-info` was read back and matched.
- **`duplicates` and `losses` are derived, not trusted.** A receipt cannot assert
  `duplicates: 0` while carrying `arrived > unique`.

`bind` additionally refuses two receipts that name the same platform — three
receipts all saying `linux` would satisfy a naive same-commit check.

---

## 2. The fixture endpoint — for the `24-C3` lane

**Yes, the journey needs one, and it is deliberately NOT inlined in the driver.**
There are two, and a `24-C3` lane can take either:

1. **`scripts/f24-sink.mjs`** (new, this lane). Standalone node process, no build.
   ```
   node scripts/f24-sink.mjs --journal <PATH> [--port N] [--stall-after N]
   ```
   Binds `127.0.0.1` (ephemeral port unless `--port`), prints
   `SINK_READY url=http://127.0.0.1:<port> journal=<abs path>` on stdout and
   flushes. Serves `POST /api/chat.postMessage`, `GET|POST /api/auth.test`,
   `POST /api/reactions.add`, `GET /_sink/health`. Writes one JSON record per
   arrival, `fsync`ed **before** the response, in the exact shape of
   `wcore_eval_scenarios::fixtures::channel::Arrival` — so the Rust
   `ArrivalTally` reader can read a journal this process wrote. `--stall-after N`
   answers N deliveries then accepts-journals-and-never-answers the next, which
   is the only way to place a delivery in the sender's outcome-unknown class from
   outside the sender. No env vars. Requires node (v22 on hetzner, v24 on
   `seandesktop`, v22 on the Mac).

2. **`wayland-channel-sink`** (pre-existing, `crates/wcore-eval-scenarios/bin/`,
   used by lane 24c). Same journal shape, same endpoints, same `--stall-after`.

**Drive it from the product** by writing a channel config into
`$WAYLAND_HOME/channels/<name>.toml` with `platform = "slack"` and
`api_base_url = "<the SINK_READY url>"`, plus a `$WAYLAND_HOME/credentials.toml`
holding a `[secrets]` table for the two `credential_handle_*` values. Read the
URL **before** writing the config; a gateway pointed at an unbound port fails its
sends in a way indistinguishable from a product defect.

**Which to pick:** if `24-C3` runs only on Linux, use the Rust one — it is
already proved. If it needs macOS, use the node one; the Rust binary cannot be
built on the Mac and this lane chose node everywhere rather than measure two
platforms with one instrument and the third with another.

---

## 3. Linux — PASS, twice, receipted

Driven on `hetzner-dsm` against the real release binary.

```
JOURNEY COMPLETE platform=linux receipt=/tmp/f24-run/linux-receipt.json
JOURNEY VERIFIED platform=linux commit=d89b81b6f4460d6c493552c3eb91eb0b8ad0eb56
  steps=17 submitted=12 arrived=12 unique=12 duplicates=0 losses=0
```

- **binary identity:** `wayland-core 0.12.25 (source d89b81b6…)`,
  sha256 `fd2ecccfdb801021…`, hashed by the verifier itself.
- **recovery was observed, not asserted.** `kill -9 3558201` → `kill -0` reports
  gone → the journey ran **no** start command → `systemctl --user show -p
  NRestarts` returned **1** and status reported a **different live pid**,
  `3575746`. The journey does not restart it by hand; that omission is the point.
- **delivery reconciliation:** 12 submitted, 12 arrived, 12 unique, 0 duplicates,
  0 losses, counted at the independent sink's own journal (13 lines total; the
  13th is the heartbeat job, excluded by body).
- **upgrade and rollback:** re-registered against
  `/tmp/f24-run/linux-upgraded-core` and the **running service** reported that
  `binary_path`; rolled back and it reported the original. This is the first time
  either clause of `24-C1` has been performed anywhere.
- **uninstall left the machine clean:** unit gone (`0 unit files listed`), no
  residual pid, final state `uninstalled` with a null pid.

Two independent clean runs, at `db87f8b6` and at `d89b81b6` — the second
confirming the Windows fix did not regress Linux.

Evidence on the Mac at `/tmp/f24-run/linux-{receipt.json,raw.txt,canary.txt,redacted.md}`.

---

## 4. Windows — RED, and the red was worth more than a green

Driven on `SeanD@seandesktop` as a scheduled task, so the run survives SSH
disconnect. The journey is a **single sequential process** — it is not `cargo
test` and there is no parallelism to serialise; no other work was run on the box
during it.

Three runs, each failing later than the last, each failure a real defect:

| Run | Reached | Failure |
|---|---|---|
| 1 | step 5 | `gateway install` → `schtasks … ERROR: Access is denied.` |
| 2 (elevated) | step 7 | `gateway status` reported `stopped, pid: null` for a gateway that was in the task list |
| 3 (after fix 1) | **step 10** | 12 submitted, **0 arrived** — `no value for credential handle "slack.f24j.bot_token"` |

### F24-J-H1 — HIGH — the Windows registration carried no home. FIXED, live-improved.

Confirmed at source and live. The launchd plist carries
`EnvironmentVariables/WAYLAND_HOME`; the systemd unit carries
`Environment=WAYLAND_HOME=`; **the schtasks registration carried nothing**, because
Task Scheduler has no mechanism for setting an environment variable on a task.
The service therefore ran against the default home while `gateway status` read
the home it was installed for — the F24-B-H1 misreport shape, one field over,
Windows only. Fixed by passing `gateway run --home <PATH>` in the registration
(an **argument**, not a `cmd /c "set …"` wrapper, which would interpolate an
operator-supplied path into a shell string). `ScopeArgs` gains `--home`. The new
test asserts the home reaches the registration for **all three** families —
Windows was the only family without such a test, which is why it was the only one
that lost it. **Live effect: steps 5–9 now pass where the run previously stopped
at 5, then 7.**

### F24-J-H2 — HIGH — `--home` was a narrower carrier than the env var. FIXED, NOT re-proven.

The run that H1 unblocked failed at step 10 with every delivery refused:
`no value for credential handle "slack.f24j.bot_token"`, `channel-health.json`
state `disconnected`, 12 submitted / 0 arrived. `--home` scoped the gateway's own
files but not `wcore_config::wayland_config_dir`, so the credentials store
resolved under `%APPDATA%\wayland-core` while the credentials file sat in the home
the task was registered for. `gateway run --home` now exports what the units
export, deferring to an already-set `WAYLAND_HOME` so a unit keeps authority over
a flag.

**This fix is landed at `cccdf14d` and has NOT been re-driven on Windows.** The
rebuild was still running when this lane ended. **Windows is therefore RED.** It
is not "green pending a rebuild"; it is red, with a named cause and a candidate
fix awaiting proof.

### A wrong conclusion I caught before reporting it

I first read H2 as *"Windows ignores `WAYLAND_HOME`"*, from
`cmd /c "set VAR=… && binary --config-path"` printing the config root under
`%APPDATA%`. That is the parse-time-expansion trap `AGENTS.md` already names —
`cmd` expands `%VAR%` before `set` runs, so the probe proved nothing. Re-measured
from a `.cmd` file, where the echo proves the value took effect:

```
VAR=[C:\f24-run\windows-home]
C:\f24-run\windows-home\config.toml
```

Windows honours `WAYLAND_HOME` correctly. Had that stood, it would have been a
confidently wrong HIGH against a shared config seam.

### F24-J-M1 — MEDIUM — `gateway install` on Windows requires elevation

`schtasks /create /sc onlogon` is denied to a non-elevated token; measured both
ways on the box (non-elevated → `Access is denied`, elevated → `SUCCESS`). The
module docs claim the Windows mechanism was chosen precisely because, like a
launch agent and a systemd user unit, it "does not require elevation". On the
measurement, it does. Not blocking; → BACKLOG.

---

## 5. macOS — NOT RUN, and why

No macOS journey ran. This is a budget-and-queue outcome, not an impossibility,
and the impossibility premise recorded at `23A-04-SUMMARY.md:40` was **not**
reused — the path was identified and set up, it simply did not deliver in time.

`ci.yml`'s `build` job uploads `wayland-core-aarch64-apple-darwin` on every push
with 14-day retention, expressly so a host that cannot compile can still be
driven. That is the correct source and it needs no Cargo on the Mac. What blocked
it: CI concurrency **cancels the older run when the branch is pushed again**, and
this lane had to re-pin the candidate four times (a thiserror compile error, a
test-only compile error, the status-parser fix, then each Windows fix). Every
push cancelled the run that would have produced the artifact, and the surviving
run sat `pending` for over an hour behind a busy runner pool.

**The correct next move is cheap:** the branch is now stable at `cccdf14d`. When
CI completes for it, `gh run download --name wayland-core-aarch64-apple-darwin`,
`chmod +x`, stage at `/tmp/f24-run/macos/`, and run
`node scripts/f24-journey.mjs --platform macos --run-dir /tmp/f24-run --binary /tmp/f24-run/macos/wayland-core`
on the Mac. `verify`/`scan`/`bind` are pure file operations and can run on the
Linux host against a copied binary, as `24-04-PLAN.md` already permits. The
driver **refuses** to run a `macos` journey on a non-macOS host, so the receipt
cannot be forged from Linux.

---

## 6. Gates, with real numbers and where each came from

| Gate | Result | Host |
|---|---|---|
| `node --test scripts/f24-journey.test.mjs` | **20 passed, 0 failed** | Mac |
| `cargo test -p wcore-eval-scenarios --lib journey::` | **18 passed, 0 failed**, 189 filtered | hetzner |
| `cargo test -p wcore-eval-scenarios --test journey_receipt_contract` | **21 passed, 0 failed, 0 filtered** | hetzner |
| `cargo fmt --all -- --check` | rc=0 | Mac |
| Linux journey | 17/17, `JOURNEY_RC=0` | hetzner |
| `wayland-journey verify` (linux) | rc=0, `duplicates=0 losses=0` | hetzner |
| Windows journey | **9/17, `JOURNEY_RC=1`** | seandesktop |

**One self-passing gate caught in my own work.** `cargo test -p
wcore-eval-scenarios --lib journey:: --test journey_receipt_contract` printed
`test result: ok` for the integration suite having run **0 of 21** — the filter
applied to both targets. Re-run unfiltered, it executed 21 with 0 filtered out.
The 21 above is the unfiltered number. No `cargo test` invocation in the journey
driver itself; the driver shells out to no test runner at all.

Not run: workspace-wide suites and clippy on either host. Contended shared hosts,
and my changes are additive; an integrator should run them post-merge.

---

## 7. Honest grades

**24-C5 — "Setup-to-recovery journeys pass on macOS, Linux, and Windows": NOT MET.**
One of three platforms passes. The instrument, the schema and the verifier now
exist and are tested, which the criterion previously had none of, but a criterion
naming three platforms is not met by one. **The criterion is not narrowed to the
platform that worked.**

**24-C1 — upgrade and rollback: PERFORMED for the first time, on Linux only.**
Both clauses were exercised against the running service and observed through
`binary_path` in the projection. The 12-of-12 clean tally is met on Linux
(`F24-C-M1` closed there). The macOS and Windows half of the clause is unchanged.

**Open, named:**
1. macOS journey never driven.
2. Windows red at step 10; `cccdf14d` is a candidate fix with no live proof.
3. `wayland-journey scan` and `bind` are unit-proved but were never run over three
   real receipts, because there is only one.
4. F24-J-M1 (Windows elevation) → BACKLOG.
5. The `unsafe { set_var }` in the H2 fix is sound where it sits (before any
   config read, before the gateway spawns work) but is a pattern worth a
   reviewer's eye.

## Self-check

Every number above was copied from captured tool output. The two Windows HIGHs
were reproduced live before being fixed and are recorded with the commands that
produced them. The wrong `WAYLAND_HOME` conclusion is recorded as it happened,
including that it was wrong. The gates that do **not** pass — Windows, macOS,
`scan`, `bind`, the workspace suites — are named as not passing rather than
sampled.
