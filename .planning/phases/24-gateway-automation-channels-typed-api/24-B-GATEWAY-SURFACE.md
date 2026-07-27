# F24-B Gateway Operator Surface

Phase 24, lane 24b (successor to the lane that landed 24-01 and 24-02). The
recorded contract for `crates/wcore-cli/src/gateway.rs`, the cross-audited
decision that shaped it, the four defects the live run found, and the exact
bound of what the Linux evidence proves.

Lane branch `lane/24b`, based on `8379f888`.
Linux host: `hetzner-dsm`, worktree `/root/wayland-24b`.
macOS: this Mac, artifact-only — see §6.

---

## 1. Why this file exists at all

It is not a new feature. It is the fix for a defect that was already merged.

`wcore-gateway::service` generates the native service unit for all three OS
families, and every one of them invokes `<binary> gateway run`:

```
launchd    ProgramArguments = ["{binary}", "gateway", "run"]
systemd    ExecStart={binary} gateway run
schtasks   /tr "\"{binary}\" gateway run"
```

There was no `gateway` subcommand. An operator who ran the install path
therefore registered a launch agent, a systemd user unit or a scheduled task
whose command fails immediately with a clap "unrecognized subcommand" error.
**The registration succeeded and the service never ran**, on every platform,
silently.

Two files in the crate name this path explicitly — `wcore-gateway/src/lib.rs`
line 11 and `wcore-gateway/src/lifecycle.rs` line 6 — so the placement was
recorded before the file was written. `crates/wcore-cli/Cargo.toml` line 115
already carried the `wcore-gateway` dependency, so no manifest and no
lockfile edit was needed.

## 2. The decision — 4/4, `journey-minimal`

Full panel record in `/tmp/f24b-run/gateway-decision/`:
`panel-question.txt` (131 lines, one identical bundle to every member),
`panel-codex.txt` (12,435 B), `panel-gemini.txt` (19,953 B),
`panel-kimi.txt` (3,597 B), `panel-internal.txt` (3,265 B),
`decision-vote-tally.txt`, `decision-chosen.txt`, `decision-rationale.txt`,
`decision-dissent.txt`.

```
member=codex    pick=journey-minimal
member=gemini   pick=journey-minimal
member=kimi     pick=journey-minimal
member=internal pick=journey-minimal
```

Unanimous, so no `MINORITY-POSITION-ADOPTED` and no `EVIDENTIARY-TIEBREAK`
applies. Four options were put: `full-nine-verbs`, `journey-minimal`,
`separate-binary`, `do-not-create`.

**Chosen: `journey-minimal`.** Build `run` plus install, uninstall, start,
stop, restart, status and drain. Defer `doctor` and `logs` from the 24-01
nine-verb contract as a named gap.

`do-not-create` was rejected because it leaves a known-broken install path
with no operator surface for Criterion 1. `separate-binary` was rejected on
three independent grounds, any one sufficient: it contradicts the crate's own
documented placement; it would require editing `service.rs` on all three
families to change the registered command; and a `[[bin]]` in `wcore-gateway`
needs `clap`, which is the Cargo.toml + Cargo.lock edit this lane is
forbidden to make.

### The condition the internal pass attached, and it was binding

The internal adversarial pass concurred but recorded that the saving is only
real if `run` is a real runtime — seven thin verbs over a hollow sleep loop
would reproduce Phase 20A's defect in a new costume. Its condition:

> `run` must acquire the pid lock, open the DeliveryLedger, host a real
> AutomationPlane and honour the DrainController, and the claim must rest on
> a live count across an ungraceful kill. If that count is not obtained, the
> honest report is that the verb surface exists and Criterion 1 remains open.

**§5 reports against that condition explicitly, including the half of it that
was not met.**

### Dissent, in its own terms

`full-nine-verbs`. The contract said nine and the code carries eight. A reader
who finds the contract before this document will believe `gateway doctor` and
`gateway logs` exist. `logs` in particular was dismissed by the three external
members as "diagnostics, not lifecycle", and that is not quite right: the only
in-product evidence of an UNATTENDED relaunch is what the relaunched process
wrote, so a journey without a `logs` verb reads the log file directly and
proves a file exists on disk rather than proving the product can show an
operator that its service restarted it. Phase 24 closes that clause on a
narrower claim than the criterion reads. `doctor` has the weaker case: the
profile-isolation clause is already observable through `status --json`.

`separate-binary`. Its one genuine merit, recorded: it is the only option
under which the two shared fenced files are never opened. Five lanes edit
those files concurrently and every additive edit is a merge hazard. That is
why the edit made here is one `pub mod` line plus two blocks adjacent to the
existing `Backend` entries, and nothing else.

## 3. The verb set

| Verb | Drives | Authority |
|---|---|---|
| `install` | writes `unit_text` to `unit_path`, then `install_argv` | `service::ServiceManager` |
| `uninstall` | `uninstall_argv`, then removes the unit | `service::ServiceManager` |
| `start` | `start_argv` | `service::ServiceManager` |
| `stop` | `stop_argv` | `service::ServiceManager` |
| `restart` | `stop` (non-strict) then `start` | — |
| `status` | pid record + published projection | `lifecycle::StatusProjection` |
| `drain` | writes the drain request, then observes | `drain::DrainController` |
| `run` | pid lock + ledger + AutomationPlane + drain | all of the above |

**RECORDED GAP: `doctor` and `logs` are not implemented.** A test
(`the_seven_lifecycle_verbs_are_all_present`) asserts their ABSENCE, so adding
either forces this document and the module header to be updated with it rather
than silently diverging again.

### Design points that are load-bearing

- **`run` defaults to FOREGROUND.** Every service manager supervises the child
  it launched; a `run` that forked and returned would make
  launchd/systemd/schtasks believe the gateway had exited immediately and
  restart it forever. `--detach` exists for an operator starting one by hand.
- **The status file is separate from the pid record.** `gateway.pid` is written
  once at acquisition and is the IDENTITY; `gateway-status.json` is rewritten
  every tick and is the STATE. `status` checks liveness first and the pid
  decides — a crashed gateway leaves both on disk, and believing the projection
  would report `running` for a process that is gone.
- **Drain is a request file, not a signal.** SIGTERM already means "stop" on
  both Unix families and Windows has no equivalent; overloading one mechanism
  with two meanings would make a drain indistinguishable from a stop in exactly
  the case where the difference matters.
- **A drain request left by a previous run is removed, not honoured.** It was
  addressed to a process that is gone; honouring it would make every start
  after one drain drain itself immediately.

## 4. The four defects the live run found

None of these was found by review, and none by the unit suite. All four came
out of running the real binary against a real service manager.

| ID | Severity | What | Status |
|---|---|---|---|
| F24-B-H1 | **HIGH** | No unit passed `--profile`, so the runtime resolved `default` from the environment while the registration was named for the operator's profile. `gateway status --profile f24b` printed `profile: default`. | **FIXED** |
| F24-B-H2 | **HIGH** | `status` answered "is it registered?" with `systemctl --user is-active`, which answers ACTIVITY. During the five seconds systemd spent restarting after a hard kill, and again after a drain, it reported `Uninstalled` for a unit on disk and enabled. | **FIXED** |
| F24-B-H3 | **HIGH** | The runtime published nothing between accepting a drain request and finishing, so the projection stayed `Running` for the whole budget. The drain contract is that the counts are OBSERVABLE. | **FIXED** |
| F24-B-H4 | **HIGH** | `DrainController::drain`'s injected clock is documented as returning TOTAL elapsed ms; the closure returned the per-iteration increment, so `elapsed` was pinned at 100 and the loop could only exit through "nothing pending". **With carried work, `gateway drain` hung in `Draining` indefinitely.** | **FIXED** |

F24-B-H1 is a Criterion 1 clause failing directly: the criterion names
*profile isolation*, and a status verb contradicting the service identity it
was asked about is that clause not holding.

**F24-B-H4 is the most instructive.** It passed the FIRST live journey,
because that gateway had zero pending deliveries and the loop broke on its
first observation. It is only reachable with real carried work. A green suite,
a green clippy and a green live journey all missed it; the second journey —
the one that seeded actual unsettled deliveries — caught it in one run. This
is the fifth entry for the standing self-passing list and a new shape: **a
live test whose scenario was too clean to reach the defect.**

### Gates proved able to go red — by measurement

Seven mutations, each reverted immediately afterwards, `git diff --stat` clean
at the end of each block:

| Mutation | Result |
|---|---|
| Delete the liveness check in `read_live_projection` | 1 FAILED (`a_status_file_from_a_dead_process_is_not_reported_as_running`) |
| Delete `proj.pid = Some(record.pid)` | 1 FAILED (`a_live_projection_takes_its_identity_from_the_record`) |
| Rename the `run` verb to `serve` (restores the original defect) | 2 FAILED (`every_generated_unit_invokes_the_verb_this_module_implements`, `the_seven_lifecycle_verbs_are_all_present`) |
| Drop `--profile` from the systemd unit only | 1 FAILED (`every_family_passes_the_profile_to_the_runtime_it_registers`) |
| Pass the RAW profile instead of the sanitised one | 1 FAILED (same test, at the sanitisation assertion) |
| Put `is_registered` back on the activity query | 1 FAILED (`an_installed_but_stopped_service_is_not_reported_uninstalled`) |
| Return the increment from the drain clock (restores F24-B-H4) | 1 FAILED (`the_drain_clock_reports_total_elapsed_not_the_increment`) |

Each mutation reddened exactly the intended test and nothing else.

## 5. LIVE EVIDENCE — Linux, the real shipped binary

Host `hetzner-dsm`, `wayland-core 0.12.25` release build,
`systemctl --user` as the real service manager, throwaway home
`/tmp/f24b-run/home`, profile `f24b`.

```
$ wayland-core gateway status --profile f24b          # before install
gateway: Uninstalled

$ wayland-core gateway install --profile f24b
wrote unit: /root/.config/systemd/user/wayland-core-gateway-f24b.service
gateway installed (systemd): wayland-core-gateway-f24b

ExecStart=/root/wayland-24b/target/release/wayland-core gateway run --profile f24b
systemctl --user is-enabled → enabled

$ wayland-core gateway start --profile f24b
MainPID=1006128  ActiveState=active  SubState=running

$ wayland-core gateway status --profile f24b --json
{ "state": "running", "pid": 1006128, "uptime_secs": 4, "profile": "f24b",
  "turns_in_flight": 0, "deliveries_pending": 12,
  "binary_path": "/root/wayland-24b/target/release/wayland-core",
  "binary_version": "0.12.25" }
```

Automation added through the shipped binary, two trigger types:

```
$ wayland-core cron add --trigger every:5 --channel f24bsink --text "f24b delivery A"
$ wayland-core cron add --trigger "cron:*/1 * * * *" --channel f24bsink --text "f24b delivery B"
on  dadaf8c6…  [interval  ] @every 5s      channel f24bsink :: f24b delivery A
on  aa5e3eaf…  [cron      ] */1 * * * *    channel f24bsink :: f24b delivery B
```

### The hard kill and the platform's own recovery

```
$ kill -9 1006128
systemd: Main process exited, code=killed, status=9/KILL
systemd: Failed with result 'signal'.
systemd: Scheduled restart job, restart counter is at 1.

RECOVERED after 5s: NewPID=1011091 (was 1006128)   NRestarts=1
```

**Nothing in the run restarted it.** The journey script polls; systemd's
`Restart=on-failure` performed the restart, and the restart counter is the
platform's own record of it.

During the restart window, `gateway status` printed `Stopped` — not
`Uninstalled` (F24-B-H2 fixed) and not a stale pid.

### Delivery continuity across the kill

Twelve unsettled deliveries were seeded into the ledger journal before start,
standing for work a previous process left. The two processes' own startup
lines:

```
[gateway] started pid=1006128 role=Owner profile=f24b carried=12 (unattempted 12 / unknown-outcome 0) quarantined=0
                      ← kill -9, no drain →
[gateway] started pid=1011091 role=Owner profile=f24b carried=12 (unattempted 12 → unknown-outcome 12) quarantined=0
```

Read out-of-process from `deliveries.jsonl` after recovery:

```
journal lines      36
distinct ids       12
SEEDED_SUBMITTED   12
SEEDED_CARRIED     12
STILL_ACCEPTED      0
state histogram    {'attempted': 12}
```

All twelve survived the ungraceful kill, none was duplicated by identity, and
the state moved from *certainly not delivered* to *outcome unknown* — which is
the four-state machine's whole point.

### Drain

```
$ wayland-core gateway drain --profile f24b --budget-ms 5000
drain requested (budget 5000ms); gateway pid 1294304
  Draining (deliveries pending 12)
drain complete: Drained (deliveries pending 0)
[rc=0]

runtime: [gateway] drain Forced: observations=51 abandoned=12 flushed=true
```

Fifty-one observations over the 5000 ms budget, correctly `Forced` because
nothing could settle those twelve, and **all twelve named as abandoned and
recorded durably** rather than lost. `status` afterwards: `Stopped`.

### Uninstall

```
unit file:        removed
is-enabled:       not-found
residual process: (none)
final status:     gateway: Uninstalled
```

### WHAT THIS DOES NOT PROVE — stated plainly

**There is no independent-sink arrival count, and the internal pass's
condition is therefore only HALF met.**

The twelve deliveries were seeded into and read back from the gateway's own
ledger journal. The read was out-of-process, which rules out a runtime
reporting its own in-memory state, but the ledger is still the gateway's own
record — it is not an independent sink in 24-04's sense, and nothing arrived
at a destination. `f24bsink` is not a registered channel, so no dispatch
reached anything.

A real arrival count needs the hermetic fixture endpoint that 24-03 Task 3
owns and that was not built (§7). Per the condition recorded in §2, the honest
grading is therefore: **the verb surface exists and is live-proven; the
delivery-arrival half of Criterion 1 remains OPEN.**

## 6. macOS — obtainable in general, NOT obtainable for THIS code

The coordinator's correction is confirmed by measurement: a current macOS
binary IS downloadable from CI artifacts without Cargo on the Mac.

```
$ gh api .../artifacts/8640601998/zip > a.zip && unzip -oq a.zip
$ ./wayland-core --build-info
wayland-core 0.12.25 (source 0e7e3c43202e70e748702a59f6ba86d11f02be64)
$ file wayland-core
Mach-O 64-bit executable arm64
```

That artifact is from today and is not expired. **It does not carry this
lane's code**, proved with a discriminating probe rather than assumed:

```
$ ./wayland-core cron --help            # control: a verb that exists
v0.8.1 U7: manage scheduled cron jobs …
Usage: wayland-core cron <COMMAND>

$ ./wayland-core --help | grep -cE "^\s+gateway"
0
```

The control prints real help; `gateway` is absent from the subcommand list.

**The blocker is a CI trigger, and it is exact.** `.github/workflows/ci.yml`
fires only on `pull_request → main`, `push → main`, and
`push → plan/f20-unified-audit-repair`. `lane/24b` is not in that list, and
the file records that `workflow_dispatch` was considered and rejected because
GitHub only exposes it for workflows already on the default branch. So there
is no route to a macOS binary carrying `gateway run` that this lane may take:
adding the branch edits a shared CI file five lanes depend on, and opening a
PR is reserved to Sean.

Filed as a seam request (§8). It is a one-line change and it unblocks the
macOS rows of both 24-03 and 24-04, not just this deliverable.

## 7. Verification summary

| Gate | Result |
|---|---|
| `cargo test -p wcore-gateway` | **45 passed, 0 failed** (21 unit + 7 ledger + 9 lifecycle + 8 pidlock) |
| `cargo test -p wcore-cli --lib gateway::` | **9 passed, 0 failed** |
| `cargo nextest run -p wcore-gateway -p wcore-cli -p wcore-cron -p wcore-channels --no-fail-fast` | **2283 tests run: 2283 passed, 9 skipped** (1 flaky, pre-existing) |
| `cargo clippy -p wcore-gateway -p wcore-cli --all-targets -- -D warnings` | **clean** (exit status captured directly, not through a pipe) |
| `cargo fmt --all -- --check` (macOS, the one permitted Cargo command there) | clean |
| Live Linux journey: install → start → status → kill -9 → platform recovery → drain → uninstall | **PASS**, transcripts in §5 |
| Independent-sink arrival count | **NOT RUN** — needs 24-03's fixture endpoint |
| macOS live journey | **NOT RUN** — §6, CI trigger |
| Windows live journey | **NOT RUN** — belongs to 24-04 |

## 8. Seam request — one line, unblocks three deliverables

**File:** `.github/workflows/ci.yml`
**Insertion point:** the `on.push.branches` list, after
`- plan/f20-unified-audit-repair`
**Exact text:**

```yaml
      - lane/24b
```

**Why:** without it no CI run fires for this branch, so no macOS or Windows
artifact carrying this lane's code can exist, and the macOS rows of 24-03 and
24-04 stay unobtainable for any lane-local change. The precedent is in the
file's own comment block: `plan/f20-unified-audit-repair` was added for
exactly this reason, and marked transient.

**NOT APPLIED HERE.** `ci.yml` is shared by five concurrently running lanes
and is not in this lane's declared files. The orchestrator should serialise
it — ideally once, for every lane branch at the same time, rather than five
separate edits to one file.

**What breaks if omitted:** macOS and Windows evidence for anything built in a
lane branch remains impossible, and every lane will independently rediscover
this and report it as a platform impossibility.
