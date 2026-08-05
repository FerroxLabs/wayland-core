---
phase: 24-gateway-automation-channels-typed-api
finding: F24-C3-H5
lane: 24-h5
branch: lane/24-h5
merge-base: d34b2fe119916b7e35ad47d28783634955d75664
status: complete
verdict: "FIXED and live-proven, both facets. Same driver, same harness commit, same config generator: the pre-fix binary fails 3 legs, the fixed binary passes 11/11. The reloaded channel now admits its allowlisted sender, runs under the Workspace posture its config asked for (identical to the startup lifecycle), and still denies a sender outside its allowlist."
fence-exposure: "zero — 0 bytes changed in crates/wcore-cli/src/{lib,main}.rs vs the merge-base SHA"
new-finding: "F24-C3-H5b (MEDIUM, fixed in-lane) — the adapter loader SKIPS a malformed channel *.toml while the policy loader hard-errors to an EMPTY set, so a naive reload would have revoked every running channel's policy over one unrelated typo."
---

# 24-H5 — a reloaded channel is healthy, returns 200, **and now actually receives**

**Verdict: F24-C3-H5 is FIXED, both facets, and proven live on the real binary.** The trap the
finding lane named explicitly — that repairing only the access policy passes a naive re-run
while the channel silently runs under the wrong tool posture — was closed structurally rather
than by discipline, and the acceptance test asserts the posture rather than merely the arrival.

Nothing merged, no PR, nothing tagged, no issue closed, no credential used or required.

---

## 1. The one-variable result

Same driver, same harness commit (`c9002ffc`), same shared config generator, same fixtures,
same host, same minted secrets. **The only variable is the binary.**

| run | binary | verdict | legs | arrivals | turns | arrivals bytes | instrument_fault |
|---|---|---|---|---|---|---|---|
| control (`--all-at-start`) | fixed | **PASS** rc=0 | **6/6** | 2 | 2 | 540 | false |
| **reload** | **fixed** | **PASS** rc=0 | **11/11** | 3 | 3 | 808 | false |
| **reload** | **pre-fix (`d34b2fe1`)** | **FAIL** rc=1 | **8/11** | 2 | 2 | 536 | false |

### The three legs that separate the two binaries

| leg | pre-fix | fixed |
|---|---|---|
| `reloaded-adapter-actually-carries-inbound` | **FAIL** — `http=200 accepted=true`, arrivals `1→1`, `tier=absent` after **90 000 ms** | **PASS** — arrivals `1→2`, `tier=exact` after 3 000 ms |
| `reloaded-adapter-runs-under-its-configured-posture` | **FAIL** — `observed_posture=null` | **PASS** — `observed_posture=Workspace` |
| `reloaded-posture-equals-startup-posture` | **FAIL** — `reload=null` vs `startup=Workspace` | **PASS** — `Workspace == Workspace` |

The gateway's own log, the same field on both runs:

```
fixed    [gateway] channel reload: added=["f24finthree"] replaced=[...] removed=[] unchanged=[] policies=3
pre-fix  [gateway] channel reload: added=["f24finthree"] replaced=[...] removed=[] unchanged=[]
```

`policies=3` is new. Its absence is the defect, stated in one field.

### The universal-denial trap, caught in the act

**`reloaded-adapter-still-denies-a-non-allowlisted-sender` PASSES on the PRE-FIX binary.** Of
course it does — the pre-fix binary denies *everything*. That leg is real and necessary (a
repair that opened the policy would fail it), but on its own it is worth nothing, and the
evidence table above shows exactly why: a lane that had written only the denial leg would have
graded the broken build green.

Every zero in this lane is therefore paired with a live positive control inside the pass
condition, and the driver force-FAILs any run whose total arrivals are zero.

---

## 2. What was wrong, and what the repair is

Root cause was handed over six call sites deep and I did not re-derive it — I verified each one
at the base commit and then designed around the part the finding lane flagged as the real risk.

**Facet 1 — the access policy.** `channel_inbound.rs` moved an owned `HashMap` into the spawned
subscriber task (`let policies = self.policies;`). Nothing outside could reach it again. A
channel added by `channel reload` was absent from a map captured before it existed, fell through
to `InboundPolicy::default()` — `dm = allowlist` over an EMPTY allowlist — and was denied.

**Facet 2 — the tool posture.** `channel_inbound_host.rs` built a second map by the same code
path and moved it into `ChannelTurnDispatcher`. It went stale identically.

**Fixing only facet 1 is worse than the bug.** Messages start arriving, so the obvious test goes
green, while the channel runs under the dispatcher's `Conversational` fallback instead of the
posture its config asked for. The bug stops being fail-closed and becomes
silently-wrong-permissions, and the green hides it.

### The design answer: make the half-fix inexpressible

Both maps now live in **one** `ChannelPolicyRegistry` (`crates/wcore-agent/src/channel_policy.rs`):

- one `std::sync::RwLock` over a snapshot holding **both** maps plus a generation counter;
- **one** derivation function, `ChannelPolicySnapshot::from_configs` — the derivation used to be
  open-coded twice (`channel_inbound_host.rs` and `bootstrap.rs`), which is precisely what made
  the two facets separately forgettable;
- **one** swap, `replace()`, under a single write lock. There is deliberately no
  `replace_policies`, no `policies_mut`, and no single-facet API. **A caller that wanted to
  refresh one facet cannot express it.**

`std::sync::RwLock` rather than tokio's: both read sites are bounded map lookups that clone out
and are never held across an `await` — the discipline `channel_inbound.rs` already applies to
its `Arc<StdMutex<AutoReplyRateLimiter>>`. A poisoned lock is recovered rather than propagated;
taking the inbound path down for the process lifetime would be strictly worse than serving the
last good snapshot of two wholesale-replaced maps.

`gateway.rs`'s reload block now calls `InboundHost::reload_policies()`. Nothing is torn down —
subscriber and webhook host hold the same `Arc`, so the swap is visible on the next inbound
event.

---

## 3. NEW, MEDIUM, found while wiring and fixed in-lane — F24-C3-H5b

Wiring the reload exposed a second route to the same defect, which I would have **introduced**
by fixing the first naively. The two loaders over `<home>/channels` disagree about a malformed
file:

- `wcore_channels_registry::auto_register_from_dir` **SKIPS** it (warn + `continue`) and returns
  `Ok` with the remaining adapters registered;
- `ChannelConfigLoader::load_all` **stops at the first failure** and returns `Err`, which
  `load_channel_policy_configs`'s `unwrap_or_default()` converts into an **EMPTY** Vec.

At startup that is merely visible (`policies=0` in the gateway log). At **reload** it would be
destructive: one newly typo'd file would swap every running channel's policy out for the
fail-closed default and turn a working gateway into universal denial — the same defect this lane
is repairing, arriving from the other direction, and shipped by the repair itself.

So `reload_policies` returns `Result` and **refuses to swap on a load error, keeping the
policies already in effect**; the gateway prints `policies=KEPT-STALE (<err>)` and sets
`registration_error` naming the file. The startup path keeps its historical lossy behaviour via
the unchanged `load_channel_policy_configs`, so no existing deployment changes how it boots.

Covered by legs 5 and 6 of the integration test: a malformed file must produce an error naming
the file, must NOT bump the generation, must leave both channels' allowlists intact — and
removing the file must let the next reload through, so the refusal is a refusal and not a wedge.

---

## 4. Gates, with the numbers read back

Executed counts parsed from `test result:` lines. Exit status is recorded but never trusted
alone — `cargo test` exits 0 having run zero tests when a filter matches no name, and every
targeted run below prints its `N filtered out` alongside `N passed`.

| gate | result |
|---|---|
| `cargo test -p wcore-agent --lib channel_policy::` | **5 passed / 0 failed** (2168 filtered out) |
| `cargo test -p wcore-agent --lib channel_inbound::tests::a_registry` | **2 passed / 0 failed** |
| `cargo test -p wcore-agent --lib channel_dispatch::tests` | **10 passed / 0 failed** |
| `cargo test -p wcore-agent --test f24_c3_h5_reload_policies_test` | **1 passed / 0 failed** (0 filtered) |
| `cargo test -p wcore-agent --lib` (serial) | **2170 passed / 0 failed / 3 ignored**, rc=0, 139.9 s |
| `cargo clippy -p wcore-agent -p wcore-cli --all-targets` | clean (only the pre-existing `imap-proto` future-incompat note, present at base) |
| `cargo fmt --all -- --check` | clean |
| `f24-c3-clauses-selftest.mjs` | **32 passed / 0 failed** (was 20 before this lane) |
| live control `--all-at-start` | **PASS rc=0, 6/6** |
| live reload, fixed binary | **PASS rc=0, 11/11** |
| live reload, PRE-FIX binary | **FAIL rc=1, 8/11** |

### The parallel run, reported honestly

`cargo test -p wcore-agent --lib` run with default parallelism showed **2155 passed / 15
failed**. All 15 are journal- and lease-contention (`session journal writer lease is already
held`), none touch anything this lane changed. Re-run **serially at the same commit: 2170
passed / 0 failed**. Both figures are stated; the serial one is the measurement.

---

## 5. Can the gates fail? Measured, two ways

### 5a. Mutation of the product (`f24-h5-mutate.py`, committed)

Counts below are per the harness's own filters, so the "unmutated" column is the same filter as
the mutated ones (the dispatcher row is `channel_dispatch::tests::a_`, the two posture tests —
not the 10-test `channel_dispatch::tests` reported in §4).

| suite | M1 "the bug" (no swap) | **M2 "the half-fix"** (policies swap, postures stale) | unmutated |
|---|---|---|---|
| unit-registry | 2 passed / **3 failed** | 2 passed / **3 failed** | 5 / 0 |
| **unit-subscriber (arrivals)** | 0 passed / **2 failed** | **2 passed / 0 failed — rc=0** | 2 / 0 |
| unit-dispatcher (posture) | 0 passed / **2 failed** | 0 passed / **2 failed** | 2 / 0 |
| integration-reload | 0 passed / **1 failed** | 0 passed / **1 failed** | 1 / 0 |

**The M2 arrivals row is the point of this lane.** On the half-fix the arrivals suite is green,
rc=0. Only the posture assertions redden. That is the mandatory third assertion in measured
form: the older, weaker test shape *would have missed it*.

The two registry tests that survive both mutations are the two that exercise no swap, so the
mutation discriminates rather than blanket-failing.

### 5b. Mutation of the instrument

Collapsing `observedPostureIn`'s `null` into the fallback posture reddens **exactly 2** of the
32 self-test assertions, both intended (27 passed / 2 failed), and restores to 32/0.

---

## 6. Instrument fault, mine, found by running and repaired in-lane (§6b-ii)

**The first live control run reported `posture=null, sightings=0` for EVERY channel** and the
leg graded FAIL. The lines were in the log the whole time. `tracing-subscriber` colourises field
names, so what is on disk is not

```
channel turn dispatch channel=f24finthree posture=Workspace
```

but

```
channel turn dispatch \x1b[3mchannel\x1b[0m\x1b[2m=\x1b[0mf24finthree \x1b[3mposture\x1b[0m\x1b[2m=\x1b[0mWorkspace
```

— the escapes sit **between the field name and the `=`**, so `channel\s*=` cannot match. Every
posture read came back null, which is indistinguishable from "the product never dispatched".
**This is the fourth instrument fault on this criterion and, like the previous three, it failed
in the direction that blames the product.**

It was caught only because the leg carried a positive control that also read null: a channel
known to have been admitted (its reply is in the sink journal) cannot honestly have no dispatch
line, so the zero had to be the instrument. That is the whole argument for pairing every zero
with a control, and it is the second time on this criterion that the control, not a red, caught
the fault.

Repaired, not noted: `stripAnsi` in `scripts/f24-gateway-log.mjs`, with three self-test
assertions over the **verbatim bytes captured from that failing run** — known-positive reads
`Workspace`, known-negative still reads `null` for an absent channel (with a liveness control in
the same test), and **the pre-repair matcher is proven blind to those same bytes** while being
proven a no-op on plain text.

The matchers were moved into a module specifically so the self-test exercises the shipped code
rather than a copy — a self-test that re-implements its instrument drifts away from it silently.

Two smaller instrument hardenings made at the same time, both with their own assertions: channel
names are regex-escaped before interpolation (an unescaped `.` was matching a neighbouring
channel), and the denial matcher counts **sighted** denial lines rather than inferring denial
from an absent arrival — a dead pipe, a wrong URL or a crashed gateway all satisfy "no arrival
landed" for free (§3b-i).

---

## 7. Files

**Product (7):**
- `crates/wcore-agent/src/channel_policy.rs` — NEW. The registry, the single derivation, the
  single swap.
- `crates/wcore-agent/src/channel_inbound.rs` — subscriber reads `policy_for` per event.
- `crates/wcore-agent/src/channel_dispatch.rs` — dispatcher reads `scope_for` per turn.
- `crates/wcore-agent/src/channel_inbound_host.rs` — owns the `Arc`, exposes `reload_policies`.
- `crates/wcore-agent/src/bootstrap.rs` — migrated to the shared derivation;
  `try_load_channel_policy_configs` added.
- `crates/wcore-agent/src/lib.rs` — module registration.
- `crates/wcore-cli/src/gateway.rs` — the reload block calls `reload_policies`, logs
  `policies=`, refuses rather than revoking.

**Tests / instruments (4):** `crates/wcore-agent/tests/f24_c3_h5_reload_policies_test.rs` (NEW,
7 legs), `scripts/f24-gateway-log.mjs` (NEW), `scripts/f24-c3-clauses.mjs`,
`scripts/f24-c3-clauses-selftest.mjs`.

**§6 fence: `git diff $BASE -- crates/wcore-cli/src/{lib,main}.rs` is EMPTY**, against the
captured merge-base SHA `d34b2fe1`, never the branch name.

---

## 8. What I did NOT do

- **Did not touch `media` or `native actions`.** Two of `24-C3`'s eight clauses are still at
  zero evidence on every adapter. **This lane does not move `24-C3` to MET and does not claim
  it.**
- **Did not measure the `reconnect` half** of reconnect/reload (drop the upstream, does the
  adapter recover?). Only `reload`. That clause is now *better* than PARTIAL on its reload half
  and still untouched on its reconnect half.
- **Did not drive matrix, signal, msteams or imessage**, and did not build the email
  `route`/`bind` knob. The prior lane's costings stand unchanged.
- **Did not measure anything on macOS or Windows.** Every figure here is Linux.
- **Did not weaken, ignore, delete or re-gate a single test.** No `#[ignore]`, no `#[allow]`, no
  raised timeout.
- **Did not run `wcore-contract generate`**, merge, open a PR, tag, or close an issue.
- **Did not use, read or require any vendor credential.** Every secret in every run was minted at
  run time and died with it. No credential reached hetzner.
- **Did not edit `.github/workflows/ci.yml`, `crates/wcore-cli/src/{lib,main}.rs`, or
  `.planning/BACKLOG.md`**, per my lane's boundaries.
- **Did not run a full-workspace build or test** — `-p wcore-agent` / `-p wcore-cli` only, per
  the disk/contention rule.
- **Did not modify any shared fixture** (`f24-sink.mjs`, `f24-llm-fixture.mjs`,
  `f24-correlate.mjs`). The driver conforms to their contracts; the new matchers went into a new
  module.

### One known limitation, stated rather than buried

`ChannelTurnDispatcher` pools one `AgentEngine` per session, and an engine is built with the
scope resolved **at first use**. So a posture *change* to a channel that already has a live
session is not retroactive — that session keeps the engine it built until it is evicted
(`WORKER_IDLE_TTL`, 300 s) or the process restarts. This lane's defect is a **newly added**
channel, which by definition has no pooled engine, so the repair is complete for it. Tightening
posture on an *existing* busy session is a separate, smaller question; it is not a regression
introduced here (the old code could not refresh at all), and I am recording it rather than
quietly widening scope. MEDIUM at most → BACKLOG.

---

## 9. For the orchestrator to serialize

**Nothing.** Zero bytes in the §6 fence, no protocol seam, no contract fixture, no dependency
change, no `Cargo.toml` edit.

One cross-lane note: `InboundSubscriber::new` and `ChannelTurnDispatcher::new` changed signature
(both now take `Arc<ChannelPolicyRegistry>` instead of a bare `HashMap`). Both are
`wcore-agent`-internal and all four production call sites are updated in this lane; a lane that
adds a fifth will need the `Arc`.

`f24-c3-clauses.mjs` gains `--posture-baseline-out` / `--posture-baseline-in`. Default behaviour
with neither flag is unchanged.

---

## 10. Evidence

`.planning/phases/24-gateway-automation-channels-typed-api/24-H5-evidence/`

| path | bytes | what it holds |
|---|---|---|
| `24-H5-NOTES.md` | — | the running log, committed at T+12 min and re-committed after every measurement |
| `f24-h5-mutate.py` | — | the product-mutation harness (M1 / M2), restores in a `finally:` |
| `mutation/mutation-M1.json` | 1491 | "the bug" — 8 of 10 redden |
| `mutation/mutation-M2.json` | 1294 | **"the half-fix" — arrivals green, posture red** |
| `mutation/*.stderr` | 0, 0 | empty, byte-counted |
| `live/ctl/` | result 5676, log 2051, arrivals 540 | startup lifecycle, PASS 6/6 |
| `live/reload/` | result 9305, log 2558, arrivals 808 | **reload lifecycle, fixed binary, PASS 11/11** |
| `live/prefix-reload/` | result 9299, log 1891, arrivals 536 | **reload lifecycle, PRE-FIX binary, FAIL 8/11** |
| `live/posture-baseline.json` | 69 | `{"posture":"Workspace","tier":"same-line","sightings":1}` |
| `selftest.txt` | 2357 | 32 passed / 0 failed, run on hetzner at lane HEAD |
| `wcore-agent-lib-serial-tail.txt` | 114 | 2170 passed / 0 failed / 3 ignored, rc=0 |

Gateway logs are stored ANSI-stripped for readability; the matchers strip at read time, and the
self-test asserts against the raw coloured bytes.
