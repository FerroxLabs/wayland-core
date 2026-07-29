---
phase: 24-gateway-automation-channels-typed-api
lane: f24-c3-h5-reload
branch: lane/f24-c3-h5-reload
merge-base: d622cb09de01329cef6f20d6f9183df171462daf
brief-premise: "FALSIFIED — F24-C3-H5 was already fixed and live-proven at HEAD before this lane started."
verdict: "F24-C3-H5 re-verified FIXED at HEAD, not by reading the prior summary but by observing `policies=1` in the gateway's own reload log on every live run. The lane's actual product of work is F24-C3-H6, a NEW HIGH with two facets found by the sibling sweep the brief asked for, both reproduced with a one-variable control and both fixed: (a) a successful `channel reload` ERASED the record of a dead inbound path, taking `channel health` from rc=1 to rc=0 while nothing about the dead path changed; (b) `ChannelManager::reload` STARTED poll tasks for a gateway that does not hold the single-owner inbound polling lease, and polling is a destructive read."
new-finding: "F24-C3-H6 (HIGH, two facets, both fixed in-lane). H6b is the more damaging of the two — silent data loss, not a misreport — and it is invisible to the obvious test shape."
fence-exposure: "zero — 0 bytes in crates/wcore-cli/src/{lib,main}.rs vs the captured merge-base SHA, with a known-positive control on the same command"
platforms: "Linux (hetzner-dsm) only. Nothing measured on macOS or Windows; see 'What I did NOT do'."
credentials: "none used, none required, none reached any host. Every secret handle in the fixtures is unresolvable by design."
---

# f24-c3-h5-reload — the brief's defect was already fixed; its adjacent question was not

**Read this first: my brief's central premise was false at HEAD**, and per LANE-BRIEF
§"Your brief's MEASUREMENTS are probably stale" that refutation is part of the deliverable
rather than an aside. The brief said `F24-C3-H5` was *"open and unfixed at HEAD"* and that my
job was *"the repair, not the discovery."* It was neither open nor unfixed.

What was genuinely open was the **adjacent question the brief also asked** — *"is the access
policy the only thing `reload` fails to reload?"* — and the answer is no. That question found a
new HIGH with two facets, one of which is worse than the finding I was sent to fix.

---

## 1. Which of the brief's claims held

| claim | verdict | evidence |
|---|---|---|
| `F24-C3-H5` is open and unfixed at HEAD | **FALSE** | `5d4bf4b9`, `44a7cc16`, `7c512fe2` are all ancestors of `d622cb09`, with a control proving the ancestry test was alive |
| It was measured with a one-variable control and not fixed | **HALF TRUE** | it *was* measured that way — and then fixed that way by `lane/24-h5`, whose `24-H5-SUMMARY.md` sits at HEAD with `status: complete` |
| The ledger row says it is open | **TRUE, and the row is stale** | the `2026-07-30` re-grade block (`CRITERIA-GAP-LEDGER.md:818-832`) reads the *finding* lane's `24-C3-FINISH.md`, which was accurate when written; the repair lane merged after |
| It is "the same family as the failure mode §3 ranks first" | **TRUE, and more so than the brief knew** | the same family had **two more live instances in the same 120-line code block**, both unfixed |

I did not manufacture work to match the falsified premise. I re-verified the existing fix, then
spent the lane on the sweep.

### Re-verification of F24-C3-H5 at HEAD (not taken on trust)

Every live run in this lane prints the gateway's own reload line, and on all of them it reads:

```
[gateway] channel reload: added=[] replaced=["f24h6"] removed=[] unchanged=[] policies=1
```

`policies=1` is the field the 24-h5 lane added and named as the one-field statement of its fix
(`policies=` absent is the defect). It is present on every run of the current binary, and the
`f24_c3_h5_reload_policies_test` integration test passes 1/0. **H5 is fixed at HEAD.**

---

## 2. The sweep: everything `channel reload` touches

| state | refreshed by reload? | verdict |
|---|---|---|
| adapter set (`ChannelManager::reload`) | yes — removes+stops, replaces on fingerprint | OK |
| `registered_n` | yes | OK |
| inbound access policy | yes (`reload_policies`) | FIXED by 24-h5 |
| tool posture | yes, in the same single swap | FIXED by 24-h5 |
| `config_fingerprint()` | **no production adapter overrides it.** The trait default (`wcore-channels/src/lib.rs:239`) returns `None`, which reload treats as CHANGED. So `unchanged` is always empty in production and every reload replaces every adapter. Deliberate, documented, and the fail-SAFE direction. | OK by design, named not changed |
| **`registration_error`** | **cleared unconditionally** | **F24-C3-H6a — FIXED HERE** |
| **the right to poll** | **taken unconditionally** | **F24-C3-H6b — FIXED HERE** |

---

## 3. F24-C3-H6a — a successful reload erased the record of a dead inbound path

**Root cause: `crates/wcore-cli/src/gateway.rs:1465` (at `d622cb09`)** — inside the reload
success branch, three lines above the 24-h5 fix:

```rust
registered_n = names;
registration_error = None;      // unconditional
```

`registration_error` is the **only** thing `channel health` fails on once
`registered >= configured` (`ChannelHealthReport::is_complete`, `channel.rs:183`; the `bail!`
at `channel.rs:445`). At startup it accumulated facts a reload does **not** re-evaluate:

- `gateway.rs:1256` — `inbound dispatch unavailable` — the process has **no inbound stack at all**
- `gateway.rs:1291` — `inbound polling owned by another process` — *"this gateway will send but not poll"*
- `gateway.rs:1302` — `start_all` failure

Reload re-runs only adapter registration, then wiped all three.

**Measured, one variable.** `channel health` rc=**1**, naming the lost lease. One `channel
reload` — documented, innocuous, the natural thing an operator tries — and `channel health`
rc=**0**, reporting complete, with the lock holder still alive and the path exactly as dead:

```
--- PRE-FIX, health after the reload ---
configured: 1   registered: 1
f24h6 (slack)
  state:      Disconnected
  reason:     start() failed: auth failed: no value for credential handle "slack.f24h6.bot_token"
  errors:     0
  reconnects: 0
                                          <- no error line, and rc=0
```

It is worse than not reporting: the operator running the recovery command is actively told the
degradation is gone.

### The fix, and the correction I had to make to my own first design

My first fix froze the boot-time lease string and restored it across the reload. **That was
wrong**, and reading further found why: `ChannelPollSupervisor` (`gateway.rs:1353`) **re-claims
the lease every tick** and wins as soon as the holder exits, exposing a live
`is_owner()` documented as *"Unlike the boot-time answer, this changes over the process's
life."* Freezing the boot value would have produced a **permanently-red** health surface —
LANE-BRIEF §3b-iii's inverse defect, no better than the false green it replaced.

So the single accumulated variable is now **three components, separated by who establishes them
and when**, joined by `compose_registration_error` at both publish sites:

| component | established | may a reload clear it? |
|---|---|---|
| `registration` | redone by every reload | **yes** — this is all the reload's `= None` can now reach |
| `inbound_absent` | once, at startup; nothing rebuilds the host | no |
| `not_polling` | **read live from the supervisor at every publish, never cached** | not applicable — it clears itself when the fact changes |

This also closes a **pre-existing bug independent of reload**: the boot-time
`inbound polling owned by another process` string was never recomputed, so it survived the
supervisor winning the lease and could never clear. The `health-CLEARS-once-the-lease-is-won-
back-with-no-reload` leg now proves it clears in ~1.5 s with no operator action.

---

## 4. F24-C3-H6b — reload took the right to poll, and this one loses data

Found by asking why my own `start_policy` change was needed, and **proved by an artifact I had
already captured** before I understood it. `ChannelManager::reload` ended in an unconditional
`let _ = self.start_all()`. The gateway's **startup** path gates `start_all` on
`poll_lease.is_owner()` (`gateway.rs:1297`) — and then reached `reload`, which started the poll
tasks regardless.

Polling is a **destructive read** (Telegram's `offset=` confirm deletes; IMAP sets `\Seen`), so
a second poller does not cause a duplicate — **the rightful owner sees nothing at all.**

The proof is one field, in the same two health documents as above:

| | before the reload | after the reload |
|---|---|---|
| pre-fix | `Unknown` / `registered; no poll observed yet` | **`Disconnected` / `start() failed: …`** |
| fixed | `Unknown` / `registered; no poll observed yet` | `Unknown` / `registered; no poll observed yet` |

`start()` was attempted on a process holding no lease. The startup path had correctly declined;
the reload overrode it.

**Fix:** `reload` takes a required `StartPolicy` argument with **no default and no `Default`
impl**, so the caller must state whether it may poll and *cannot omit the decision*. This is the
24-h5 lane's own design principle — make the half-fix inexpressible — applied to the sibling.
The gateway derives it from the **supervisor's live** ownership, not the boot lease, so a gateway
that has since won the lease does start polling and one that has not does not.

---

## 5. Controls in both directions, and the demonstration that the old shape misses H6b

**This is the part of the lane I would keep if I could keep only one paragraph.**

`scripts/f24-c3-h6-mutate.py` has three arms. The interesting one is **M2, the half-fix**: H6a
repaired, H6b left broken.

| arm | gateway unit suite | live driver |
|---|---|---|
| M1 "the bug" (lease component dropped) | **13 passed / 3 failed** | — |
| **M2 "the HALF-FIX"** | **16 passed / 0 failed, rc=0 — FULLY GREEN** | **6 / 1 — only the lease-theft leg** |
| M3 (H6b kept, H6a broken) | **12 passed / 4 failed** | — |
| unmutated | 16 / 0 | 7 / 0 |

On M2 the unit suite is green **and every exit-code leg of the live driver passes**, including
`health-STILL-fails-after-a-successful-reload` at rc=1. A driver that only read `channel
health`'s exit status — the obvious shape, **and the shape I first wrote** — grades a build that
still silently steals a destructive read as fully fixed. Only the leg added afterwards, which
reads the adapter's own post-reload state, catches it. The M1/M3 split additionally shows the two
facets are detected **independently** rather than one assertion covering both.

### Can the gates PASS as well as fail? (§3b-iii)

Yes, and it is asserted live, not argued: releasing the lease takes health from rc=1 to rc=0 in
~1.5 s with no reload. Two unit tests carry the same control
(`a_healthy_gateway_composes_no_error_at_all`,
`winning_the_lease_back_clears_the_lease_component_without_a_reload`).

### One leg that passes on the broken build, kept and labelled

`health-fails-while-the-lease-is-lost--OLD-SHAPE` passes on **every** binary including pre-fix.
It is a necessary control (it proves the health surface can fail at all) and on its own it is
worth nothing — so it is named `--OLD-SHAPE` in the output so no reader can mistake it for the
discriminating leg. Same for the pre-fix run's `health-CLEARS…` leg, which passes for the wrong
reason: that binary was already rc=0.

---

## 6. Instrument fault of my own, found by running and REPAIRED in-lane (§6b-ii)

**My driver manufactured a false red against a correct fix.** The can-it-pass leg released the
lease with `kill $!` — which kills `flock` but **not the `sleep` child that inherited the locked
descriptor.** The lock was never released, the product correctly went on reporting the
degradation, and the driver graded that as *"the surface is stuck red."*

I caught it only by refusing to accept the product verdict without checking the precondition:
`flock -n` reported the lock **still held**, and `fuser` named the surviving `sleep`.

Repaired, not merely noted:
- the holder runs under `setsid` and is killed as a **process group**;
- `release_lease` **proves the lock is free** before anything downstream is graded, and exits
  **2 INSTRUMENT-FAULT** — deliberately distinct from a product failure — if it is not;
- setup now asserts the lock is *actually held* as well as that the holder announced itself, so
  a run where the gateway would win the lease cannot be silently measured (§6a-i: an actor that
  never launched is a dead instrument).

`--selftest` carries the three assertions §6b-ii requires, and the third is the one that matters:

```
PASS  1-known-positive-holder-takes-the-lock — lock reads as held (pgid 1733388)
PASS  2-known-negative-repaired-release-frees-it — lock reads as free after a group kill
PASS  3-the-old-release-would-have-missed-it — old kill left the lock HELD (…/l2: 1733402) — this is what graded a correct fix as red
selftest: 3 passed / 0 failed
```

I also cleaned up the stray `sleep 900` holders my broken release left on the host.

### A second instrument trap, in the product's own test output

`cargo test -p wcore-cli --lib` prints a `test result: FAILED`, a `failures:` block and
`error: test failed` that are **not this crate's**: `plugin::scaffold::tests::
plugin_test_propagates_a_failing_suite` (which PASSES) runs a **nested cargo** over a scaffolded
crate containing a deliberate `always_fails`. On this crate any matcher keyed on `^test result:`,
`FAILED` or `^error` yields a **false red against a passing suite**. Resolved by taking the outer
process's exit code (rc=0) and reading the failing test's own name and path back
(`src/lib.rs:2:21` does not exist in this workspace). Real result: **1897 passed / 0 failed /
1 ignored**. Recorded in `gates.txt` for the next lane.

---

## 7. Gates — every count from an unproxied `/root/.cargo/bin/cargo`

Full output in `f24-c3-h5-reload-evidence/gates.txt`. The `0 ignored; 0 filtered out` fields
survive in every line, which is itself the evidence the `rtk` proxy was bypassed (§3b).

| gate | result |
|---|---|
| `cargo test -p wcore-cli --lib gateway::tests` | **16 passed / 0 failed / 0 ignored / 1882 filtered out** |
| `cargo test -p wcore-channels --test framework_matrix` | **19 passed / 0 failed / 0 ignored / 0 filtered out** |
| `cargo test -p wcore-channels` (whole crate) | **139 executed / 0 failed** |
| `cargo test -p wcore-agent --test f24_c3_h5_reload_policies_test` | **1 passed / 0 failed** (H5 still holds) |
| `cargo test -p wcore-agent --lib channel_policy::` | **5 passed / 0 failed / 2232 filtered out** |
| `cargo test -p wcore-agent --lib channel_dispatch::tests` | **10 passed / 0 failed / 2227 filtered out** |
| `cargo test -p wcore-cli --lib` | **1897 passed / 0 failed / 1 ignored**, outer rc=0 |
| **`cargo check --workspace --all-targets`** | **rc=0, 0 errors** (workspace-wide, never `-p`) |
| `cargo clippy -p wcore-channels -p wcore-cli --all-targets` | rc=0, clean but for the base `imap-proto` note |
| `cargo fmt --all -- --check` | rc=0 |
| live driver, **pre-fix** binary | **5 passed / 2 FAILED** |
| live driver, **M2 half-fix** | **6 passed / 1 FAILED** |
| live driver, **fixed** binary | **7 passed / 0 failed** |
| driver `--selftest` | **3 passed / 0 failed** |

§6 fence: `git diff d622cb09 --stat -- crates/wcore-cli/src/{lib,main}.rs` is **EMPTY**, against
the captured SHA and never the branch name — with a known-positive on the same command (6 files,
898 insertions) proving it was alive.

---

## 8. Files

**Product (3):**
- `crates/wcore-channels/src/manager.rs` — `reload` takes `StartPolicy`; the unconditional
  `start_all` is gone.
- `crates/wcore-channels/src/lib.rs` — one-word re-export of `StartPolicy`.
- `crates/wcore-cli/src/gateway.rs` — three separated degradation components,
  `compose_registration_error`, live supervisor reads at both publish sites, lease-gated
  `StartPolicy` at the reload call.

**Tests / instruments (3):** `crates/wcore-channels/tests/framework_matrix.rs` (+2 tests, both
directions; 4 existing call sites updated to preserve their previous behaviour exactly),
`crates/wcore-cli/src/gateway.rs` `mod tests` (+5), `scripts/f24-c3-h6-reload-clears-error.sh`
(NEW, 7 legs + `--selftest`), `scripts/f24-c3-h6-mutate.py` (NEW, 3 arms, restores in `finally:`).

---

## 9. For the orchestrator to serialize

**Nothing in the fence, no protocol seam, no contract fixture, no `Cargo.toml` change.**

One cross-lane note: **`ChannelManager::reload` changed signature** — it now takes a second
`StartPolicy` argument. There was exactly **one** production caller (`gateway.rs`) and four in
`framework_matrix.rs`; all five are updated here. A lane that adds a caller must state a policy,
which is the intent — there is deliberately no default to fall back on.

`.planning/CRITERIA-GAP-LEDGER.md`'s `24-C3` row is **stale in two ways** and I did not edit it
(it is a shared file and other lanes are re-grading concurrently): `F24-C3-H5` is described as
open and unfixed when it is fixed at HEAD, and `F24-C3-H6` is not in it at all. Recommend the
ledger lane fold in both.

---

## 10. What I did NOT do

- **Did not measure anything on macOS or Windows.** Every figure here is Linux on
  `hetzner-dsm`. The defect is in platform-independent Rust and the lease is `flock`-based, so I
  expect it to hold on all three — but **I did not measure that and do not claim it.** The
  Windows path in particular uses a mandatory rather than advisory lock, which is exactly the
  kind of difference that deserves a real run rather than an inference.
- **Did not use the §0 Darwin exception** — nothing here is Darwin-only behaviour.
- **Did not move `24-C3` to MET and do not claim it.** `media` and `native actions` remain at
  zero evidence on every adapter, and the `reconnect` half of reconnect/reload is still
  untouched. This lane closed one HIGH with two facets; it did not deliver the criterion.
- **Did not fix, and name rather than bury:**
  - **Every `channel reload` replaces every running adapter**, because no production adapter
    overrides `config_fingerprint()`, so `unchanged` is always empty and the buffered-state
    preservation `reload` is carefully written to provide never actually happens. Fail-safe and
    deliberate, but the documented benefit is not being realised. MEDIUM → BACKLOG.
  - **A per-channel `Disconnected` row does not fail `channel health`.** In the pre-fix capture
    the surface printed `start() failed: auth failed` and still exited 0. `gateway.rs:1151`
    argues this is intentional ("a Disconnected row in health" being better than refusing to
    boot), and I did not overturn a deliberate decision on my own authority — but "health exits
    0 while naming an auth failure" is the same *family* as this lane's finding and deserves an
    explicit ruling. MEDIUM → BACKLOG.
  - **`start_all`'s failure at startup is still cleared by a reload.** `ChannelManager::reload`
    re-attempts `start_all` internally but discards the result, so a reload neither establishes
    success nor preserves the failure. I left this in the `registration` component (cleared by
    reload) because the per-channel health rows do carry start failures; making it honest needs
    `reload` to surface start errors, which is a wider change than this finding warrants.
    MEDIUM → BACKLOG.
- **Did not weaken, ignore, delete, re-gate or re-time a single test.** No `#[ignore]`, no
  `#[allow]`, no raised timeout. The two reds reported above are reported red, with output.
- **Did not merge, open a PR, tag, release, close an issue, or run `wcore-contract generate`.**
- **Did not `git rebase`, `git reset --hard`, `git stash`, `git clean`, or `git add -A`.**
- **Did not edit `.planning/CRITERIA-GAP-LEDGER.md`, `.planning/BACKLOG.md`, or any shared
  fixture**, nor `crates/wcore-cli/src/{lib,main}.rs`.
- **Did not run a full-workspace test run** — targeted `-p` suites plus the required
  workspace-wide `cargo check --all-targets`, per the disk/contention rule.
- **Did not use or require any credential.** The fixture's secret handles are deliberately
  unresolvable; `start()` failing on them is part of the pre-fix evidence.

## 11. Housekeeping

hetzner worktree `/root/wayland-f24-c3-h5` (branch `hz/f24-c3-h5-reload`) and its `target/` are
still present at the time of writing — see the final report for disposition. Disk was 519G free
after all runs. Stray `sleep 900` lock holders left by my broken release were cleaned up and
verified gone.
